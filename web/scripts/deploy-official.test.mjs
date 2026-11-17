/* global process */
// Tests use temp git repos (a bare origin and a clone) and a fake runner: git is real, but pnpm, the branding
// script and wrangler are never executed. The real wrangler is never called.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, test } from "node:test";
import { AUDIT_ACTIONS } from "../worker/src/modules/audit/actions.ts";
import { buildPlan, COMMIT_RE, defaultRun, main, parseArgs, VERSION_RE } from "./deploy-official.mjs";

const GIT_ENV = {
  ...process.env,
  GIT_AUTHOR_NAME: "t",
  GIT_AUTHOR_EMAIL: "t@example.invalid",
  GIT_COMMITTER_NAME: "t",
  GIT_COMMITTER_EMAIL: "t@example.invalid",
};
const sh = (cwd, ...args) => execFileSync("git", args, { cwd, env: GIT_ENV, encoding: "utf8" });

const REAL_CFG = '{ "d1_databases": [{ "database_id": "3f2b1c9e-7a44-4d6e-9b10-5c0d8e2a6f71" }], "vars": { "VAPID_PUBLIC_KEY": "abc" } }\n';

let tmp, origin, work;

beforeEach(() => {
  tmp = mkdtempSync(join(tmpdir(), "deploy-test-"));
  origin = join(tmp, "origin.git");
  work = join(tmp, "work");
  sh(tmp, "init", "--bare", "-b", "main", origin);
  sh(tmp, "init", "-q", "-b", "main", work);
  sh(work, "remote", "add", "origin", origin);
  mkdirSync(join(work, "web", "app"), { recursive: true });
  writeFileSync(join(work, "web", "wrangler.jsonc"), REAL_CFG);
  writeFileSync(join(work, "web", "app", "package.json"), '{ "version": "1.2.3" }\n');
  sh(work, "add", "-A");
  sh(work, "commit", "-q", "-m", "init");
  sh(work, "push", "-q", "-u", "origin", "main");
});

afterEach(() => rmSync(tmp, { recursive: true, force: true }));

/** Fake runner: real git, scripted everything else. `fail` lists command keys that exit 1. */
function makeRunner(fail = []) {
  const calls = [];
  const run = (cmd, args, opts) => {
    if (cmd === "git") return defaultRun(cmd, args, { cwd: opts.cwd });
    const key = cmd.endsWith("check-fork-branding.sh") ? "branding" : cmd.endsWith("wrangler") ? `wrangler ${args[0]}` : `${cmd} ${args[0]}`;
    calls.push(key + (key === "wrangler d1" ? ` ${args[1]}` : ""));
    if (cmd === "whoami") return { status: 0, stdout: "tester\n", stderr: "" };
    return { status: fail.includes(key) ? 1 : 0, stdout: "", stderr: "" };
  };
  return { run, calls };
}

function go(argv, { env = {}, platform = "linux", fail = [] } = {}) {
  const { run, calls } = makeRunner(fail);
  const out = [];
  const errs = [];
  const origErr = console.error;
  console.error = (...a) => errs.push(a.join(" "));
  let code;
  try {
    code = main(argv, { root: work, env, platform, run, log: (m) => out.push(m) });
  } finally {
    console.error = origErr;
  }
  return { code, calls, out: out.join("\n"), err: errs.join("\n") };
}

test("parseArgs accepts only --dry-run", () => {
  assert.deepEqual(parseArgs(["--dry-run"]), { dryRun: true });
  assert.throws(() => parseArgs(["--force"]), /unknown argument/);
});

test("refuses under CI, even with --dry-run", () => {
  const r = go(["--dry-run"], { env: { CI: "1" } });
  assert.equal(r.code, 1);
  assert.match(r.err, /CI is set/);
  assert.deepEqual(r.calls, []);
});

test("a real run refuses outside macOS; dry-run does not", () => {
  const r = go([], { platform: "linux" });
  assert.equal(r.code, 1);
  assert.match(r.err, /not macOS/);
  assert.equal(go(["--dry-run"], { platform: "linux" }).code, 0);
});

test("refuses on a dirty tree", () => {
  writeFileSync(join(work, "web", "wrangler.jsonc"), REAL_CFG + "\n");
  const r = go(["--dry-run"]);
  assert.equal(r.code, 1);
  assert.match(r.err, /not clean/);
});

test("refuses on an untracked file", () => {
  writeFileSync(join(work, "stray.txt"), "x");
  const r = go(["--dry-run"]);
  assert.equal(r.code, 1);
  assert.match(r.err, /not clean/);
});

test("refuses on a branch that is not main", () => {
  sh(work, "checkout", "-q", "-b", "feature");
  const r = go(["--dry-run"]);
  assert.equal(r.code, 1);
  assert.match(r.err, /not main/);
});

test("refuses when main is ahead of origin/main", () => {
  writeFileSync(join(work, "a.txt"), "a");
  sh(work, "add", "a.txt");
  sh(work, "commit", "-q", "-m", "local");
  const r = go(["--dry-run"]);
  assert.equal(r.code, 1);
  assert.match(r.err, /differs from origin\/main \(1 ahead, 0 behind\)/);
});

test("refuses when main is behind origin/main", () => {
  const other = join(tmp, "other");
  sh(tmp, "clone", "-q", origin, other);
  writeFileSync(join(other, "b.txt"), "b");
  sh(other, "add", "b.txt");
  sh(other, "commit", "-q", "-m", "remote");
  sh(other, "push", "-q", "origin", "main");
  const r = go(["--dry-run"]);
  assert.equal(r.code, 1);
  assert.match(r.err, /differs from origin\/main \(0 ahead, 1 behind\)/);
});

test("refuses on placeholder database_id and VAPID key", () => {
  writeFileSync(join(work, "web", "wrangler.jsonc"), REAL_CFG.replace(/3f2b1c9e-7a44-4d6e-9b10-5c0d8e2a6f71/, "00000000-0000-0000-0000-000000000000"));
  sh(work, "commit", "-qam", "placeholder");
  sh(work, "push", "-q", "origin", "main");
  assert.match(go(["--dry-run"]).err, /placeholder database_id/);
  writeFileSync(join(work, "web", "wrangler.jsonc"), REAL_CFG.replace('"abc"', '"REPLACE_WITH_VAPID_PUBLIC_KEY_BASE64URL"'));
  sh(work, "commit", "-qam", "vapid");
  sh(work, "push", "-q", "origin", "main");
  assert.match(go(["--dry-run"]).err, /REPLACE_WITH/);
});

test("refuses when pnpm check fails", () => {
  const r = go(["--dry-run"], { fail: ["pnpm check"] });
  assert.equal(r.code, 1);
  assert.match(r.err, /pnpm check` failed/);
  assert.ok(!r.calls.includes("branding"));
});

test("refuses when the branding check fails", () => {
  const r = go(["--dry-run"], { fail: ["branding"] });
  assert.equal(r.code, 1);
  assert.match(r.err, /check-fork-branding/);
});

test("dry-run on a clean main equal to origin/main prints a plan with migrations before deploy and runs no remote command", () => {
  const r = go(["--dry-run"]);
  assert.equal(r.code, 0, r.err);
  assert.deepEqual(r.calls, ["pnpm install", "pnpm check", "branding"]);
  const mig = r.out.indexOf("migrations apply");
  const dep = r.out.indexOf("deploy -c");
  assert.ok(mig > 0 && dep > mig, r.out);
  assert.ok(r.out.indexOf("build") < mig);
  assert.ok(r.out.indexOf("audit") > dep);
});

test("a real run builds, migrates, deploys, then records the audit row, and prints the runbook line", () => {
  const r = go([], { platform: "darwin" });
  assert.equal(r.code, 0, r.err);
  assert.deepEqual(r.calls, ["pnpm install", "pnpm check", "branding", "pnpm --filter", "wrangler d1 migrations", "wrangler deploy", "wrangler d1 execute", "whoami undefined"]);
  const sha = sh(work, "rev-parse", "HEAD").trim();
  assert.match(r.out, new RegExp(`\\| \\d{4}-\\d{2}-\\d{2} \\| ${sha} \\| 1\\.2\\.3 \\| tester \\|`));
});

test("a failed migration stops before the deploy", () => {
  const r = go([], { platform: "darwin", fail: ["wrangler d1"] });
  assert.equal(r.code, 1);
  assert.ok(!r.calls.includes("wrangler deploy"));
});

test("refuses when pnpm install --frozen-lockfile fails, before pnpm check", () => {
  const r = go(["--dry-run"], { fail: ["pnpm install"] });
  assert.equal(r.code, 1);
  assert.match(r.err, /frozen-lockfile` failed/);
  assert.ok(!r.calls.includes("pnpm check"));
});

const GOOD_COMMIT = "0123456789abcdef0123456789abcdef01234567";

test("the audit INSERT uses the registry action and its meta passes the registry schema", () => {
  const plan = buildPlan({ webDir: "/w", commit: GOOD_COMMIT, version: "1.2.3+abc", now: 1700000000, id: "id-1" });
  const sql = plan.find((s) => s.name === "audit").args.at(-1);
  const m = /VALUES \('id-1', 1700000000, NULL, '(\w+)', '([\w.]+)', NULL, NULL, '(.*)'\);$/.exec(sql);
  assert.ok(m, sql);
  const [, kind, action, metaJson] = m;
  assert.equal(kind, "system");
  assert.equal(action, "system.deploy");
  const spec = AUDIT_ACTIONS[action];
  assert.ok(spec.actorKinds.includes(kind));
  assert.deepEqual(spec.meta.parse(JSON.parse(metaJson)), { commit: GOOD_COMMIT, version: "1.2.3+abc" });
});

test("the script patterns match the registry schema", () => {
  const meta = AUDIT_ACTIONS["system.deploy"].meta;
  const cases = [
    [GOOD_COMMIT, "1.2.3"],
    [GOOD_COMMIT.toUpperCase(), "1.2.3"],
    [GOOD_COMMIT.slice(1), "1.2.3"],
    [GOOD_COMMIT, "1'; DROP TABLE audit_log; --"],
    [GOOD_COMMIT, "a".repeat(33)],
    [GOOD_COMMIT, "a".repeat(32)],
    [GOOD_COMMIT, ""],
  ];
  for (const [commit, version] of cases)
    assert.equal(COMMIT_RE.test(commit) && VERSION_RE.test(version), meta.safeParse({ commit, version }).success, `${commit} ${version}`);
});

test("buildPlan refuses a commit or version that is not validated, so nothing is interpolated into SQL", () => {
  assert.throws(() => buildPlan({ webDir: "/w", commit: "x' OR 1=1 --", version: "1.2.3" }), /40-character/);
  assert.throws(() => buildPlan({ webDir: "/w", commit: GOOD_COMMIT, version: "1'); DROP TABLE audit_log; --" }), /does not match/);
});

test("a real run with an invalid app version refuses before any remote step", () => {
  writeFileSync(join(work, "web", "app", "package.json"), '{ "version": "1.2.3 bad" }\n');
  sh(work, "commit", "-qam", "bad version");
  sh(work, "push", "-q", "origin", "main");
  const r = go([], { platform: "darwin" });
  assert.equal(r.code, 1);
  assert.match(r.err, /does not match/);
  assert.ok(!r.calls.includes("wrangler deploy"));
});
