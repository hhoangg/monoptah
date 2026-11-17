import "fake-indexeddb/auto";
import { IDBFactory } from "fake-indexeddb";
import { QueryClient } from "@tanstack/react-query";
import { isRedirect } from "@tanstack/react-router";
import type { ParsedLocation } from "@tanstack/react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { registerPersistAdapter, MemoryPersistAdapter } from "@/data";
import { ApiError, setApiTransport } from "@/features/accounts/api";
import { authGuard } from "@/features/accounts/authGuard";
import { createMockServer } from "@/features/accounts/mock/transport";
import { clearAccountCache, desktopsQuery, meQuery } from "@/features/accounts/queries";
import { restoreAccountSnapshots, startAccountPersistence } from "./accountCache";
import { EncryptedPersistAdapter } from "./encryptedPersist";
import { resetPersistentStorageState } from "./storage";

let restoreAdapter: () => void;
let restoreTransport: (() => void) | undefined;
beforeEach(() => {
  globalThis.indexedDB = new IDBFactory();
  resetPersistentStorageState();
  const previous = registerPersistAdapter(new EncryptedPersistAdapter());
  restoreAdapter = () => registerPersistAdapter(previous);
});
afterEach(() => {
  restoreAdapter();
  restoreTransport?.();
  restoreTransport = undefined;
  Reflect.deleteProperty(navigator, "storage");
});

const location = (pathname: string) => ({ pathname, searchStr: "", hash: "", href: pathname }) as unknown as ParsedLocation;
const offlineTransport = () => Promise.reject(new TypeError("Failed to fetch"));

async function signedInSession(): Promise<void> {
  restoreTransport = setApiTransport(createMockServer({ signedIn: true }).transport);
  const client = new QueryClient();
  const stop = startAccountPersistence(client);
  await client.fetchQuery(meQuery);
  await client.fetchQuery(desktopsQuery);
  stop();
  await new Promise((resolve) => setTimeout(resolve, 50));
  restoreTransport();
  restoreTransport = undefined;
}

describe("offline start (F-05)", () => {
  it("lets the guard pass on cached data when the network is down", async () => {
    await signedInSession();
    restoreTransport = setApiTransport(offlineTransport);
    const client = new QueryClient();
    await expect(authGuard({ location: location("/desktops"), context: { queryClient: client } })).resolves.toBeUndefined();
    expect(client.getQueryData(meQuery.queryKey)).toBeDefined();
    expect(client.getQueryData(desktopsQuery.queryKey)).toBeDefined();
  });

  it("throws the network error when nothing was saved", async () => {
    restoreTransport = setApiTransport(offlineTransport);
    const error = await Promise.resolve(authGuard({ location: location("/desktops"), context: { queryClient: new QueryClient() } })).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(ApiError);
    expect((error as ApiError).code).toBe("network");
  });

  it("never uses a snapshot for a 401, and a 401 wipes it", async () => {
    await signedInSession();
    restoreTransport = setApiTransport(createMockServer({ signedIn: false }).transport);
    const error = await Promise.resolve(authGuard({ location: location("/desktops"), context: { queryClient: new QueryClient() } })).catch((e: unknown) => e);
    expect(isRedirect(error)).toBe(true);
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(await restoreAccountSnapshots(new QueryClient())).toBeUndefined();
  });

  it("drops the snapshots when the account cache is cleared (sign-out)", async () => {
    await signedInSession();
    clearAccountCache(new QueryClient());
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(await restoreAccountSnapshots(new QueryClient())).toBeUndefined();
  });

  it("refuses an unreadable snapshot", async () => {
    const memory = new MemoryPersistAdapter();
    await memory.set("account:me", { nonsense: true });
    registerPersistAdapter(memory);
    expect(await restoreAccountSnapshots(new QueryClient())).toBeUndefined();
  });

  it("asks for persistent storage once, after the first successful sign-in answer", async () => {
    const persist = vi.fn().mockResolvedValue(true);
    Object.defineProperty(navigator, "storage", { value: { persist }, configurable: true });
    restoreTransport = setApiTransport(createMockServer({ signedIn: true }).transport);
    const client = new QueryClient();
    const stop = startAccountPersistence(client);
    expect(persist).not.toHaveBeenCalled();
    await client.fetchQuery(meQuery);
    await client.fetchQuery({ ...meQuery, staleTime: 0 });
    stop();
    expect(persist).toHaveBeenCalledTimes(1);
  });
});
