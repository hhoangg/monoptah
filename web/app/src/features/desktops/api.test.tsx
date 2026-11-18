import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { renderHook, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ApiError, setApiTransport } from "@/features/accounts/api";
import type * as KeysModule from "@/link/keys";
import type * as PinsModule from "@/link/pins";

const webauthn = vi.hoisted(() => ({ createPasskey: vi.fn(), getPasskey: vi.fn() }));
vi.mock("@/features/accounts/webauthn", () => webauthn);

const keys = vi.hoisted(() => ({ getDeviceKeyInfo: vi.fn(), forgetDeviceKey: vi.fn() }));
const pins = vi.hoisted(() => ({ forgetPins: vi.fn() }));
vi.mock("@/link/keys", async (importOriginal) => ({ ...(await importOriginal<typeof KeysModule>()), ...keys }));
vi.mock("@/link/pins", async (importOriginal) => ({ ...(await importOriginal<typeof PinsModule>()), ...pins }));

import {
  desktopKeys,
  forgetDevice,
  renameDesktop,
  requestGrant,
  revokeDesktop,
  revokeGrant,
  setDesktopColor,
  setRequestedPermission,
  useDesktop,
  useDesktopGrants,
  useDesktops,
  useDevices,
} from "./api";
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
    await setRequestedPermission(queryClient, "d1", "dev1", "read");
    await setRequestedPermission(queryClient, "d1", "dev1", "control");
    expect(server.calls.filter((call) => call.method === "PUT")).toEqual([
      { method: "PUT", path: "/api/desktops/d1/grants/dev1", body: { requestedPermission: "read" } },
      { method: "PUT", path: "/api/desktops/d1/grants/dev1", body: { requestedPermission: "control" } },
    ]);
    // The server never writes the applied permission.
    expect(server.grants.d1?.[0]?.permission).toBe("read");
  });

  it("revokeGrant re-authenticates once on fresh_required and retries", async () => {
    await revokeGrant(queryClient, "d1", "dev1");
    const puts = server.calls.filter((call) => call.method === "PUT");
    expect(puts).toHaveLength(2);
    expect(puts[1]?.body).toEqual({ state: "revoked" });
    expect(server.reauths).toBe(1);
    expect(server.grants.d1?.[0]?.state).toBe("revoked");
  });

  it("revokeGrant surfaces a cancelled passkey prompt and sends nothing more", async () => {
    webauthn.getPasskey.mockResolvedValue({ status: "cancelled" });
    await expect(revokeGrant(queryClient, "d1", "dev1")).rejects.toMatchObject({ name: "ReauthCancelledError" });
    expect(server.calls.filter((call) => call.method === "PUT")).toHaveLength(1);
    expect(server.grants.d1?.[0]?.state).toBe("confirmed");
  });

  it("revokeDesktop re-authenticates once, deletes, and drops the desktop caches", async () => {
    await queryClient.prefetchQuery({ queryKey: desktopKeys.detail("d1"), queryFn: () => 1 });
    await revokeDesktop(queryClient, "d1");
    expect(server.calls.filter((call) => call.method === "DELETE")).toHaveLength(2);
    expect(server.reauths).toBe(1);
    expect(server.desktops.map((item) => item.id)).toEqual(["d2"]);
    expect(queryClient.getQueryData(desktopKeys.detail("d1"))).toBeUndefined();
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
    expect(server.devices).toHaveLength(0);
  });

  it("clears the local device key and pins when it is this browser's device", async () => {
    keys.getDeviceKeyInfo.mockResolvedValue({ keyId: "k", deviceId: "dev1", publicKey: new Uint8Array(32) });
    const result = await forgetDevice(queryClient, "dev1");
    expect(result.wasThisDevice).toBe(true);
    expect(keys.forgetDeviceKey).toHaveBeenCalledTimes(1);
    expect(pins.forgetPins).toHaveBeenCalledTimes(1);
  });

  it("keeps the local key when the server call fails", async () => {
    keys.getDeviceKeyInfo.mockResolvedValue({ keyId: "k", deviceId: "gone", publicKey: new Uint8Array(32) });
    await expect(forgetDevice(queryClient, "gone")).rejects.toMatchObject({ code: "not_found" });
    expect(keys.forgetDeviceKey).not.toHaveBeenCalled();
    expect(pins.forgetPins).not.toHaveBeenCalled();
  });
});
