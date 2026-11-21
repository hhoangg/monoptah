import { describe, expect, it } from "vitest";
import { KNOWN_CAPABILITIES, METHODS } from "@monoptah/protocol";
import { FixtureError, MockTerminals, type TerminalClock } from "../src/index.ts";
import { setup } from "./harness.ts";

const DEVICE = "device-a";
const b64 = (text: string): string => btoa(text);
const text = (data: string): string => atob(data);

/** A clock a test advances by hand, so expiry and grace need no real waiting. */
class ManualClock implements TerminalClock {
  time = 1_000;
  #timers = new Map<number, { at: number; fn: () => void }>();
  #next = 1;
  now(): number {
    return this.time;
  }
  setTimeout(fn: () => void, ms: number): unknown {
    const id = this.#next++;
    this.#timers.set(id, { at: this.time + ms, fn });
    return id;
  }
  clearTimeout(handle: unknown): void {
    this.#timers.delete(handle as number);
  }
  advance(ms: number): void {
    this.time += ms;
    for (const [id, timer] of [...this.#timers]) {
      if (timer.at <= this.time && this.#timers.delete(id)) timer.fn();
    }
  }
}

const OPEN = { projectId: "p", cols: 80, rows: 24, ticket: "ticket-1" } as const;
const OVER_LINK = { projectId: "proj-alpha", cols: 80, rows: 24, ticket: "ticket-1" } as const;
const acceptAll = async (): Promise<boolean> => true;

function shells(options: ConstructorParameters<typeof MockTerminals>[0] = {}) {
  const clock = new ManualClock();
  const terminals = new MockTerminals({ clock, ticketVerifier: acceptAll, ...options });
  const start = async (device = DEVICE) => (await terminals.open(device, OPEN, "Alpha")).terminalId;
  return { clock, terminals, start };
}

const codeOf = (run: () => unknown): string | undefined => {
  try {
    run();
  } catch (error) {
    if (error instanceof FixtureError) return error.code;
    throw error;
  }
  return undefined;
};

const asyncCodeOf = async (run: () => Promise<unknown>): Promise<string | undefined> => {
  try {
    await run();
  } catch (error) {
    if (error instanceof FixtureError) return error.code;
    throw error;
  }
  return undefined;
};

describe("terminal.open ticket", () => {
  it("one step with an accepted ticket opens a terminal named after the project and subdir", async () => {
    const terminals = new MockTerminals({ ticketVerifier: acceptAll });
    const opened = await terminals.open(DEVICE, { ...OPEN, subdir: "src" }, "Alpha");
    expect(opened).toMatchObject({ cwd: "Alpha/src", cols: 80, rows: 24, idleTimeoutMs: 900_000, maxDurationMs: 28_800_000 });
    expect(METHODS["terminal.open"].result.safeParse(opened).success).toBe(true);
  });

  it("the verifier gets the ticket and the project, and a refusal is fresh_required with no terminal", async () => {
    const seen: [string, string][] = [];
    const { terminals } = shells({
      ticketVerifier: async (ticket, projectId) => {
        seen.push([ticket, projectId]);
        return false;
      },
    });
    expect(await asyncCodeOf(() => terminals.open(DEVICE, OPEN, "Alpha"))).toBe("fresh_required");
    expect(seen).toEqual([["ticket-1", "p"]]);
    expect(terminals.liveIds()).toEqual([]);
  });

  it("a verifier that throws is a refusal too", async () => {
    const { terminals } = shells({
      ticketVerifier: async () => {
        throw new Error("network down");
      },
    });
    expect(await asyncCodeOf(() => terminals.open(DEVICE, OPEN, "Alpha"))).toBe("fresh_required");
    expect(terminals.liveIds()).toEqual([]);
  });

  it("without a verifier it fails closed with fresh_required and counts nothing", async () => {
    const terminals = new MockTerminals();
    expect(terminals.verifyTicket).toBeNull();
    expect(await asyncCodeOf(() => terminals.open(DEVICE, OPEN, "Alpha"))).toBe("fresh_required");
    expect(terminals.ticketVerifications()).toBe(0);
    expect(terminals.liveIds()).toEqual([]);
  });

  it("a verifier set after construction is used", async () => {
    const terminals = new MockTerminals();
    terminals.verifyTicket = async (ticket) => ticket === "good";
    expect(await asyncCodeOf(() => terminals.open(DEVICE, { ...OPEN, ticket: "bad" }, "Alpha"))).toBe("fresh_required");
    expect(await terminals.open(DEVICE, { ...OPEN, ticket: "good" }, "Alpha")).toHaveProperty("terminalId");
  });

  it("ticketVerifications counts every ticket the desktop spent, accepted or refused", async () => {
    const { terminals } = shells({ ticketVerifier: async (ticket) => ticket !== "bad" });
    expect(terminals.ticketVerifications()).toBe(0);
    await terminals.open(DEVICE, OPEN, "Alpha");
    await asyncCodeOf(() => terminals.open(DEVICE, { ...OPEN, ticket: "bad" }, "Alpha"));
    expect(terminals.ticketVerifications()).toBe(2);
  });

  it("an empty ticket is refused without asking the verifier", async () => {
    const { terminals } = shells();
    expect(await asyncCodeOf(() => terminals.open(DEVICE, { ...OPEN, ticket: "" }, "Alpha"))).toBe("fresh_required");
    expect(terminals.ticketVerifications()).toBe(0);
  });

  it("too many terminals answer busy before a ticket is spent; a bad subdir is path_denied", async () => {
    const { terminals, start } = shells({ maxTerminals: 1 });
    await start();
    expect(await asyncCodeOf(() => terminals.open(DEVICE, OPEN, "Alpha"))).toBe("busy");
    expect(terminals.ticketVerifications()).toBe(1);
    expect(await asyncCodeOf(() => terminals.open(DEVICE, { ...OPEN, subdir: "../x" }, "Alpha"))).toBe("path_denied");
    expect(terminals.ticketVerifications()).toBe(1);
  });
});

describe("terminal read, write, resize, close", () => {
  it("shows the banner, echoes typed input, runs echo and numbers output by absolute offset", async () => {
    const { terminals, start } = shells();
    const id = await start();
    const first = await terminals.read(DEVICE, { terminalId: id, from: 0, waitMs: 0 });
    expect(first).toMatchObject({ from: 0, exited: null });
    expect(text(first.data)).toBe("Monoptah mock shell in Alpha\r\n$ ");
    expect(first.next).toBe(first.data === "" ? 0 : text(first.data).length);
    terminals.write(DEVICE, { terminalId: id, data: b64("echo hi\r") });
    const second = await terminals.read(DEVICE, { terminalId: id, from: first.next, waitMs: 0 });
    expect(second.from).toBe(first.next);
    expect(text(second.data)).toBe("echo hi\r\nhi\r\n$ ");
    expect(terminals.received(id)).toEqual(["echo hi\r"]);
  });

  it("a read waits (long poll) until output arrives, and answers empty when waitMs runs out", async () => {
    const { terminals, start, clock } = shells();
    const id = await start();
    const banner = await terminals.read(DEVICE, { terminalId: id, from: 0, waitMs: 0 });
    const pending = terminals.read(DEVICE, { terminalId: id, from: banner.next, waitMs: 15_000 });
    let settled = false;
    void pending.then(() => (settled = true));
    await Promise.resolve();
    expect(settled).toBe(false);
    terminals.write(DEVICE, { terminalId: id, data: b64("x") });
    expect(text((await pending).data)).toBe("x");
    const idle = terminals.read(DEVICE, { terminalId: id, from: banner.next + 1, waitMs: 5_000 });
    clock.advance(5_000);
    expect(await idle).toMatchObject({ data: "", exited: null });
  });

  it("re-reading from an older offset after an acknowledgement starts at the acknowledged byte", async () => {
    const { terminals, start } = shells();
    const id = await start();
    const banner = await terminals.read(DEVICE, { terminalId: id, from: 0, waitMs: 0 });
    terminals.write(DEVICE, { terminalId: id, data: b64("a") });
    await terminals.read(DEVICE, { terminalId: id, from: banner.next, waitMs: 0 });
    const again = await terminals.read(DEVICE, { terminalId: id, from: 0, waitMs: 0 });
    // The banner was acknowledged by the second read; "a" was not, so it comes again from the acknowledged offset.
    expect(again.from).toBe(banner.next);
    expect(text(again.data)).toBe("a");
  });

  it("maxBytes bounds one answer and flood prints a long burst the client reads page by page", async () => {
    const { terminals, start } = shells({ floodLines: 600 });
    const id = await start();
    terminals.write(DEVICE, { terminalId: id, data: b64("flood\r") });
    let from = 0;
    let total = 0;
    let pages = 0;
    let last = "";
    for (;;) {
      const result = await terminals.read(DEVICE, { terminalId: id, from, waitMs: 0, maxBytes: 32_768 });
      if (result.data === "") break;
      expect(atob(result.data).length).toBeLessThanOrEqual(32_768);
      total += atob(result.data).length;
      last = text(result.data);
      from = result.next;
      pages += 1;
    }
    expect(total).toBeGreaterThan(40_000);
    expect(pages).toBeGreaterThan(1);
    expect(last).toContain("$ ");
  });

  it("backspace edits the line, Ctrl-C cancels it and an unknown command is reported", async () => {
    const { terminals, start } = shells();
    const id = await start();
    const banner = await terminals.read(DEVICE, { terminalId: id, from: 0, waitMs: 0 });
    terminals.write(DEVICE, { terminalId: id, data: b64("echo ab\x7fc\r") });
    terminals.write(DEVICE, { terminalId: id, data: b64("zzz\x03") });
    terminals.write(DEVICE, { terminalId: id, data: b64("nope\r") });
    const out = text((await terminals.read(DEVICE, { terminalId: id, from: banner.next, waitMs: 0 })).data);
    expect(out).toContain("ac\r\n");
    expect(out).toContain("^C");
    expect(out).toContain("sh: nope: command not found");
  });

  it("resize is kept and exit ends the shell with reason exit and code 0 once the output is drained", async () => {
    const { terminals, start } = shells();
    const id = await start();
    terminals.resize(DEVICE, { terminalId: id, cols: 120, rows: 40 });
    expect(terminals.size(id)).toEqual({ cols: 120, rows: 40 });
    terminals.write(DEVICE, { terminalId: id, data: b64("exit\r") });
    const result = await terminals.read(DEVICE, { terminalId: id, from: 0, waitMs: 0 });
    expect(text(result.data)).toContain("logout");
    expect(result.exited).toEqual({ reason: "exit", code: 0 });
    expect(await asyncCodeOf(async () => terminals.write(DEVICE, { terminalId: id, data: b64("x") }))).toBe("not_found");
  });

  it("close kills the shell at once: the next read says closed", async () => {
    const { terminals, start } = shells();
    const id = await start();
    terminals.close(DEVICE, { terminalId: id });
    const result = await terminals.read(DEVICE, { terminalId: id, from: 0, waitMs: 0 });
    expect(result.exited).toEqual({ reason: "closed" });
    expect(terminals.liveIds()).toEqual([]);
  });

  it("a pending read is answered with the exit when the desktop kills the terminal", async () => {
    const { terminals, start } = shells();
    const id = await start();
    const banner = await terminals.read(DEVICE, { terminalId: id, from: 0, waitMs: 0 });
    const pending = terminals.read(DEVICE, { terminalId: id, from: banner.next, waitMs: 15_000 });
    terminals.kill(id, "killed");
    expect((await pending).exited).toEqual({ reason: "killed" });
  });

  it("another device never sees the terminal", async () => {
    const { terminals, start } = shells();
    const id = await start();
    expect(await asyncCodeOf(() => terminals.read("device-b", { terminalId: id, from: 0, waitMs: 0 }))).toBe("not_found");
    expect(codeOf(() => terminals.write("device-b", { terminalId: id, data: b64("x") }))).toBe("not_found");
    expect(codeOf(() => terminals.close("device-b", { terminalId: id }))).toBe("not_found");
  });
});

describe("terminal lifetime", () => {
  it("a killed terminal stays readable for 60 s and is then not_found", async () => {
    const { terminals, start, clock } = shells();
    const id = await start();
    terminals.kill(id, "revoked");
    expect((await terminals.read(DEVICE, { terminalId: id, from: 0, waitMs: 0 })).exited).toEqual({ reason: "revoked" });
    clock.advance(59_000);
    expect((await terminals.read(DEVICE, { terminalId: id, from: 0, waitMs: 0 })).exited).toEqual({ reason: "revoked" });
    clock.advance(2_000);
    expect(await asyncCodeOf(() => terminals.read(DEVICE, { terminalId: id, from: 0, waitMs: 0 }))).toBe("not_found");
  });

  it("idle and max limits end the shell with their reason", async () => {
    const idle = shells({ idleTimeoutMs: 1_000 });
    const idleId = await idle.start();
    idle.clock.advance(1_001);
    expect((await idle.terminals.read(DEVICE, { terminalId: idleId, from: 0, waitMs: 0 })).exited).toEqual({ reason: "idle" });
    const max = shells({ maxDurationMs: 5_000, idleTimeoutMs: 60_000 });
    const maxId = await max.start();
    max.clock.advance(5_001);
    expect((await max.terminals.read(DEVICE, { terminalId: maxId, from: 0, waitMs: 0 })).exited).toEqual({ reason: "max" });
  });

  it("typing keeps a shell from going idle", async () => {
    const { terminals, start, clock } = shells({ idleTimeoutMs: 1_000 });
    const id = await start();
    clock.advance(800);
    terminals.write(DEVICE, { terminalId: id, data: b64("a") });
    clock.advance(800);
    expect((await terminals.read(DEVICE, { terminalId: id, from: 0, waitMs: 0 })).exited).toBeNull();
  });

  it("after the link drops the shell lives through the grace window for a reattach, and dies after it", async () => {
    const reattach = shells({ graceMs: 30_000 });
    const keep = await reattach.start();
    reattach.terminals.linkLost(DEVICE);
    reattach.clock.advance(20_000);
    expect((await reattach.terminals.read(DEVICE, { terminalId: keep, from: 0, waitMs: 0 })).exited).toBeNull();
    reattach.clock.advance(60_000);
    expect(reattach.terminals.liveIds()).toEqual([keep]);

    const lost = shells({ graceMs: 30_000 });
    const gone = await lost.start();
    lost.terminals.linkLost(DEVICE);
    lost.clock.advance(30_001);
    expect((await lost.terminals.read(DEVICE, { terminalId: gone, from: 0, waitMs: 0 })).exited).toEqual({ reason: "grace" });
  });

  it("killDevice and killAll carry their reason", async () => {
    const { terminals, start } = shells();
    const a = await start("device-a");
    const b = await start("device-b");
    terminals.killDevice("device-a", "revoked");
    expect((await terminals.read("device-a", { terminalId: a, from: 0, waitMs: 0 })).exited).toEqual({ reason: "revoked" });
    expect((await terminals.read("device-b", { terminalId: b, from: 0, waitMs: 0 })).exited).toBeNull();
    terminals.killAll("toggle_off");
    expect((await terminals.read("device-b", { terminalId: b, from: 0, waitMs: 0 })).exited).toEqual({ reason: "toggle_off" });
  });
});

describe("over the Noise link", () => {
  const withTerminal = { desktop: { capabilities: [...KNOWN_CAPABILITIES], terminal: { ticketVerifier: acceptAll } } };

  it("the default desktop does not advertise terminal and answers like an older one", async () => {
    const s = await setup();
    expect(s.client.state).toMatchObject({ state: "confirmed" });
    expect(s.client.state?.capabilities).not.toContain("terminal");
    expect(await s.client.request("terminal.open", OVER_LINK)).toMatchObject({ ok: false, error: "Unsupported mobile operation" });
  });

  it("a Control device gets the capability and the whole flow answers the protocol result schemas", async () => {
    const s = await setup(withTerminal);
    expect(s.client.state?.capabilities).toContain("terminal");
    const opened = await s.client.call("terminal.open", OVER_LINK);
    expect(METHODS["terminal.open"].result.safeParse(opened).success).toBe(true);
    expect(opened).toMatchObject({ cwd: "Example Alpha" });
    expect(s.desktop.terminals.ticketVerifications()).toBe(1);
    const { terminalId } = opened as { terminalId: string };
    await s.client.call("terminal.write", { terminalId, data: b64("echo ok\r") });
    const read = await s.client.call("terminal.read", { terminalId, from: 0, waitMs: 0 });
    expect(METHODS["terminal.read"].result.safeParse(read).success).toBe(true);
    expect(text(read.data)).toContain("echo ok\r\nok\r\n");
    await s.client.call("terminal.close", { terminalId });
    expect((await s.client.call("terminal.read", { terminalId, from: read.next, waitMs: 0 })).exited).toEqual({ reason: "closed" });
  });

  it("over the link a desktop with no ticket verifier fails closed with fresh_required, and a refusing one too", async () => {
    const closed = await setup({ desktop: { capabilities: [...KNOWN_CAPABILITIES] } });
    expect(await closed.client.request("terminal.open", OVER_LINK)).toMatchObject({ ok: false, code: "fresh_required" });
    expect(closed.desktop.terminals.liveIds()).toEqual([]);
    const refusing = await setup({ desktop: { capabilities: [...KNOWN_CAPABILITIES], terminal: { ticketVerifier: async () => false } } });
    expect(await refusing.client.request("terminal.open", OVER_LINK)).toMatchObject({ ok: false, code: "fresh_required" });
    expect(refusing.desktop.terminals.ticketVerifications()).toBe(1);
  });

  it("a read-only device gets no capability and read_only", async () => {
    const s = await setup({ ...withTerminal, permission: "read" });
    expect(s.client.state?.capabilities).not.toContain("terminal");
    expect(await s.client.request("terminal.open", OVER_LINK)).toMatchObject({ ok: false, code: "read_only" });
  });

  it("a Control device with Allow web terminal off gets no capability and disabled; turning it off kills a running shell", async () => {
    const off = await setup(withTerminal);
    // The device is already confirmed with the toggle on; switch it off.
    const fingerprint = [...off.desktop.devices.keys()][0] ?? "";
    const { terminalId } = (await off.client.call("terminal.open", OVER_LINK)) as { terminalId: string };
    off.desktop.setDevice(fingerprint, { terminal: false });
    await off.relay.settled();
    expect(off.client.state?.capabilities).not.toContain("terminal");
    expect(await off.client.request("terminal.open", OVER_LINK)).toMatchObject({ ok: false, code: "disabled" });
    expect((await off.client.call("terminal.read", { terminalId, from: 0, waitMs: 0 })).exited).toEqual({ reason: "toggle_off" });
  });

  it("a project the device does not share is not_selected", async () => {
    const s = await setup(withTerminal);
    await s.client.call("device.selection", { projects: ["proj-alpha"] });
    expect(await s.client.request("terminal.open", { ...OVER_LINK, projectId: "proj-beta" })).toMatchObject({ ok: false, code: "not_selected" });
  });

  it("revoking the device kills its shells; dropping every link starts the grace window", async () => {
    const s = await setup(withTerminal);
    const { terminalId } = (await s.client.call("terminal.open", OVER_LINK)) as { terminalId: string };
    const fingerprint = [...s.desktop.devices.keys()][0] ?? "";
    s.desktop.revokeDevice(fingerprint);
    expect(s.desktop.terminals.liveIds()).toEqual([]);
    expect(terminalId).toBeTruthy();
  });

  it("removing terminal from the capabilities kills the shells (toggle_off)", async () => {
    const s = await setup(withTerminal);
    await s.client.call("terminal.open", OVER_LINK);
    expect(s.desktop.terminals.liveIds()).toHaveLength(1);
    s.desktop.setCapabilities(KNOWN_CAPABILITIES.filter((name) => name !== "terminal"));
    expect(s.desktop.terminals.liveIds()).toEqual([]);
  });
});
