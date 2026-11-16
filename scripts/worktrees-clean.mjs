// Fork-local (Monoptah). Removes worktrees whose work is safely merged, plus the
// local/remote branch and stale native-dev temp logs. Fails closed: any error or
// unknown in a check means the worktree is kept.
import { spawnSync } from "node:child_process";
import { existsSync, lstatSync, readdirSync, realpathSync } from "node:fs";
import { rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { basename, join, resolve, sep } from "node:path";
import { createInterface } from "node:readline";
import { pathToFileURL } from "node:url";

const PROTECTED_BRANCHES = new Set(["main", "master"]);
const MAIN_REF = "origin/main";
// Ignored directories that a build regenerates; any other ignored file (.env, local DBs, notes) blocks removal.
export const REGENERABLE_IGNORED = new Set(["target", "node_modules", "dist", "dist-ssr", "build", ".turbo", ".vite", "coverage", "playwright-report", "test-results", ".next", ".cache", ".tsbuildinfo"]);
const LOG_PREFIX = "monoptah-native-dev-";
const LOG_MAX_AGE_MS = 24 * 60 * 60 * 1000;
// Cargo target pruning. The native workspace builds into <worktree>/crates/monocode-gpui/target
// and the root workspace into <worktree>/target.
const NATIVE_TARGET = join("crates", "monocode-gpui", "target");
const ROOT_TARGET = "target";
// The only profile the native workspace uses is native-dev, so its plain `debug` dir is leftover.
// Only removed from the native target: the root workspace (src-tauri) legitimately builds into debug.
const UNUSED_NATIVE_PROFILE = "debug";
// Worktrees whose build artifacts are needed as the seed for new worktrees and for releases: only
// stale incremental data is pruned there, never a profile dir.
const SEED_WORKTREE_NAMES = new Set(["release-main"]);
// An incremental dir (<crate>-<hash>) nobody wrote to for this long belongs to a hash that is no
// longer built (changed path, flags or toolchain).
const INCREMENTAL_ORPHAN_MAX_AGE_MS = 14 * 24 * 60 * 60 * 1000;
// Inside a live incremental dir only the newest session (s-*) is read; older ones are dead weight
// once they are this old.
const SESSION_MAX_AGE_MS = 24 * 60 * 60 * 1000;

export function run(command, args, { cwd } = {}) {
  const result = spawnSync(command, args, { cwd, encoding: "utf8", maxBuffer: 64 * 1024 * 1024, stdio: ["ignore", "pipe", "pipe"] });
  return { status: result.status, stdout: result.stdout ?? "", stderr: result.stderr ?? "", error: result.error };
}

function must(result, what) {
  if (result.error || result.status !== 0) {
    throw new Error(`${what} failed: ${result.error?.message || result.stderr.trim() || `exit ${result.status}`}`);
  }
  return result.stdout.trim();
}

export function realPath(path) {
  try { return realpathSync(path); } catch { return resolve(path); }
}

export function isInside(path, dir) {
  const a = realPath(path);
  const b = realPath(dir);
  return a === b || a.startsWith(b.endsWith(sep) ? b : b + sep);
}

export function parseWorktrees(porcelain) {
  const entries = [];
  for (const block of porcelain.split(/\n\n+/)) {
    const entry = {};
    for (const line of block.split("\n")) {
      if (!line) continue;
      const space = line.indexOf(" ");
      const key = space === -1 ? line : line.slice(0, space);
      const value = space === -1 ? "" : line.slice(space + 1);
      if (key === "worktree") entry.path = value;
      else if (key === "HEAD") entry.head = value;
      else if (key === "branch") entry.branch = value.replace(/^refs\/heads\//, "");
      else if (key === "detached") entry.detached = true;
      else if (key === "bare") entry.bare = true;
      else if (key === "locked") entry.locked = value || true;
      else if (key === "prunable") entry.prunable = value || true;
    }
    if (entry.path) entries.push(entry);
  }
  return entries;
}

// `lsof -a -d cwd -Fpcn` -> [{ pid, command, cwd }]
export function parseLsof(output) {
  const procs = [];
  let current;
  for (const line of output.split("\n")) {
    if (line.startsWith("p")) { current = { pid: Number(line.slice(1)), command: "", cwd: "" }; procs.push(current); }
    else if (line.startsWith("c") && current) current.command = line.slice(1);
    else if (line.startsWith("n") && current) current.cwd = line.slice(1);
  }
  return procs.filter((p) => p.cwd);
}

// `ps -axo pid=,command=` -> [{ pid, command }]
export function parsePs(output) {
  return output.split("\n").flatMap((line) => {
    const match = /^\s*(\d+)\s+(.*)$/.exec(line);
    return match ? [{ pid: Number(match[1]), command: match[2] }] : [];
  });
}

export function isNativeDevProcess(command) {
  return command.includes("Monoptah Native Dev.app") || /scripts[\\/]dev-native\.mjs/.test(command);
}

// Returns human-readable reasons for processes that pin the worktree.
export function blockingProcesses(worktree, { cwds, ps, selfPid = process.pid }) {
  const reasons = [];
  for (const proc of cwds) {
    if (proc.pid !== selfPid && isInside(proc.cwd, worktree)) {
      reasons.push(`process ${proc.pid} (${proc.command}) has its cwd inside the worktree`);
    }
  }
  const resolved = realPath(worktree);
  for (const proc of ps) {
    if (proc.pid === selfPid || !isNativeDevProcess(proc.command)) continue;
    const cwd = cwds.find((c) => c.pid === proc.pid)?.cwd;
    if ((cwd && isInside(cwd, worktree)) || proc.command.includes(resolved) || proc.command.includes(worktree)) {
      reasons.push(`Native Dev (pid ${proc.pid}) runs from this worktree`);
    }
  }
  return [...new Set(reasons)];
}

function lookupPrs(branch, deps, cwd) {
  const result = deps.run("gh", ["pr", "list", "--state", "merged", "--head", branch, "--base", "main",
    "--json", "number,headRefOid,mergedAt,baseRefName", "--limit", "50"], { cwd });
  if (result.error || result.status !== 0) {
    throw new Error(`gh unavailable or failed: ${result.error?.message || result.stderr.trim() || `exit ${result.status}`}`);
  }
  const prs = JSON.parse(result.stdout || "[]");
  if (!Array.isArray(prs)) throw new Error("unexpected gh output");
  return prs.filter((pr) => pr.mergedAt && pr.baseRefName === "main" && typeof pr.headRefOid === "string");
}

// Decides whether one worktree may be removed. Never throws: every error is a refusal.
export function evaluateWorktree(wt, ctx) {
  const { deps, main, explicit = false } = ctx;
  const reasons = [];
  const verdict = { wt, path: wt.path, branch: wt.branch, ok: false, reasons, pr: null, remoteDelete: false, prError: null };
  const guard = (name, fn) => {
    try { fn(); } catch (error) { reasons.push(`${name}: ${error.message} (treated as unsafe)`); }
  };
  const git = (args, cwd = wt.path) => deps.run("git", args, { cwd });

  if (!existsSync(wt.path)) { reasons.push("directory is missing (run git worktree prune)"); return verdict; }
  if (realPath(wt.path) === realPath(main)) { reasons.push("main checkout"); return verdict; }
  if (wt.bare) { reasons.push("bare repository"); return verdict; }
  if (wt.locked) { reasons.push(`locked${typeof wt.locked === "string" ? ` (${wt.locked})` : ""}`); return verdict; }
  if (wt.branch && PROTECTED_BRANCHES.has(wt.branch)) { reasons.push(`protected branch ${wt.branch}`); return verdict; }

  let tip;
  guard("resolve HEAD", () => { tip = must(git(["rev-parse", "--verify", "HEAD"]), "git rev-parse HEAD"); });
  if (!tip) return verdict;
  verdict.tip = tip;

  guard("working tree", () => {
    const status = must(git(["status", "--porcelain=v1", "--untracked-files=normal", "--ignore-submodules=none"]), "git status");
    if (status) reasons.push(`uncommitted or untracked changes (${status.split("\n").length} paths)`);
  });

  guard("ignored files", () => {
    const out = must(git(["status", "--ignored", "--porcelain=v1", "--untracked-files=normal"]), "git status --ignored");
    const keep = out.split("\n").filter((l) => l.startsWith("!! "))
      .map((l) => l.slice(3).replace(/\/$/, ""))
      .filter((path) => !path.split("/").some((seg) => REGENERABLE_IGNORED.has(seg)));
    if (keep.length) reasons.push(`ignored files that are not regenerable: ${keep.slice(0, 5).join(", ")}${keep.length > 5 ? `, +${keep.length - 5} more` : ""}`);
  });

  guard("submodules", () => {
    if (existsSync(join(wt.path, ".gitmodules"))) reasons.push("has submodules");
  });

  guard("hooks", () => {
    const result = git(["config", "--get", "core.hooksPath"]);
    if (result.error) throw result.error;
    const hooksPath = result.stdout.trim();
    if (result.status === 0 && hooksPath && isInside(resolve(wt.path, hooksPath), wt.path)) {
      reasons.push("core.hooksPath points inside the worktree");
    }
  });

  let ancestor = false;
  guard("merge state", () => {
    const result = git(["merge-base", "--is-ancestor", tip, MAIN_REF]);
    if (result.error || (result.status !== 0 && result.status !== 1)) {
      throw new Error(result.error?.message || result.stderr.trim() || `exit ${result.status}`);
    }
    ancestor = result.status === 0;
  });

  let prMatch = null;
  if (wt.branch) {
    try {
      const prs = lookupPrs(wt.branch, deps, main);
      prMatch = prs.find((pr) => pr.headRefOid === tip) ?? null;
      verdict.pr = prMatch;
      if (!prMatch && prs.length) verdict.prNote = `PR #${prs[0].number} merged, but the branch tip differs from its head (local commits after merge)`;
    } catch (error) {
      verdict.prError = error.message;
    }
  }

  const merged = ancestor || Boolean(prMatch);
  if (!merged) {
    reasons.push(verdict.prNote
      || (verdict.prError ? `not merged into ${MAIN_REF} and PR lookup failed (${verdict.prError})` : `not merged into ${MAIN_REF} and no merged PR with this exact head`));
  }

  if (merged) {
    guard("unpushed commits", () => {
      const out = must(git(["rev-list", tip, "--not", "--remotes"]), "git rev-list");
      if (out && !prMatch) reasons.push(`${out.split("\n").length} commit(s) not on any remote branch`);
    });
  }

  if (wt.branch && ancestor && !prMatch && !explicit) {
    guard("fresh branch", () => {
      const log = must(git(["reflog", "show", "--format=%gs", `refs/heads/${wt.branch}`]), "git reflog");
      const lines = log.split("\n").filter(Boolean);
      if (lines.length <= 1 && (lines[0] ?? "").startsWith("branch: Created from")) {
        reasons.push("branch has no commits of its own yet (looks like new work, name it explicitly to remove)");
      }
    });
  }

  guard("running processes", () => {
    if (!ctx.processes) throw new Error("process list unavailable");
    reasons.push(...blockingProcesses(wt.path, ctx.processes));
  });

  verdict.remoteDelete = Boolean(prMatch);
  verdict.ok = reasons.length === 0;
  return verdict;
}

export function formatSize(kib) {
  if (kib >= 1024 * 1024) return `${(kib / 1024 / 1024).toFixed(1)} GiB`;
  if (kib >= 1024) return `${(kib / 1024).toFixed(1)} MiB`;
  return `${kib} KiB`;
}

export function dirSizeKib(path, deps) {
  const result = deps.run("du", ["-sk", path]);
  const kib = Number.parseInt(result.stdout, 10);
  return result.status === 0 && Number.isFinite(kib) ? kib : 0;
}

export function staleLogDirs(tmp, now = Date.now(), maxAge = LOG_MAX_AGE_MS) {
  let names;
  try { names = readdirSync(tmp); } catch { return []; }
  return names.flatMap((name) => {
    if (!name.startsWith(LOG_PREFIX)) return [];
    const path = join(tmp, name);
    try {
      const stat = lstatSync(path);
      return stat.isDirectory() && now - stat.mtimeMs > maxAge ? [path] : [];
    } catch { return []; }
  });
}

function entries(path) {
  try { return readdirSync(path, { withFileTypes: true }); } catch { return []; }
}

function newestMtime(path) {
  let newest = 0;
  try { newest = lstatSync(path).mtimeMs; } catch { return 0; }
  for (const entry of entries(path)) {
    try { newest = Math.max(newest, lstatSync(join(path, entry.name)).mtimeMs); } catch { /* vanished */ }
  }
  return newest;
}

// Profile dirs directly under a target dir (and under <target>/<triple> for cross builds).
function profileDirs(target) {
  const found = [];
  for (const entry of entries(target)) {
    if (!entry.isDirectory()) continue;
    const dir = join(target, entry.name);
    if (existsSync(join(dir, "incremental"))) found.push({ dir, name: entry.name });
    else for (const inner of entries(dir)) {
      if (inner.isDirectory() && existsSync(join(dir, inner.name, "incremental"))) found.push({ dir: join(dir, inner.name), name: inner.name });
    }
  }
  return found;
}

// Lists stale cargo build data of one worktree as [{ path, reason }] (never deletes anything).
// - `crates/monocode-gpui/target/debug` (unused profile) unless the worktree seeds others.
// - Incremental dirs: a <crate>-<hash> dir untouched for 14 days; in the others, every s-* session
//   dir (and its .lock) except the newest that is older than 1 day. `-working` dirs are never touched.
// Every path is a real directory (symlinks are skipped) inside the worktree.
export function cargoPruneCandidates(worktree, { now = Date.now(), seed = false } = {}) {
  const out = [];
  const inside = (path) => {
    try { return !lstatSync(path).isSymbolicLink() && isInside(path, worktree); } catch { return false; }
  };
  const nativeTarget = join(worktree, NATIVE_TARGET);
  if (!seed && inside(join(nativeTarget, UNUSED_NATIVE_PROFILE)) && lstatSync(join(nativeTarget, UNUSED_NATIVE_PROFILE)).isDirectory()) {
    out.push({ path: join(nativeTarget, UNUSED_NATIVE_PROFILE), reason: "unused cargo profile (the native workspace uses native-dev)" });
  }
  for (const target of [nativeTarget, join(worktree, ROOT_TARGET)]) {
    if (!inside(target)) continue;
    for (const { dir, name } of profileDirs(target)) {
      if (target === nativeTarget && dir === join(nativeTarget, UNUSED_NATIVE_PROFILE) && !seed) continue; // removed as a whole
      const incremental = join(dir, "incremental");
      if (!inside(incremental)) continue;
      for (const crate of entries(incremental)) {
        const crateDir = join(incremental, crate.name);
        if (!crate.isDirectory() || !inside(crateDir)) continue;
        if (now - newestMtime(crateDir) > INCREMENTAL_ORPHAN_MAX_AGE_MS) {
          out.push({ path: crateDir, reason: `incremental dir for ${crate.name} unused for over 14 days (${name})` });
          continue;
        }
        const sessions = entries(crateDir).filter((e) => e.isDirectory() && e.name.startsWith("s-") && !e.name.endsWith("-working"))
          .map((e) => ({ name: e.name, mtime: lstatSync(join(crateDir, e.name)).mtimeMs }))
          .sort((a, b) => b.mtime - a.mtime);
        for (const session of sessions.slice(1)) {
          if (now - session.mtime <= SESSION_MAX_AGE_MS) continue;
          out.push({ path: join(crateDir, session.name), reason: `old incremental session of ${crate.name} (${name})` });
          // Cargo's lock file for the session is s-<ts>-<hash>.lock, next to the session dir.
          const lock = join(crateDir, `${session.name.replace(/-[a-z0-9]+$/, "")}.lock`);
          if (existsSync(lock) && inside(lock)) out.push({ path: lock, reason: "lock of the old session above" });
        }
      }
    }
  }
  return out;
}

export function isSeedWorktree(wt, main) {
  return realPath(wt.path) === realPath(main) || SEED_WORKTREE_NAMES.has(basename(wt.path));
}

export function parseArgs(argv) {
  const options = { targets: [], merged: false, dryRun: false, yes: false, pruneTarget: false, noPruneTarget: false };
  for (const arg of argv) {
    if (arg === "--merged") options.merged = true;
    else if (arg === "--prune-target") options.pruneTarget = true;
    else if (arg === "--no-prune-target") options.noPruneTarget = true;
    else if (arg === "--dry-run") options.dryRun = true;
    else if (arg === "--yes" || arg === "-y") options.yes = true;
    else if (arg === "--help" || arg === "-h") options.help = true;
    else if (arg.startsWith("--")) throw new Error(`Unknown option ${arg}`);
    else options.targets.push(arg);
  }
  if (options.pruneTarget && options.noPruneTarget) throw new Error("--prune-target and --no-prune-target conflict.");
  const pruneOnly = options.pruneTarget && !options.merged && options.targets.length === 0;
  if (!options.help && !pruneOnly && options.merged === (options.targets.length > 0)) {
    throw new Error("Pass either --merged or one or more worktree paths/branches (or --prune-target alone).");
  }
  return options;
}

const USAGE = "Usage: npm run worktrees:clean -- [<worktree path|branch>...] [--merged] [--prune-target] [--no-prune-target] [--dry-run] [--yes]\n"
  + "  --merged           also prunes stale cargo build data of the worktrees that stay (skip with --no-prune-target)\n"
  + "  --prune-target     prune stale cargo build data only (no worktree is removed)";

export function resolveTargets(targets, worktrees) {
  return targets.map((target) => {
    const byBranch = worktrees.find((wt) => wt.branch === target);
    const byPath = worktrees.find((wt) => realPath(wt.path) === realPath(target));
    const wt = byPath ?? byBranch;
    if (!wt) throw new Error(`No worktree matches ${target}`);
    return wt;
  });
}

function collectProcesses(deps) {
  const lsof = deps.run("lsof", ["-a", "-d", "cwd", "-Fpcn"]);
  if (lsof.error) throw new Error(`lsof unavailable: ${lsof.error.message}`);
  // lsof exits 1 when nothing matches; that is only "no processes" if it also printed nothing to stderr.
  if (lsof.status !== 0 && !(lsof.status === 1 && lsof.stderr.trim() === "") && !lsof.stdout.trim()) {
    throw new Error(`lsof failed: ${lsof.stderr.trim() || `exit ${lsof.status}`}`);
  }
  const cwds = parseLsof(lsof.stdout);
  const ps = deps.run("ps", ["-axo", "pid=,command="]);
  if (ps.error || ps.status !== 0) throw new Error("ps failed");
  return { cwds, ps: parsePs(ps.stdout) };
}

async function confirm(question) {
  const rl = createInterface({ input: process.stdin, output: process.stdout });
  try {
    return await new Promise((done) => rl.question(question, (answer) => done(/^y(es)?$/i.test(answer.trim()))));
  } finally { rl.close(); }
}

export async function runClean(options, {
  deps = { run },
  log = console.log,
  isTty = Boolean(process.stdin.isTTY && process.stdout.isTTY),
  ask = confirm,
  tmp = tmpdir(),
  now = Date.now(),
  cwd = process.cwd(),
} = {}) {
  const worktrees = parseWorktrees(must(deps.run("git", ["worktree", "list", "--porcelain"], { cwd }), "git worktree list"));
  const main = worktrees[0].path;
  must(deps.run("git", ["fetch", "--prune", "origin"], { cwd: main }), "git fetch --prune origin");

  let processes;
  try { processes = collectProcesses(deps); } catch (error) { log(`Cannot inspect running processes: ${error.message}. Every worktree will be kept.`); }

  const pruneOnly = options.pruneTarget && !options.merged && options.targets.length === 0;
  const explicit = !options.merged;
  const candidates = pruneOnly ? []
    : explicit
      ? resolveTargets(options.targets, worktrees)
      : worktrees.filter((wt) => realPath(wt.path) !== realPath(main));
  const verdicts = candidates.map((wt) => evaluateWorktree(wt, { deps, main, processes, explicit }));

  const removable = verdicts.filter((v) => v.ok).map((v) => ({ ...v, kib: dirSizeKib(v.path, deps) }));
  for (const v of removable) {
    log(`DELETE ${v.path}  [${v.branch ?? "detached"}]  ${formatSize(v.kib)}${v.pr ? `  (PR #${v.pr.number} merged)` : ""}`);
  }
  for (const v of verdicts.filter((x) => !x.ok)) log(`KEEP   ${v.path}  [${v.branch ?? "detached"}]  ${v.reasons.join("; ")}`);

  const logs = staleLogDirs(tmp, now).map((path) => ({ path, kib: dirSizeKib(path, deps) }));
  for (const l of logs) log(`DELETE ${l.path}  ${formatSize(l.kib)}  (temp log older than 1 day)`);

  // Stale cargo build data of the worktrees that stay. A worktree with a process inside, or when the
  // process list is unknown, is skipped: pruning under a running cargo could break its build.
  const prune = [];
  if ((options.merged || options.pruneTarget) && !options.noPruneTarget) {
    const removing = new Set(removable.map((v) => realPath(v.path)));
    for (const wt of worktrees) {
      if (!existsSync(wt.path) || wt.bare || removing.has(realPath(wt.path))) continue;
      if (!processes) { log(`NOPRUNE ${wt.path}  process list unavailable`); continue; }
      const busy = blockingProcesses(wt.path, processes);
      if (busy.length) { log(`NOPRUNE ${wt.path}  ${busy.join("; ")}`); continue; }
      for (const item of cargoPruneCandidates(wt.path, { now, seed: isSeedWorktree(wt, main) })) {
        prune.push({ ...item, worktree: wt.path, kib: dirSizeKib(item.path, deps) });
      }
    }
    for (const item of prune) log(`PRUNE  ${item.path}  ${formatSize(item.kib)}  (${item.reason})`);
  }

  const total = [...removable, ...logs, ...prune].reduce((sum, item) => sum + item.kib, 0);
  if (removable.length === 0 && logs.length === 0 && prune.length === 0) { log("Nothing to delete."); return { removed: [], freedKib: 0, dryRun: true }; }

  let dryRun = options.dryRun;
  if (!dryRun && !options.yes) {
    if (isTty) dryRun = !(await ask(`Delete ${removable.length} worktree(s), ${logs.length} log dir(s) and ${prune.length} stale cargo path(s), freeing ${formatSize(total)}? [y/N] `));
    else { log("Not a TTY and no --yes: dry run only."); dryRun = true; }
  }
  if (dryRun) { log(`Dry run: would free ${formatSize(total)}.`); return { removed: [], freedKib: 0, dryRun: true }; }

  let freed = 0;
  const removed = [];
  for (const v of removable) {
    // State may have changed during the prompt: evaluate again and skip on any difference.
    let again;
    try { again = evaluateWorktree(v.wt, { deps, main, processes: collectProcesses(deps), explicit }); } catch (error) { again = { ok: false, reasons: [error.message] }; }
    if (!again.ok || again.tip !== v.tip) { log(`SKIPPED ${v.path}: state changed before removal (${again.reasons?.join("; ") || "HEAD moved"})`); continue; }
    const result = deps.run("git", ["worktree", "remove", v.path], { cwd: main });
    if (result.error || result.status !== 0) { log(`FAILED ${v.path}: ${result.stderr.trim() || result.error?.message}`); continue; }
    freed += v.kib;
    removed.push(v.path);
    log(`removed worktree ${v.path}`);
    if (v.branch) {
      const del = deps.run("git", ["branch", "-d", v.branch], { cwd: main });
      if (del.status === 0) log(`deleted local branch ${v.branch}`);
      else log(`kept local branch ${v.branch} (git branch -d refused: ${del.stderr.trim().split("\n")[0]}). Delete it by hand if you are sure.`);
      if (v.remoteDelete) {
        const remote = deps.run("git", ["ls-remote", "--heads", "origin", v.branch], { cwd: main });
        const sha = remote.status === 0 ? remote.stdout.trim().split(/\s+/)[0] : "";
        if (sha && sha === v.pr.headRefOid) {
          const push = deps.run("git", ["push", "origin", "--delete", v.branch], { cwd: main });
          log(push.status === 0 ? `deleted remote branch ${v.branch}` : `kept remote branch ${v.branch}: ${push.stderr.trim().split("\n")[0]}`);
        } else if (sha) log(`kept remote branch ${v.branch} (its tip is not the merged PR head)`);
      }
    }
  }
  deps.run("git", ["worktree", "prune"], { cwd: main });
  for (const l of logs) {
    try { await rm(l.path, { recursive: true, force: true }); freed += l.kib; log(`removed ${l.path}`); } catch (error) { log(`FAILED ${l.path}: ${error.message}`); }
  }
  // Processes may have started during the confirmation prompt: look again, per worktree, before any rm.
  let freshProcesses = null;
  if (prune.length) { try { freshProcesses = collectProcesses(deps); } catch { /* unknown: prune nothing */ } }
  const blocked = new Map();
  for (const item of prune) {
    if (!blocked.has(item.worktree)) {
      const busy = freshProcesses ? blockingProcesses(item.worktree, freshProcesses) : ["process list unavailable"];
      blocked.set(item.worktree, busy);
      if (busy.length) log(`NOPRUNE ${item.worktree}  ${busy.join("; ")}`);
    }
  }
  for (const item of prune) {
    if (blocked.get(item.worktree).length) continue;
    // Re-check right before deleting: still a real path inside its own worktree, not a symlink.
    let safe = false;
    try { safe = !lstatSync(item.path).isSymbolicLink() && isInside(item.path, item.worktree) && realPath(item.path) !== realPath(item.worktree); } catch { /* gone */ }
    if (!safe) continue;
    try { await rm(item.path, { recursive: true, force: true }); freed += item.kib; log(`pruned ${item.path}`); } catch (error) { log(`FAILED ${item.path}: ${error.message}`); }
  }
  log(`Freed ${formatSize(freed)}.`);
  return { removed, freedKib: freed, dryRun: false };
}

async function main() {
  let options;
  try { options = parseArgs(process.argv.slice(2)); } catch (error) { console.error(`${error.message}\n${USAGE}`); process.exitCode = 2; return; }
  if (options.help) { console.log(USAGE); return; }
  await runClean(options);
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch((error) => { console.error(`[worktrees:clean] ${error.message}`); process.exitCode = 1; });
}
