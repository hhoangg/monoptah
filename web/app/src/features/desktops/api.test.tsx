import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { renderHook, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ApiError, setApiTransport } from "@/features/accounts/api";
import type * as KeysModule from "@/link/keys";
import type * as PinsModule from "@/link/pins";
import type * as PersistModule from "@/data/persist";
import type * as LinkModule from "@/link";

const webauthn = vi.hoisted(() => ({ createPasskey: vi.fn(), getPasskey: vi.fn() }));
vi.mock("@/features/accounts/webauthn", () => webauthn);

const keys = vi.hoisted(() => ({ getDeviceKeyInfo: vi.fn(), forgetDeviceKey: vi.fn() }));
const pins = vi.hoisted(() => ({ forgetPins: vi.fn() }));
vi.mock("@/link/keys", async (importOriginal) => ({ ...(await importOriginal<typeof KeysModule>()), ...keys }));
vi.mock("@/link/pins", async (importOriginal) => ({ ...(await importOriginal<typeof PinsModule>()), ...pins }));
const side = vi.hoisted(() => ({ clearPersistedCache: vi.fn(), resetLinkConnections: vi.fn(), stop: vi.fn() }));
vi.mock("@/data/persist", async (importOriginal) => ({ ...(await importOriginal<typeof PersistModule>()), clearPersistedCache: side.clearPersistedCache }));
vi.mock("@/link", async (importOriginal) => ({
  ...(await importOriginal<typeof LinkModule>()),
  resetLinkConnections: side.resetLinkConnections,
  getLinkConnection: () => ({ stop: side.stop }),
}));

import {
  desktopKeys,
  forgetDevice,
  renameDesktop,
  requestGrant,
  requestSelectionTicket,
  revokeDesktop,
  revokeGrant,
  setDesktopColor,
  setRequestedPermission,
  useDesktop,
  useDesktopGrants,
  useDesktops,
  useDevices,
} from "./api";
import { onForgetDesktop } from "@/features/settings/desktopLink";
import { createFakeServer, desktop, device, grant, type FakeServer } from "./testServer";

const ASSERTION = { status: "ok", response: { id: "cred", rawId: "cred", type: "public-key", response: {}, clientExtensionResults: {} } };
let queryClient: QueryClient;
let server: FakeServer;
let restore: (() => void) | undefined;

function install(options: Parameters<typeof createFakeServer>[0] = {}) {
  server = createFakeServer({ desktops: [desktop(), desktop({ id: "d2", name: "Linux box", online: false, pendingCount: 2 })], devices: [device()], grants: { d1: [grant()] }, ...options });
  restore = setApiTransport(server.transport);
}

function wrapper({ children }: { children: ReactNode }) {
  return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>;
}

beforeEach(() => {
  queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  webauthn.getPasskey.mockResolvedValue(ASSERTION);
  keys.getDeviceKeyInfo.mockResolvedValue(null);
  keys.forgetDeviceKey.mockResolvedValue(undefined);
  pins.forgetPins.mockResolvedValue(undefined);
  side.clearPersistedCache.mockResolvedValue(undefined);
  install();
});
afterEach(() => {
  restore?.();
  vi.clearAllMocks();
  vi.useRealTimers();
});

describe("query hooks", () => {
  it("useDesktops, useDesktop, useDesktopGrants and useDevices read the routes", async () => {
    const all = renderHook(() => useDesktops(), { wrapper });
    await waitFor(() => expect(all.result.current.data?.desktops).toHaveLength(2));
    const one = renderHook(() => useDesktop("d1"), { wrapper });
    await waitFor(() => expect(one.result.current.data?.fingerprint).toBeTruthy());
    const grants = renderHook(() => useDesktopGrants("d1"), { wrapper });
    await waitFor(() => expect(grants.result.current.data?.grants[0]?.deviceId).toBe("dev1"));
    const devices = renderHook(() => useDevices(), { wrapper });
    await waitFor(() => expect(devices.result.current.data?.devices[0]?.id).toBe("dev1"));
  });

  it("refetches the list every 15 s while the page is visible", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval", "setTimeout", "clearTimeout", "Date"] });
    renderHook(() => useDesktops(), { wrapper });
    await vi.advanceTimersByTimeAsync(10);
    const initial = server.calls.filter((call) => call.path === "/api/desktops").length;
    expect(initial).toBe(1);
    server.desktops[0]!.online = false;
    await vi.advanceTimersByTimeAsync(15_100);
    expect(server.calls.filter((call) => call.path === "/api/desktops").length).toBe(initial + 1);
    expect(queryClient.getQueryData<{ desktops: Array<{ online: boolean }> }>(desktopKeys.list)?.desktops[0]?.online).toBe(false);
  });

  it("does not refetch while the document is hidden", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval", "setTimeout", "clearTimeout", "Date"] });
    const visibility = vi.spyOn(document, "visibilityState", "get").mockReturnValue("hidden");
    renderHook(() => useDesktops(), { wrapper });
    await vi.advanceTimersByTimeAsync(10);
    await vi.advanceTimersByTimeAsync(60_000);
    expect(server.calls.filter((call) => call.path === "/api/desktops")).toHaveLength(1);
    visibility.mockRestore();
  });
});

describe("mutations", () => {
  it("renameDesktop and setDesktopColor send PATCH bodies and refresh the list", async () => {
    await queryClient.prefetchQuery({ queryKey: desktopKeys.list, queryFn: () => ({ stale: true }) });
    await renameDesktop(queryClient, "d1", "Office");
    await setDesktopColor(queryClient, "d1", "#ea603e");
    expect(server.calls.filter((call) => call.method === "PATCH")).toEqual([
      { method: "PATCH", path: "/api/desktops/d1", body: { name: "Office" } },
      { method: "PATCH", path: "/api/desktops/d1", body: { color: "#ea603e" } },
    ]);
    expect(queryClient.getQueryState(desktopKeys.list)?.isInvalidated).toBe(true);
    expect(server.desktops[0]).toMatchObject({ name: "Office", color: "#ea603e" });
  });

  it("requestGrant is plain async, POSTs the device id, and is idempotent", async () => {
    const first = await requestGrant("d2", "dev9");
    expect(first).toMatchObject({ deviceId: "dev9", state: "pending" });
    const again = await requestGrant("d2", "dev9");
    expect(again.deviceId).toBe("dev9");
    expect(server.grants.d2).toHaveLength(1);
    expect(server.calls.at(-1)).toEqual({ method: "POST", path: "/api/desktops/d2/grants", body: { deviceId: "dev9" } });
  });

  it("requestGrant rejects with a 409 conflict ApiError for a revoked grant", async () => {
    server.conflicts.add("POST /api/desktops/d1/grants");
    await expect(requestGrant("d1", "dev1")).rejects.toMatchObject({ status: 409, code: "conflict" });
    await expect(requestGrant("d1", "dev1")).rejects.toBeInstanceOf(ApiError);
  });

  it("setRequestedPermission lowers with requestedPermission read and raises with control", async () => {
    server.grants.d1 = [grant({ permission: "control" })];
    await queryClient.prefetchQuery({ queryKey: desktopKeys.grants("d1"), queryFn: () => ({ grants: [grant({ permission: "control" })] }) });
    await setRequestedPermission(queryClient, "d1", "dev1", "read");
    await setRequestedPermission(queryClient, "d1", "dev1", "control");
    expect(server.calls.filter((call) => call.method === "PUT")).toEqual([
      { method: "PUT", path: "/api/desktops/d1/grants/dev1", body: { requestedPermission: "read" } },
      { method: "PUT", path: "/api/desktops/d1/grants/dev1", body: { requestedPermission: "control" } },
    ]);
    // The client never patches the applied permission itself: the cache keeps what the desktop reported
    // (control) and is only marked stale, so the next fetch shows what the desktop applied.
    expect(queryClient.getQueryData<{ grants: Array<{ permission: string }> }>(desktopKeys.grants("d1"))?.grants[0]?.permission).toBe("control");
    expect(queryClient.getQueryState(desktopKeys.grants("d1"))?.isInvalidated).toBe(true);
    for (const put of server.calls.filter((call) => call.method === "PUT")) expect(Object.keys(put.body as object)).toEqual(["requestedPermission"]);
  });

  it("revokeGrant re-authenticates once on fresh_required and retries", async () => {
    await revokeGrant(queryClient, "d1", "dev1");
    const puts = server.calls.filter((call) => call.method === "PUT");
    expect(puts).toHaveLength(2);
    expect(puts[1]?.body).toEqual({ state: "revoked" });
    expect(server.reauths).toBe(1);
    expect(server.grants.d1?.[0]?.state).toBe("revoked");
  });

  it("requestSelectionTicket re-authenticates once on fresh_required, then returns the ticket", async () => {
    server.grants.d1 = [grant({ permission: "control" })];
    await expect(requestSelectionTicket("d1", "dev1")).resolves.toBe("ticket-1");
    const posts = server.calls.filter((call) => call.path.endsWith("/selection-ticket"));
    expect(posts.map((call) => `${call.method} ${call.path}`)).toEqual(Array(2).fill("POST /api/desktops/d1/grants/dev1/selection-ticket"));
    expect(server.reauths).toBe(1);
    expect(server.tickets).toEqual(["ticket-1"]);
    // Fresh now: the next ticket needs no prompt.
    await expect(requestSelectionTicket("d1", "dev1")).resolves.toBe("ticket-2");
    expect(server.reauths).toBe(1);
  });

  it("requestSelectionTicket sends nothing more after a cancelled prompt and is refused for a read grant", async () => {
    server.grants.d1 = [grant({ permission: "control" })];
    webauthn.getPasskey.mockResolvedValue({ status: "cancelled" });
    await expect(requestSelectionTicket("d1", "dev1")).rejects.toMatchObject({ name: "ReauthCancelledError" });
    expect(server.calls.filter((call) => call.path.endsWith("/selection-ticket"))).toHaveLength(1);
    expect(server.tickets).toEqual([]);
    webauthn.getPasskey.mockResolvedValue(ASSERTION);
    server.grants.d1 = [grant({ permission: "read" })];
    await expect(requestSelectionTicket("d1", "dev1")).rejects.toMatchObject({ code: "forbidden" });
  });

  it("revokeGrant surfaces a cancelled passkey prompt and sends nothing more", async () => {
    webauthn.getPasskey.mockResolvedValue({ status: "cancelled" });
    await expect(revokeGrant(queryClient, "d1", "dev1")).rejects.toMatchObject({ name: "ReauthCancelledError" });
    expect(server.calls.filter((call) => call.method === "PUT")).toHaveLength(1);
    expect(server.grants.d1?.[0]?.state).toBe("confirmed");
  });

  it("revokeDesktop re-authenticates once, deletes, and drops the desktop caches", async () => {
    await queryClient.prefetchQuery({ queryKey: desktopKeys.detail("d1"), queryFn: () => 1 });
    await queryClient.prefetchQuery({ queryKey: ["link", "d1", "catalog"], queryFn: () => 1 });
    await queryClient.prefetchQuery({ queryKey: ["link", "d2", "catalog"], queryFn: () => 1 });
    await revokeDesktop(queryClient, "d1");
    expect(server.calls.filter((call) => call.method === "DELETE")).toHaveLength(2);
    expect(server.reauths).toBe(1);
    expect(server.desktops.map((item) => item.id)).toEqual(["d2"]);
    expect(queryClient.getQueryData(desktopKeys.detail("d1"))).toBeUndefined();
    // Also its persisted cache, its live link and its link queries (not those of other desktops).
    expect(side.clearPersistedCache).toHaveBeenCalledWith("d1");
    expect(side.stop).toHaveBeenCalledTimes(1);
    expect(queryClient.getQueryData(["link", "d1", "catalog"])).toBeUndefined();
    expect(queryClient.getQueryData(["link", "d2", "catalog"])).toBe(1);
  });

  it("revokeDesktop runs the forget handlers of the other features, so their records of the desktop go too", async () => {
    const forgotten: string[] = [];
    const off = onForgetDesktop((id) => void forgotten.push(id));
    try {
      await revokeDesktop(queryClient, "d1");
    } finally {
      off();
    }
    expect(forgotten).toEqual(["d1"]);
  });

  it("revokeDesktop still finishes when a forget handler fails", async () => {
    const off = onForgetDesktop(() => {
      throw new Error("boom");
    });
    const afterRevoke = vi.fn();
    try {
      await revokeDesktop(queryClient, "d1", { afterRevoke });
    } finally {
      off();
    }
    expect(afterRevoke).toHaveBeenCalledTimes(1);
  });

  it("revokeDesktop runs afterRevoke before the queries are removed, and never fetches the deleted desktop again", async () => {
    await queryClient.prefetchQuery({ queryKey: desktopKeys.detail("d1"), queryFn: () => 1 });
    let stillThere = false;
    await revokeDesktop(queryClient, "d1", {
      afterRevoke: () => {
        stillThere = queryClient.getQueryData(desktopKeys.detail("d1")) === 1;
      },
    });
    expect(stillThere).toBe(true);
    expect(queryClient.getQueryData(desktopKeys.detail("d1"))).toBeUndefined();
    expect(side.clearPersistedCache).toHaveBeenCalledTimes(1);
  });

  it("revokeDesktop clears nothing when the server call fails", async () => {
    server.conflicts.add("DELETE /api/desktops/d1");
    await expect(revokeDesktop(queryClient, "d1")).rejects.toMatchObject({ code: "conflict" });
    expect(side.clearPersistedCache).not.toHaveBeenCalled();
  });

  it("revokeDesktop does not re-authenticate when the session is already fresh", async () => {
    install({ requireFresh: false, desktops: [desktop()] });
    await revokeDesktop(queryClient, "d1");
    expect(server.reauths).toBe(0);
    expect(server.calls.filter((call) => call.method === "DELETE")).toHaveLength(1);
  });
});

describe("forgetDevice", () => {
  it("forgets another device on the server without touching the local key or pins", async () => {
    keys.getDeviceKeyInfo.mockResolvedValue({ keyId: "k", deviceId: "other", publicKey: new Uint8Array(32) });
    const result = await forgetDevice(queryClient, "dev1");
    expect(result.wasThisDevice).toBe(false);
    expect(server.calls.at(-1)).toMatchObject({ method: "DELETE", path: "/api/devices/dev1" });
    expect(keys.forgetDeviceKey).not.toHaveBeenCalled();
    expect(pins.forgetPins).not.toHaveBeenCalled();
    expect(side.clearPersistedCache).not.toHaveBeenCalled();
    expect(side.resetLinkConnections).not.toHaveBeenCalled();
    expect(server.devices).toHaveLength(0);
  });

  it("clears the local device key and pins when it is this browser's device", async () => {
    keys.getDeviceKeyInfo.mockResolvedValue({ keyId: "k", deviceId: "dev1", publicKey: new Uint8Array(32) });
    const result = await forgetDevice(queryClient, "dev1");
    expect(result.wasThisDevice).toBe(true);
    expect(keys.forgetDeviceKey).toHaveBeenCalledTimes(1);
    expect(pins.forgetPins).toHaveBeenCalledTimes(1);
    // Scoped to the forgotten device, never "every pin".
    expect(pins.forgetPins).toHaveBeenCalledWith("dev1");
  });

  it("also clears the persisted cache of every desktop, stops the live links and drops the link queries", async () => {
    keys.getDeviceKeyInfo.mockResolvedValue({ keyId: "k", deviceId: "dev1", publicKey: new Uint8Array(32) });
    await queryClient.prefetchQuery({ queryKey: ["link", "d1", "catalog"], queryFn: () => 1 });
    await queryClient.prefetchQuery({ queryKey: ["link", "d2", "catalog"], queryFn: () => 1 });
    await forgetDevice(queryClient, "dev1");
    expect(side.clearPersistedCache).toHaveBeenCalledTimes(1);
    expect(side.clearPersistedCache).toHaveBeenCalledWith();
    expect(side.resetLinkConnections).toHaveBeenCalledTimes(1);
    expect(queryClient.getQueryData(["link", "d1", "catalog"])).toBeUndefined();
    expect(queryClient.getQueryData(["link", "d2", "catalog"])).toBeUndefined();
  });

  it("still cleans up locally when the server already forgot this browser's own device (404 on retry)", async () => {
    keys.getDeviceKeyInfo.mockResolvedValue({ keyId: "k", deviceId: "gone", publicKey: new Uint8Array(32) });
    const result = await forgetDevice(queryClient, "gone");
    expect(result.wasThisDevice).toBe(true);
    expect(keys.forgetDeviceKey).toHaveBeenCalledTimes(1);
    expect(pins.forgetPins).toHaveBeenCalledTimes(1);
    expect(side.clearPersistedCache).toHaveBeenCalledTimes(1);
  });

  it("runs every cleanup step even when one fails, and reports the failure", async () => {
    keys.getDeviceKeyInfo.mockResolvedValue({ keyId: "k", deviceId: "dev1", publicKey: new Uint8Array(32) });
    keys.forgetDeviceKey.mockRejectedValue(new Error("idb"));
    await expect(forgetDevice(queryClient, "dev1")).rejects.toThrow("idb");
    expect(pins.forgetPins).toHaveBeenCalledTimes(1);
    expect(side.clearPersistedCache).toHaveBeenCalledTimes(1);
  });

  it("answers 404 for another device as an error and leaves local state alone", async () => {
    keys.getDeviceKeyInfo.mockResolvedValue({ keyId: "k", deviceId: "me", publicKey: new Uint8Array(32) });
    await expect(forgetDevice(queryClient, "gone")).rejects.toMatchObject({ code: "not_found" });
    expect(keys.forgetDeviceKey).not.toHaveBeenCalled();
  });

  it("keeps the local key when the server call fails", async () => {
    keys.getDeviceKeyInfo.mockResolvedValue({ keyId: "k", deviceId: "dev1", publicKey: new Uint8Array(32) });
    server.conflicts.add("DELETE /api/devices/dev1");
    await expect(forgetDevice(queryClient, "dev1")).rejects.toMatchObject({ code: "conflict" });
    expect(keys.forgetDeviceKey).not.toHaveBeenCalled();
    expect(pins.forgetPins).not.toHaveBeenCalled();
    expect(side.clearPersistedCache).not.toHaveBeenCalled();
  });
});
