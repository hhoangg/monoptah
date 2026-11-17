import { QueryClient } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ApiError, setApiTransport } from "./api";
import { createMockServer } from "./mock/transport";
import { SEED_INVITE_CODE } from "./mock/state";

const webauthn = vi.hoisted(() => ({
  createPasskey: vi.fn(),
  getPasskey: vi.fn(),
}));
vi.mock("./webauthn", () => webauthn);

import { addPasskey, deletePasskey, meQuery, regenerateRecoveryCodes, ReauthCancelledError, registerAccount, signIn, withFreshAuth } from "./queries";

const ASSERTION = { status: "ok", response: { id: "cred", rawId: "cred", type: "public-key", response: {}, clientExtensionResults: {} } };
let restore: (() => void) | undefined;
let queryClient: QueryClient;

beforeEach(() => {
  vi.clearAllMocks();
  queryClient = new QueryClient();
  webauthn.createPasskey.mockResolvedValue(ASSERTION);
  webauthn.getPasskey.mockResolvedValue(ASSERTION);
});
afterEach(() => restore?.());

function useMock(options = { signedIn: true }) {
  const server = createMockServer(options);
  restore = setApiTransport(server.transport);
  return server;
}

describe("withFreshAuth", () => {
  const fresh = () => new ApiError({ status: 403, code: "fresh_required", message: "fresh" });

  it("returns the result without re-auth when the call succeeds", async () => {
    const reauthFn = vi.fn();
    expect(await withFreshAuth(async () => 7, reauthFn)).toBe(7);
    expect(reauthFn).not.toHaveBeenCalled();
  });

  it("re-auths once on fresh_required and retries once", async () => {
    const fn = vi.fn().mockRejectedValueOnce(fresh()).mockResolvedValueOnce("done");
    const reauthFn = vi.fn().mockResolvedValue({ status: "ok", reauthAt: 1 });
    expect(await withFreshAuth(fn, reauthFn)).toBe("done");
    expect(reauthFn).toHaveBeenCalledTimes(1);
    expect(fn).toHaveBeenCalledTimes(2);
  });

  it("does not retry a second time", async () => {
    const fn = vi.fn().mockRejectedValue(fresh());
    const reauthFn = vi.fn().mockResolvedValue({ status: "ok", reauthAt: 1 });
    await expect(withFreshAuth(fn, reauthFn)).rejects.toMatchObject({ code: "fresh_required" });
    expect(fn).toHaveBeenCalledTimes(2);
  });

  it("throws ReauthCancelledError when the prompt is dismissed", async () => {
    const fn = vi.fn().mockRejectedValue(fresh());
    await expect(withFreshAuth(fn, async () => ({ status: "cancelled" }))).rejects.toBeInstanceOf(ReauthCancelledError);
    expect(fn).toHaveBeenCalledTimes(1);
  });

  it("passes other errors through untouched", async () => {
    const error = new ApiError({ status: 409, code: "last_passkey", message: "x" });
    const reauthFn = vi.fn();
    await expect(withFreshAuth(() => Promise.reject(error), reauthFn)).rejects.toBe(error);
    expect(reauthFn).not.toHaveBeenCalled();
  });
});

describe("flows against the mock", () => {
  it("runs the passkey ceremony transparently when the server asks for fresh auth", async () => {
    useMock();
    const codes = await regenerateRecoveryCodes(queryClient);
    expect(codes).toHaveLength(10);
    expect(webauthn.getPasskey).toHaveBeenCalledTimes(1);
    // The window is open now: a second call needs no ceremony.
    await regenerateRecoveryCodes(queryClient);
    expect(webauthn.getPasskey).toHaveBeenCalledTimes(1);
  });

  it("surfaces last_passkey from deletePasskey", async () => {
    useMock({ signedIn: true, passkeyCount: 1 } as never);
    await expect(deletePasskey(queryClient, "pk-seed-1")).rejects.toMatchObject({ code: "last_passkey" });
  });

  it("registers and signs in, returning cancelled when the prompt is dismissed", async () => {
    useMock({ signedIn: false } as never);
    webauthn.createPasskey.mockResolvedValueOnce({ status: "cancelled" });
    expect(await registerAccount(queryClient, { invite: SEED_INVITE_CODE, handle: "flow-user", displayName: "Flow" })).toEqual({ status: "cancelled" });
    const registered = await registerAccount(queryClient, { invite: SEED_INVITE_CODE, handle: "flow-user", displayName: "Flow" });
    expect(registered).toMatchObject({ status: "ok", user: { handle: "flow-user" } });
    expect(registered.status === "ok" && registered.recoveryCodes).toHaveLength(10);

    webauthn.getPasskey.mockResolvedValueOnce({ status: "cancelled" });
    expect(await signIn(queryClient)).toEqual({ status: "cancelled" });
    expect(await signIn(queryClient)).toMatchObject({ status: "ok" });
  });

  it("addPasskey reports cancelled without calling verify", async () => {
    useMock();
    webauthn.createPasskey.mockResolvedValueOnce({ status: "cancelled" });
    expect(await addPasskey(queryClient)).toEqual({ status: "cancelled" });
  });

  it("meQuery does not retry a 401", async () => {
    restore = setApiTransport(async () => Response.json({ error: "no", code: "unauthorized" }, { status: 401 }));
    const retry = meQuery.retry as (count: number, error: Error) => boolean;
    expect(retry(0, new ApiError({ status: 401, code: "unauthorized", message: "no" }))).toBe(false);
    expect(retry(0, new ApiError({ status: 0, code: "network", message: "no" }))).toBe(true);
    await expect(queryClient.fetchQuery(meQuery)).rejects.toMatchObject({ status: 401 });
  });
});
