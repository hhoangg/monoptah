import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmod, mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

// The sandbox build box scripts (scripts/sandbox, docs/native/sandbox-build-box.md) are bash, so
// these tests run them with a fake `sbx` on PATH: nothing here reaches Docker.
const dir = fileURLToPath(new URL("./sandbox/", import.meta.url));
const skip = process.platform === "win32";

async function fixture(t) {
  const root = await mkdtemp(join(tmpdir(), "sandbox-scripts-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  const calls = join(root, "calls.log");
  const sbx = join(root, "sbx");
  // Records every call; `ls -q` lists no sandboxes, plain `ls` prints the empty-state line.
  await writeFile(sbx, `#!/bin/sh\nprintf '%s\\n' "$*" >> '${calls}'\ncase "$*" in\n  *"ls -q"*) ;;\n  *"ls") echo "No sandboxes found" ;;\nesac\nexit 0\n`);
  await chmod(sbx, 0o755);
  const run = (script, args, env = {}) => spawnSync("bash", [join(dir, script), ...args], {
    encoding: "utf8",
    env: { ...process.env, PATH: `${root}:${process.env.PATH}`, BOX_STATE_DIR: join(root, "state"), ...env },
  });
  const recorded = async () => (await readFile(calls, "utf8").catch(() => "")).split("\n").filter(Boolean);
  return { run, recorded };
}

test("every script is valid bash", { skip }, async () => {
  for (const file of (await readdir(dir)).filter((name) => name.endsWith(".sh"))) {
    const result = spawnSync("bash", ["-n", join(dir, file)], { encoding: "utf8" });
    assert.equal(result.status, 0, `${file}: ${result.stderr}`);
  }
});

test("--help prints usage and never calls sbx", { skip }, async (t) => {
  const f = await fixture(t);
  for (const script of ["warm.sh", "refresh.sh", "check.sh", "clean.sh"]) {
    const result = f.run(script, ["--help"]);
    assert.equal(result.status, 0, `${script}: ${result.stderr}`);
    assert.match(result.stdout, /Usage:/, script);
  }
  assert.deepEqual(await f.recorded(), []);
});

test("warm.sh refuses a repo URL that carries credentials, before any sbx call", { skip }, async (t) => {
  const f = await fixture(t);
  const result = f.run("warm.sh", ["--repo", "https://user:not-a-real-token@example.invalid/x.git"]);
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /must not contain credentials/);
  assert.deepEqual(await f.recorded(), []);
});

test("warm.sh refuses a bad sandbox name and an unknown flag", { skip }, async (t) => {
  const f = await fixture(t);
  assert.notEqual(f.run("warm.sh", ["--name", "bad name!"]).status, 0);
  assert.notEqual(f.run("warm.sh", ["--bogus"]).status, 0);
  assert.equal((await f.recorded()).some((call) => call.includes("create")), false);
});

test("clean.sh removes exactly the named sandbox with rm --force and verifies the listing", { skip }, async (t) => {
  const f = await fixture(t);
  const result = f.run("clean.sh", ["box-one"]);
  assert.equal(result.status, 0, result.stderr);
  const calls = await f.recorded();
  assert.ok(calls.includes("--cloud rm --force box-one"), calls.join("\n"));
  assert.ok(calls.some((call) => call.endsWith("--cloud ls")), "lists afterwards to verify");
  assert.equal(calls.some((call) => /\brm\b(?!.*--force)/.test(call)), false, "never rm without --force");
});

test("clean.sh with nothing recorded removes nothing", { skip }, async (t) => {
  const f = await fixture(t);
  const result = f.run("clean.sh", []);
  assert.equal(result.status, 0, result.stderr);
  assert.equal((await f.recorded()).some((call) => call.includes(" rm ")), false);
});
