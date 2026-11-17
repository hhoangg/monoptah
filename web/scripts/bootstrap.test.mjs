/* global process */
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import {
  INVITE_TTL_SECONDS,
  assertNotCi,
  buildInviteSql,
  hashInviteCode,
  newInviteCode,
  parseArgs,
  readRpOrigin,
  registerLink,
} from "./bootstrap.mjs";

const webDir = join(dirname(fileURLToPath(import.meta.url)), "..");
const wrangler = join(webDir, "worker", "node_modules", ".bin", "wrangler");

test("parseArgs requires an explicit mode", () => {
  assert.throws(() => parseArgs([]), /--local or --remote/);
  assert.deepEqual(parseArgs(["--", "--remote"]), { mode: "remote", persistTo: null, origin: null });
  assert.deepEqual(parseArgs(["--local", "--persist-to", "/x", "--origin", "https://a.b"]), {
    mode: "local",
    persistTo: "/x",
    origin: "https://a.b",
  });
});

test("parseArgs rejects bad input", () => {
  assert.throws(() => parseArgs(["--local", "--remote"]), /only one/);
  assert.throws(() => parseArgs(["--remote", "--persist-to", "/x"]), /only applies to --local/);
  assert.throws(() => parseArgs(["--local", "--nope"]), /unknown argument/);
  assert.throws(() => parseArgs(["--local", "--origin"]), /needs a value/);
});

test("remote is refused when CI is set, local is not", () => {
  assert.throws(() => assertNotCi({ mode: "remote" }, { CI: "true" }), /CI/);
  assert.doesNotThrow(() => assertNotCi({ mode: "remote" }, {}));
  assert.doesNotThrow(() => assertNotCi({ mode: "local" }, { CI: "true" }));
});

test("code is 128 bit base64url and hash is lowercase hex sha-256", () => {
  const code = newInviteCode();
  assert.match(code, /^[A-Za-z0-9_-]{22}$/);
  assert.notEqual(code, newInviteCode());
  const h = hashInviteCode(code);
  assert.equal(h, createHash("sha256").update(code, "utf8").digest("hex"));
  assert.match(h, /^[0-9a-f]{64}$/);
});

test("SQL is a bootstrap invite valid 24 h and never contains the code", () => {
  const code = newInviteCode();
  const sql = buildInviteSql(hashInviteCode(code), 1_000_000, "11111111-2222-3333-4444-555555555555");
  assert.ok(!sql.includes(code));
  assert.ok(sql.includes(hashInviteCode(code)));
  assert.match(sql, /VALUES \('[^']+', '[0-9a-f]{64}', NULL, 'bootstrap', 1086400, 1000000\)/);
  assert.equal(INVITE_TTL_SECONDS, 86400);
  assert.throws(() => buildInviteSql("nothex", 1), /64 lowercase hex/);
  assert.throws(() => buildInviteSql(hashInviteCode(code), 1, "x'); DROP TABLE users;--"), /UUID/);
});

test("origin and link", () => {
  assert.equal(readRpOrigin('{ // c\n "vars": { "RP_ORIGIN": "https://zptah.app", } }'), "https://zptah.app");
  assert.throws(() => readRpOrigin("{}"), /RP_ORIGIN/);
  assert.equal(registerLink("https://zptah.app/", "CODE"), "https://zptah.app/register#invite=CODE");
});

test("local run creates the invite row (integration, temp persist dir)", () => {
  const dir = mkdtempSync(join(tmpdir(), "bootstrap-test-"));
  try {
    const env = { ...process.env };
    delete env.CI;
    const cfg = join(webDir, "wrangler.jsonc");
    const opts = { cwd: webDir, encoding: "utf8", env };
    const mig = spawnSync(wrangler, ["d1", "migrations", "apply", "monoptah", "--local", "-c", cfg, "--persist-to", dir], opts);
    assert.equal(mig.status, 0, mig.stdout + mig.stderr);

    const run = spawnSync(process.execPath, [join(webDir, "scripts", "bootstrap.mjs"), "--local", "--persist-to", dir], opts);
    assert.equal(run.status, 0, run.stdout + run.stderr);
    const m = /\/register#invite=([A-Za-z0-9_-]{22})/.exec(run.stdout);
    assert.ok(m, run.stdout);
    const code = m[1];

    const q = spawnSync(
      wrangler,
      ["d1", "execute", "monoptah", "--local", "-c", cfg, "--persist-to", dir, "--json", "--command", "SELECT * FROM invites"],
      opts,
    );
    assert.equal(q.status, 0, q.stdout + q.stderr);
    const rows = JSON.parse(q.stdout)[0].results;
    assert.equal(rows.length, 1);
    const row = rows[0];
    assert.equal(row.kind, "bootstrap");
    assert.equal(row.created_by, null);
    assert.equal(row.code_hash, hashInviteCode(code));
    assert.equal(row.expires_at - row.created_at, 86400);
    assert.equal(row.used_by, null);
    assert.ok(!q.stdout.includes(code), "the code must not be stored");
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
