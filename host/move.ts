import { execFile } from "node:child_process";
import { createHash } from "node:crypto";
import {
  closeSync,
  copyFileSync,
  existsSync,
  fsyncSync,
  linkSync,
  mkdirSync,
  mkdtempSync,
  openSync,
  readFileSync,
  renameSync,
  rmSync,
  statSync,
  unlinkSync,
  writeSync,
} from "node:fs";
import { homedir, tmpdir } from "node:os";
import { isAbsolute, join, resolve, sep } from "node:path";
import { promisify } from "node:util";
import { RUNTIME_MODES, type Session } from "../src/features/sessions/model/session";
import { modelSettings, type HostEngine } from "./engine";
import { hostWorktrees, resolveHostWorktreeAsync } from "./git-worktrees";
import {
  CLAUDE_NAME_LIMIT,
  claudeProjectDirName,
  decide,
  forkNotice,
  forkTranscript,
  parseChain,
  probeLine,
  rewriteCwd,
  type Decision,
} from "./transcript-sync";
import { type HostUploads, validateUploadId } from "./uploads";

/**
 * Restoring a session that moved to this Host (design 4.2.2 steps 5-6, 4.9 R1/R4/R9):
 * `workspace.applySnapshot` puts the code back, `sessions.import` puts the conversation
 * back under the SAME Claude session id.
 */

const exec = promisify(execFile);
const HEX = /^(?:[0-9a-f]{40}|[0-9a-f]{64})$/;
const SESSION_ID = /^[A-Za-z0-9_-]{1,128}$/;
const GIT_TIMEOUT_MS = 120_000;
const MAX_BUFFER = 64 * 1024 * 1024;

function failure(error: unknown): Error {
  const stderr = (error as { stderr?: unknown })?.stderr;
  const first =
    typeof stderr === "string"
      ? stderr.split("\n").find((line) => line.trim())
      : undefined;
  if (first) return new Error(first.trim());
  return error instanceof Error ? error : new Error(String(error));
}

async function git(
  cwd: string,
  args: string[],
  env: NodeJS.ProcessEnv = {},
  timeout = GIT_TIMEOUT_MS,
  input?: string,
): Promise<string> {
  try {
    const child = exec("git", args, {
      cwd,
      timeout,
      maxBuffer: MAX_BUFFER,
      encoding: "utf8",
      env: { ...process.env, GIT_TERMINAL_PROMPT: "0", ...env },
    });
    if (input !== undefined) child.child.stdin?.end(input);
    const { stdout } = await child;
    return stdout;
  } catch (error) {
    throw failure(error);
  }
}

const gitOk = (cwd: string, args: string[]) =>
  git(cwd, args).then(
    () => true,
    () => false,
  );

function hexId(value: unknown, label: string): string {
  const id = typeof value === "string" ? value.toLowerCase() : "";
  if (!HEX.test(id)) throw new Error(`Invalid ${label}`);
  return id;
}

/** A branch name that is a valid ref and cannot be read as an option or a revision. */
export async function validateBranch(cwd: string, value: unknown): Promise<string> {
  if (
    typeof value !== "string" ||
    !value ||
    value.length > 120 ||
    value.startsWith("-") ||
    value.startsWith("@") ||
    value.includes("\0")
  )
    throw new Error("Invalid branch name");
  if (!(await gitOk(cwd, ["check-ref-format", `refs/heads/${value}`])))
    throw new Error("Invalid branch name");
  return value;
}

/** Why a path inside a patch is refused, or undefined when it is acceptable. */
export function patchPathProblem(path: string): string | undefined {
  if (!path || path.includes("\0")) return "an empty path";
  if (isAbsolute(path) || path.startsWith("/") || path.startsWith("\\") || /^[A-Za-z]:/.test(path))
    return "an absolute path";
  for (const part of path.split(/[\\/]/)) {
    if (part === "..") return "a path that leaves the target";
    // NTFS also resolves ".git." and "git~1" to the git directory.
    const normal = part.toLowerCase().replace(/[. ]+$/, "");
    if (normal === ".git" || normal === "git~1") return "a path inside .git";
  }
  return undefined;
}

/** Every path a patch would touch, as git itself parses them (no changes are made). */
export async function patchPaths(cwd: string, patch: string): Promise<string[]> {
  const out = await git(cwd, ["apply", "--numstat", "-z", "--", patch]);
  const tokens = out.split("\0");
  const paths: string[] = [];
  for (let at = 0; at < tokens.length; at++) {
    const token = tokens[at];
    if (!token) continue;
    const match = /^(?:\d+|-)\t(?:\d+|-)\t(.*)$/s.exec(token);
    if (!match) throw new Error("The patch could not be read");
    if (match[1]) paths.push(match[1]);
    else {
      // A rename or copy lists the old and the new path as the next two entries.
      paths.push(tokens[at + 1] ?? "", tokens[at + 2] ?? "");
      at += 2;
    }
  }
  return paths;
}

/** Refuses a patch that touches `.git/`, an absolute path, or anything outside `target`. */
export async function validatePatch(target: string, patch: string): Promise<string[]> {
  const paths = await patchPaths(target, patch);
  for (const path of paths) {
    const problem = patchPathProblem(path);
    if (problem) throw new Error(`The patch is refused: it contains ${problem}`);
    const full = resolve(target, path);
    if (full !== target && !full.startsWith(target + sep))
      throw new Error("The patch is refused: it contains a path that leaves the target");
  }
  return paths;
}

/**
 * The tree id of everything in `cwd` that is not ignored: a temporary index starts from
 * HEAD, takes `add -A`, and is written with `write-tree`. Objects it creates go to a scratch
 * directory (the real objects are read through an alternate), so the repository is not
 * polluted and the real index is never touched.
 */
export async function worktreeTree(cwd: string, extra: string[] = []): Promise<string> {
  const scratch = mkdtempSync(join(tmpdir(), "monoptah-snapshot-"));
  try {
    const common = (await git(cwd, ["rev-parse", "--git-common-dir"])).trim();
    const objects = join(resolve(cwd, common), "objects");
    mkdirSync(join(scratch, "objects"));
    const alternates = [objects, process.env.GIT_ALTERNATE_OBJECT_DIRECTORIES]
      .filter(Boolean)
      .join(process.platform === "win32" ? ";" : ":");
    const env = {
      GIT_INDEX_FILE: join(scratch, "index"),
      GIT_OBJECT_DIRECTORY: join(scratch, "objects"),
      GIT_ALTERNATE_OBJECT_DIRECTORIES: alternates,
      GIT_OPTIONAL_LOCKS: "0",
    };
    await git(cwd, ["read-tree", "HEAD"], env);
    await git(cwd, ["add", "-A"], env);
    // The sender starts from a copy of its REAL index, so a staged file that .gitignore
    // matches (a force-added one) is in its tree and its patch, but `add -A` skips it
    // here. The files the patch touches are put in explicitly (present: added, gone:
    // removed), which does not consult .gitignore.
    if (extra.length)
      await git(cwd, ["update-index", "--add", "--remove", "-z", "--stdin"], env, GIT_TIMEOUT_MS, extra.join("\0") + "\0");
    return (await git(cwd, ["write-tree"], env)).trim();
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
}

/**
 * Branches that may be origin's default (the Rust `default_branches`, without failing
 * closed when origin cannot be asked: the pushed branch must be fetchable anyway):
 * the target of `origin/HEAD`, `main` / `master` when they exist on origin, and what a
 * read-only `ls-remote --symref` reports.
 */
export async function defaultBranches(root: string): Promise<string[]> {
  const names = new Set<string>();
  const head = (await git(root, ["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"]).catch(() => "")).trim();
  const prefix = "refs/remotes/origin/";
  if (head.startsWith(prefix)) names.add(head.slice(prefix.length));
  for (const name of ["main", "master"])
    if (await gitOk(root, ["rev-parse", "--verify", "--quiet", `refs/remotes/origin/${name}`]))
      names.add(name);
  const remote = await git(root, ["ls-remote", "--symref", "origin", "HEAD"], {}, 15_000).catch(() => "");
  for (const line of remote.split("\n")) {
    const match = /^ref: refs\/heads\/(\S+)/.exec(line);
    if (match) names.add(match[1]);
  }
  const heads = await git(root, ["ls-remote", "--heads", "origin", "refs/heads/main", "refs/heads/master"], {}, 15_000).catch(() => "");
  for (const line of heads.split("\n")) {
    const match = /\trefs\/heads\/(main|master)$/.exec(line);
    if (match) names.add(match[1]);
  }
  return [...names];
}

export interface SnapshotResult {
  /** The working copy that now holds the snapshot. */
  cwd: string;
  branch: string;
  head: string;
  tree: string;
  /** Whether this call created a new Host worktree. */
  createdWorktree: boolean;
  /** True when the snapshot was already in place (a retried call). */
  reused: boolean;
  patchApplied: boolean;
}

export interface ImportResult {
  sessionId: string;
  providerSessionId: string;
  decision: "installed" | "replaced" | "already_current" | "forked";
  /** The id the transcript was forked from, when it diverged. */
  forkedFrom?: string;
  /** Text for the user when the transcript was forked. */
  notice?: string;
  /** True when an untouched session from an earlier import was returned. */
  reused: boolean;
}

/** Fixed limit for a transcript upload read by `sessions.import`. */
export const MAX_JSONL_BYTES = 50 * 1024 * 1024;

/**
 * The id a diverged copy is forked to, derived from the source id and the incoming chain
 * so that a retry after a lost response lands on the same file and the same Host session.
 */
export function forkSessionId(sourceId: string, text: string): string {
  const h = createHash("sha256")
    .update(sourceId)
    .update("\0")
    .update(parseChain(text).join("\n"))
    .digest("hex");
  return `${h.slice(0, 8)}-${h.slice(8, 12)}-4${h.slice(13, 16)}-${"89ab"[parseInt(h[16], 16) % 4]}${h.slice(17, 20)}-${h.slice(20, 32)}`;
}

export interface InstalledTranscript {
  decision: Decision;
  /** The session id the transcript lives under (new when it forked). */
  id: string;
  file: string;
  /** Set when an existing file was replaced and kept. */
  backup?: string;
  /** False when a fork from an earlier attempt was already there and is reused. */
  created?: boolean;
}

const nanos = () => `${process.hrtime.bigint()}`;

function writeSynced(path: string, text: string): void {
  const fd = openSync(path, "w", 0o600);
  try {
    const bytes = Buffer.from(text);
    for (let done = 0; done < bytes.length; )
      done += writeSync(fd, bytes, done, bytes.length - done);
    fsyncSync(fd);
  } finally {
    closeSync(fd);
  }
}

/**
 * Installs a (cwd-rewritten) transcript by the F4-J rule: a fresh id is written, an
 * extension replaces the file and keeps the old one as `<id>.jsonl.pre-cloud`, an equal file
 * is left alone, and a divergent one is never overwritten: the incoming copy is written
 * under a new id instead. A writer that races in is decided again, never trusted.
 */
export function installTranscript(input: {
  dir: string;
  id: string;
  text: string;
  newId?: () => string;
}): InstalledTranscript {
  const { dir, id, text } = input;
  const dest = join(dir, `${id}.jsonl`);
  for (let attempt = 0; attempt < 3; attempt++) {
    let existing: Buffer | null = null;
    try {
      existing = readFileSync(dest);
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
    }
    const decision = decide(existing, text);
    if (decision === "already_current") return { decision, id, file: dest };
    mkdirSync(dir, { recursive: true, mode: 0o700 });
    const tmp = join(dir, `.${id}.${nanos()}.tmp`);
    try {
      if (decision === "install") {
        writeSynced(tmp, text);
        // A hard link fails when the name exists, so a concurrent writer is never replaced.
        try {
          linkSync(tmp, dest);
        } catch (error) {
          if ((error as NodeJS.ErrnoException).code === "EEXIST") continue;
          throw error;
        }
        return { decision, id, file: dest };
      }
      if (decision === "replace") {
        writeSynced(tmp, text);
        const backup = join(dir, `${id}.jsonl.pre-cloud`);
        const staged = join(dir, `.${id}.${nanos()}.bak`);
        try {
          linkSync(dest, staged);
        } catch {
          copyFileSync(dest, staged);
        }
        try {
          renameSync(staged, backup);
        } catch (error) {
          rmSync(staged, { force: true });
          throw error;
        }
        renameSync(tmp, dest);
        return { decision, id, file: dest, backup };
      }
      const forkId = input.newId ? input.newId() : forkSessionId(id, text);
      const forked = join(dir, `${forkId}.jsonl`);
      const forkText = forkTranscript(text, forkId);
      writeSynced(tmp, forkText);
      try {
        linkSync(tmp, forked);
      } catch (error) {
        if ((error as NodeJS.ErrnoException).code !== "EEXIST") throw error;
        // A retry: the fork of this very transcript is already there.
        if (decide(readFileSync(forked), forkText) !== "already_current")
          throw new Error("A fork with this id already exists and differs");
        return { decision, id: forkId, file: forked, created: false };
      }
      return { decision, id: forkId, file: forked, created: true };
    } finally {
      rmSync(tmp, { force: true });
    }
  }
  throw new Error("The transcript on this machine keeps changing");
}

/**
 * The session's own cwd: the top-level `cwd` of the first entry that has one. Other cwds in
 * the file (a subdirectory the session moved into, a path outside the project) are NOT
 * sources: the Rust side rewrites only the project cwd and the session cwd, and
 * `rewriteCwd` keeps the rest of a path (`/src/app/pkg` becomes `/dst/app/pkg`).
 */
function firstSessionCwd(text: string): string | undefined {
  for (const line of text.split("\n")) {
    if (!line.includes('"cwd"')) continue;
    const body = line.replace(/\r$/, "");
    if (!probeLine(body)) continue;
    try {
      const cwd = (JSON.parse(body) as { cwd?: unknown }).cwd;
      if (typeof cwd === "string" && cwd) return cwd;
    } catch {
      // Not an entry: nothing to rewrite.
    }
  }
  return undefined;
}

function str(value: unknown, label: string, max: number): string {
  if (typeof value !== "string" || !value.trim() || value.length > max || value.includes("\0"))
    throw new Error(`Invalid ${label}`);
  return value;
}

export interface MoveOptions {
  /** Claude's config root; defaults to `$CLAUDE_CONFIG_DIR` or `~/.claude`. */
  claudeConfigDir?: () => string;
  /** Called after a Host worktree was created, so cached roots can be refreshed. */
  onWorktreeCreated?: () => void;
}

export class HostMoves {
  constructor(
    private readonly engine: HostEngine,
    private readonly uploads: HostUploads,
    private readonly options: MoveOptions = {},
  ) {}

  private claudeDir(): string {
    return (
      this.options.claudeConfigDir?.() ??
      (process.env.CLAUDE_CONFIG_DIR || join(homedir(), ".claude"))
    );
  }

  /** `workspace.applySnapshot {projectId, instance?, branch, head, tree, patchUploadId?}` */
  async applySnapshot(params: Record<string, unknown>): Promise<SnapshotResult> {
    const project = this.engine.store.project(str(params.projectId, "project ID", 128));
    const instance = params.instance ?? "shared";
    if (instance !== "new" && instance !== "shared") throw new Error("Invalid instance");
    const head = hexId(params.head, "head");
    const tree = hexId(params.tree, "tree");
    const root = project.cwd;
    const branch = await validateBranch(root, params.branch);
    const patch =
      params.patchUploadId === undefined || params.patchUploadId === null
        ? undefined
        : this.uploads.committedPath(validateUploadId(params.patchUploadId));
    if (patch && !statSync(patch).isFile()) throw new Error("The patch upload is not a file");
    return this.engine.withIdleProject(
      project.id,
      async () => {
        const result = await this.restore({ root, projectId: project.id, instance, branch, head, tree, patch });
        if (result.createdWorktree) this.options.onWorktreeCreated?.();
        return result;
      },
      "restoring a moved workspace",
    );
  }

  private async restore(input: {
    root: string;
    projectId: string;
    instance: "new" | "shared";
    branch: string;
    head: string;
    tree: string;
    patch?: string;
  }): Promise<SnapshotResult> {
    const { root, branch, head, tree, patch } = input;
    // The default branch of origin is never the target of a move.
    if ((await defaultBranches(root)).includes(branch))
      throw new Error("A moved session never uses the default branch of origin");
    // Refuse a bad patch before anything is created.
    const paths = patch ? await validatePatch(root, patch) : [];
    await this.ensureCommit(root, head, branch);
    const result = (cwd: string, created: boolean, reused: boolean): SnapshotResult => ({
      cwd,
      branch,
      head,
      tree,
      createdWorktree: created,
      reused,
      patchApplied: !!patch && !reused,
    });

    if (input.instance === "new") {
      // The new instance's own checkout: it must already be on the branch, clean.
      const current = (await git(root, ["symbolic-ref", "--quiet", "--short", "HEAD"]).catch(() => "")).trim();
      if (current !== branch) throw new Error(`The workspace is on "${current || "a detached HEAD"}", not on "${branch}"`);
      const at = (await git(root, ["rev-parse", "HEAD"])).trim();
      if (at === head && (await worktreeTree(root, paths)) === tree) return result(root, false, true);
      // "new" is only the client's claim: a checkout that sessions already use is not new.
      for (const entry of this.engine.store.summaries(input.projectId)) {
        if (entry.cwd !== root) continue;
        if (entry.status !== "idle" || this.engine.store.session(entry.id).session.blocks.length > 0)
          throw new Error("The workspace is already in use by a session; it is not a new instance");
      }
      if ((await git(root, ["status", "--porcelain", "--untracked-files=all"])).trim())
        throw new Error("The workspace has changes; it cannot take a snapshot");
      let moved = false;
      if (at !== head) {
        if (!(await gitOk(root, ["merge-base", "--is-ancestor", at, head])))
          throw new Error("HEAD of the workspace is not an ancestor of the snapshot head");
        await git(root, ["merge", "--ff-only", head]);
        moved = true;
      }
      // The tree was clean at the start (checked above), so putting the branch back to its
      // old tip with reset --hard loses nothing.
      const restoreTip = async () => {
        if (moved) await git(root, ["reset", "--hard", "--quiet", at]);
      };
      return this.finish(root, root, patch, paths, tree, head, restoreTip, result(root, false, false));
    }

    const listed = await hostWorktrees(root);
    const existing = listed.worktrees.find((entry) => entry.branch === branch && !entry.missing);
    if (existing) {
      if (existing.head === head && (await worktreeTree(existing.path, paths)) === tree)
        return result(existing.path, false, true);
      throw new Error("This branch already has a working copy. Select it from the picker.");
    }
    const path = join(listed.defaultRoot, `wt-${branch.replace(/[^a-zA-Z0-9_-]/g, "-")}`);
    if (existsSync(path)) throw new Error(`${path} already exists. Choose another branch name.`);
    const local = (
      await git(root, ["rev-parse", "--verify", "--quiet", `refs/heads/${branch}`]).catch(() => "")
    ).trim();
    if (local && local !== head)
      throw new Error("The branch exists here at a different commit; it is not moved");
    mkdirSync(listed.defaultRoot, { recursive: true });
    if (local) await git(root, ["worktree", "add", "--", path, branch]);
    else await git(root, ["worktree", "add", "--no-track", "-b", branch, "--", path, head]);
    const created = (await hostWorktrees(root)).worktrees.find((entry) => entry.path !== root && entry.branch === branch);
    const target = created?.path ?? path;
    const rollback = async () => {
      await git(root, ["worktree", "remove", "--force", "--", target]).catch(() => {
        rmSync(target, { recursive: true, force: true });
      });
      await git(root, ["worktree", "prune"]).catch(() => undefined);
      if (!local) {
        const now = (await git(root, ["rev-parse", "--verify", "--quiet", `refs/heads/${branch}`]).catch(() => "")).trim();
        if (now === head) await git(root, ["branch", "-D", "--", branch]).catch(() => undefined);
      }
    };
    return this.finish(root, target, patch, paths, tree, head, rollback, result(target, true, false));
  }

  /** Checks HEAD, applies the patch, verifies the tree; rolls back on any failure. */
  private async finish(
    root: string,
    target: string,
    patch: string | undefined,
    paths: string[],
    tree: string,
    head: string,
    rollback: () => Promise<void>,
    ok: SnapshotResult,
  ): Promise<SnapshotResult> {
    let applied = false;
    try {
      const at = (await git(target, ["rev-parse", "HEAD"])).trim();
      if (at !== head) throw new Error("HEAD does not match the snapshot head");
      if (patch) {
        // Checked again against the real target, now that it exists.
        await validatePatch(target, patch);
        await git(target, ["apply", "--binary", "--whitespace=nowarn", "--", patch]);
        applied = true;
      }
      const actual = await worktreeTree(target, paths);
      if (actual !== tree)
        throw new Error(`The restored files do not match the snapshot (expected tree ${tree}, got ${actual})`);
      return ok;
    } catch (error) {
      if (applied && target === root)
        await git(target, ["apply", "-R", "--binary", "--whitespace=nowarn", "--", patch!]).catch(
          () => undefined,
        );
      await rollback();
      throw error;
    }
  }

  /** Makes sure `head` is a commit here; fetches the pushed branch (never forced) if not. */
  private async ensureCommit(root: string, head: string, branch: string): Promise<void> {
    if (await gitOk(root, ["cat-file", "-e", `${head}^{commit}`])) return;
    await git(root, [
      "fetch",
      "--no-tags",
      "origin",
      "--",
      `refs/heads/${branch}:refs/remotes/origin/${branch}`,
    ]);
    if (!(await gitOk(root, ["cat-file", "-e", `${head}^{commit}`])))
      throw new Error("The snapshot head is not on the pushed branch");
  }

  /**
   * Contract for the caller (F4-3a): a second move of the same conversation to this Host is
   * refused while an earlier Host session that has turns (or is running) still binds the
   * Claude session id. Returning to local only tombstones the session in the app, so
   * F4-3a must delete (`sessions.delete`) or unbind the old Host session BEFORE it imports
   * again. An untouched session from an earlier import is reused, which makes retries safe.
   *
   * `sessions.import {projectId, providerSessionId, uploadId, cwd, title?, model,
   * modelSettings?, runtimeMode?, fromCwds?}`. The session has NO blocks and is bound to the
   * installed Claude session id, so the first turn passes `--resume <id>`.
   */
  async importSession(params: Record<string, unknown>): Promise<ImportResult> {
    const project = this.engine.store.project(str(params.projectId, "project ID", 128));
    const wanted = params.providerSessionId;
    if (typeof wanted !== "string" || !SESSION_ID.test(wanted))
      throw new Error("Invalid Claude session ID");
    const model = str(params.model, "model", 200);
    const title = params.title === undefined ? "Moved session" : str(params.title, "title", 200);
    const settings = modelSettings(params.modelSettings);
    const runtimeMode = (params.runtimeMode ?? "supervised") as Session["runtimeMode"];
    if (!RUNTIME_MODES.includes(runtimeMode)) throw new Error("Invalid permission mode");
    const given = params.fromCwds ?? [];
    if (
      !Array.isArray(given) ||
      given.length > 16 ||
      given.some((entry) => typeof entry !== "string" || entry.length > 4096 || entry.includes("\0"))
    )
      throw new Error("Invalid source paths");
    if (typeof params.cwd !== "string") throw new Error("Invalid working copy");
    const cwd = await resolveHostWorktreeAsync(project.cwd, params.cwd);
    const name = claudeProjectDirName(cwd);
    if (name.length > CLAUDE_NAME_LIMIT)
      throw new Error("This project path is too long for Claude's session folder");

    let incoming: string;
    try {
      const file = this.uploads.committedPath(validateUploadId(params.uploadId));
      // Checked before the file is read into memory.
      if (statSync(file).size > MAX_JSONL_BYTES)
        throw new Error("The transcript is larger than 50 MB");
      incoming = new TextDecoder("utf-8", { fatal: true }).decode(readFileSync(file));
    } catch (error) {
      throw error instanceof TypeError ? new Error("The transcript is not valid UTF-8") : error;
    }
    if (parseChain(incoming).length === 0) throw new Error("The transcript has no entries");

    // One writer at a time: a Host session that already uses this conversation blocks it,
    // unless it is the untouched result of an earlier import (a retry).
    let reusable: string | undefined;
    for (const bound of this.engine.sessionsBoundTo(wanted)) {
      const untouched =
        bound.status === "idle" && this.engine.store.session(bound.id).session.blocks.length === 0;
      if (!untouched)
        throw new Error("A session on this machine already uses this Claude conversation");
      reusable ??= bound.id;
    }

    // Only the roots the client names (project cwd and session cwd) are rewritten; with
    // none named, only the session's own cwd. Anything outside them stays as it was.
    const named = (given as string[]).length ? (given as string[]) : [firstSessionCwd(incoming)];
    const sources = [
      ...new Set([...named, params.cwd].filter((entry): entry is string => !!entry)),
    ].filter((entry) => entry !== cwd);
    const text = rewriteCwd(incoming, sources, cwd);
    const dir = join(this.claudeDir(), "projects", name);
    const installed = installTranscript({ dir, id: wanted, text });

    const forked = installed.decision === "diverged";
    if (forked) {
      // A retry finds the fork of an earlier attempt: its untouched session is returned.
      for (const bound of this.engine.sessionsBoundTo(installed.id)) {
        const untouched =
          bound.status === "idle" && this.engine.store.session(bound.id).session.blocks.length === 0;
        if (!untouched)
          throw new Error("A session on this machine already uses this Claude conversation");
        return {
          sessionId: bound.id,
          providerSessionId: installed.id,
          decision: "forked",
          forkedFrom: wanted,
          notice: forkNotice(wanted, installed.id),
          reused: true,
        };
      }
    }
    if (reusable && !forked) {
      return {
        sessionId: reusable,
        providerSessionId: installed.id,
        decision: decisionName(installed.decision),
        reused: true,
      };
    }
    try {
      const saved = this.engine.importClaudeSession({
        projectId: project.id,
        cwd,
        providerSessionId: installed.id,
        title,
        model,
        modelSettings: settings,
        runtimeMode,
      });
      return {
        sessionId: saved.session.id,
        providerSessionId: installed.id,
        decision: decisionName(installed.decision),
        ...(forked
          ? { forkedFrom: wanted, notice: forkNotice(wanted, installed.id) }
          : {}),
        reused: false,
      };
    } catch (error) {
      // Nothing uses the file we just created: take it back so a retry starts clean.
      if (installed.decision === "install" || (forked && installed.created)) {
        try {
          unlinkSync(installed.file);
        } catch {
          // Already gone.
        }
      }
      throw error;
    }
  }
}

function decisionName(decision: Decision): ImportResult["decision"] {
  return decision === "install"
    ? "installed"
    : decision === "replace"
      ? "replaced"
      : decision === "diverged"
        ? "forked"
        : "already_current";
}
