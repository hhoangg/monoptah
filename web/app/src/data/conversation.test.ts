import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Turn } from "@monoptah/protocol";
import { createQueryClient } from "@/lib/queryClient";
import { catalogKey, conversationKey } from "./queries.ts";
import { emptyConversation, type ConversationState } from "./conversationState.ts";
import { ConversationWatcher, READ_POLL_MS, WATCH_TAIL, openConversation, openConversationWatchers } from "./conversation.ts";
import { MemoryPersistAdapter, loadPersistedTurns, registerPersistAdapter, type PersistAdapter } from "./persist.ts";
import { FakeClient, flush, requestError } from "./testing.ts";

const text = (id: string, value: string): Turn => ({ id, items: [{ type: "text", id: `${id}:a`, text: value }] });
const user = (id: string, value: string): Turn => ({ id, items: [{ type: "user", id: `${id}:u`, text: value }] });

let client: FakeClient;
let queryClient: ReturnType<typeof createQueryClient>;
let previous: PersistAdapter;
const key = () => conversationKey("dk1", "c1");
const state = () => queryClient.getQueryData<ConversationState>(key())!;

beforeEach(() => {
  client = new FakeClient();
  queryClient = createQueryClient();
  previous = registerPersistAdapter(new MemoryPersistAdapter());
});
afterEach(() => {
  registerPersistAdapter(previous);
});

function watcher(): ConversationWatcher {
  const w = new ConversationWatcher({ client: client.asClient(), queryClient, conversationId: "c1" });
  w.start();
  return w;
}

describe("watch", () => {
  it("registers conversation.watch with incremental and the tail, and writes frames to the conversation key", async () => {
    watcher();
    expect(client.live.registration.method).toBe("conversation.watch");
    expect(client.live.openedWith[0]).toEqual({ conversationId: "c1", tail: WATCH_TAIL, incremental: true });
    client.frame({ busy: true, older: true, order: ["a", "b"], turns: [text("a", "one"), text("b", "two")] });
    expect(state().turns.map((turn) => turn.id)).toEqual(["a", "b"]);
    expect(state()).toMatchObject({ busy: true, older: true, error: null });
    client.frame({ order: ["a", "b"], turns: [], patches: [{ id: "b", edits: [{ path: ["items", 0, "text"], from: 3, append: "!" }] }] });
    expect(JSON.stringify(state().turns[1])).toContain("two!");
  });

  it("a null apply closes the watch and reopens it with from set to the first held turn", async () => {
    watcher();
    client.frame({ order: ["a", "b"], turns: [text("a", "one"), text("b", "two")] });
    const first = client.live;
    // A patch for a turn the client never got cannot be applied.
    client.frame({ order: ["a", "b", "c"], turns: [], patches: [{ id: "c", edits: [] }] });
    await flush();
    expect(first.unregistered).toBe(true);
    expect(client.live).not.toBe(first);
    expect(client.live.openedWith[0]).toEqual({ conversationId: "c1", tail: WATCH_TAIL, incremental: true, from: "a" });
    // Late frames of the closed watch are ignored.
    first.registration.onFrame({ order: ["z"], turns: [text("z", "stale")] } as never);
    expect(state().turns.map((turn) => turn.id)).toEqual(["a", "b"]);
  });

  it("a second null in a row opens on the latest turns again (no from)", async () => {
    watcher();
    client.frame({ order: ["a"], turns: [text("a", "one")] });
    client.frame({ order: ["a", "x"], turns: [] });
    await flush();
    client.frame({ order: ["a", "x"], turns: [] });
    await flush();
    expect(client.live.openedWith[0]).toEqual({ conversationId: "c1", tail: WATCH_TAIL, incremental: true });
  });

  it("after a reconnect the connection asks reopenParams again and it carries from", () => {
    watcher();
    client.frame({ order: ["a", "b"], turns: [text("a", "one"), text("b", "two")] });
    client.reconnect();
    expect(client.live.openedWith).toHaveLength(2);
    expect(client.live.openedWith[1]).toMatchObject({ from: "a", incremental: true });
    // The reopened watch sends every turn again; unchanged turns keep their identity.
    const held = state().turns[0];
    client.frame({ order: ["a", "b", "c"], turns: [text("a", "one"), text("b", "two"), text("c", "three")] });
    expect(state().turns).toHaveLength(3);
    expect(held).toBeDefined();
  });

  it("an error from the desktop is shown until the next good frame", () => {
    watcher();
    client.live.registration.onError?.(requestError("not_found"));
    expect(state().error).toBe("not_found");
    client.frame({ order: [], turns: [] });
    expect(state().error).toBeNull();
  });

  it("shows persisted turns first, marked cachedOnly, and opens without from", async () => {
    await registerAndSeed();
    watcher();
    await flush();
    expect(state()).toMatchObject({ cachedOnly: true });
    expect(state().turns.map((turn) => turn.id)).toEqual(["p1"]);
    expect(client.live.openedWith[0]).not.toHaveProperty("from");
    client.frame({ order: ["a"], turns: [text("a", "fresh")] });
    expect(state()).toMatchObject({ cachedOnly: false });
    expect(state().turns.map((turn) => turn.id)).toEqual(["a"]);
  });

  it("stop unregisters and saves the last turns", async () => {
    const w = watcher();
    client.frame({ order: ["a"], turns: [text("a", "one")] });
    const live = client.live;
    w.stop();
    expect(live.unregistered).toBe(true);
    await flush();
    expect(await loadPersistedTurns("dk1", "c1")).toEqual([text("a", "one")]);
  });

  it("the registry shares one watcher and unregisters on the last close", () => {
    const one = openConversation(queryClient, client.asClient(), "c1");
    const two = openConversation(queryClient, client.asClient(), "c1");
    expect(one.watcher).toBe(two.watcher);
    expect(client.watches).toHaveLength(1);
    expect(openConversationWatchers(queryClient, "dk1")).toHaveLength(1);
    one.close();
    expect(client.watches[0]?.unregistered).toBe(false);
    two.close();
    expect(client.watches[0]?.unregistered).toBe(true);
    expect(openConversationWatchers(queryClient, "dk1")).toHaveLength(0);
  });
});

async function registerAndSeed(): Promise<void> {
  const { persistTurns } = await import("./persist.ts");
  await persistTurns("dk1", "c1", [text("p1", "old")]);
  expect(await loadPersistedTurns("dk1", "c1")).toHaveLength(1);
}

describe("optimistic turns", () => {
  it("inserts an optimistic user turn, then drops it when the desktop turn arrives", async () => {
    const w = watcher();
    client.frame({ order: ["a"], turns: [text("a", "hello")] });
    const sending = w.send("hi there");
    expect(state().optimistic).toMatchObject([{ id: "pending:1", text: "hi there", status: "sending", baseline: 0 }]);
    expect(await sending).toEqual({ sent: true });
    expect(state().optimistic[0]?.status).toBe("sent");
    client.frame({ order: ["a", "u1"], turns: [user("u1", "hi there")] });
    expect(state().optimistic).toEqual([]);
  });

  it("a failed send is marked not_sent and keeps the text", async () => {
    const w = watcher();
    client.request.mockRejectedValueOnce(requestError("session_ended"));
    expect(await w.send("lost")).toEqual({ sent: false, error: "session_ended" });
    expect(state().optimistic).toMatchObject([{ text: "lost", status: "not_sent", error: "session_ended" }]);
  });

  it("is never re-sent after a reconnect; only retry() re-sends, with the same command id", async () => {
    const w = watcher();
    client.request.mockRejectedValueOnce(requestError("session_ended"));
    await w.send("lost");
    const first = client.request.mock.calls.filter((call) => call[0] === "conversation.send");
    expect(first).toHaveLength(1);
    client.reconnect();
    client.setStatus("ready");
    await flush();
    expect(client.request.mock.calls.filter((call) => call[0] === "conversation.send")).toHaveLength(1);
    expect(state().optimistic[0]?.status).toBe("not_sent");
    expect(await w.retry("pending:1")).toEqual({ sent: true });
    const sends = client.request.mock.calls.filter((call) => call[0] === "conversation.send");
    expect(sends).toHaveLength(2);
    expect((sends[1]?.[2] as { id: string }).id).toBe((sends[0]?.[2] as { id: string }).id);
    expect(state().optimistic[0]?.status).toBe("sent");
  });

  it("the read after a reconnect reconciles a send whose reply was lost", async () => {
    const w = watcher();
    client.request.mockRejectedValueOnce(requestError("session_ended"));
    await w.send("arrived anyway");
    expect(state().optimistic[0]?.status).toBe("not_sent");
    client.request.mockResolvedValueOnce({ turns: [user("u1", "arrived anyway")], older: false, busy: false });
    await w.read();
    expect(state().optimistic).toEqual([]);
  });

  it("a later message landing does not drop a turn marked not_sent (I2)", async () => {
    const w = watcher();
    client.request.mockRejectedValueOnce(requestError("busy"));
    await w.send("A");
    expect(state().optimistic[0]).toMatchObject({ id: "pending:1", status: "not_sent", error: "busy" });
    expect(await w.send("B")).toEqual({ sent: true });
    client.frame({ order: ["u1"], turns: [user("u1", "B")] });
    expect(state().optimistic).toMatchObject([{ id: "pending:1", text: "A", status: "not_sent" }]);
    // A desktop message typed elsewhere does not drop it either.
    client.frame({ order: ["u1", "u2"], turns: [user("u1", "B"), user("u2", "typed on the desktop")] });
    expect(state().optimistic).toMatchObject([{ id: "pending:1", text: "A", status: "not_sent" }]);
    // The user retries it and it lands.
    expect(await w.retry("pending:1")).toEqual({ sent: true });
    client.frame({ order: ["u1", "u2", "u3"], turns: [user("u1", "B"), user("u2", "typed on the desktop"), user("u3", "A")] });
    expect(state().optimistic).toEqual([]);
  });

  it("an ambiguous not_sent turn is dropped only by a user item with its own text", async () => {
    const w = watcher();
    client.request.mockRejectedValueOnce(requestError("timeout"));
    await w.send("maybe arrived");
    client.frame({ order: ["u1"], turns: [user("u1", "something else")] });
    expect(state().optimistic).toHaveLength(1);
    client.frame({ order: ["u1", "u2"], turns: [user("u1", "something else"), user("u2", "maybe arrived")] });
    expect(state().optimistic).toEqual([]);
  });

  it("stop writes nothing for a revoked desktop (L11)", async () => {
    const adapter = new MemoryPersistAdapter();
    registerPersistAdapter(adapter);
    const w = watcher();
    client.frame({ order: ["a"], turns: [text("a", "one")] });
    client.store.setState({ revoked: true });
    w.stop();
    await flush();
    expect(await loadPersistedTurns("dk1", "c1")).toBeUndefined();
  });

  it("retry only works for a not_sent turn", async () => {
    const w = watcher();
    expect(await w.retry("pending:9")).toEqual({ sent: false, error: "nothing_to_retry" });
    await w.send("ok");
    expect(await w.retry("pending:1")).toEqual({ sent: false, error: "nothing_to_retry" });
  });
});

describe("cloud and approvals", () => {
  const cloud = { kind: "codespace" as const, instance: "fake-codespace-one", state: "available" as const };
  const approval = { requestId: "41", label: "Run example command", detail: "example-cli --check (fake detail)" };

  it("keeps them from frames, leaves them when absent, and clears approvals with []", () => {
    watcher();
    client.frame({ order: [], turns: [], cloud, approvals: [approval] });
    expect(state()).toMatchObject({ cloud, approvals: [approval] });
    client.frame({ order: [], turns: [], busy: true });
    expect(state()).toMatchObject({ cloud, approvals: [approval], busy: true });
    client.frame({ approvals: [] });
    expect(state().approvals).toEqual([]);
    expect(state().cloud).toEqual(cloud);
  });

  it("takes them from conversation.read, which replaces the fields", async () => {
    const w = watcher();
    client.request.mockResolvedValueOnce({ turns: [], older: false, cloud, approvals: [approval] });
    await w.read();
    expect(state()).toMatchObject({ cloud, approvals: [approval] });
    client.request.mockResolvedValueOnce({ turns: [], older: false });
    await w.read();
    expect(state().cloud).toBeUndefined();
    expect(state().approvals).toEqual([]);
  });

  it("emptyConversation has no cloud and no approvals", () => {
    expect(emptyConversation()).toMatchObject({ cloud: undefined, approvals: [] });
  });
});

const sendCalls = () => client.request.mock.calls.filter((call) => call[0] === "conversation.send");

describe("wake consent (architecture.md 6.6)", () => {
  it("never sends wake unless the caller passes it, and a refused send leaves no Not sent turn behind", async () => {
    const w = watcher();
    client.request.mockRejectedValueOnce(requestError("wake_required"));
    expect(await w.send("hello")).toEqual({ sent: false, error: "wake_required" });
    expect(sendCalls()[0]?.[1]).toEqual({ conversationId: "c1", text: "hello" });
    // The caller still holds the text: nothing is left in the transcript or retry list.
    expect(state().optimistic).toEqual([]);
  });

  it("sends wake: true once when the caller passes it (a new command id, as the signature differs)", async () => {
    const w = watcher();
    client.request.mockRejectedValueOnce(requestError("wake_required"));
    await w.send("hello");
    expect(await w.send("hello", undefined, { wake: true })).toEqual({ sent: true });
    expect(sendCalls()).toHaveLength(2);
    expect(sendCalls()[1]?.[1]).toEqual({ conversationId: "c1", text: "hello", wake: true });
    expect((sendCalls()[1]?.[2] as { id: string }).id).not.toBe((sendCalls()[0]?.[2] as { id: string }).id);
    expect(state().optimistic).toMatchObject([{ text: "hello", status: "sent" }]);
  });

  it("a wake_required answer to a retry keeps the Not sent turn (its text lives only there); retry with wake sends it", async () => {
    const w = watcher();
    client.request.mockRejectedValueOnce(requestError("session_ended"));
    await w.send("lost");
    client.request.mockRejectedValueOnce(requestError("wake_required"));
    expect(await w.retry("pending:1")).toEqual({ sent: false, error: "wake_required" });
    expect(state().optimistic).toMatchObject([{ text: "lost", status: "not_sent", error: "wake_required" }]);
    expect(await w.retry("pending:1", { wake: true })).toEqual({ sent: true });
    expect(sendCalls().map((call) => (call[1] as { wake?: true }).wake)).toEqual([undefined, undefined, true]);
  });
});

describe("a session that is local again", () => {
  const cloud = { kind: "sandbox" as const, instance: "fake-sandbox-one", state: "stopped" as const };
  const card = { id: "c1", projectId: "p1", title: "T", harness: "claude", model: "m", branch: "main", updatedAt: 1, target: { kind: "sandbox", instance: "fake-sandbox-one", state: "stopped" }, cloudState: "stopped", startup: { step: "create" } };
  const seedCatalog = () => queryClient.setQueryData(catalogKey("dk1"), { projects: [], sessions: [card, { ...card, id: "other" }] });
  const cards = () => (queryClient.getQueryData(catalogKey("dk1")) as { sessions: Array<Record<string, unknown>> }).sessions;

  it("a read without cloud clears the cloud field and the card's target, state and startup", async () => {
    const w = watcher();
    seedCatalog();
    client.frame({ order: [], turns: [], cloud });
    client.request.mockResolvedValueOnce({ turns: [], older: false });
    await w.read();
    expect(state().cloud).toBeUndefined();
    expect(cards().find((c) => c.id === "c1")).toMatchObject({ target: { kind: "local" } });
    expect(cards().find((c) => c.id === "c1")).not.toHaveProperty("cloudState");
    expect(cards().find((c) => c.id === "c1")).not.toHaveProperty("startup");
    // Another session's card is untouched.
    expect(cards().find((c) => c.id === "other")).toMatchObject({ cloudState: "stopped", startup: { step: "create" } });
  });

  it("a frame with cloud: null clears it, a frame without the field leaves it", () => {
    watcher();
    seedCatalog();
    client.frame({ order: [], turns: [], cloud });
    client.frame({ busy: true });
    expect(state().cloud).toEqual(cloud);
    expect(cards().find((c) => c.id === "c1")).toMatchObject({ cloudState: "stopped" });
    client.frame({ cloud: null } as never);
    expect(state().cloud).toBeUndefined();
    expect(cards().find((c) => c.id === "c1")).toMatchObject({ target: { kind: "local" } });
    expect(cards().find((c) => c.id === "c1")).not.toHaveProperty("cloudState");
  });
});

describe("read poll while the watch is down (F-32)", () => {
  afterEach(() => vi.useRealTimers());
  const reads = () => client.request.mock.calls.filter((call) => call[0] === "conversation.read");

  it("reads every second after the watch ended with an error and stops at the next frame", async () => {
    vi.useFakeTimers();
    watcher();
    expect(reads()).toHaveLength(0);
    client.live.registration.onError?.(requestError("not_found"));
    client.request.mockResolvedValue({ turns: [text("a", "polled")], older: false });
    await vi.advanceTimersByTimeAsync(READ_POLL_MS * 3);
    expect(reads()).toHaveLength(3);
    expect(state().turns.map((turn) => turn.id)).toEqual(["a"]);
    client.frame({ order: ["a"], turns: [text("a", "polled")] });
    await vi.advanceTimersByTimeAsync(READ_POLL_MS * 3);
    expect(reads()).toHaveLength(3);
  });

  it("does not read while the link is not ready or the watcher is stopped, and never overlaps reads", async () => {
    vi.useFakeTimers();
    const w = watcher();
    client.live.registration.onDead?.();
    client.setStatus("reconnecting");
    await vi.advanceTimersByTimeAsync(READ_POLL_MS * 2);
    expect(reads()).toHaveLength(0);
    client.setStatus("ready");
    let release!: () => void;
    client.request.mockImplementation(() => new Promise((resolve) => (release = () => resolve({ turns: [], older: false }))));
    await vi.advanceTimersByTimeAsync(READ_POLL_MS * 3);
    expect(reads()).toHaveLength(1);
    release();
    await vi.advanceTimersByTimeAsync(0);
    w.stop();
    client.request.mockClear();
    await vi.advanceTimersByTimeAsync(READ_POLL_MS * 3);
    expect(reads()).toHaveLength(0);
  });
});
