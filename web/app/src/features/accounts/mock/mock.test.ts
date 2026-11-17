import { API_ROUTES, findRoute } from "@monoptah/protocol";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ApiError, apiFetch, setApiTransport } from "../api";
import { endpoints } from "../endpoints";
import { seedFromQuery } from "./index";
import { createMockServer, type MockTransportOptions } from "./transport";
import { SEED_HANDLE, SEED_INVITE_CODE } from "./state";

let restore: (() => void) | undefined;
afterEach(() => {
  restore?.();
  vi.useRealTimers();
});

function setup(options: MockTransportOptions = { signedIn: true }) {
  const server = createMockServer(options);
  restore = setApiTransport(server.transport);
  return server;
}

const RESPONSE = { id: "cred", rawId: "cred", type: "public-key" as const, response: {}, clientExtensionResults: {} };

async function register(invite: string, handle = "new-user") {
  const { challengeId } = await apiFetch(endpoints.registerOptions, { body: { invite, handle, displayName: "New" } });
  return apiFetch(endpoints.registerVerify, { body: { challengeId, response: RESPONSE } });
}

async function reauth() {
  const { challengeId } = await apiFetch(endpoints.reauthOptions);
  await apiFetch(endpoints.reauthVerify, { body: { challengeId, response: RESPONSE } });
}

describe("mock server: registration and invites", () => {
  it("registers with the seed invite once, then refuses it with the generic error", async () => {
    setup({ signedIn: false });
    const done = await register(SEED_INVITE_CODE);
    expect(done.recoveryCodes).toHaveLength(10);
    for (const code of done.recoveryCodes) expect(code).toMatch(/^[0-9A-HJKMNP-TV-Z]{4}(-[0-9A-HJKMNP-TV-Z]{4}){3}$/);
    const second = await register(SEED_INVITE_CODE, "another").catch((e: unknown) => e);
    expect(second).toBeInstanceOf(ApiError);
    expect(second).toMatchObject({ status: 400, code: "invalid_invite" });
  });

  it("gives the same generic error for an unknown invite and for paused invites", async () => {
    setup({ signedIn: false, invitesPaused: true });
    const paused = await register(SEED_INVITE_CODE).catch((e: unknown) => e);
    const unknown = await register("NOPE").catch((e: unknown) => e);
    expect((paused as ApiError).message).toBe((unknown as ApiError).message);
    expect(paused).toMatchObject({ code: "invalid_invite" });
  });

  it("refuses a taken handle", async () => {
    setup({ signedIn: false });
    await expect(register(SEED_INVITE_CODE, SEED_HANDLE)).rejects.toMatchObject({ status: 409, code: "conflict" });
  });

  it("generates fresh recovery codes on every registration", async () => {
    const server = setup({ signedIn: false });
    const first = (await register(SEED_INVITE_CODE)).recoveryCodes;
    server.state.registrationCodes.add("DEV-INVITE-0002");
    const second = (await register("DEV-INVITE-0002", "second")).recoveryCodes;
    expect(new Set([...first, ...second]).size).toBe(20);
  });

  it("enforces the quota of 3, frees a slot on revoke and consumes a created invite on register", async () => {
    setup();
    const codes: string[] = [];
    const ids: string[] = [];
    for (let i = 0; i < 3; i += 1) {
      const created = await apiFetch(endpoints.createInvite);
      codes.push(created.code);
      ids.push(created.invite.id);
    }
    await expect(apiFetch(endpoints.createInvite)).rejects.toMatchObject({ code: "quota_exceeded" });
    expect((await apiFetch(endpoints.invites)).remaining).toBe(0);

    await apiFetch(endpoints.revokeInvite, { params: { id: ids[0]! } });
    expect((await apiFetch(endpoints.invites)).remaining).toBe(1);
    expect((await apiFetch(endpoints.me)).invitesLeft).toBe(1);

    // A revoked code no longer registers; a live one does, and a used one frees no slot.
    await apiFetch(endpoints.logout);
    await expect(register(codes[0]!)).rejects.toMatchObject({ code: "invalid_invite" });
    await register(codes[1]!);
    expect(await register(codes[1]!, "again").catch((e: unknown) => e)).toMatchObject({ code: "invalid_invite" });
  });

  it("answers 409 invites_paused when creating while paused", async () => {
    setup({ signedIn: true, invitesPaused: true });
    await expect(apiFetch(endpoints.createInvite)).rejects.toMatchObject({ status: 409, code: "invites_paused" });
  });
});

describe("mock server: passkeys and fresh re-auth", () => {
  it("never deletes the last passkey", async () => {
    setup({ signedIn: true, passkeyCount: 2 });
    await reauth();
    const { passkeys } = await apiFetch(endpoints.passkeys);
    await apiFetch(endpoints.deletePasskey, { params: { id: passkeys[0]!.id } });
    await expect(apiFetch(endpoints.deletePasskey, { params: { id: passkeys[1]!.id } })).rejects.toMatchObject({ status: 409, code: "last_passkey" });
    expect((await apiFetch(endpoints.passkeys)).passkeys).toHaveLength(1);
  });

  it("requires a fresh window for sensitive routes and expires it after 5 minutes", async () => {
    vi.useFakeTimers({ toFake: ["Date"] });
    setup();
    await expect(apiFetch(endpoints.regenerateRecoveryCodes)).rejects.toMatchObject({ status: 403, code: "fresh_required" });
    await reauth();
    expect((await apiFetch(endpoints.regenerateRecoveryCodes)).recoveryCodes).toHaveLength(10);
    vi.setSystemTime(Date.now() + 301_000);
    await expect(apiFetch(endpoints.regenerateRecoveryCodes)).rejects.toMatchObject({ code: "fresh_required" });
  });

  it("renames a passkey and adds one", async () => {
    setup();
    await reauth();
    const { passkeys } = await apiFetch(endpoints.passkeys);
    const renamed = await apiFetch(endpoints.renamePasskey, { params: { id: passkeys[0]!.id }, body: { name: "Renamed" } });
    expect(renamed.passkey.name).toBe("Renamed");
    const { challengeId, options } = await apiFetch(endpoints.addPasskeyOptions);
    expect(options.challenge).toBeTruthy();
    const added = await apiFetch(endpoints.addPasskeyVerify, { body: { challengeId, response: RESPONSE, name: "Added" } });
    expect(added.passkey.name).toBe("Added");
    expect((await apiFetch(endpoints.passkeys)).passkeys).toHaveLength(3);
  });

  it("issues single-use challenges", async () => {
    setup();
    const { challengeId } = await apiFetch(endpoints.reauthOptions);
    await apiFetch(endpoints.reauthVerify, { body: { challengeId, response: RESPONSE } });
    await expect(apiFetch(endpoints.reauthVerify, { body: { challengeId, response: RESPONSE } })).rejects.toMatchObject({ code: "invalid" });
  });
});

describe("mock server: sessions, recovery, gates", () => {
  it("answers 401 without a session and after logout", async () => {
    setup({ signedIn: false });
    await expect(apiFetch(endpoints.me)).rejects.toMatchObject({ status: 401, code: "unauthorized" });
    const { challengeId } = await apiFetch(endpoints.loginOptions);
    await apiFetch(endpoints.loginVerify, { body: { challengeId, response: RESPONSE } });
    expect((await apiFetch(endpoints.me)).user.handle).toBe(SEED_HANDLE);
    await apiFetch(endpoints.logout);
    await expect(apiFetch(endpoints.me)).rejects.toMatchObject({ status: 401 });
  });

  it("recovers with a valid code once, confines the session, then lets it add a passkey", async () => {
    const server = setup({ signedIn: false });
    const [code] = [...server.state.recoveryCodes];
    await expect(apiFetch(endpoints.recover, { body: { handle: SEED_HANDLE, code: "WRONG" } })).rejects.toMatchObject({ status: 401 });
    const recovered = await apiFetch(endpoints.recover, { body: { handle: SEED_HANDLE, code: code!.toLowerCase() } });
    expect(recovered).toMatchObject({ recoveryOnly: true, codesLeft: 9 });
    await expect(apiFetch(endpoints.sessions)).rejects.toMatchObject({ code: "recovery_only" });
    // The Worker allows GET /api/me for a recovery-only session.
    expect((await apiFetch(endpoints.me)).recoveryOnly).toBe(true);
    const { challengeId } = await apiFetch(endpoints.addPasskeyOptions);
    await apiFetch(endpoints.addPasskeyVerify, { body: { challengeId, response: RESPONSE } });
    // Like the Worker, the session stays recovery-only after the passkey: the client signs out and signs in again.
    expect((await apiFetch(endpoints.me)).recoveryOnly).toBe(true);
    await expect(apiFetch(endpoints.regenerateRecoveryCodes)).rejects.toMatchObject({ status: 403, code: "recovery_only" });
    await apiFetch(endpoints.logout);
    await expect(apiFetch(endpoints.recover, { body: { handle: SEED_HANDLE, code: code! } })).rejects.toMatchObject({ status: 401 });
  });

  it("rate limits repeated recover failures with Retry-After", async () => {
    setup({ signedIn: false });
    for (let i = 0; i < 5; i += 1) {
      await apiFetch(endpoints.recover, { body: { handle: SEED_HANDLE, code: "BAD" } }).catch(() => undefined);
    }
    await expect(apiFetch(endpoints.recover, { body: { handle: SEED_HANDLE, code: "BAD" } })).rejects.toMatchObject({ status: 429, code: "rate_limited", retryAfter: 60 });
  });

  it("lists and revokes sessions, and signs out when the current one is revoked", async () => {
    setup();
    expect((await apiFetch(endpoints.sessions)).sessions).toHaveLength(3);
    await apiFetch(endpoints.revokeSession, { params: { id: "sess-other-1" } });
    await expect(apiFetch(endpoints.revokeSession, { params: { id: "missing" } })).rejects.toMatchObject({ status: 404 });
    await apiFetch(endpoints.revokeSession, { params: { id: "sess-current" } });
    await expect(apiFetch(endpoints.me)).rejects.toMatchObject({ status: 401 });
  });

  it("patches the display name, lists desktops and stores push prefs", async () => {
    setup();
    expect((await apiFetch(endpoints.patchMe, { body: { displayName: "Renamed" } })).user.displayName).toBe("Renamed");
    const { desktops } = await apiFetch(endpoints.desktops);
    expect(desktops).toHaveLength(2);
    const prefs = { enabled: false, categories: { agentFinished: true, agentInput: false, monoMessages: true } };
    expect(await apiFetch(endpoints.putPushPrefs, { params: { desktopId: desktops[0]!.id }, body: prefs })).toEqual(prefs);
    await expect(apiFetch(endpoints.putPushPrefs, { params: { desktopId: "missing" }, body: prefs })).rejects.toMatchObject({ status: 404 });
  });

  it("rejects a non-GET without the CSRF header or JSON content-type", async () => {
    const { transport } = createMockServer({ signedIn: true });
    const noCsrf = await transport("/api/auth/logout", { method: "POST", headers: { "Content-Type": "application/json" } });
    expect(noCsrf.status).toBe(403);
    const wrongType = await transport("/api/auth/logout", { method: "POST", headers: { "X-Monoptah-CSRF": "1" } });
    expect(wrongType.status).toBe(415);
  });

  it("returns options a browser can use: rp, base64url ids, ES256/RS256, resident key, UV required", async () => {
    setup({ signedIn: false });
    const { options } = await apiFetch(endpoints.registerOptions, { body: { invite: SEED_INVITE_CODE, handle: "opts-user", displayName: "Opts" } });
    expect(options).toMatchObject({
      rp: { id: location.hostname, name: "Monoptah" },
      pubKeyCredParams: [{ alg: -7 }, { alg: -257 }],
      authenticatorSelection: { residentKey: "required", userVerification: "required" },
    });
    expect((options as unknown as { user: { id: string } }).user.id).toMatch(/^[A-Za-z0-9_-]+$/);
    expect(options.challenge).toMatch(/^[A-Za-z0-9_-]{43}$/);
    const login = await apiFetch(endpoints.loginOptions);
    expect(login.options).not.toHaveProperty("allowCredentials");
    expect(login.options).toMatchObject({ userVerification: "required" });
  });

  it("every successful response matches its registry schema", async () => {
    // apiFetch already parses with the endpoint schema; this checks the registry schema too.
    const server = createMockServer({ signedIn: true });
    for (const [method, path] of [["GET", "/api/me"], ["GET", "/api/sessions"], ["GET", "/api/passkeys"], ["GET", "/api/invites"], ["GET", "/api/desktops"]] as const) {
      const response = await server.transport(path, { method });
      expect(findRoute(method, path)!.response.safeParse(await response.json()).success).toBe(true);
    }
    expect(API_ROUTES.length).toBeGreaterThan(0);
  });
});

describe("dev presets", () => {
  it("reads the ?mock= flags", () => {
    expect(seedFromQuery("?mock=recoveryonly,noack,paused,onepasskey", false)).toEqual({
      signedIn: true,
      recoveryOnly: true,
      recoveryAckAt: null,
      invitesPaused: true,
      passkeyCount: 1,
    });
    expect(seedFromQuery("", true)).toEqual({ signedIn: true });
    expect(seedFromQuery("?mock=signedout", true)).toEqual({ signedIn: false });
  });

  it("seeds one passkey and a null recoveryAckAt on request", async () => {
    setup({ signedIn: true, passkeyCount: 1, recoveryAckAt: null });
    expect((await apiFetch(endpoints.passkeys)).passkeys).toHaveLength(1);
    expect((await apiFetch(endpoints.me)).recoveryAckAt).toBeNull();
  });
});

beforeEach(() => sessionStorage.clear());
