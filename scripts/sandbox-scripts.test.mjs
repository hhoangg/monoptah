import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmod, mkdir, mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { createHash } from "node:crypto";

// The sandbox build box scripts (scripts/sandbox, docs/native/sandbox-build-box.md) are bash, so
// these tests run them with a fake `sbx` on PATH: nothing here reaches Docker.
const dir = fileURLToPath(new URL("./sandbox/", import.meta.url));
const skip = process.platform === "win32";

async function fixture(t) {
  const root = await mkdtemp(join(tmpdir(), "sandbox-scripts-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  const calls = join(root, "calls.log");
  const sbx = join(root, "sbx");
  // Records every call. `ls -q` lists no sandboxes, plain `ls` prints the empty-state line. `cp` can be
  // made to fail (FAKE_SBX_CP_FAIL) and keeps the uploaded patch; an `exec` that carries the poll
  // marker answers "finished, exit 0"; the launch exec is saved to launch.sh; `exec ... test` is "no".
  await writeFile(sbx, `#!/bin/sh
printf '%s\\n' "$*" >> "$FAKE_DIR/calls.log"
case "$*" in
  *"ls -q"*) exit 0 ;;
  *" ls") echo "No sandboxes found"; exit 0 ;;
esac
case "$2" in
  cp) case "$3" in *check.patch) cp "$3" "$FAKE_DIR/uploaded.patch" ;; esac
      [ -z "$FAKE_SBX_CP_FAIL" ] || exit 1 ;;
  exec) case "$6" in
          *@@N*) printf '@@N 0\\n@@RC 0\\n' ;;
          *setsid*) printf '%s' "$6" > "$FAKE_DIR/launch.sh" ;;
        esac
        [ "$4" != test ] || exit 1 ;;
esac
exit 0
`);
  await chmod(sbx, 0o755);
  const run = (script, args, vars = {}, cwd = undefined) => spawnSync("bash", [join(dir, script), ...args], {
    encoding: "utf8",
    cwd,
    env: { ...process.env, PATH: `${root}:${process.env.PATH}`, FAKE_DIR: root, BOX_STATE_DIR: join(root, "state"), BOX_POLL: "0", ...vars },
  });
  const recorded = async () => (await readFile(calls, "utf8").catch(() => "")).split("\n").filter(Boolean);
  return { root, run, recorded };
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

// --- check.sh: base choice, untracked files, quoting ----------------------------------------------
const MAINTAINER_FORK = "https://github.com/hhoangg/monoptah.git";
const UPSTREAM_PROJECT = "https://github.com/hardbeat920/monocode.git";

function git(cwd, ...args) {
  const result = spawnSync("git", ["-c", "user.name=t", "-c", "user.email=t@example.invalid", "-c", "commit.gpgsign=false", ...args], { cwd, encoding: "utf8" });
  assert.equal(result.status, 0, `git ${args.join(" ")}: ${result.stderr}`);
  return result.stdout.trim();
}

// A repo with three commits: r.txt (ROOT), a.txt (MID) and b.txt (HEAD). `remotes` maps a remote name
// to [url, "root" | "mid"]: where that remote's main points. The diff against a remote at "mid" is
// only b.txt; against one at "root" it also holds a.txt, so the patch shows which base was used.
async function repo(f, remotes) {
  const top = join(f.root, "repo");
  await mkdir(top);
  git(top, "init", "-q", "-b", "main");
  await writeFile(join(top, ".gitignore"), "ignored.key\n");
  await writeFile(join(top, "r.txt"), "r\n");
  git(top, "add", "-A");
  git(top, "commit", "-q", "-m", "root");
  const root = git(top, "rev-parse", "HEAD");
  await writeFile(join(top, "a.txt"), "a\n");
  git(top, "add", "-A");
  git(top, "commit", "-q", "-m", "mid");
  const mid = git(top, "rev-parse", "HEAD");
  await writeFile(join(top, "b.txt"), "b\n");
  git(top, "add", "-A");
  git(top, "commit", "-q", "-m", "head");
  for (const [name, [url, at]] of Object.entries(remotes)) {
    git(top, "remote", "add", name, url);
    git(top, "update-ref", `refs/remotes/${name}/main`, at === "root" ? root : mid);
  }
  return top;
}

const runCheck = (f, top, args = [], vars = {}) => f.run("check.sh", ["--name", "box-one", ...args], { FAKE_SBX_CP_FAIL: "1", ...vars }, top);
const uploaded = (f) => readFile(join(f.root, "uploaded.patch"), "utf8");

for (const [title, remotes, args, vars, chosen, inPatch] of [
  ["maintainer layout: origin is the repo the box clones, upstream is the original project",
    { origin: [MAINTAINER_FORK, "mid"], upstream: [UPSTREAM_PROJECT, "root"] }, [], {}, "origin/main", false],
  ["this Mac's layout: origin is a fork, upstream (ssh URL) is the repo the box clones",
    { origin: ["https://github.com/someone/monoptah.git", "root"], upstream: ["git@github.com:hhoangg/monoptah.git", "mid"] }, [], {}, "upstream/main", false],
  ["URL match ignores a missing .git, a trailing slash and case",
    { fork: ["https://GitHub.com/HHoangg/Monoptah/", "mid"], origin: ["https://github.com/someone/else.git", "root"] }, [], {}, "fork/main", false],
  ["no matching remote falls back to origin/main",
    { origin: ["https://github.com/someone/else.git", "root"], upstream: [UPSTREAM_PROJECT, "mid"] }, [], {}, "origin/main", true],
  ["--base overrides the match",
    { origin: [MAINTAINER_FORK, "mid"], upstream: [UPSTREAM_PROJECT, "root"] }, ["--base", "upstream/main"], {}, "upstream/main", true],
  ["BOX_BASE overrides the match",
    { origin: [MAINTAINER_FORK, "mid"], upstream: [UPSTREAM_PROJECT, "root"] }, [], { BOX_BASE: "upstream/main" }, "upstream/main", true],
  ["BOX_REPO_URL picks the remote of another project",
    { origin: [MAINTAINER_FORK, "mid"], upstream: [UPSTREAM_PROJECT, "root"] }, [], { BOX_REPO_URL: "https://github.com/hardbeat920/monocode" }, "upstream/main", true],
]) {
  test(`check.sh base: ${title}`, { skip }, async (t) => {
    const f = await fixture(t);
    const result = runCheck(f, await repo(f, remotes), args, vars);
    assert.match(result.stderr, /could not copy the patch/, result.stderr);
    assert.ok(result.stderr.includes(`(${chosen}: `), `chosen ${chosen}\n${result.stderr}`);
    const patch = await uploaded(f);
    assert.match(patch, /diff --git a\/b\.txt/);
    assert.equal(/diff --git a\/a\.txt/.test(patch), inPatch, "a.txt is in the patch only when the base is the older commit");
  });
}

test("check.sh base: no matching remote and no origin/main fails and names the escape hatch", { skip }, async (t) => {
  const f = await fixture(t);
  const result = runCheck(f, await repo(f, { upstream: [UPSTREAM_PROJECT, "mid"] }));
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /no base ref: pass --base REF or set BOX_BASE/);
  assert.equal((await f.recorded()).some((call) => call.includes(" cp ")), false);
});

async function withUntracked(t, files) {
  const f = await fixture(t);
  const top = await repo(f, { origin: [MAINTAINER_FORK, "mid"] });
  for (const [name, text] of Object.entries(files)) await writeFile(join(top, name), text);
  return { f, top };
}

test("check.sh lists untracked files, uploads them, and skips git-ignored ones", { skip }, async (t) => {
  const { f, top } = await withUntracked(t, { "notes.txt": "n\n", "ignored.key": "ignored\n" });
  const result = runCheck(f, top);
  assert.match(result.stderr, /uploading 1 untracked, non-ignored file\(s\)/);
  assert.match(result.stderr, /^ {2}notes\.txt$/m);
  assert.doesNotMatch(result.stderr, /ignored\.key/);
  assert.match(await uploaded(f), /diff --git a\/notes\.txt/);
});

test("check.sh refuses secret-looking untracked files before any upload", { skip }, async (t) => {
  for (const secret of [".env", ".env.production", "server.pem", "deploy.key", "id_rsa", "id_rsa.pub", "credentials.json", "cert.p12", "api.token", "sub/ID_ED25519"]) {
    const { f, top } = await withUntracked(t, { notes: "n\n" });
    await mkdir(join(top, "sub"), { recursive: true });
    await writeFile(join(top, secret), "not a real secret\n");
    const result = runCheck(f, top);
    assert.notEqual(result.status, 0, secret);
    assert.match(result.stderr, /WARNING: secret-looking untracked file/, secret);
    assert.match(result.stderr, /refusing to upload them/, secret);
    assert.equal((await f.recorded()).some((call) => call.includes(" cp ")), false, `${secret} must not be uploaded`);
    await rm(join(f.root, "repo"), { recursive: true, force: true });
  }
});

test("check.sh uploads secret-looking untracked files only with BOX_UPLOAD_UNTRACKED_SECRETS=1", { skip }, async (t) => {
  const { f, top } = await withUntracked(t, { ".env": "not a real secret\n" });
  const result = runCheck(f, top, [], { BOX_UPLOAD_UNTRACKED_SECRETS: "1" });
  assert.match(result.stderr, /WARNING: secret-looking/);
  assert.match(await uploaded(f), /diff --git a\/\.env/);
});

test("check.sh --no-untracked and BOX_NO_UNTRACKED=1 leave untracked files out, secrets included", { skip }, async (t) => {
  for (const [args, vars] of [[["--no-untracked"], {}], [[], { BOX_NO_UNTRACKED: "1" }]]) {
    const { f, top } = await withUntracked(t, { "notes.txt": "n\n", ".env": "not a real secret\n" });
    const result = runCheck(f, top, args, vars);
    assert.match(result.stderr, /2 untracked file\(s\) left out/);
    assert.doesNotMatch(result.stderr, /refusing/);
    const patch = await uploaded(f);
    assert.match(patch, /diff --git a\/b\.txt/);
    assert.doesNotMatch(patch, /notes\.txt|\.env/);
  }
});

test("check.sh keeps a BOX_HOME with spaces as one argument through the remote launch", { skip }, async (t) => {
  const f = await fixture(t);
  const top = await repo(f, { origin: [MAINTAINER_FORK, "mid"] });
  await writeFile(join(f.root, "setsid"), '#!/bin/sh\nexec "$@"\n'); // setsid is absent on macOS
  await chmod(join(f.root, "setsid"), 0o755);
  const boxHome = join(f.root, "box home", "x y");
  const result = f.run("check.sh", ["--name", "box-one"], { BOX_HOME: boxHome }, top);
  assert.equal(result.status, 0, result.stderr);
  assert.ok((await f.recorded()).some((call) => call.endsWith(`box-one:${boxHome}/check.patch`)), "patch copied to the spaced path");
  // Run the launch command the way the sandbox's bash would, with a stub inner.sh recording its argv.
  const home = join(f.root, "sandbox-home");
  await mkdir(home);
  await writeFile(join(home, "box-inner.sh"), 'printf "%s\\n" "$#" "$@" > "$HOME/args"\n');
  const launch = await readFile(join(f.root, "launch.sh"), "utf8");
  const ran = spawnSync("bash", ["-c", launch], { encoding: "utf8", env: { ...process.env, HOME: home, PATH: `${f.root}:${process.env.PATH}` } });
  assert.equal(ran.status, 0, ran.stderr);
  for (let i = 0; i < 100 && !(await readFile(join(home, "box.rc"), "utf8").catch(() => "")); i += 1) await new Promise((r) => setTimeout(r, 50));
  assert.deepEqual((await readFile(join(home, "args"), "utf8")).split("\n").slice(0, 3), ["2", "check", `${boxHome}/check.patch`]);
});

// --- inner.sh: downloads are verified ---------------------------------------------------------------
const sha256 = (text) => createHash("sha256").update(text).digest("hex");

async function fetchVerified(t, { sha, body }) {
  const root = await mkdtemp(join(tmpdir(), "sandbox-fetch-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  await writeFile(join(root, "curl"), `#!/bin/sh\necho called >> '${join(root, "curl.log")}'\nwhile [ $# -gt 0 ]; do [ "$1" = -o ] && printf '%s' '${body}' > "$2"; shift; done\n`);
  await chmod(join(root, "curl"), 0o755);
  const source = await readFile(join(dir, "inner.sh"), "utf8");
  const fn = /^fetch_verified\(\) \{[\s\S]*?^\}/m.exec(source)[0];
  const out = join(root, "download");
  const result = spawnSync("bash", ["-c", `ts() { echo "$*" >&2; }\n${fn}\nfetch_verified "$@"`, "_", "https://example.invalid/x", sha, out], {
    encoding: "utf8", env: { ...process.env, PATH: `${root}:${process.env.PATH}` },
  });
  const called = (await readFile(join(root, "curl.log"), "utf8").catch(() => "")).length > 0;
  const kept = await readFile(out, "utf8").catch(() => null);
  return { result, called, kept };
}

const haveSha256sum = spawnSync("sha256sum", ["--version"]).error === undefined;

test("fetch_verified keeps a download whose sha256 matches", { skip: skip || !haveSha256sum }, async (t) => {
  const r = await fetchVerified(t, { sha: sha256("payload"), body: "payload" });
  assert.equal(r.result.status, 0, r.result.stderr);
  assert.equal(r.kept, "payload");
});

test("fetch_verified fails closed on a wrong hash, an empty hash and a malformed hash", { skip: skip || !haveSha256sum }, async (t) => {
  const wrong = await fetchVerified(t, { sha: sha256("other"), body: "payload" });
  assert.notEqual(wrong.result.status, 0);
  assert.match(wrong.result.stderr, /sha256 mismatch/);
  assert.equal(wrong.kept, null, "the mismatching file is removed");
  const empty = await fetchVerified(t, { sha: "", body: "payload" });
  assert.notEqual(empty.result.status, 0);
  assert.match(empty.result.stderr, /no pinned sha256/);
  assert.equal(empty.called, false, "nothing is downloaded without a hash");
  const bad = await fetchVerified(t, { sha: "abc123", body: "payload" });
  assert.notEqual(bad.result.status, 0);
  assert.equal(bad.called, false);
});

test("inner.sh no longer pipes a download straight into tar and pins four hashes", { skip }, async () => {
  const source = await readFile(join(dir, "inner.sh"), "utf8");
  assert.doesNotMatch(source, /curl[^\n]*\|\s*tar|\\\n\s*\|\s*tar/);
  assert.equal((source.match(/\b[0-9a-f]{64}\b/g) ?? []).length, 4, "node and nextest, x86_64 and aarch64");
});
