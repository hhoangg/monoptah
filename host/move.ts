import { execFile } from "node:child_process";
import { randomUUID } from "node:crypto";
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
): Promise<string> {
  try {
    const { stdout } = await exec("git", args, {
      cwd,
      timeout,
      maxBuffer: MAX_BUFFER,
      encoding: "utf8",
      env: { ...process.env, GIT_TERMINAL_PROMPT: "0", ...env },
    });
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
export async function worktreeTree(cwd: string): Promise<string> {
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
    return (await git(cwd, ["write-tree"], env)).trim();
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
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

export interface InstalledTranscript {
  decision: Decision;
  /** The session id the transcript lives under (new when it forked). */
  id: string;
  file: string;
  /** Set when an existing file was replaced and kept. */
  backup?: string;
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
      const forkId = (input.newId ?? randomUUID)();
      const forked = join(dir, `${forkId}.jsonl`);
      writeSynced(tmp, forkTranscript(text, forkId));
      linkSync(tmp, forked);
      return { decision, id: forkId, file: forked };
    } finally {
      rmSync(tmp, { force: true });
    }
  }
  throw new Error("The transcript on this machine keeps changing");
}

/** Distinct top-level `cwd` strings of the entries. */
function transcriptCwds(text: string): string[] {
  const found = new Set<string>();
  for (const line of text.split("\n")) {
    if (!line.includes('"cwd"') || !probeLine(line.replace(/\r$/, ""))) continue;
    try {
      const cwd = (JSON.parse(line) as { cwd?: unknown }).cwd;
      if (typeof cwd === "string" && cwd) found.add(cwd);
    } catch {
      // Not an entry: nothing to rewrite.
    }
  }
  return [...found];
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
        const result = await this.restore({ root, instance, branch, head, tree, patch });
        if (result.createdWorktree) this.options.onWorktreeCreated?.();
        return result;
      },
      "restoring a moved workspace",
    );
  }

  private async restore(input: {
    root: string;
    instance: "new" | "shared";
    branch: string;
    head: string;
    tree: string;
    patch?: string;
  }): Promise<SnapshotResult> {
    const { root, branch, head, tree, patch } = input;
    // The default branch of origin is never the target of a move.
    const upstream = (
      await git(root, ["symbolic-ref", "--quiet", "--short", "refs/remotes/origin/HEAD"]).catch(
        () => "",
      )
    ).trim();
    if (upstream && upstream === `origin/${branch}`)
      throw new Error("A moved session never uses the default branch of origin");
    // Refuse a bad patch before anything is created.
    if (patch) await validatePatch(root, patch);
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
      if (at === head && (await worktreeTree(root)) === tree) return result(root, false, true);
      if ((await git(root, ["status", "--porcelain", "--untracked-files=all"])).trim())
        throw new Error("The workspace has changes; it cannot take a snapshot");
      if (at !== head) {
        if (!(await gitOk(root, ["merge-base", "--is-ancestor", at, head])))
          throw new Error("HEAD of the workspace is not an ancestor of the snapshot head");
        await git(root, ["merge", "--ff-only", head]);
      }
      return this.finish(root, root, patch, tree, head, async () => {}, result(root, false, false));
    }

    const listed = await hostWorktrees(root);
    const existing = listed.worktrees.find((entry) => entry.branch === branch && !entry.missing);
    if (existing) {
      if (existing.head === head && (await worktreeTree(existing.path)) === tree)
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
    return this.finish(root, target, patch, tree, head, rollback, result(target, true, false));
  }

  /** Checks HEAD, applies the patch, verifies the tree; rolls back on any failure. */
  private async finish(
    root: string,
    target: string,
    patch: string | undefined,
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
      const actual = await worktreeTree(target);
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
      incoming = new TextDecoder("utf-8", { fatal: true }).decode(
        readFileSync(this.uploads.committedPath(validateUploadId(params.uploadId))),
      );
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

    const sources = [
      ...new Set([
        ...(given as string[]),
        params.cwd,
        ...transcriptCwds(incoming),
      ]),
    ].filter((entry) => entry !== cwd);
    const text = rewriteCwd(incoming, sources, cwd);
    const dir = join(this.claudeDir(), "projects", name);
    const installed = installTranscript({ dir, id: wanted, text });

    const forked = installed.decision === "diverged";
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
      if (installed.decision === "install" || forked) {
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
