import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { QueryObserver } from "@tanstack/react-query";
import { createQueryClient } from "@/lib/queryClient";
import { catalogKey, conversationKey, desktopKey } from "./queries.ts";
import { CATALOG_POLL_MS, acquireLinkDataSync, startLinkDataSync } from "./sync.ts";
import { openConversation } from "./conversation.ts";
import { MemoryPersistAdapter, loadPersistedCatalog, loadPersistedTurns, persistTurns, persistCatalog, registerPersistAdapter, type PersistAdapter } from "./persist.ts";
import { FakeClient, flush, requestError } from "./testing.ts";
import { linkQueryOptions } from "./queries.ts";

let client: FakeClient;
let queryClient: ReturnType<typeof createQueryClient>;
let hidden = false;
let previous: PersistAdapter;
let stop: (() => void) | null = null;
const catalog = { projects: [], sessions: [] };

const listCalls = () => client.request.mock.calls.filter((call) => call[0] === "workspace.list").length;

beforeEach(() => {
  vi.useFakeTimers();
  hidden = false;
  Object.defineProperty(document, "hidden", { configurable: true, get: () => hidden });
  client = new FakeClient();
  queryClient = createQueryClient();
  previous = registerPersistAdapter(new MemoryPersistAdapter());
  client.request.mockImplementation(async (method: string) => (method === "workspace.list" ? catalog : { turns: [], older: false }));
});
afterEach(() => {
  stop?.();
  stop = null;
  registerPersistAdapter(previous);
  vi.useRealTimers();
  delete (document as { hidden?: boolean }).hidden;
});

function visibility(next: boolean): void {
  hidden = next;
  document.dispatchEvent(new Event("visibilitychange"));
}

describe("catalog polling", () => {
  it("refetches workspace.list every 5 s while visible and ready, and fetches at once when ready", async () => {
    stop = startLinkDataSync(client.asClient(), queryClient);
    await vi.advanceTimersByTimeAsync(0);
    expect(listCalls()).toBe(1);
    await vi.advanceTimersByTimeAsync(CATALOG_POLL_MS);
    expect(listCalls()).toBe(2);
    await vi.advanceTimersByTimeAsync(CATALOG_POLL_MS * 2);
    expect(listCalls()).toBe(4);
    expect(queryClient.getQueryData(catalogKey("dk1"))).toEqual(catalog);
  });

  it("does not poll while hidden, and fetches on visibilitychange to visible", async () => {
    stop = startLinkDataSync(client.asClient(), queryClient);
    await vi.advanceTimersByTimeAsync(0);
    visibility(true);
    const before = listCalls();
    await vi.advanceTimersByTimeAsync(CATALOG_POLL_MS * 3);
    expect(listCalls()).toBe(before);
    visibility(false);
    await vi.advanceTimersByTimeAsync(0);
    expect(listCalls()).toBe(before + 1);
  });

  it("does not poll while the link is not ready, and fetches when it becomes ready", async () => {
    client.setStatus("reconnecting");
    stop = startLinkDataSync(client.asClient(), queryClient);
    await vi.advanceTimersByTimeAsync(CATALOG_POLL_MS * 3);
    expect(listCalls()).toBe(0);
    client.setStatus("ready");
    await vi.advanceTimersByTimeAsync(0);
    expect(listCalls()).toBe(1);
  });

  it("stops polling when stopped", async () => {
    stop = startLinkDataSync(client.asClient(), queryClient);
    await vi.advanceTimersByTimeAsync(0);
    stop();
    stop = null;
    const before = listCalls();
    await vi.advanceTimersByTimeAsync(CATALOG_POLL_MS * 3);
    expect(listCalls()).toBe(before);
  });

  it("ref counted acquire shares one sync", async () => {
    const releaseA = acquireLinkDataSync(client.asClient(), queryClient);
    const releaseB = acquireLinkDataSync(client.asClient(), queryClient);
    await vi.advanceTimersByTimeAsync(0);
    expect(listCalls()).toBe(1);
    releaseA();
    await vi.advanceTimersByTimeAsync(CATALOG_POLL_MS);
    expect(listCalls()).toBe(2);
    releaseB();
    await vi.advanceTimersByTimeAsync(CATALOG_POLL_MS * 2);
    expect(listCalls()).toBe(2);
  });
});

describe("resync", () => {
  it("on resynced the catalog is invalidated and refetched and the open conversation is read again", async () => {
    stop = startLinkDataSync(client.asClient(), queryClient);
    const handle = openConversation(queryClient, client.asClient(), "c1");
    await vi.advanceTimersByTimeAsync(0);
    const lists = listCalls();
    const reads = () => client.request.mock.calls.filter((call) => call[0] === "conversation.read").length;
    expect(reads()).toBe(0);
    client.reconnect();
    await vi.advanceTimersByTimeAsync(0);
    expect(listCalls()).toBe(lists + 1);
    expect(reads()).toBe(1);
    expect(client.request.mock.calls.find((call) => call[0] === "conversation.read")?.[1]).toEqual({ conversationId: "c1" });
    expect(queryClient.getQueryState(catalogKey("dk1"))?.isInvalidated).toBe(false);
    handle.close();
    client.reconnect();
    await vi.advanceTimersByTimeAsync(0);
    expect(reads()).toBe(1);
  });
});

describe("persistence and revoked", () => {
  it("writes the catalog through the adapter and shows it again from there", async () => {
    stop = startLinkDataSync(client.asClient(), queryClient);
    await vi.advanceTimersByTimeAsync(0);
    await flush();
    expect(await loadPersistedCatalog("dk1")).toEqual(catalog);

    // A fresh client (a reload) while the desktop is unreachable still has the catalog.
    stop();
    const offline = new FakeClient();
    offline.setStatus("closed");
    const fresh = createQueryClient();
    stop = startLinkDataSync(offline.asClient(), fresh);
    await vi.advanceTimersByTimeAsync(0);
    await flush();
    expect(fresh.getQueryData(catalogKey("dk1"))).toEqual(catalog);
  });

  it("clears the persisted cache and the desktop queries when the link becomes revoked", async () => {
    await persistCatalog("dk1", catalog);
    stop = startLinkDataSync(client.asClient(), queryClient);
    await vi.advanceTimersByTimeAsync(0);
    await flush();
    queryClient.setQueryData(conversationKey("dk1", "c1"), { turns: [] });
    queryClient.setQueryData(["me"], { id: "u" });
    client.store.setState({ revoked: true, blocked: "revoked", status: "closed" });
    await flush();
    expect(await loadPersistedCatalog("dk1")).toBeUndefined();
    expect(queryClient.getQueryCache().findAll({ queryKey: desktopKey("dk1") })).toHaveLength(0);
    expect(queryClient.getQueryData(["me"])).toEqual({ id: "u" });
  });

  it("an already revoked link clears at start", async () => {
    await persistCatalog("dk1", catalog);
    client.store.setState({ revoked: true });
    stop = startLinkDataSync(client.asClient(), queryClient);
    await flush();
    expect(await loadPersistedCatalog("dk1")).toBeUndefined();
  });
});

describe("review fixes", () => {
  it("a revoke wipes only the persisted data of that desktop (M5)", async () => {
    await persistCatalog("dk1", catalog);
    await persistCatalog("dk2", catalog);
    await persistTurns("dk2", "s1", [{ id: "t", items: [] }]);
    stop = startLinkDataSync(client.asClient(), queryClient);
    client.store.setState({ revoked: true });
    await flush();
    expect(await loadPersistedCatalog("dk1")).toBeUndefined();
    expect(await loadPersistedCatalog("dk2")).toEqual(catalog);
    expect(await loadPersistedTurns("dk2", "s1")).toHaveLength(1);
  });

  it("a link query that failed with not_ready is fetched when the link becomes ready (I3)", async () => {
    client.setStatus("connecting");
    client.request.mockImplementation(async (method: string) => {
      if (client.store.getState().status !== "ready") throw requestError("not_ready");
      return method === "workspace.list" ? catalog : { sessions: [] };
    });
    stop = startLinkDataSync(client.asClient(), queryClient);
    const options = linkQueryOptions("dk1", "sessions.list", { projectId: "p1" }, { connection: client.asClient() });
    const observer = new QueryObserver(queryClient, options as never);
    const unsubscribe = observer.subscribe(() => undefined);
    await vi.advanceTimersByTimeAsync(0);
    expect(observer.getCurrentResult().status).toBe("error");
    client.setStatus("ready");
    await vi.advanceTimersByTimeAsync(0);
    expect(observer.getCurrentResult().status).toBe("success");
    unsubscribe();
  });

  it("after a resync the active catalog queries are refetched (M11)", async () => {
    stop = startLinkDataSync(client.asClient(), queryClient);
    const options = linkQueryOptions("dk1", "sessions.list", { projectId: "p1" }, { connection: client.asClient() });
    const unsubscribe = new QueryObserver(queryClient, options as never).subscribe(() => undefined);
    await vi.advanceTimersByTimeAsync(0);
    const calls = () => client.request.mock.calls.filter((call) => call[0] === "sessions.list").length;
    expect(calls()).toBe(1);
    client.reconnect();
    await vi.advanceTimersByTimeAsync(0);
    expect(calls()).toBe(2);
    unsubscribe();
  });
});
