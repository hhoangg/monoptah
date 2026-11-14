// Fork-local (Monoptah). Safety checks of scripts/worktrees-clean.mjs against
// throwaway git repos. gh, lsof and ps are faked; git is real.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { existsSync, realpathSync, utimesSync } from "node:fs";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test as nodeTest } from "node:test";
import {
  REGENERABLE_IGNORED, blockingProcesses, evaluateWorktree, parseArgs, parseLsof, parseWorktrees, run, runClean, staleLogDirs,
} from "./worktrees-clean.mjs";

// Needs POSIX git/lsof behaviour; CI also runs native:test:dev on Windows.
const test = (name, fn) => nodeTest(name, { skip: process.platform === "win32" }, fn);

const git = (cwd, ...args) => {
  const r = spawnSync("git", ["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false", ...args], { cwd, encoding: "utf8" });
  assert.equal(r.status, 0, `git ${args.join(" ")}: ${r.stderr}`);
  return r.stdout.trim();
};

async function fixture(t) {
  const root = realpathSync(await mkdtemp(join(tmpdir(), "wt-clean-test-")));
  t.after(() => rm(root, { recursive: true, force: true }));
  const remote = join(root, "remote.git");
  const main = join(root, "main");
  git(root, "init", "--bare", "-b", "main", remote);
  git(root, "clone", remote, main);
  await writeFile(join(main, "a.txt"), "a\n");
  await writeFile(join(main, ".gitignore"), "target/\n");
  git(main, "add", ".");
  git(main, "commit", "-m", "init");
  git(main, "push", "origin", "HEAD:main");
  git(main, "fetch", "origin");
  const addWorktree = (name, branch) => {
    const path = join(root, name);
    git(main, "worktree", "add", "-b", branch, path, "origin/main");
    return { path, branch };
  };
  const commit = async (path, file = "b.txt") => {
    await writeFile(join(path, file), `${Math.random()}\n`);
    git(path, "add", ".");
    git(path, "commit", "-m", `edit ${file}`);
  };
  return { root, main, addWorktree, commit };
}

const noPrs = { run: (cmd, args, opts) => (cmd === "gh" ? { status: 0, stdout: "[]", stderr: "" } : run(cmd, args, opts)) };
const withPrs = (prs) => ({ run: (cmd, args, opts) => (cmd === "gh" ? { status: 0, stdout: JSON.stringify(prs), stderr: "" } : run(cmd, args, opts)) });
const noGh = { run: (cmd, args, opts) => (cmd === "gh" ? { status: null, stdout: "", stderr: "", error: new Error("spawn gh ENOENT") } : run(cmd, args, opts)) };
const idle = { cwds: [], ps: [] };

function evaluate(f, name, branch, deps, extra = {}) {
  const wt = parseWorktrees(git(f.main, "worktree", "list", "--porcelain")).find((w) => w.branch === branch || w.path.endsWith(name));
  return evaluateWorktree(wt, { deps, main: f.main, processes: idle, explicit: true, ...extra });
}

test("parses porcelain output including detached and locked entries", () => {
  const out = parseWorktrees("worktree /a\nHEAD 1\nbranch refs/heads/main\n\nworktree /b\nHEAD 2\ndetached\nlocked why\n");
  assert.deepEqual(out.map((w) => [w.path, w.branch, w.detached, w.locked]), [["/a", "main", undefined, undefined], ["/b", undefined, true, "why"]]);
});

test("parseArgs requires --merged or targets, not both", () => {
  assert.deepEqual(parseArgs(["--merged", "--dry-run"]), { targets: [], merged: true, dryRun: true, yes: false });
  assert.deepEqual(parseArgs(["feat/x", "--yes"]).targets, ["feat/x"]);
  assert.throws(() => parseArgs([]));
  assert.throws(() => parseArgs(["--merged", "feat/x"]));
  assert.throws(() => parseArgs(["--force"]));
});

test("refuses the main checkout", async (t) => {
  const f = await fixture(t);
  const wt = parseWorktrees(git(f.main, "worktree", "list", "--porcelain"))[0];
  const v = evaluateWorktree(wt, { deps: noPrs, main: f.main, processes: idle });
  assert.equal(v.ok, false);
  assert.match(v.reasons[0], /main checkout/);
});

test("allows a clean worktree merged into origin/main", async (t) => {
  const f = await fixture(t);
  const { path, branch } = f.addWorktree("w1", "feat/one");
  await f.commit(path);
  git(path, "push", "origin", "feat/one");
  git(f.main, "push", "origin", "feat/one:main");
  git(f.main, "fetch", "--prune", "origin");
  const v = evaluate(f, "w1", branch, noPrs);
  assert.deepEqual(v.reasons, []);
  assert.equal(v.ok, true);
});

test("refuses an unmerged branch", async (t) => {
  const f = await fixture(t);
  const { path, branch } = f.addWorktree("w2", "feat/two");
  await f.commit(path);
  git(path, "push", "origin", "feat/two");
  const v = evaluate(f, "w2", branch, noPrs);
  assert.equal(v.ok, false);
  assert.match(v.reasons.join(), /not merged/);
});

test("refuses uncommitted and untracked changes but ignores ignored files", async (t) => {
  const f = await fixture(t);
  const { path, branch } = f.addWorktree("w3", "feat/three");
  await mkdir(join(path, "target"));
  await writeFile(join(path, "target", "big.bin"), "x");
  assert.equal(evaluate(f, "w3", branch, noPrs).ok, true);
  await writeFile(join(path, "new.txt"), "x");
  assert.match(evaluate(f, "w3", branch, noPrs).reasons.join(), /uncommitted or untracked/);
  await rm(join(path, "new.txt"));
  await writeFile(join(path, "a.txt"), "changed\n");
  assert.match(evaluate(f, "w3", branch, noPrs).reasons.join(), /uncommitted or untracked/);
});

test("squash merge: allowed only when the PR head equals the local tip", async (t) => {
  const f = await fixture(t);
  const { path, branch } = f.addWorktree("w4", "feat/four");
  await f.commit(path);
  const tip = git(path, "rev-parse", "HEAD");
  const pr = { number: 7, headRefOid: tip, mergedAt: "2026-01-01T00:00:00Z", baseRefName: "main" };
  // Remote branch gone (pruned), commits exist nowhere remote.
  const ok = evaluate(f, "w4", branch, withPrs([pr]));
  assert.deepEqual(ok.reasons, []);
  assert.equal(ok.remoteDelete, true);
  await f.commit(path, "later.txt");
  const stale = evaluate(f, "w4", branch, withPrs([pr]));
  assert.equal(stale.ok, false);
  assert.match(stale.reasons.join(), /local commits after merge/);
});

test("unmerged commits not on any remote are refused even if gh is down", async (t) => {
  const f = await fixture(t);
  const { path, branch } = f.addWorktree("w5", "feat/five");
  await f.commit(path);
  const v = evaluate(f, "w5", branch, noGh);
  assert.equal(v.ok, false);
  assert.match(v.reasons.join(), /PR lookup failed/);
});

test("gh missing does not block a branch merged by ancestry, but disables remote delete", async (t) => {
  const f = await fixture(t);
  const { branch } = f.addWorktree("w6", "feat/six");
  const v = evaluate(f, "w6", branch, noGh);
  assert.equal(v.ok, true);
  assert.equal(v.remoteDelete, false);
});

test("scan mode keeps a fresh branch with no commits of its own", async (t) => {
  const f = await fixture(t);
  const { branch } = f.addWorktree("w7", "feat/seven");
  assert.match(evaluate(f, "w7", branch, noPrs, { explicit: false }).reasons.join(), /no commits of its own/);
});

test("detached HEAD is allowed only when it is an ancestor of origin/main", async (t) => {
  const f = await fixture(t);
  const safe = join(f.root, "d1");
  git(f.main, "worktree", "add", "--detach", safe, "origin/main");
  const unsafe = join(f.root, "d2");
  git(f.main, "worktree", "add", "--detach", unsafe, "origin/main");
  await f.commit(unsafe);
  const list = parseWorktrees(git(f.main, "worktree", "list", "--porcelain"));
  const ev = (p) => evaluateWorktree(list.find((w) => w.path === p), { deps: noPrs, main: f.main, processes: idle, explicit: true });
  assert.equal(ev(safe).ok, true);
  const bad = ev(unsafe);
  assert.equal(bad.ok, false);
  assert.match(bad.reasons.join(), /not merged/);
});

test("locked worktrees and unknown process state are refused (fail closed)", async (t) => {
  const f = await fixture(t);
  const { path, branch } = f.addWorktree("w8", "feat/eight");
  assert.equal(evaluate(f, "w8", branch, noPrs, { processes: undefined }).ok, false);
  git(f.main, "worktree", "lock", path);
  assert.match(evaluate(f, "w8", branch, noPrs).reasons.join(), /locked/);
});

test("refuses a worktree with a process cwd inside it or Native Dev running from it", () => {
  const wt = realpathSync(tmpdir());
  const inside = join(wt, "sub");
  const cwds = parseLsof(`p10\ncnode\nn${inside}\np11\ncvim\nn/elsewhere\n`);
  assert.equal(blockingProcesses(wt, { cwds, ps: [], selfPid: 1 }).length, 1);
  const dev = blockingProcesses(wt, {
    cwds: [{ pid: 20, command: "node", cwd: wt }],
    ps: [{ pid: 20, command: "node scripts/dev-native.mjs" }],
    selfPid: 1,
  });
  assert.ok(dev.some((r) => /Native Dev/.test(r)));
  assert.deepEqual(blockingProcesses(wt, { cwds: [{ pid: 1, command: "self", cwd: wt }], ps: [], selfPid: 1 }), []);
});

test("staleLogDirs picks only old monoptah-native-dev-* directories", async (t) => {
  const tmp = await mkdtemp(join(tmpdir(), "wt-clean-logs-"));
  t.after(() => rm(tmp, { recursive: true, force: true }));
  for (const name of ["monoptah-native-dev-old", "monoptah-native-dev-new", "other-old"]) await mkdir(join(tmp, name));
  const old = new Date(Date.now() - 3 * 86400_000);
  for (const name of ["monoptah-native-dev-old", "other-old"]) utimesSync(join(tmp, name), old, old);
  assert.deepEqual(staleLogDirs(tmp), [join(tmp, "monoptah-native-dev-old")]);
});

test("runClean removes a merged worktree and its branches, and dry-run removes nothing", async (t) => {
  const f = await fixture(t);
  const { path } = f.addWorktree("w9", "feat/nine");
  await mkdir(join(path, "target"));
  await writeFile(join(path, "target", "big.bin"), "x");
  const lines = [];
  const base = { deps: noPrs, log: (l) => lines.push(l), isTty: false, tmp: join(f.root, "empty"), cwd: f.main };
  // Fake lsof/ps through deps.
  const deps = { run: (cmd, args, opts) => {
    if (cmd === "lsof") return { status: 0, stdout: `p1\ncinit\nn/\n`, stderr: "" };
    if (cmd === "ps") return { status: 0, stdout: "1 init\n", stderr: "" };
    return noPrs.run(cmd, args, opts);
  } };
  const dry = await runClean({ targets: ["feat/nine"], merged: false, dryRun: false, yes: false }, { ...base, deps });
  assert.equal(dry.dryRun, true);
  assert.ok(existsSync(path));
  const real = await runClean({ targets: ["feat/nine"], merged: false, dryRun: false, yes: true }, { ...base, deps });
  assert.deepEqual(real.removed, [path]);
  assert.equal(existsSync(path), false);
  assert.equal(git(f.main, "branch", "--list", "feat/nine"), "");
});

test("refuses non-regenerable ignored files but not target/ or node_modules/", async (t) => {
  const f = await fixture(t);
  const { path, branch } = f.addWorktree("w10", "feat/ten");
  await writeFile(join(path, ".gitignore"), "target/\nnode_modules/\n.env\n*.db\n");
  git(path, "add", ".gitignore");
  git(path, "commit", "-m", "ignore");
  await mkdir(join(path, "node_modules"));
  await writeFile(join(path, "node_modules", "x"), "x");
  assert.ok(REGENERABLE_IGNORED.has("target"));
  assert.equal(evaluate(f, "w10", branch, noPrs).ok, false); // unpushed commit; only checking the reasons below
  await writeFile(join(path, ".env"), "SECRET=1\n");
  await writeFile(join(path, "local.db"), "x");
  const reasons = evaluate(f, "w10", branch, noPrs).reasons.join();
  assert.match(reasons, /ignored files that are not regenerable: .*\.env/);
  assert.match(reasons, /local\.db/);
  assert.doesNotMatch(reasons, /node_modules/);
});

test("runClean skips a worktree whose state changed after the plan was shown", async (t) => {
  const f = await fixture(t);
  const { path } = f.addWorktree("w11", "feat/eleven");
  const deps = { run: (cmd, args, opts) => {
    if (cmd === "lsof") return { status: 0, stdout: "p1\ncinit\nn/\n", stderr: "" };
    if (cmd === "ps") return { status: 0, stdout: "1 init\n", stderr: "" };
    return noPrs.run(cmd, args, opts);
  } };
  const lines = [];
  // The confirmation prompt is where a user could keep working: dirty the tree there.
  const ask = async () => { await writeFile(join(path, "late.txt"), "x"); return true; };
  const res = await runClean({ targets: ["feat/eleven"], merged: false, dryRun: false, yes: false },
    { deps, log: (l) => lines.push(l), isTty: true, ask, tmp: join(f.root, "empty"), cwd: f.main });
  assert.deepEqual(res.removed, []);
  assert.ok(existsSync(path));
  assert.ok(lines.some((l) => /SKIPPED .*state changed/.test(l)));
});

test("lsof: exit 1 with no output is no processes; stderr errors are unknown", async (t) => {
  const f = await fixture(t);
  f.addWorktree("w12", "feat/twelve");
  const mk = (lsof) => ({ run: (cmd, args, opts) => {
    if (cmd === "lsof") return lsof;
    if (cmd === "ps") return { status: 0, stdout: "1 init\n", stderr: "" };
    return noPrs.run(cmd, args, opts);
  } });
  const go = (lsof) => { const lines = []; return runClean({ targets: ["feat/twelve"], merged: false, dryRun: true, yes: false },
    { deps: mk(lsof), log: (l) => lines.push(l), isTty: false, tmp: join(f.root, "empty"), cwd: f.main }).then(() => lines.join("\n")); };
  assert.match(await go({ status: 1, stdout: "", stderr: "lsof: WARNING: can't stat() fuse" }), /Cannot inspect running processes/);
  assert.match(await go({ status: 2, stdout: "", stderr: "" }), /Cannot inspect running processes/);
  assert.doesNotMatch(await go({ status: 1, stdout: "", stderr: "" }), /Cannot inspect/);
});
