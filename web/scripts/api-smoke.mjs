/* global console, process, fetch, setTimeout, AbortSignal */
// Smoke run of the accounts API against a real `wrangler dev --local` (Miniflare: local D1 and Durable Objects),
// not the vitest Workers pool. Gate 1 of docs/roadmap.md: auth, invite, recovery, admin, rate limit, CSRF and
// Origin, and "no secret in logs".
//
//   pnpm smoke:dev
//
// It is NOT part of `pnpm check` (it starts a server). Everything lives in a temp dir that is removed at the end;
// a `web/.dev.vars` of yours is never read for the secrets (fake ones are passed with --var) and never changed.
// The output holds step names and HTTP statuses only. No invite code, recovery code or token is printed.
// Needs Node 24 (it imports the TypeScript virtual authenticator of the tests, which Node strips natively).
import { spawn, spawnSync } from "node:child_process";
import { randomBytes } from "node:crypto";
import { existsSync, mkdtempSync, readdirSync, readFileSync, rmSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { VirtualAuthenticator } from "../worker/test/support/authenticator.ts";
import { findSecretHits, parseSessionCookie } from "./api-smoke-lib.mjs";

const webDir = join(dirname(fileURLToPath(import.meta.url)), "..");
const wranglerBin = join(webDir, "worker", "node_modules", ".bin", "wrangler");
const wranglerConfig = join(webDir, "wrangler.jsonc");
const RP_ID = "localhost";

/** Every secret value seen in the run, by label. The log scan looks for each of them. */
const secrets = [];
const remember = (label, value) => {
  if (typeof value === "string" && value) secrets.push([label, value]);
};

let failures = 0;
const pass = (name, note = "") => console.log(`PASS ${name}${note ? ` (${note})` : ""}`);
const fail = (name, note) => {
  failures += 1;
  console.log(`FAIL ${name}: ${note}`);
};
class StepFailed extends Error {}
function expect(condition, message) {
  if (!condition) throw new StepFailed(message);
}

/** Runs one named step. A failing step aborts the journey (later steps depend on it), the log scan still runs. */
async function step(name, fn) {
  try {
    const note = await fn();
    pass(name, typeof note === "string" ? note : "");
  } catch (err) {
    fail(name, err instanceof Error ? err.message : String(err));
    throw err;
  }
}

function freePort() {
  return new Promise((resolve, reject) => {
    const server = createServer();
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const { port } = server.address();
      server.close(() => resolve(port));
    });
  });
}

function runSync(cmd, args, label) {
  const r = spawnSync(cmd, args, { cwd: webDir, encoding: "utf8", env: { ...process.env, WRANGLER_SEND_METRICS: "false" } });
  if (r.status !== 0) throw new Error(`${label} failed (exit ${r.status}): ${(r.stderr || "").slice(-600)}`);
  return r.stdout;
}

/** One HTTP client with its own cookie jar. */
class Client {
  constructor(origin, ip) {
    this.origin = origin;
    this.ip = ip;
    this.token = null;
  }

  async call(method, path, { body, origin = this.origin, csrf = true, ip = this.ip, cookie = true } = {}) {
    const headers = { "Content-Type": "application/json", "CF-Connecting-IP": ip };
    if (origin) headers.Origin = origin;
    if (csrf) headers["X-Monoptah-CSRF"] = "1";
    if (cookie && this.token) headers.Cookie = `__Host-mc_session=${this.token}`;
    const init = { method, headers };
    if (method !== "GET" && method !== "HEAD") init.body = JSON.stringify(body ?? {});
    const res = await fetch(`${this.origin}${path}`, { ...init, signal: AbortSignal.timeout(20_000) });
    const cookieChange = parseSessionCookie(res.headers.getSetCookie());
    if (cookieChange !== undefined) {
      this.token = cookieChange.token;
      remember("session token", cookieChange.token);
    }
    const text = await res.text();
    let json = null;
    try {
      json = JSON.parse(text);
    } catch {
      // not JSON: leave null
    }
    return { status: res.status, json, text, headers: res.headers };
  }
}

const ok = (res, what, status = 200) => expect(res.status === status, `${what}: expected ${status}, got ${res.status} ${JSON.stringify(res.json)}`);
const errorOf = (res, status, code, what) =>
  expect(res.status === status && res.json?.code === code, `${what}: expected ${status} ${code}, got ${res.status} ${JSON.stringify(res.json)}`);

/** Register an account with an invite code through the virtual authenticator. */
async function register(client, invite, handle) {
  const key = await VirtualAuthenticator.generate();
  const options = await client.call("POST", "/api/auth/register/options", { body: { invite, handle, displayName: handle } });
  ok(options, `register options of ${handle}`);
  const response = await key.create(options.json.options, { origin: client.origin, rpId: RP_ID });
  const verify = await client.call("POST", "/api/auth/register/verify", { body: { challengeId: options.json.challengeId, response } });
  ok(verify, `register verify of ${handle}`);
  for (const code of verify.json.recoveryCodes) remember(`recovery code of ${handle}`, code);
  return { key, user: verify.json.user, recoveryCodes: verify.json.recoveryCodes };
}

/** Sign in with a passkey. Returns the verify response (the caller decides what it expects). */
async function login(client, key) {
  const options = await client.call("POST", "/api/auth/login/options", { body: {} });
  ok(options, "login options");
  const response = await key.get(options.json.options, { origin: client.origin, rpId: RP_ID });
  return client.call("POST", "/api/auth/login/verify", { body: { challengeId: options.json.challengeId, response } });
}

/** Passkey re-auth: starts the 5 minute fresh window. */
async function reauth(client, key) {
  const options = await client.call("POST", "/api/auth/reauth/options", { body: {} });
  ok(options, "reauth options");
  const response = await key.get(options.json.options, { origin: client.origin, rpId: RP_ID });
  const verify = await client.call("POST", "/api/auth/reauth/verify", { body: { challengeId: options.json.challengeId, response } });
  ok(verify, "reauth verify");
  return verify;
}

async function waitForHealth(origin, wrangler) {
  const deadline = Date.now() + 120_000;
  while (Date.now() < deadline) {
    if (wrangler.exitCode !== null) throw new Error(`wrangler dev exited early with code ${wrangler.exitCode}`);
    try {
      const res = await fetch(`${origin}/api/health`, { signal: AbortSignal.timeout(2000) });
      if (res.status === 200) return;
    } catch {
      // not up yet
    }
    await new Promise((r) => setTimeout(r, 500));
  }
  throw new Error("wrangler dev did not answer /api/health within 120 s");
}

function killTree(child) {
  if (!child || child.exitCode !== null) return;
  try {
    process.kill(-child.pid, "SIGTERM");
  } catch {
    child.kill("SIGTERM");
  }
}

async function main() {
  if (!existsSync(wranglerBin)) throw new Error("wrangler not found: run `pnpm install` in web/ first");
  const tmp = mkdtempSync(join(tmpdir(), "monoptah-smoke-"));
  const persist = join(tmp, "state");
  const logDir = join(tmp, "wrangler-logs");
  let output = "";
  let wrangler = null;
  const cleanup = () => {
    killTree(wrangler);
    rmSync(tmp, { recursive: true, force: true });
  };
  process.once("SIGINT", () => {
    cleanup();
    process.exit(130);
  });
  process.once("SIGTERM", () => {
    cleanup();
    process.exit(143);
  });

  try {
    const port = await freePort();
    const origin = `http://localhost:${port}`;
    const fake = {
      VAPID_PRIVATE_KEY: `smoke-vapid-${randomBytes(18).toString("hex")}`,
      RECOVERY_PEPPER: `smoke-pepper-${randomBytes(18).toString("hex")}`,
      AUDIT_IP_SALT: `smoke-salt-${randomBytes(18).toString("hex")}`,
    };
    for (const [name, value] of Object.entries(fake)) remember(`fake ${name}`, value);

    let inviteCode;
    await step("setup: local migrations and bootstrap invite", async () => {
      runSync(process.execPath, [join(webDir, "worker", "scripts", "ensure-assets.mjs")], "ensure-assets");
      runSync(wranglerBin, ["d1", "migrations", "apply", "monoptah", "--local", "--persist-to", persist, "-c", wranglerConfig], "d1 migrations apply");
      const printed = runSync(process.execPath, [join(webDir, "scripts", "bootstrap.mjs"), "--local", "--persist-to", persist, "--origin", origin], "bootstrap");
      const match = /#invite=([A-Za-z0-9_-]+)/.exec(printed);
      expect(match !== null, "bootstrap printed no invite link");
      inviteCode = match[1];
      remember("bootstrap invite", inviteCode);
    });

    await step("setup: wrangler dev starts and /api/health answers", async () => {
      const args = [
        "dev", "--local", "--persist-to", persist, "--port", String(port), "--ip", "127.0.0.1", "-c", wranglerConfig,
        "--show-interactive-dev-session=false",
        "--var", `RP_ID:${RP_ID}`, "--var", `RP_ORIGIN:${origin}`,
        ...Object.entries(fake).flatMap(([name, value]) => ["--var", `${name}:${value}`]),
      ];
      wrangler = spawn(wranglerBin, args, {
        cwd: webDir,
        detached: true,
        stdio: ["ignore", "pipe", "pipe"],
        env: { ...process.env, WRANGLER_SEND_METRICS: "false", WRANGLER_LOG_PATH: logDir, NO_COLOR: "1" },
      });
      wrangler.stdout.on("data", (d) => (output += d));
      wrangler.stderr.on("data", (d) => (output += d));
      await waitForHealth(origin, wrangler);
    });

    const ipOf = (n) => `198.51.100.${n}`;
    const anon = new Client(origin, ipOf(1));
    let admin;
    let adminLogin;
    let adminLogin2;
    let u2;
    let u3;
    let inviteForPause;
    const ids = {};

    try {
      await step("register the admin with the bootstrap invite", async () => {
        const c = new Client(origin, ipOf(10));
        admin = { client: c, ...(await register(c, inviteCode, "smokeadmin")) };
        expect(admin.user.role === "admin", `role is ${admin.user.role}`);
        expect(admin.client.token, "no session cookie after register");
      });

      await step("GET /api/me: admin, recoveryAckAt null", async () => {
        const me = await admin.client.call("GET", "/api/me");
        ok(me, "me");
        expect(me.json.user.role === "admin" && me.json.recoveryAckAt === null && me.json.recoveryOnly === false, JSON.stringify(me.json));
        ids.admin = me.json.user.id;
      });

      await step("passkey login (second session)", async () => {
        adminLogin = new Client(origin, ipOf(10));
        const res = await login(adminLogin, admin.key);
        ok(res, "login verify");
        expect(adminLogin.token && adminLogin.token !== admin.client.token, "no distinct session cookie");
      });

      await step("reauth opens the fresh window", async () => {
        const res = await reauth(adminLogin, admin.key);
        expect(typeof res.json.reauthAt === "number", "no reauthAt");
      });

      await step("admin creates invites; user 2 registers", async () => {
        inviteForPause = await adminLogin.call("POST", "/api/invites", { body: {} });
        ok(inviteForPause, "create invite (kept for the pause check)", 201);
        remember("invite (pause check)", inviteForPause.json.code);
        const inv = await adminLogin.call("POST", "/api/invites", { body: {} });
        ok(inv, "create invite", 201);
        remember("invite", inv.json.code);
        const c = new Client(origin, ipOf(11));
        u2 = { client: c, ...(await register(c, inv.json.code, "smokeuser2")) };
        ids.u2 = u2.user.id;
      });

      await step("user 2 invites user 3 (invite tree)", async () => {
        const inv = await u2.client.call("POST", "/api/invites", { body: {} });
        ok(inv, "create invite as user 2", 201);
        remember("invite", inv.json.code);
        const c = new Client(origin, ipOf(12));
        u3 = { client: c, ...(await register(c, inv.json.code, "smokeuser3")) };
        ids.u3 = u3.user.id;
      });

      await step("list sessions and revoke one", async () => {
        const list = await adminLogin.call("GET", "/api/sessions");
        ok(list, "list sessions");
        expect(list.json.sessions.length === 2, `expected 2 sessions, got ${list.json.sessions.length}`);
        const other = list.json.sessions.find((s) => !s.current);
        expect(other && list.json.sessions.some((s) => s.current), "current flag missing");
        ok(await adminLogin.call("DELETE", `/api/sessions/${other.id}`), "revoke session");
        const gone = await admin.client.call("GET", "/api/me");
        expect(gone.status === 401, `revoked session still answers ${gone.status}`);
      });

      await step("GET /api/audit and admin GET /api/admin/users", async () => {
        const audit = await adminLogin.call("GET", "/api/audit");
        ok(audit, "audit");
        expect(audit.json.events.length > 0, "no own audit events");
        const users = await adminLogin.call("GET", "/api/admin/users");
        ok(users, "admin users");
        expect(users.json.users.length === 3, `expected 3 users, got ${users.json.users.length}`);
        const adminAudit = await adminLogin.call("GET", "/api/admin/audit");
        ok(adminAudit, "admin audit");
        const leaks = findSecretHits(audit.text + adminAudit.text, secrets);
        expect(leaks.length === 0, `audit bodies hold secrets: ${leaks.join(", ")}`);
      });

      await step("invites paused: create gives invites_paused, register gives invalid_invite, unpause", async () => {
        ok(await adminLogin.call("PUT", "/api/admin/settings", { body: { invitesPaused: true } }), "pause");
        const create = await adminLogin.call("POST", "/api/invites", { body: {} });
        errorOf(create, 409, "invites_paused", "create while paused");
        const reg = await anon.call("POST", "/api/auth/register/options", { body: { invite: inviteForPause.json.code, handle: "smokelate", displayName: "late" } });
        expect(reg.json?.code === "invalid_invite", `register while paused: ${reg.status} ${JSON.stringify(reg.json)}`);
        ok(await adminLogin.call("PUT", "/api/admin/settings", { body: { invitesPaused: false } }), "unpause");
        const settings = await adminLogin.call("GET", "/api/admin/settings");
        expect(settings.json.invitesPaused === false, "still paused");
        return `create ${create.status}, register ${reg.status}`;
      });

      await step("admin blocks user 2 with cascade: user 3 blocked too", async () => {
        await reauth(adminLogin, admin.key);
        const block = await adminLogin.call("POST", `/api/admin/users/${ids.u2}/block`, { body: { cascade: true, reason: "smoke" } });
        ok(block, "block");
        expect(block.json.blocked === 2, `blocked ${block.json.blocked}, expected 2`);
        const me2 = await u2.client.call("GET", "/api/me");
        const me3 = await u3.client.call("GET", "/api/me");
        expect([401, 403].includes(me2.status) && [401, 403].includes(me3.status), `cookies still work: ${me2.status}, ${me3.status}`);
        const relogin = await login(new Client(origin, ipOf(12)), u3.key);
        errorOf(relogin, 403, "blocked", "blocked user 3 passkey login");
        const users = await adminLogin.call("GET", "/api/admin/users");
        const status = (id) => users.json.users.find((u) => u.id === id)?.status;
        expect(status(ids.u2) === "blocked" && status(ids.u3) === "blocked", "list does not show both blocked");
        return `cookie statuses ${me2.status} and ${me3.status}`;
      });

      await step("admin unblocks user 2: user 3 stays blocked", async () => {
        ok(await adminLogin.call("POST", `/api/admin/users/${ids.u2}/unblock`, { body: {} }), "unblock");
        const users = await adminLogin.call("GET", "/api/admin/users");
        const status = (id) => users.json.users.find((u) => u.id === id)?.status;
        expect(status(ids.u2) === "active" && status(ids.u3) === "blocked", `statuses ${status(ids.u2)}, ${status(ids.u3)}`);
      });

      const recovered = new Client(origin, ipOf(13));
      await step("recovery code: recovery_only session, other routes 403, add a passkey", async () => {
        const res = await recovered.call("POST", "/api/auth/recover", { body: { handle: "smokeadmin", code: admin.recoveryCodes[0] } });
        ok(res, "recover");
        expect(res.json.recoveryOnly === true && res.json.codesLeft === 9, JSON.stringify(res.json));
        const me = await recovered.call("GET", "/api/me");
        ok(me, "me in recovery session");
        expect(me.json.recoveryOnly === true, "me.recoveryOnly is not true");
        errorOf(await recovered.call("GET", "/api/passkeys"), 403, "recovery_only", "GET /api/passkeys in recovery session");
        const options = await recovered.call("POST", "/api/passkeys/options", { body: {} });
        ok(options, "passkey options");
        admin.key2 = await VirtualAuthenticator.generate();
        const response = await admin.key2.create(options.json.options, { origin, rpId: RP_ID });
        ok(await recovered.call("POST", "/api/passkeys/verify", { body: { challengeId: options.json.challengeId, response, name: "second" } }), "passkey verify");
      });

      await step("login with the new passkey", async () => {
        adminLogin2 = new Client(origin, ipOf(10));
        ok(await login(adminLogin2, admin.key2), "login with new passkey");
        const me = await adminLogin2.call("GET", "/api/me");
        expect(me.json.recoveryOnly === false, "session is still recovery_only");
      });

      await step("regenerate recovery codes; old code fails", async () => {
        await reauth(adminLogin2, admin.key2);
        const res = await adminLogin2.call("POST", "/api/recovery-codes/regenerate", { body: {} });
        ok(res, "regenerate");
        for (const code of res.json.recoveryCodes) remember("regenerated recovery code", code);
        expect(res.json.recoveryCodes.length === 10 && !res.json.recoveryCodes.includes(admin.recoveryCodes[1]), "codes not fresh");
        const old = await new Client(origin, ipOf(14)).call("POST", "/api/auth/recover", { body: { handle: "smokeadmin", code: admin.recoveryCodes[1] } });
        expect(old.status >= 400 && old.status < 500, `old code answered ${old.status}`);
        return `old code ${old.status} ${old.json?.code}`;
      });

      await step("rename and delete a passkey; deleting the last gives last_passkey", async () => {
        await reauth(adminLogin2, admin.key2);
        const list = await adminLogin2.call("GET", "/api/passkeys");
        ok(list, "list passkeys");
        expect(list.json.passkeys.length === 2, `expected 2 passkeys, got ${list.json.passkeys.length}`);
        const second = list.json.passkeys.find((p) => p.name === "second");
        const first = list.json.passkeys.find((p) => p.name !== "second");
        expect(second && first, "passkey names not as expected");
        const renamed = await adminLogin2.call("PATCH", `/api/passkeys/${second.id}`, { body: { name: "renamed" } });
        ok(renamed, "rename");
        expect(renamed.json.passkey.name === "renamed", "rename not applied");
        ok(await adminLogin2.call("DELETE", `/api/passkeys/${first.id}`), "delete first passkey");
        const last = await adminLogin2.call("DELETE", `/api/passkeys/${second.id}`);
        errorOf(last, 409, "last_passkey", "delete last passkey");
      });

      await step("logout ends the session", async () => {
        ok(await adminLogin2.call("POST", "/api/auth/logout", { body: {} }), "logout");
        const me = await adminLogin2.call("GET", "/api/me");
        expect(me.status === 401, `after logout /api/me answers ${me.status}`);
      });

      await step("negative: non-GET without Origin is 403 bad_origin, wrong Origin too", async () => {
        errorOf(await anon.call("POST", "/api/auth/login/options", { body: {}, origin: "" }), 403, "bad_origin", "no Origin");
        errorOf(await anon.call("POST", "/api/auth/login/options", { body: {}, origin: "http://evil.example" }), 403, "bad_origin", "wrong Origin");
      });

      await step("negative: non-GET without the CSRF header is 403 csrf", async () => {
        errorOf(await anon.call("POST", "/api/auth/login/options", { body: {}, csrf: false }), 403, "csrf", "no CSRF header");
      });

      await step("rate limit: login/options from one IP ends in 429 with Retry-After", async () => {
        const hammer = new Client(origin, "203.0.113.200");
        let tooMany = null;
        let calls = 0;
        for (; calls < 60 && !tooMany; calls++) {
          const res = await hammer.call("POST", "/api/auth/login/options", { body: {} });
          if (res.status === 429) tooMany = res;
          else expect(res.status === 200, `unexpected ${res.status} before the limit`);
        }
        expect(tooMany !== null, "no 429 after 60 calls");
        expect(tooMany.json?.code === "rate_limited", `429 body ${JSON.stringify(tooMany.json)}`);
        const retry = Number(tooMany.headers.get("Retry-After"));
        expect(Number.isInteger(retry) && retry > 0, `Retry-After is ${tooMany.headers.get("Retry-After")}`);
        return `429 on call ${calls}, Retry-After ${retry}`;
      });
    } catch {
      // the failing step is already reported; fall through to the log scan
    }

    // The wrangler log files (WRANGLER_LOG_PATH) are part of "logs" too.
    let logs = output;
    if (existsSync(logDir)) for (const f of readdirSync(logDir)) logs += readFileSync(join(logDir, f), "utf8");
    try {
      const hits = findSecretHits(logs, secrets);
      if (hits.length > 0) fail("no secret in wrangler logs", `${hits.length} secret(s) found: ${[...new Set(hits)].join(", ")}`);
      else pass("no secret in wrangler logs", `${secrets.length} values checked in ${logs.length} bytes`);
    } catch (err) {
      fail("no secret in wrangler logs", String(err));
    }
    if (failures > 0) console.log(`--- wrangler output tail (secrets redacted by the scan, not printed if matched) ---\n${findSecretHits(logs, secrets).length ? "(withheld: it holds a secret)" : output.slice(-3000)}`);
  } finally {
    cleanup();
  }
}

try {
  await main();
} catch (err) {
  if (!(err instanceof StepFailed)) {
    failures += 1;
    console.log(`FAIL setup: ${err instanceof Error ? err.message : String(err)}`);
  } else if (failures === 0) failures = 1;
}
console.log(failures === 0 ? "SMOKE PASS" : `SMOKE FAIL (${failures})`);
process.exit(failures === 0 ? 0 : 1);
