// E-01 flows that pass today: the accounts API in a real browser with a virtual authenticator, against the local
// Worker. The endpoints and payloads are the ones of web/scripts/api-smoke.mjs. One serial journey: later tests use
// the account and the credential created by the earlier ones.
import { bootstrapInvite } from "../lib/env.ts";
import { test, expect } from "../lib/fixtures.ts";
import { SESSION_COOKIE, type Actor, type Json } from "../lib/actor.ts";
import type { VirtualCredential } from "../lib/webauthn.ts";

const HANDLE = "e2eadmin";

interface Journey {
  adminId: string;
  adminCredential: VirtualCredential;
  recoveryCodes: string[];
  recoveredCredential?: VirtualCredential;
}
let journey: Journey;

/** The authenticator bumps the signature counter on each use and the server rejects a stale one (clone detection),
 * so a credential carried to another context must be re-read after every login that used it. */
async function current(actor: Actor): Promise<VirtualCredential> {
  return (await actor.authenticator.credentials())[0]!;
}

function expectError(res: { status: number; json: Json }, status: number, code: string): void {
  expect({ status: res.status, code: res.json["code"] }).toEqual({ status, code });
}

test.describe.serial("accounts (E-01)", () => {
  test("a. register the admin with the bootstrap invite", async ({ newActor }) => {
    const actor = await newActor();
    const verify = await actor.register(bootstrapInvite(), HANDLE);
    expect(verify.status).toBe(200);
    const codes = verify.json["recoveryCodes"] as string[];
    expect(codes).toHaveLength(10);

    const credentials = await actor.authenticator.credentials();
    expect(credentials).toHaveLength(1);
    const credential = credentials[0]!;
    expect(credential.rpId).toBe("localhost");
    expect(credential.isResidentCredential).toBe(true);

    const me = await actor.call("GET", "/api/me");
    expect(me.status).toBe(200);
    expect(me.json["user"].role).toBe("admin");
    expect(me.json["user"].handle).toBe(HANDLE);

    const cookie = await actor.sessionCookie();
    expect(cookie?.httpOnly).toBe(true);
    expect(cookie?.sameSite).not.toBe("None");
    // The page cannot read it.
    expect(await actor.page.evaluate(() => document.cookie)).not.toContain("mc_session");

    journey = { adminId: me.json["user"].id, adminCredential: credential, recoveryCodes: codes };
  });

  test("b. passkey login in a fresh context; negative: credential removed", async ({ newActor }) => {
    const fresh = await newActor();
    expect(await fresh.sessionCookie()).toBeUndefined();
    await fresh.authenticator.addCredential(journey.adminCredential);
    const verify = await fresh.login();
    expect(verify.status).toBe(200);
    const me = await fresh.call("GET", "/api/me");
    expect(me.status).toBe(200);
    expect(me.json["user"].id).toBe(journey.adminId);
    expect((await fresh.sessionCookie())?.httpOnly).toBe(true);
    journey.adminCredential = await current(fresh);

    // Negative: the same context with the credential removed cannot sign in (the browser refuses, no assertion).
    const none = await newActor();
    await none.authenticator.addCredential(journey.adminCredential);
    await none.authenticator.removeCredential(journey.adminCredential.credentialId);
    expect(await none.authenticator.credentials()).toHaveLength(0);
    const options = await none.call("POST", "/api/auth/login/options", { body: {} });
    expect(options.status).toBe(200);
    await expect(none.getAssertion(options.json["options"])).rejects.toThrow();
    expect(await none.sessionCookie()).toBeUndefined();
  });

  test("c. recovery code gives a recovery_only session; add a passkey; code is single use", async ({ newActor }) => {
    const actor = await newActor();
    const code = journey.recoveryCodes[0]!;
    const recover = await actor.call("POST", "/api/auth/recover", { body: { handle: HANDLE, code } });
    expect(recover.status).toBe(200);
    expect(recover.json["recoveryOnly"]).toBe(true);
    expect(recover.json["codesLeft"]).toBe(9);

    const me = await actor.call("GET", "/api/me");
    expect(me.status).toBe(200);
    expect(me.json["recoveryOnly"]).toBe(true);
    expectError(await actor.call("GET", "/api/passkeys"), 403, "recovery_only");
    expectError(await actor.call("GET", "/api/sessions"), 403, "recovery_only");

    // Add a new passkey with this context's virtual authenticator (it holds no credential yet).
    const options = await actor.call("POST", "/api/passkeys/options", { body: {} });
    expect(options.status).toBe(200);
    const response = await actor.createCredential(options.json["options"]);
    const verify = await actor.call("POST", "/api/passkeys/verify", { body: { challengeId: options.json["challengeId"], response, name: "second" } });
    expect(verify.status).toBe(200);
    const created = await actor.authenticator.credentials();
    expect(created).toHaveLength(1);
    journey.recoveredCredential = created[0]!;

    // Sign in with the new passkey only (the first credential is not in this context).
    const second = await newActor();
    await second.authenticator.addCredential(journey.recoveredCredential);
    expect((await second.login()).status).toBe(200);
    const me2 = await second.call("GET", "/api/me");
    expect(me2.status).toBe(200);
    expect(me2.json["recoveryOnly"]).toBe(false);

    // The used code is rejected the second time.
    const again = await (await newActor()).call("POST", "/api/auth/recover", { body: { handle: HANDLE, code } });
    expect(again.status).toBeGreaterThanOrEqual(400);
    expect(again.status).toBeLessThan(500);
  });

  test("d. admin: invites, second user, list, pause, block with reauth, unblock", async ({ newActor }) => {
    const admin = await newActor();
    await admin.authenticator.addCredential(journey.adminCredential);
    expect((await admin.login()).status).toBe(200);
    journey.adminCredential = await current(admin);

    // Two invites: one used now, one kept for the pause check.
    const first = await admin.call("POST", "/api/invites", { body: {} });
    expect(first.status).toBe(201);
    const kept = await admin.call("POST", "/api/invites", { body: {} });
    expect(kept.status).toBe(201);

    // A second user registers in another context.
    const user = await newActor();
    const registered = await user.register(first.json["code"], "e2euser2");
    expect(registered.status).toBe(200);
    let userCredential = await current(user);
    const userId = registered.json["user"].id as string;
    expect((await user.call("GET", "/api/me")).status).toBe(200);

    const users = await admin.call("GET", "/api/admin/users");
    expect(users.status).toBe(200);
    expect(users.json["users"]).toHaveLength(2);

    // Pause invites: create gives invites_paused, register gives invalid_invite.
    expect((await admin.call("PUT", "/api/admin/settings", { body: { invitesPaused: true } })).status).toBe(200);
    expectError(await admin.call("POST", "/api/invites", { body: {} }), 409, "invites_paused");
    const late = await (await newActor()).call("POST", "/api/auth/register/options", {
      body: { invite: kept.json["code"], handle: "e2elate", displayName: "late" },
    });
    expect(late.json["code"]).toBe("invalid_invite");
    expect((await admin.call("PUT", "/api/admin/settings", { body: { invitesPaused: false } })).status).toBe(200);
    expect((await admin.call("GET", "/api/admin/settings")).json["invitesPaused"]).toBe(false);

    // Block with a fresh re-auth: the user's cookie stops working and passkey login answers 403 blocked.
    expect((await admin.reauth()).status).toBe(200);
    const block = await admin.call("POST", `/api/admin/users/${userId}/block`, { body: { cascade: false, reason: "e2e" } });
    expect(block.status).toBe(200);
    expect([401, 403]).toContain((await user.call("GET", "/api/me")).status);
    const relogin = await newActor();
    await relogin.authenticator.addCredential(userCredential);
    expectError(await relogin.login(), 403, "blocked");
    userCredential = await current(relogin);

    // Unblock: the user signs in again.
    expect((await admin.call("POST", `/api/admin/users/${userId}/unblock`, { body: {} })).status).toBe(200);
    const back = await newActor();
    await back.authenticator.addCredential(userCredential);
    expect((await back.login()).status).toBe(200);
  });

  test("e. negatives: wrong invite, missing CSRF header", async ({ newActor }) => {
    const actor = await newActor();
    const wrong = await actor.register("this-is-not-a-real-invite", "e2ewrong");
    expect(wrong.status).toBeGreaterThanOrEqual(400);
    expect(wrong.json["code"]).toBe("invalid_invite");
    expect(await actor.authenticator.credentials()).toHaveLength(0);

    const noCsrf = await actor.call("POST", "/api/auth/login/options", { body: {}, csrf: false });
    expectError(noCsrf, 403, "csrf");
    // The cookie is never created by a refused call.
    expect(await actor.sessionCookie()).toBeUndefined();
    expect(SESSION_COOKIE).toBe("__Host-mc_session");
  });
});
