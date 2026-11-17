// R-03: the pairing API (architecture.md 6.4). Real routes, real D1; the room is a double where a test looks at it.
import { USER_CODE_ALPHABET } from "@monoptah/protocol";
import { describe, expect, it } from "vitest";
import { sha256Hex } from "../../src/lib/crypto.ts";
import { clientIp, ipHash } from "../../src/lib/ip.ts";
import { cleanup as pairingCleanup } from "../../src/modules/pairing/cleanup.ts";
import { normalizeUserCode } from "../../src/modules/pairing/codes.ts";
import { CONSUME_PAIRING_SQL, INSERT_TOKEN_SQL } from "../../src/modules/pairing/handlers.ts";
import { authedCall, freshIp, loginAs } from "../support/accounts.ts";
import {
  approve,
  env,
  fakeRoom,
  newAccount,
  pairDesktop,
  poll,
  randomKey,
  rawStart,
  sha256HexBytes,
  startPairing,
  tokenCall,
  withEnv,
} from "./support/api-support.ts";

async function pairingRow(deviceCode: string): Promise<Record<string, unknown>> {
  return (await env.DB.prepare("SELECT * FROM pairing_requests WHERE device_code_hash = ?").bind(await sha256Hex(deviceCode)).first()) as Record<string, unknown>;
}

describe("pair/start", () => {
  it("returns an 8 character user code, a 6 digit confirmation code, expiresIn 600 and interval 3", async () => {
    const pairing = await startPairing();
    expect(pairing.userCode).toMatch(new RegExp(`^[${USER_CODE_ALPHABET}]{4}-[${USER_CODE_ALPHABET}]{4}$`));
    expect(pairing.confirmationCode).toMatch(/^\d{6}$/);
    const res = await rawStart({ publicKey: randomKey().b64, machineName: "M", os: "linux", appVersion: "1", challenge: pairing.challenge });
    expect(await res.json()).toMatchObject({ expiresIn: 600, interval: 3 });
  });

  it("stores only the SHA-256 of the device code, the user code without a dash, and a 10 minute expiry", async () => {
    const before = Math.floor(Date.now() / 1000);
    const pairing = await startPairing();
    const row = await pairingRow(pairing.deviceCode);
    expect(row.device_code_hash).toBe(await sha256Hex(pairing.deviceCode));
    expect(JSON.stringify(row)).not.toContain(pairing.deviceCode);
    expect(row.user_code).toBe(pairing.userCode.replace("-", ""));
    expect(row.status).toBe("pending");
    expect(row.expires_at as number).toBeGreaterThanOrEqual(before + 600);
    expect(row.expires_at as number).toBeLessThanOrEqual(before + 603);
  });

  it("stores the salted ip hash of the caller and never the address", async () => {
    const ip = freshIp();
    const pairing = await startPairing({ ip });
    const row = await pairingRow(pairing.deviceCode);
    expect(row.ip_hash).toBe(await ipHash(env, ip));
    expect(JSON.stringify(row)).not.toContain(ip);
    expect(clientIp(new Request("http://x", { headers: { "CF-Connecting-IP": ip } }))).toBe(ip);
  });

  it("refuses the 4th pending request from one IP with 429 and Retry-After", async () => {
    const ip = freshIp();
    for (let i = 0; i < 3; i += 1) await startPairing({ ip });
    const res = await rawStart({ publicKey: randomKey().b64, machineName: "M", os: "x", appVersion: "1", challenge: (await startPairing()).challenge }, ip);
    expect(res.status).toBe(429);
    expect(res.headers.get("Retry-After")).not.toBeNull();
  });

  it("refuses a public key or challenge that is not 32 bytes of base64url with 400", async () => {
    const good = (await startPairing()).challenge;
    const base = { machineName: "M", os: "x", appVersion: "1" };
    const short = btoa("short").replaceAll("=", "");
    const cases = [
      { ...base, publicKey: short, challenge: good },
      { ...base, publicKey: randomKey().b64, challenge: short },
      { ...base, publicKey: `${randomKey().b64}=`, challenge: good },
      { ...base, publicKey: "not base64 !", challenge: good },
    ];
    for (const body of cases) expect((await rawStart(body)).status).toBe(400);
    expect((await rawStart({ ...base, publicKey: randomKey().b64 })).status).toBe(400);
  });
});

describe("GET /api/pair/:userCode", () => {
  it("shows machine name, os, the full fingerprint of the key bytes, the confirmation code and the expiry", async () => {
    const { session } = await newAccount();
    const pairing = await startPairing({ machineName: "Studio", os: "macos" });
    const res = await authedCall(session, `/api/pair/${pairing.userCode}`);
    expect(res.status).toBe(200);
    const body = (await res.json()) as Record<string, unknown>;
    expect(body).toMatchObject({ machineName: "Studio", os: "macos", confirmationCode: pairing.confirmationCode });
    expect(body.fingerprint).toBe(await sha256HexBytes(pairing.publicKeyBytes));
    expect(body.fingerprint).toMatch(/^[0-9a-f]{64}$/);
    expect(body.expiresAt as number).toBeGreaterThan(Date.now() / 1000);
  });

  it("accepts the code lower case and without the dash, and answers 404 for a malformed or unknown code", async () => {
    const { session } = await newAccount();
    const pairing = await startPairing();
    expect(normalizeUserCode(pairing.userCode.toLowerCase())).toBe(pairing.userCode.replace("-", ""));
    expect((await authedCall(session, `/api/pair/${pairing.userCode.toLowerCase().replace("-", "")}`)).status).toBe(200);
    expect((await authedCall(session, "/api/pair/ABC")).status).toBe(404);
    expect((await authedCall(session, "/api/pair/0000-0000")).status).toBe(404);
    expect((await authedCall(session, "/api/pair/ZZZZ-ZZZZ")).status).toBe(404);
  });

  it("answers 404 for an expired request and requires a session", async () => {
    const { session } = await newAccount();
    const pairing = await startPairing();
    expect((await authedCall({ cookie: "" }, `/api/pair/${pairing.userCode}`)).status).toBe(401);
    await env.DB.prepare("UPDATE pairing_requests SET expires_at = ? WHERE device_code_hash = ?").bind(Math.floor(Date.now() / 1000) - 1, await sha256Hex(pairing.deviceCode)).run();
    expect((await authedCall(session, `/api/pair/${pairing.userCode}`)).status).toBe(404);
  });
});

describe("approve and deny", () => {
  it("needs a fresh session (403 fresh_required) and writes nothing without it", async () => {
    const stale = await newAccount({ fresh: false });
    const pairing = await startPairing();
    const res = await approve(stale.session, pairing.userCode);
    expect(res.status).toBe(403);
    expect(await res.json()).toMatchObject({ code: "fresh_required" });
    expect((await pairingRow(pairing.deviceCode)).status).toBe("pending");
  });

  it("creates the desktop from the request (name, os, key, fingerprint), binds the request and audits it", async () => {
    const account = await newAccount();
    const pairing = await startPairing({ machineName: "Studio", os: "linux" });
    const res = await approve(account.session, pairing.userCode.toLowerCase());
    expect(res.status).toBe(200);
    const { desktopId, name } = (await res.json()) as { desktopId: string; name: string };
    expect(name).toBe("Studio");
    const desktop = await env.DB.prepare("SELECT * FROM desktops WHERE id = ?").bind(desktopId).first<Record<string, unknown>>();
    expect(desktop).toMatchObject({ user_id: account.user.id, name: "Studio", os: "linux", public_key: pairing.publicKey, app_version: null, revoked_at: null });
    expect(desktop?.fingerprint).toBe(await sha256HexBytes(pairing.publicKeyBytes));
    const row = await pairingRow(pairing.deviceCode);
    expect(row).toMatchObject({ status: "approved", user_id: account.user.id, desktop_id: desktopId });
    const { results } = await env.DB.prepare("SELECT action, meta FROM audit_log WHERE actor_user_id = ? ORDER BY rowid").bind(account.user.id).all<{ action: string; meta: string }>();
    expect(results.map((r) => r.action)).toEqual(["pairing.approve", "desktop.create"]);
    expect(JSON.stringify(results)).not.toContain(pairing.deviceCode);
  });

  it("makes the user code single use: a second approve is 409 and a deny after approve is 404", async () => {
    const account = await newAccount();
    const pairing = await startPairing();
    expect((await approve(account.session, pairing.userCode)).status).toBe(200);
    const again = await approve((await newAccount()).session, pairing.userCode);
    expect(again.status).toBe(409);
    expect(await again.json()).toMatchObject({ code: "conflict" });
    expect((await authedCall(account.session, `/api/pair/${pairing.userCode}/deny`, { method: "POST" })).status).toBe(404);
    expect(await env.DB.prepare("SELECT COUNT(*) AS n FROM desktops WHERE user_id = ?").bind(account.user.id).first()).toEqual({ n: 1 });
  });

  it("refuses an approve after the 10 minute expiry with 404 and creates nothing", async () => {
    const account = await newAccount();
    const pairing = await startPairing();
    await env.DB.prepare("UPDATE pairing_requests SET expires_at = ? WHERE device_code_hash = ?").bind(Math.floor(Date.now() / 1000) - 1, await sha256Hex(pairing.deviceCode)).run();
    expect((await approve(account.session, pairing.userCode)).status).toBe(404);
    expect(await env.DB.prepare("SELECT COUNT(*) AS n FROM desktops WHERE user_id = ?").bind(account.user.id).first()).toEqual({ n: 0 });
  });

  it("deny moves a pending request to denied; poll then answers denied and the code cannot be approved", async () => {
    const account = await newAccount();
    const pairing = await startPairing();
    expect((await authedCall(account.session, `/api/pair/${pairing.userCode}/deny`, { method: "POST" })).status).toBe(200);
    expect(await (await poll(pairing.deviceCode, pairing.verifier)).json()).toEqual({ status: "denied" });
    expect((await approve(account.session, pairing.userCode)).status).toBe(404);
    expect((await authedCall(account.session, `/api/pair/${pairing.userCode}/deny`, { method: "POST" })).status).toBe(404);
  });

  it("answers quota_exceeded at the 6th desktop and leaves the request pending", async () => {
    const account = await newAccount();
    for (let i = 0; i < 5; i += 1) await pairDesktop(account);
    const pairing = await startPairing();
    const res = await approve(account.session, pairing.userCode);
    expect(res.status).toBe(409);
    expect(await res.json()).toMatchObject({ code: "quota_exceeded" });
    expect((await pairingRow(pairing.deviceCode)).status).toBe("pending");
  });

  it("honours MAX_DESKTOPS_PER_USER: 2 allows two desktops, a revoked desktop frees a slot, a bad value falls back to 5", async () => {
    const two = withEnv({ MAX_DESKTOPS_PER_USER: "2" });
    const account = await newAccount();
    const first = await pairDesktop(account, { testEnv: two });
    await pairDesktop(account, { testEnv: two });
    const blocked = await approve(account.session, (await startPairing()).userCode, {}, two);
    expect(blocked.status).toBe(409);
    await env.DB.prepare("UPDATE desktops SET revoked_at = 1 WHERE id = ?").bind(first.desktopId).run();
    expect((await approve(account.session, (await startPairing()).userCode, {}, two)).status).toBe(200);

    const bad = withEnv({ MAX_DESKTOPS_PER_USER: "abc" });
    const other = await newAccount();
    for (let i = 0; i < 5; i += 1) expect((await approve(other.session, (await startPairing()).userCode, {}, bad)).status).toBe(200);
    expect((await approve(other.session, (await startPairing()).userCode, {}, bad)).status).toBe(409);
  });
});

describe("pair/poll", () => {
  it("runs the whole flow: pending, approve, then the token once with desktopId, handle and relayUrl", async () => {
    const account = await newAccount({ handle: "alice-w" });
    const pairing = await startPairing();
    expect(await (await poll(pairing.deviceCode, pairing.verifier)).json()).toEqual({ status: "pending" });
    const approved = await approve(account.session, pairing.userCode);
    const { desktopId } = (await approved.json()) as { desktopId: string };
    const res = await poll(pairing.deviceCode, pairing.verifier);
    const body = (await res.json()) as { status: string; desktopId: string; desktopToken: string; account: { handle: string }; relayUrl: string };
    expect(body).toMatchObject({ status: "approved", desktopId, account: { handle: "alice-w" }, relayUrl: `ws://localhost:8787/relay/desktop/${desktopId}` });
    expect(body.desktopToken).toMatch(/^[A-Za-z0-9_-]{43}$/);
    expect((await pairingRow(pairing.deviceCode)).status).toBe("consumed");
    const me = await tokenCall("/api/desktop/me", body.desktopToken);
    expect(me.status).toBe(200);
    expect(await me.json()).toEqual({ desktopId, name: "Work Mac", account: { handle: "alice-w" } });
  });

  it("builds a wss relayUrl when RP_ORIGIN is https", async () => {
    const https = withEnv({ RP_ORIGIN: "https://zptah.app" });
    const account = await newAccount();
    const pairing = await startPairing();
    const approved = await approve(account.session, pairing.userCode);
    const { desktopId } = (await approved.json()) as { desktopId: string };
    const body = (await (await poll(pairing.deviceCode, pairing.verifier, https)).json()) as { relayUrl: string };
    expect(body.relayUrl).toBe(`wss://zptah.app/relay/desktop/${desktopId}`);
  });

  it("stores only the token hash, with scope relay push grants", async () => {
    const account = await newAccount();
    const { token, desktopId } = await pairDesktop(account);
    const row = await env.DB.prepare("SELECT token_hash, scope FROM desktop_tokens WHERE desktop_id = ?").bind(desktopId).first<{ token_hash: string; scope: string }>();
    expect(row).toEqual({ token_hash: await sha256Hex(token), scope: "relay push grants" });
    const all = await env.DB.prepare("SELECT * FROM desktop_tokens").all();
    expect(JSON.stringify(all.results)).not.toContain(token);
  });

  it("returns the token only once: a second poll after consumption is expired, with no token", async () => {
    const account = await newAccount();
    const pairing = await startPairing();
    const { desktopId } = (await (await approve(account.session, pairing.userCode)).json()) as { desktopId: string };
    expect(((await (await poll(pairing.deviceCode, pairing.verifier)).json()) as { status: string }).status).toBe("approved");
    const second = await (await poll(pairing.deviceCode, pairing.verifier)).json();
    expect(second).toEqual({ status: "expired" });
    expect(await env.DB.prepare("SELECT COUNT(*) AS n FROM desktop_tokens WHERE desktop_id = ?").bind(desktopId).first()).toEqual({ n: 1 });
  });

  it("mints one token only: a second consume-and-insert batch on the same approved request writes nothing", async () => {
    const account = await newAccount();
    const pairing = await startPairing();
    const { desktopId } = (await (await approve(account.session, pairing.userCode)).json()) as { desktopId: string };
    const request = await pairingRow(pairing.deviceCode);
    const mint = async (hash: string) => {
      const [consumed, inserted] = await env.DB.batch([
        env.DB.prepare(CONSUME_PAIRING_SQL).bind(request.id),
        env.DB.prepare(INSERT_TOKEN_SQL).bind(crypto.randomUUID(), hash, "relay push grants", 1, request.id, desktopId, 1),
      ]);
      return [consumed?.meta.changes, inserted?.meta.changes];
    };
    expect(await mint("hash-one")).toEqual([1, 1]);
    expect(await mint("hash-two")).toEqual([0, 0]);
    expect(await env.DB.prepare("SELECT COUNT(*) AS n FROM desktop_tokens WHERE desktop_id = ?").bind(desktopId).first()).toEqual({ n: 1 });
  });

  it("refuses a wrong verifier with 403, no token and no state change, even after approval", async () => {
    const account = await newAccount();
    const pairing = await startPairing();
    const { desktopId } = (await (await approve(account.session, pairing.userCode)).json()) as { desktopId: string };
    const wrong = randomKey().b64;
    const res = await poll(pairing.deviceCode, wrong);
    expect(res.status).toBe(403);
    expect(JSON.stringify(await res.json())).not.toContain("desktopToken");
    expect((await pairingRow(pairing.deviceCode)).status).toBe("approved");
    expect(await env.DB.prepare("SELECT COUNT(*) AS n FROM desktop_tokens WHERE desktop_id = ?").bind(desktopId).first()).toEqual({ n: 0 });
    expect((await poll(pairing.deviceCode, "not base64 !")).status).toBe(403);
    // The right verifier still collects it.
    expect(((await (await poll(pairing.deviceCode, pairing.verifier)).json()) as { status: string }).status).toBe("approved");
  });

  it("refuses a wrong or unknown device code with 404 even with the right verifier", async () => {
    const account = await newAccount();
    const pairing = await startPairing();
    await approve(account.session, pairing.userCode);
    expect((await poll(randomKey().b64, pairing.verifier)).status).toBe(404);
    expect((await poll(pairing.deviceCode.slice(1), pairing.verifier)).status).toBe(404);
  });

  it("does not hand over the token to the user code alone (it is not a device code)", async () => {
    const account = await newAccount();
    const pairing = await startPairing();
    await approve(account.session, pairing.userCode);
    expect((await poll(pairing.userCode, pairing.verifier)).status).toBe(404);
    expect((await poll(pairing.userCode.replace("-", ""), pairing.verifier)).status).toBe(404);
  });

  it("answers pending before approval and never a token", async () => {
    const pairing = await startPairing();
    const res = await poll(pairing.deviceCode, pairing.verifier);
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({ status: "pending" });
  });

  it("answers slow_down for a second poll inside 2 seconds, and pending again after the window", async () => {
    const pairing = await startPairing();
    expect(await (await poll(pairing.deviceCode, pairing.verifier)).json()).toEqual({ status: "pending" });
    expect(await (await poll(pairing.deviceCode, pairing.verifier)).json()).toEqual({ status: "slow_down" });
    await env.DB.prepare("UPDATE pairing_requests SET last_polled_at = last_polled_at - 3 WHERE device_code_hash = ?").bind(await sha256Hex(pairing.deviceCode)).run();
    expect(await (await poll(pairing.deviceCode, pairing.verifier)).json()).toEqual({ status: "pending" });
  });

  it("answers expired for a pending request past its 10 minutes, and never issues a token after expiry without approval", async () => {
    const account = await newAccount();
    const pairing = await startPairing();
    await env.DB.prepare("UPDATE pairing_requests SET expires_at = ? WHERE device_code_hash = ?").bind(Math.floor(Date.now() / 1000) - 1, await sha256Hex(pairing.deviceCode)).run();
    expect(await (await poll(pairing.deviceCode, pairing.verifier)).json()).toEqual({ status: "expired" });
    expect((await approve(account.session, pairing.userCode)).status).toBe(404);
  });

  it("answers expired and no token when the desktop was revoked between approve and poll", async () => {
    const account = await newAccount();
    const pairing = await startPairing();
    const { desktopId } = (await (await approve(account.session, pairing.userCode)).json()) as { desktopId: string };
    await env.DB.prepare("UPDATE desktops SET revoked_at = 1 WHERE id = ?").bind(desktopId).run();
    expect(await (await poll(pairing.deviceCode, pairing.verifier)).json()).toEqual({ status: "expired" });
    expect(await env.DB.prepare("SELECT COUNT(*) AS n FROM desktop_tokens WHERE desktop_id = ?").bind(desktopId).first()).toEqual({ n: 0 });
  });

  it("is public: a session cookie is ignored and a body without a device code is 400", async () => {
    const res = await tokenCall("/api/pair/poll", null, { method: "POST", body: JSON.stringify({ verifier: "x" }) });
    expect(res.status).toBe(400);
  });
});

describe("replace (R-14a: approve with replaces)", () => {
  it("keeps id, name and color, swaps key and fingerprint, revokes old tokens, resets every grant to pending, and uses no quota", async () => {
    const room = fakeRoom();
    const roomEnv = withEnv({ ROOM: room.namespace });
    const account = await newAccount();
    const first = await pairDesktop(account, { machineName: "Old Mac", testEnv: roomEnv });
    await env.DB.prepare("UPDATE desktops SET color = 'teal' WHERE id = ?").bind(first.desktopId).run();
    // Two live devices (one confirmed, one pending) and one revoked grant.
    const ids = [crypto.randomUUID(), crypto.randomUUID(), crypto.randomUUID()];
    for (const id of ids) await env.DB.prepare("INSERT INTO devices (id, user_id, name, public_key, fingerprint, created_at) VALUES (?, ?, 'D', 'k', ?, 1)").bind(id, account.user.id, id).run();
    await env.DB.prepare("INSERT INTO device_grants (device_id, desktop_id, state, permission, requested_permission, confirmed_at, updated_at) VALUES (?, ?, 'confirmed', 'control', 'read', 5, 5)").bind(ids[0], first.desktopId).run();
    await env.DB.prepare("INSERT INTO device_grants (device_id, desktop_id, state, updated_at) VALUES (?, ?, 'pending', 5)").bind(ids[1], first.desktopId).run();
    await env.DB.prepare("INSERT INTO device_grants (device_id, desktop_id, state, updated_at) VALUES (?, ?, 'revoked', 5)").bind(ids[2], first.desktopId).run();

    // Other desktops stay untouched: a second desktop with its own grant and token.
    const other = await pairDesktop(account, { machineName: "Other", testEnv: roomEnv });
    await env.DB.prepare("INSERT INTO device_grants (device_id, desktop_id, state, permission, confirmed_at, updated_at) VALUES (?, ?, 'confirmed', 'read', 5, 5)").bind(ids[0], other.desktopId).run();

    const pairing = await startPairing({ machineName: "New Name Ignored", os: "windows" });
    const res = await approve(account.session, pairing.userCode, { replaces: first.desktopId }, roomEnv);
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({ desktopId: first.desktopId, name: "Old Mac" });

    const desktop = await env.DB.prepare("SELECT * FROM desktops WHERE id = ?").bind(first.desktopId).first<Record<string, unknown>>();
    expect(desktop).toMatchObject({ name: "Old Mac", color: "teal", public_key: pairing.publicKey, os: "windows", revoked_at: null });
    expect(desktop?.fingerprint).toBe(await sha256HexBytes(pairing.publicKeyBytes));
    expect(await env.DB.prepare("SELECT COUNT(*) AS n FROM desktops WHERE user_id = ?").bind(account.user.id).first()).toEqual({ n: 2 });

    // Old token no longer works; the other desktop's token does.
    expect((await tokenCall("/api/desktop/me", first.token, {}, roomEnv)).status).toBe(403);
    expect((await tokenCall("/api/desktop/me", other.token, {}, roomEnv)).status).toBe(200);

    const grants = await env.DB.prepare("SELECT device_id, state, permission, requested_permission, confirmed_at FROM device_grants WHERE desktop_id = ? ORDER BY device_id").bind(first.desktopId).all<Record<string, unknown>>();
    const byId = new Map(grants.results.map((g) => [g.device_id as string, g]));
    expect(byId.get(ids[0] as string)).toMatchObject({ state: "pending", permission: null, requested_permission: null, confirmed_at: null });
    expect(byId.get(ids[1] as string)).toMatchObject({ state: "pending", permission: null, confirmed_at: null });
    expect(byId.get(ids[2] as string)).toMatchObject({ state: "revoked" });
    expect(await env.DB.prepare("SELECT state, permission FROM device_grants WHERE desktop_id = ?").bind(other.desktopId).first()).toEqual({ state: "confirmed", permission: "read" });

    // Old key holders are closed (every socket, 4003) after the commit.
    expect(room.calls.revokes.filter((r) => r.desktopId === first.desktopId)).toEqual([{ desktopId: first.desktopId, target: { all: true }, code: 4003 }]);
    const audit = await env.DB.prepare("SELECT meta FROM audit_log WHERE actor_user_id = ? AND action = 'pairing.approve' ORDER BY rowid DESC").bind(account.user.id).first<{ meta: string }>();
    expect(JSON.parse(audit?.meta ?? "{}")).toEqual({ desktopId: first.desktopId, replaced: true });

    // The new pairing collects a fresh token for the same desktop.
    const polled = (await (await poll(pairing.deviceCode, pairing.verifier, roomEnv)).json()) as { desktopId: string; desktopToken: string };
    expect(polled.desktopId).toBe(first.desktopId);
    expect((await tokenCall("/api/desktop/me", polled.desktopToken, {}, roomEnv)).status).toBe(200);
  });

  it("is allowed at the quota and leaves no orphan counted against it", async () => {
    const account = await newAccount();
    const desktops: string[] = [];
    for (let i = 0; i < 5; i += 1) desktops.push((await pairDesktop(account)).desktopId);
    const pairing = await startPairing();
    expect((await approve(account.session, pairing.userCode, { replaces: desktops[0] })).status).toBe(200);
    expect(await env.DB.prepare("SELECT COUNT(*) AS n FROM desktops WHERE user_id = ? AND revoked_at IS NULL").bind(account.user.id).first()).toEqual({ n: 5 });
  });

  it("answers 404 for another user's desktop or a revoked one and leaves the request pending", async () => {
    const mine = await newAccount();
    const theirs = await newAccount();
    const theirDesktop = await pairDesktop(theirs);
    const revoked = await pairDesktop(mine);
    await env.DB.prepare("UPDATE desktops SET revoked_at = 1 WHERE id = ?").bind(revoked.desktopId).run();
    const pairing = await startPairing();
    expect((await approve(mine.session, pairing.userCode, { replaces: theirDesktop.desktopId })).status).toBe(404);
    expect((await approve(mine.session, pairing.userCode, { replaces: revoked.desktopId })).status).toBe(404);
    expect((await approve(mine.session, pairing.userCode, { replaces: crypto.randomUUID() })).status).toBe(404);
    expect((await pairingRow(pairing.deviceCode)).status).toBe("pending");
    expect(await env.DB.prepare("SELECT public_key FROM desktops WHERE id = ?").bind(theirDesktop.desktopId).first()).toEqual({ public_key: theirDesktop.pairing.publicKey });
  });

  it("needs a fresh session like any approve", async () => {
    const account = await newAccount();
    const paired = await pairDesktop(account);
    const stale = await loginAs(account.user.id, { fresh: false });
    const pairing = await startPairing();
    expect((await approve(stale, pairing.userCode, { replaces: paired.desktopId })).status).toBe(403);
  });
});

describe("cleanup", () => {
  it("deletes pairing requests 24 hours after they expired and keeps newer ones", async () => {
    const old = await startPairing();
    const recent = await startPairing();
    const now = Math.floor(Date.now() / 1000);
    await env.DB.prepare("UPDATE pairing_requests SET expires_at = ? WHERE device_code_hash = ?").bind(now - 86_401, await sha256Hex(old.deviceCode)).run();
    await env.DB.prepare("UPDATE pairing_requests SET expires_at = ? WHERE device_code_hash = ?").bind(now - 3_600, await sha256Hex(recent.deviceCode)).run();
    await pairingCleanup(env, now);
    expect(await env.DB.prepare("SELECT COUNT(*) AS n FROM pairing_requests WHERE device_code_hash = ?").bind(await sha256Hex(old.deviceCode)).first()).toEqual({ n: 0 });
    expect(await env.DB.prepare("SELECT COUNT(*) AS n FROM pairing_requests WHERE device_code_hash = ?").bind(await sha256Hex(recent.deviceCode)).first()).toEqual({ n: 1 });
  });
});

it("never logs a token, a code or a verifier while pairing", async () => {
  const lines: string[] = [];
  const spies = (["log", "warn", "error", "info"] as const).map((level) => {
    const original = console[level];
    console[level] = (...args: unknown[]) => void lines.push(args.map(String).join(" "));
    return () => (console[level] = original);
  });
  try {
    const account = await newAccount();
    const paired = await pairDesktop(account);
    const joined = lines.join("\n");
    for (const secret of [paired.token, paired.pairing.deviceCode, paired.pairing.verifier, paired.pairing.userCode]) expect(joined).not.toContain(secret);
  } finally {
    for (const restore of spies) restore();
  }
});
