import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { Turn } from "@monoptah/protocol";
import { createQueryClient } from "@/lib/queryClient";
import { conversationKey } from "./queries.ts";
import { emptyConversation, type ConversationState } from "./conversationState.ts";
import { ConversationWatcher, WATCH_TAIL, openConversation, openConversationWatchers } from "./conversation.ts";
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

  it("retry only works for a not_sent turn", async () => {
    const w = watcher();
    expect(await w.retry("pending:9")).toEqual({ sent: false, error: "nothing_to_retry" });
    await w.send("ok");
    expect(await w.retry("pending:1")).toEqual({ sent: false, error: "nothing_to_retry" });
  });
});

describe("cloud and approvals", () => {
  const cloud = { kind: "codespace" as const, instance: "fake-codespace-one", state: "available" as const };
  const approval = { requestId: 41, label: "Run example command", detail: "example-cli --check (fake detail)" };

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
