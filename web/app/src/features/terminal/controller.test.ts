import { TERMINAL_PENDING_INPUT_MAX_BYTES, TERMINAL_READ_MIN_GAP_MS, TERMINAL_RENDER_SLICE_BYTES, TERMINAL_WRITE_CHUNK_BYTES, TERMINAL_WRITE_FLUSH_MS } from "@monoptah/protocol";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { TerminalController } from "./controller";
import { INPUT_DROPPED_NOTICE, INPUT_LOST_NOTICE } from "./copy";
import { fromBase64 } from "./input";
import type { ResumeEntry, ResumeStore } from "./resume";
import { ReauthCancelledError } from "@/features/accounts/queries";
import { FakeError, FakeLink, FakeScreen, b64, readResult } from "./testing";

const OPENED = { terminalId: "term-1", cols: 80, rows: 24, cwd: "Example Alpha", idleTimeoutMs: 900_000, maxDurationMs: 28_800_000 };

class MemoryStore implements ResumeStore {
  entry: ResumeEntry | null = null;
  load = () => this.entry;
  save = (entry: ResumeEntry) => void (this.entry = entry);
  clear = () => void (this.entry = null);
}

/** Lets promise continuations run (the controller awaits several times per step). */
const settle = async (): Promise<void> => {
  for (let i = 0; i < 12; i++) await Promise.resolve();
};
const advance = async (ms: number): Promise<void> => {
  await vi.advanceTimersByTimeAsync(ms);
  await settle();
};

interface Rig {
  tickets: string[];
  requestTicket: ReturnType<typeof vi.fn<(projectId: string) => Promise<string>>>;
  link: FakeLink;
  screen: FakeScreen;
  store: MemoryStore;
  controller: TerminalController;
}

function rig(): Rig {
  const link = new FakeLink();
  const screen = new FakeScreen();
  const store = new MemoryStore();
  const tickets: string[] = [];
  const requestTicket = vi.fn(async (projectId: string) => {
    tickets.push(projectId);
    return "ticket-1";
  });
  const controller = new TerminalController({ link, store, requestTicket });
  controller.attachScreen(screen);
  return { tickets, requestTicket, link, screen, store, controller };
}

/** Walks the open flow to a running shell and leaves its first read waiting. */
async function running(r: Rig): Promise<void> {
  void r.controller.begin({ projectId: "p1" });
  await settle();
  r.link.open("terminal.open")[0]!.resolve(OPENED);
  await settle();
}

beforeEach(() => {
  vi.useFakeTimers({ now: 1_000_000 });
});
afterEach(() => {
  vi.useRealTimers();
});

describe("open flow", () => {
  it("asks for a passkey ticket for the project, then opens in one step with the ticket, the folder and the window size", async () => {
    const r = rig();
    r.screen.dims = { cols: 120, rows: 40 };
    r.controller.attachScreen(r.screen);
    void r.controller.begin({ projectId: "p1", subdir: " src/app " });
    await settle();
    expect(r.controller.getSnapshot().phase).toBe("starting");
    expect(r.tickets).toEqual(["p1"]);
    const open = r.link.open("terminal.open");
    expect(open).toHaveLength(1);
    expect(open[0]!.params).toEqual({ projectId: "p1", subdir: "src/app", cols: 120, rows: 40, ticket: "ticket-1" });
    open[0]!.resolve(OPENED);
    await settle();
    expect(r.controller.getSnapshot()).toMatchObject({ phase: "running", session: { terminalId: "term-1", cwd: "Example Alpha" } });
    expect(r.store.entry).toMatchObject({ terminalId: "term-1", next: 0 });
  });

  it("sends nothing to the desktop while the passkey prompt is open, and nothing at all when it is cancelled", async () => {
    const r = rig();
    let release: (ticket: string) => void = () => undefined;
    r.requestTicket.mockImplementationOnce(() => new Promise<string>((resolve) => (release = resolve)));
    void r.controller.begin({ projectId: "p1" });
    await settle();
    expect(r.controller.getSnapshot().phase).toBe("starting");
    expect(r.link.calls).toHaveLength(0);
    release("ticket-9");
    await settle();
    expect(r.link.open("terminal.open")[0]!.params).toMatchObject({ ticket: "ticket-9" });

    const cancelled = rig();
    cancelled.requestTicket.mockRejectedValueOnce(new ReauthCancelledError());
    await cancelled.controller.begin({ projectId: "p1" });
    expect(cancelled.link.calls).toHaveLength(0);
    expect(cancelled.controller.getSnapshot()).toMatchObject({ phase: "failed", error: expect.stringContaining("cancelled") });
  });

  it("a refused ticket request (forbidden) fails with a message and never calls the desktop", async () => {
    const r = rig();
    r.requestTicket.mockRejectedValueOnce(new FakeError("forbidden"));
    await r.controller.begin({ projectId: "p1" });
    expect(r.link.calls).toHaveLength(0);
    expect(r.controller.getSnapshot()).toMatchObject({ phase: "failed", error: expect.stringContaining("not allowed") });
  });

  it("every open asks for its own ticket", async () => {
    const r = rig();
    r.requestTicket.mockResolvedValueOnce("first").mockResolvedValueOnce("second");
    void r.controller.begin({ projectId: "p1" });
    await settle();
    r.link.open("terminal.open")[0]!.reject(new FakeError("busy"));
    await settle();
    void r.controller.begin({ projectId: "p1" });
    await settle();
    expect(r.link.all("terminal.open").map((call) => call.params["ticket"])).toEqual(["first", "second"]);
  });

  it("leaves an empty folder out and refuses an unsafe one before any request", async () => {
    const r = rig();
    void r.controller.begin({ projectId: "p1", subdir: "  " });
    await settle();
    expect(r.link.open("terminal.open")[0]!.params).toEqual({ projectId: "p1", cols: 80, rows: 24, ticket: "ticket-1" });
    const other = rig();
    await other.controller.begin({ projectId: "p1", subdir: "../etc" });
    expect(other.link.calls).toHaveLength(0);
    expect(other.requestTicket).not.toHaveBeenCalled();
    expect(other.controller.getSnapshot()).toMatchObject({ phase: "failed", error: expect.stringContaining("not allowed") });
  });

  it.each([
    ["fresh_required", "passkey"],
    ["disabled", "Allow web terminal"],
    ["read_only", "only read"],
    ["not_selected", "not shared"],
    ["busy", "too many"],
    ["unsupported", "Update Monoptah"],
  ])("a refused open (%s) fails with a message that says what to do", async (code, text) => {
    const r = rig();
    void r.controller.begin({ projectId: "p1" });
    await settle();
    r.link.open("terminal.open")[0]!.reject(new FakeError(code));
    await settle();
    expect(r.controller.getSnapshot()).toMatchObject({ phase: "failed", error: expect.stringContaining(text) });
  });

  it("cancel goes back to idle and a late ticket or answer changes nothing", async () => {
    const r = rig();
    void r.controller.begin({ projectId: "p1" });
    await settle();
    r.controller.cancel();
    r.link.open("terminal.open")[0]!.resolve(OPENED);
    await settle();
    expect(r.controller.getSnapshot().phase).toBe("idle");
    expect(r.link.open("terminal.read")).toHaveLength(0);
    expect(r.link.all("terminal.close").map((c) => c.params)).toEqual([{ terminalId: "term-1" }]);

    const late = rig();
    let release: (ticket: string) => void = () => undefined;
    late.requestTicket.mockImplementationOnce(() => new Promise<string>((resolve) => (release = resolve)));
    void late.controller.begin({ projectId: "p1" });
    await settle();
    late.controller.cancel();
    release("late");
    await settle();
    expect(late.link.calls).toHaveLength(0);
    expect(late.controller.getSnapshot().phase).toBe("idle");
  });
});

describe("closing while the open is in flight", () => {
  it("close sends terminal.close for the shell that answers late and leaves no session", async () => {
    const r = rig();
    void r.controller.begin({ projectId: "p1" });
    await settle();
    await r.controller.close();
    r.link.open("terminal.open")[0]!.resolve(OPENED);
    await settle();
    expect(r.link.all("terminal.close").map((c) => c.params)).toEqual([{ terminalId: "term-1" }]);
    expect(r.controller.getSnapshot().phase).toBe("idle");
    expect(r.link.open("terminal.read")).toHaveLength(0);
  });
});

describe("read loop", () => {
  it("asks for one read at a time, from the offset after the last one, and draws what comes", async () => {
    const r = rig();
    await running(r);
    expect(r.link.open("terminal.read")).toHaveLength(1);
    expect(r.link.open("terminal.read")[0]!.params).toEqual({ terminalId: "term-1", from: 0, waitMs: 15_000, maxBytes: 32_768 });
    r.link.open("terminal.read")[0]!.resolve(readResult(0, "hello "));
    await settle();
    expect(r.screen.text()).toBe("hello ");
    // Data flowed: the next read waits for the gap.
    expect(r.link.open("terminal.read")).toHaveLength(0);
    await advance(TERMINAL_READ_MIN_GAP_MS);
    expect(r.link.open("terminal.read")).toHaveLength(1);
    expect(r.link.open("terminal.read")[0]!.params).toMatchObject({ from: 6 });
    r.link.open("terminal.read")[0]!.resolve(readResult(6, "world"));
    await advance(TERMINAL_READ_MIN_GAP_MS);
    expect(r.screen.text()).toBe("hello world");
    expect(r.link.open("terminal.read")[0]!.params).toMatchObject({ from: 11 });
    expect(r.link.all("terminal.read").filter((call) => !call.settled)).toHaveLength(1);
  });

  it("asks again at once after an empty answer (the long poll ran out), with no gap", async () => {
    const r = rig();
    await running(r);
    r.link.open("terminal.read")[0]!.resolve({ from: 0, next: 0, data: "", exited: null });
    await settle();
    expect(r.link.open("terminal.read")).toHaveLength(1);
  });

  it("waits for the screen before the next read and draws a big answer in bounded slices", async () => {
    const r = rig();
    r.screen.hold = true;
    await running(r);
    const text = "x".repeat(TERMINAL_RENDER_SLICE_BYTES * 2 + 100);
    r.link.open("terminal.read")[0]!.resolve(readResult(0, text));
    await settle();
    expect(r.screen.writes).toHaveLength(1);
    expect(r.screen.writes[0]!.length).toBe(TERMINAL_RENDER_SLICE_BYTES);
    await advance(1_000);
    expect(r.link.open("terminal.read")).toHaveLength(0);
    r.screen.releaseOne();
    await settle();
    expect(r.screen.writes).toHaveLength(2);
    r.screen.releaseOne();
    await settle();
    expect(r.screen.writes.map((w) => w.length)).toEqual([TERMINAL_RENDER_SLICE_BYTES, TERMINAL_RENDER_SLICE_BYTES, 100]);
    // Still no read: the last slice is not parsed yet.
    await advance(1_000);
    expect(r.link.open("terminal.read")).toHaveLength(0);
    r.screen.releaseOne();
    await advance(TERMINAL_READ_MIN_GAP_MS);
    expect(r.link.open("terminal.read")).toHaveLength(1);
    expect(r.link.open("terminal.read")[0]!.params).toMatchObject({ from: text.length });
  });

  it("a read that starts later than asked (a reattach) is drawn as it is and next follows its own numbers", async () => {
    const r = rig();
    await running(r);
    r.link.open("terminal.read")[0]!.resolve({ from: 500, next: 507, data: b64("later!\n"), exited: null });
    await advance(TERMINAL_READ_MIN_GAP_MS);
    expect(r.screen.text()).toBe("later!\n");
    expect(r.link.open("terminal.read")[0]!.params).toMatchObject({ from: 507 });
  });

  it("draws only the new part of an answer that starts before the offset", async () => {
    const r = rig();
    await running(r);
    r.link.open("terminal.read")[0]!.resolve(readResult(0, "abc"));
    await advance(TERMINAL_READ_MIN_GAP_MS);
    r.link.open("terminal.read")[0]!.resolve({ from: 1, next: 5, data: b64("bcde"), exited: null });
    await advance(TERMINAL_READ_MIN_GAP_MS);
    expect(r.screen.text()).toBe("abcde");
  });

  it("keeps the resume entry's offset current without storing any output", async () => {
    const r = rig();
    await running(r);
    r.link.open("terminal.read")[0]!.resolve(readResult(0, "secret output"));
    await advance(300);
    expect(r.store.entry).toMatchObject({ terminalId: "term-1", next: 13 });
    expect(JSON.stringify(r.store.entry)).not.toContain("secret");
  });

  it("retries a malformed answer and ends after repeated failures", async () => {
    const r = rig();
    await running(r);
    for (let i = 0; i < 6; i++) {
      r.link.open("terminal.read")[0]!.resolve({ nonsense: true });
      await advance(6_000);
    }
    expect(r.controller.getSnapshot()).toMatchObject({ phase: "ended", ended: { reason: "error" } });
  });

  it("a read that never answers is given up on after waitMs plus 10 s and asked again from the same offset", async () => {
    const r = rig();
    await running(r);
    const first = r.link.open("terminal.read")[0]!;
    await advance(25_001);
    expect(first.settled).toBe(true);
    await advance(1_000);
    const again = r.link.open("terminal.read");
    expect(again).toHaveLength(1);
    expect(again[0]!.params).toMatchObject({ from: 0 });
  });
});

describe("the end of a session", () => {
  it.each([
    ["exit", 0, "the shell exited with code 0."],
    ["closed", undefined, "you closed the terminal."],
    ["revoked", undefined, "this device was removed"],
    ["idle", undefined, "idle for 15 minutes"],
    ["max", undefined, "time limit"],
    ["toggle_off", undefined, "Allow web terminal was turned off"],
    ["killed", undefined, "closed on the desktop"],
    ["grace", undefined, "connection was lost"],
    ["desktop", undefined, "desktop closed"],
  ])("exited %s ends the session, draws the last bytes, stops reading and clears the resume entry", async (reason, code, text) => {
    const r = rig();
    await running(r);
    r.link.open("terminal.read")[0]!.resolve(readResult(0, "bye\r\n", { reason, ...(code === undefined ? {} : { code }) }));
    await settle();
    expect(r.screen.text()).toBe("bye\r\n");
    const snapshot = r.controller.getSnapshot();
    expect(snapshot.phase).toBe("ended");
    expect(snapshot.ended?.reason).toBe(reason);
    expect(snapshot.ended?.text).toContain(text);
    await advance(5_000);
    expect(r.link.open("terminal.read")).toHaveLength(0);
    expect(r.store.entry).toBeNull();
  });

  it("not_found ends it as gone, or as grace when the link had dropped or the page was reloaded", async () => {
    const gone = rig();
    await running(gone);
    gone.link.open("terminal.read")[0]!.reject(new FakeError("not_found"));
    await settle();
    expect(gone.controller.getSnapshot().ended?.reason).toBe("gone");

    const dropped = rig();
    await running(dropped);
    dropped.link.setReady(false);
    dropped.link.open("terminal.read")[0]?.reject(new FakeError("session_ended"));
    await settle();
    dropped.link.setReady(true);
    await settle();
    dropped.link.open("terminal.read")[0]!.reject(new FakeError("not_found"));
    await settle();
    expect(dropped.controller.getSnapshot().ended?.reason).toBe("grace");
  });

  it("disabled and revoked read answers end the session with their own reason", async () => {
    const off = rig();
    await running(off);
    off.link.open("terminal.read")[0]!.reject(new FakeError("disabled"));
    await settle();
    expect(off.controller.getSnapshot().ended?.reason).toBe("toggle_off");
    const revoked = rig();
    await running(revoked);
    revoked.link.open("terminal.read")[0]!.reject(new FakeError("revoked"));
    await settle();
    expect(revoked.controller.getSnapshot().ended?.reason).toBe("revoked");
  });

  it("a busy answer is retried after a pause, not ended", async () => {
    const r = rig();
    await running(r);
    r.link.open("terminal.read")[0]!.reject(new FakeError("busy"));
    await settle();
    expect(r.link.open("terminal.read")).toHaveLength(0);
    await advance(1_000);
    expect(r.link.open("terminal.read")).toHaveLength(1);
    expect(r.controller.getSnapshot().phase).toBe("running");
  });

  it("close sends terminal.close, stops everything and says the user closed it, even when the link is down", async () => {
    const r = rig();
    await running(r);
    const closing = r.controller.close();
    await settle();
    expect(r.link.open("terminal.close")[0]!.params).toEqual({ terminalId: "term-1" });
    expect(r.link.open("terminal.read")).toHaveLength(0);
    r.link.open("terminal.close")[0]!.resolve({});
    await closing;
    expect(r.controller.getSnapshot()).toMatchObject({ phase: "ended", ended: { reason: "closed" } });
    expect(r.store.entry).toBeNull();

    const offline = rig();
    await running(offline);
    offline.link.ready = false;
    const closed = offline.controller.close();
    await settle();
    await closed;
    expect(offline.controller.getSnapshot().ended?.reason).toBe("closed");
  });

  it("dispose stops the loops and sends no close: the shell stays for a reattach", async () => {
    const r = rig();
    await running(r);
    r.controller.dispose();
    await advance(5_000);
    expect(r.link.all("terminal.close")).toHaveLength(0);
    expect(r.link.open("terminal.read")).toHaveLength(0);
    expect(r.store.entry).toMatchObject({ terminalId: "term-1" });
  });

  it("reset after an end goes back to the chooser", async () => {
    const r = rig();
    await running(r);
    r.link.open("terminal.read")[0]!.resolve(readResult(0, "", { reason: "exit", code: 0 }));
    await settle();
    r.controller.reset();
    expect(r.controller.getSnapshot()).toMatchObject({ phase: "idle", ended: null, session: null });
  });
});

describe("a dropped link", () => {
  it("keeps the terminal id, waits for the link and resumes the read from the last next", async () => {
    const r = rig();
    await running(r);
    r.link.open("terminal.read")[0]!.resolve(readResult(0, "before"));
    await advance(TERMINAL_READ_MIN_GAP_MS);
    const waiting = r.link.open("terminal.read")[0]!;
    r.link.setReady(false);
    waiting.reject(new FakeError("session_ended"));
    await settle();
    expect(r.controller.getSnapshot()).toMatchObject({ phase: "reconnecting", session: { terminalId: "term-1" } });
    const before = r.link.calls.length;
    await advance(10_000);
    expect(r.link.calls.length).toBe(before);
    r.link.setReady(true);
    await settle();
    const resumed = r.link.open("terminal.read");
    expect(resumed).toHaveLength(1);
    // The first read after a drop answers at once (waitMs 0), so the page learns the shell is still there.
    expect(resumed[0]!.params).toEqual({ terminalId: "term-1", from: 6, waitMs: 0, maxBytes: 32_768 });
    resumed[0]!.resolve(readResult(6, " after"));
    await settle();
    expect(r.controller.getSnapshot().phase).toBe("running");
    await advance(TERMINAL_READ_MIN_GAP_MS);
    expect(r.link.open("terminal.read")[0]!.params).toMatchObject({ from: 12, waitMs: 15_000 });
    expect(r.screen.text()).toBe("before after");
  });

  it("a page reload resumes from the saved offset and shows whatever the desktop answers", async () => {
    const r = rig();
    r.controller.resume({ terminalId: "term-9", next: 120, cwd: "Example Alpha", idleTimeoutMs: 900_000, maxDurationMs: 28_800_000, openedAt: 999_000 });
    await settle();
    expect(r.controller.getSnapshot()).toMatchObject({ phase: "reconnecting", session: { terminalId: "term-9" } });
    const read = r.link.open("terminal.read")[0]!;
    expect(read.params).toMatchObject({ terminalId: "term-9", from: 120, waitMs: 0 });
    read.resolve({ from: 140, next: 145, data: b64("$ ls\n"), exited: null });
    await settle();
    expect(r.controller.getSnapshot().phase).toBe("running");
    expect(r.screen.text()).toBe("$ ls\n");
  });

  it("a reload after the grace window ends as grace", async () => {
    const r = rig();
    r.controller.resume({ terminalId: "term-9", next: 0, cwd: "x", idleTimeoutMs: 1, maxDurationMs: 1, openedAt: 1 });
    await settle();
    r.link.open("terminal.read")[0]!.reject(new FakeError("not_found"));
    await settle();
    expect(r.controller.getSnapshot()).toMatchObject({ phase: "ended", ended: { reason: "grace" } });
  });

  it("does not wait for a screen it does not have before reading", async () => {
    const link = new FakeLink();
    const controller = new TerminalController({ link, requestTicket: async () => "ticket" });
    controller.resume({ terminalId: "term-9", next: 0, cwd: "x", idleTimeoutMs: 1, maxDurationMs: 1, openedAt: 1 });
    await settle();
    expect(link.open("terminal.read")).toHaveLength(0);
    controller.attachScreen(new FakeScreen());
    await settle();
    expect(link.open("terminal.read")).toHaveLength(1);
  });
});

describe("write side", () => {
  const decoded = (params: Record<string, unknown>): string => new TextDecoder().decode(fromBase64(String(params["data"]))!);

  it("sends the first key at once, and keys typed while it is in flight go out together in the next write", async () => {
    const r = rig();
    await running(r);
    r.controller.input("a");
    await settle();
    const first = r.link.open("terminal.write");
    expect(first).toHaveLength(1);
    expect(first[0]!.params).toEqual({ terminalId: "term-1", data: b64("a") });
    await advance(10);
    r.controller.input("b");
    await advance(10);
    r.controller.input("c");
    await advance(10);
    expect(r.link.all("terminal.write")).toHaveLength(1);
    first[0]!.resolve({});
    await settle();
    // The flush window has not passed since the first write started (30 ms of 50): wait for it.
    expect(r.link.all("terminal.write")).toHaveLength(1);
    await advance(TERMINAL_WRITE_FLUSH_MS);
    const second = r.link.all("terminal.write");
    expect(second).toHaveLength(2);
    expect(decoded(second[1]!.params)).toBe("bc");
  });

  it("never has two writes in flight and starts a write at most once per flush window", async () => {
    const r = rig();
    await running(r);
    for (let i = 0; i < 20; i++) {
      r.controller.input("k");
      await advance(5);
      const open = r.link.open("terminal.write");
      expect(open.length).toBeLessThanOrEqual(1);
      for (const call of open) call.resolve({});
      await settle();
    }
    await advance(200);
    for (const call of r.link.open("terminal.write")) call.resolve({});
    await settle();
    const total = r.link.all("terminal.write").map((call) => decoded(call.params)).join("");
    expect(total).toBe("k".repeat(20));
    // 20 keys over about 100 ms: far fewer writes than keys.
    expect(r.link.all("terminal.write").length).toBeLessThan(10);
  });

  it("splits a big paste into chunks of at most 4096 bytes, one after the other, never inside a character", async () => {
    const r = rig();
    await running(r);
    const text = "€".repeat(3000); // 9000 bytes
    r.controller.input(text);
    await settle();
    const sent: string[] = [];
    for (let i = 0; i < 6 && r.link.open("terminal.write").length > 0; i++) {
      const call = r.link.open("terminal.write")[0]!;
      const bytes = fromBase64(String(call.params["data"]))!;
      expect(bytes.length).toBeLessThanOrEqual(TERMINAL_WRITE_CHUNK_BYTES);
      sent.push(new TextDecoder("utf-8", { fatal: true }).decode(bytes));
      call.resolve({});
      await settle();
      await advance(TERMINAL_WRITE_FLUSH_MS);
    }
    expect(sent.join("")).toBe(text);
    expect(sent.length).toBe(3);
  });

  it("caps pending input at 64 KiB: the overflow is dropped with a hint that clears by itself", async () => {
    const r = rig();
    await running(r);
    r.controller.input("a");
    await settle();
    // The first write is in flight and never answers: everything else queues.
    r.controller.input("b".repeat(TERMINAL_PENDING_INPUT_MAX_BYTES));
    expect(r.controller.getSnapshot().inputNotice).toBeNull();
    r.controller.input("c");
    expect(r.controller.getSnapshot().inputNotice).toBe(INPUT_DROPPED_NOTICE);
    await advance(6_000);
    expect(r.controller.getSnapshot().inputNotice).toBeNull();
  });

  it("holds input while the link is down and sends it once it is back", async () => {
    const r = rig();
    await running(r);
    r.link.setReady(false);
    r.link.open("terminal.read")[0]?.reject(new FakeError("session_ended"));
    await settle();
    r.controller.input("ls\r");
    await advance(500);
    expect(r.link.all("terminal.write")).toHaveLength(0);
    r.link.setReady(true);
    await settle();
    expect(decoded(r.link.open("terminal.write")[0]!.params)).toBe("ls\r");
  });

  it("a write that failed after it left is not sent twice, and says so", async () => {
    const r = rig();
    await running(r);
    r.controller.input("x");
    await settle();
    r.link.open("terminal.write")[0]!.reject(new FakeError("session_ended"));
    await settle();
    expect(r.controller.getSnapshot().inputNotice).toBe(INPUT_LOST_NOTICE);
    await advance(500);
    expect(r.link.all("terminal.write")).toHaveLength(1);
  });

  it("a write answered not_found ends the session", async () => {
    const r = rig();
    await running(r);
    r.controller.input("x");
    await settle();
    r.link.open("terminal.write")[0]!.reject(new FakeError("not_found"));
    await settle();
    expect(r.controller.getSnapshot().phase).toBe("ended");
  });

  it("ignores input before a shell is running and after it ended", async () => {
    const r = rig();
    r.controller.input("a");
    await running(r);
    expect(r.link.all("terminal.write")).toHaveLength(0);
    r.link.open("terminal.read")[0]!.resolve(readResult(0, "", { reason: "exit", code: 0 }));
    await settle();
    r.controller.input("b");
    await advance(500);
    expect(r.link.all("terminal.write")).toHaveLength(0);
  });
});

describe("resize", () => {
  it("clamps to the protocol limits, waits for the window to settle and sends only the last size", async () => {
    const r = rig();
    await running(r);
    r.controller.resize(100, 30);
    r.controller.resize(900, 1);
    await advance(100);
    expect(r.link.all("terminal.resize")).toHaveLength(0);
    await advance(100);
    const sent = r.link.all("terminal.resize");
    expect(sent).toHaveLength(1);
    expect(sent[0]!.params).toEqual({ terminalId: "term-1", cols: 500, rows: 2 });
  });

  it("sends nothing for the size the shell already has", async () => {
    const r = rig();
    await running(r);
    r.controller.resize(80, 24);
    await advance(500);
    expect(r.link.all("terminal.resize")).toHaveLength(0);
  });

  it("a size that changes while one is in flight is sent after it", async () => {
    const r = rig();
    await running(r);
    r.controller.resize(100, 30);
    await advance(200);
    r.controller.resize(110, 31);
    await advance(200);
    expect(r.link.all("terminal.resize")).toHaveLength(1);
    r.link.open("terminal.resize")[0]!.resolve({});
    await advance(200);
    expect(r.link.all("terminal.resize").map((call) => call.params["cols"])).toEqual([100, 110]);
  });
});
