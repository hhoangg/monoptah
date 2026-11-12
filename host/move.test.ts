import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  realpathSync,
  rmSync,
  statSync,
  truncateSync,
  utimesSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { HostProvider } from "./providers";
import { HostEngine } from "./engine";
import { HostStore } from "./store";
import { HostUploads } from "./uploads";
import {
  HostMoves,
  MAX_JSONL_BYTES,
  forkSessionId,
  installTranscript,
  patchPathProblem,
  defaultBranches,
  removePatchLeftovers,
  validateBranch,
  worktreeTree,
} from "./move";
import { claudeProjectDirName, decide, parseChain } from "./transcript-sync";

const FIXTURES = join(__dirname, "..", "crates/monocode-core/tests/fixtures/transcript_sync");
const fixture = (name: string) => readFileSync(join(FIXTURES, name), "utf8");

const cleanups: Array<() => Promise<void> | void> = [];
afterEach(async () => {
  for (const cleanup of cleanups.splice(0).reverse()) await cleanup();
});

const IDENTITY = {
  GIT_AUTHOR_NAME: "Test",
  GIT_AUTHOR_EMAIL: "test@example.com",
  GIT_COMMITTER_NAME: "Test",
  GIT_COMMITTER_EMAIL: "test@example.com",
};
const git = (cwd: string, ...args: string[]) =>
  execFileSync("git", ["-c", "commit.gpgsign=false", ...args], {
    cwd,
    encoding: "utf8",
    env: { ...process.env, ...IDENTITY },
  }).trim();

/** What a sender computes: the tree and patch of every non-ignored change over HEAD. */
function snapshotOf(repo: string, copyIndex = false) {
  const scratch = mkdtempSync(join(tmpdir(), "move-test-index-"));
  const env = { ...process.env, GIT_INDEX_FILE: join(scratch, "index") };
  const run = (...args: string[]) =>
    execFileSync("git", args, { cwd: repo, env, maxBuffer: 1 << 26 });
  // The real sender starts from a copy of its REAL index (staged work included).
  if (copyIndex) copyFileSync(join(repo, ".git", "index"), join(scratch, "index"));
  else run("read-tree", "HEAD");
  run("add", "-A");
  const tree = run("write-tree").toString().trim();
  const patch = run("diff", "--cached", "--binary", "HEAD");
  rmSync(scratch, { recursive: true, force: true });
  return { head: git(repo, "rev-parse", "HEAD"), tree, patch };
}

function upload(uploads: HostUploads, name: string, data: Buffer | string) {
  const bytes = Buffer.from(data);
  const { uploadId } = uploads.begin({
    name,
    size: bytes.length,
    sha256: createHash("sha256").update(bytes).digest("hex"),
  });
  uploads.chunk({ uploadId, offset: 0, data: bytes.toString("base64") });
  uploads.commit({ uploadId });
  return uploadId;
}

interface World {
  dir: string;
  origin: string;
  local: string;
  root: string;
  claudeDir: string;
  store: HostStore;
  engine: HostEngine;
  provider: HostProvider;
  uploads: HostUploads;
  moves: HostMoves;
  projectId: string;
  turns: Array<{ finish: () => void }>;
}

/**
 * origin (bare, default branch main) <- local (the sender, on `feat` with dirty changes)
 * and root (the Host project, a clone of origin on main).
 */
function world(options: { cloneRootAfterPush?: boolean; rootBranch?: string } = {}): World {
  const dir = realpathSync(mkdtempSync(join(tmpdir(), "move-test-")));
  const origin = join(dir, "origin.git");
  const local = join(dir, "local");
  const root = join(dir, "root");
  mkdirSync(origin);
  git(origin, "init", "-q", "--bare", "-b", "main");
  git(dir, "clone", "-q", origin, local);
  git(local, "checkout", "-q", "-b", "main");
  writeFileSync(join(local, "a.txt"), "one\n");
  writeFileSync(join(local, ".gitignore"), "secret.env\n");
  git(local, "add", ".");
  git(local, "commit", "-q", "-m", "initial");
  git(local, "push", "-q", "origin", "main");
  git(origin, "symbolic-ref", "HEAD", "refs/heads/main");
  git(local, "remote", "set-head", "origin", "main");
  const rootAfterPush = options.cloneRootAfterPush || !!options.rootBranch;
  if (!rootAfterPush) git(dir, "clone", "-q", origin, root);
  git(local, "checkout", "-q", "-b", "feat");
  writeFileSync(join(local, "a.txt"), "one\ntwo\n");
  git(local, "commit", "-q", "-am", "second");
  git(local, "push", "-q", "origin", "feat");
  if (rootAfterPush) git(dir, "clone", "-q", origin, root);
  if (options.rootBranch) git(root, "checkout", "-q", options.rootBranch);
  // Uncommitted work: a change, a new file, a binary file, and an ignored file.
  writeFileSync(join(local, "a.txt"), "one\ntwo\nthree\n");
  writeFileSync(join(local, "b.txt"), "new file\n");
  writeFileSync(join(local, "bin.dat"), Buffer.from([0, 1, 2, 255, 254, 0, 9]));
  writeFileSync(join(local, "secret.env"), "TOKEN=1\n");

  const directory = join(dir, "host");
  mkdirSync(directory);
  const store = new HostStore(join(directory, "host.db"));
  const project = store.addProject(realpathSync(root), "root");
  const turns: World["turns"] = [];
  const provider: HostProvider = {
    send: vi.fn(
      () =>
        new Promise<void>((resolve) => {
          turns.push({ finish: resolve });
        }),
    ),
    cancel: vi.fn(async () => turns.at(-1)?.finish()),
    stop: vi.fn(async () => turns.at(-1)?.finish()),
    bind: vi.fn(),
    approve: vi.fn(),
    answer: vi.fn(),
  };
  const engine = new HostEngine(store, { claude: provider, codex: provider });
  const uploads = new HostUploads(join(directory, "uploads"));
  const claudeDir = join(dir, "claude-config");
  const moves = new HostMoves(engine, uploads, { claudeConfigDir: () => claudeDir });
  cleanups.push(async () => {
    await engine.close();
    store.close();
    rmSync(dir, { recursive: true, force: true });
  });
  return {
    dir, origin, local, root: realpathSync(root), claudeDir, store, engine, provider, uploads, moves,
    projectId: project.id, turns,
  };
}

const listing = (repo: string) => git(repo, "worktree", "list", "--porcelain");

describe("workspace.applySnapshot", () => {
  it("restores code on a new Host worktree and the tree verification passes", async () => {
    const w = world();
    const snap = snapshotOf(w.local);
    const patchUploadId = upload(w.uploads, "changes.patch", snap.patch);
    const result = await w.moves.applySnapshot({
      projectId: w.projectId, instance: "shared", branch: "feat", ...snap, patchUploadId,
    });
    expect(result).toMatchObject({ branch: "feat", head: snap.head, tree: snap.tree, createdWorktree: true, reused: false });
    // A shared instance: a new worktree, never the project root.
    expect(result.cwd).not.toBe(w.root);
    expect(realpathSync(result.cwd).startsWith(`${w.root}-worktrees`)).toBe(true);
    expect(listing(w.root)).toContain(`branch refs/heads/feat`);
    expect(readFileSync(join(result.cwd, "a.txt"), "utf8")).toBe("one\ntwo\nthree\n");
    expect(readFileSync(join(result.cwd, "b.txt"), "utf8")).toBe("new file\n");
    expect([...readFileSync(join(result.cwd, "bin.dat"))]).toEqual([0, 1, 2, 255, 254, 0, 9]);
    // Ignored files never travel; the changes are unstaged; the root is untouched.
    expect(existsSync(join(result.cwd, "secret.env"))).toBe(false);
    expect(git(result.cwd, "diff", "--cached", "--name-only")).toBe("");
    expect(git(w.root, "status", "--porcelain")).toBe("");
    expect(git(w.root, "symbolic-ref", "--short", "HEAD")).toBe("main");
    expect(await worktreeTree(result.cwd)).toBe(snap.tree);
  });

  it("fails the tree verification, rolls back what it created and reports", async () => {
    const w = world();
    const snap = snapshotOf(w.local);
    const patchUploadId = upload(w.uploads, "changes.patch", snap.patch);
    const wrong = snap.tree.replace(/^./, (c) => (c === "0" ? "1" : "0"));
    await expect(
      w.moves.applySnapshot({ projectId: w.projectId, branch: "feat", head: snap.head, tree: wrong, patchUploadId }),
    ).rejects.toThrow(/do not match the snapshot \(expected tree .* got /);
    expect(listing(w.root)).not.toContain("feat");
    expect(git(w.root, "branch", "--list", "feat")).toBe("");
    expect(existsSync(`${w.root}-worktrees/wt-feat`)).toBe(false);
    expect(git(w.root, "status", "--porcelain")).toBe("");
  });

  it("fails when the patch does not produce the snapshot tree", async () => {
    const w = world();
    const snap = snapshotOf(w.local);
    // A patch for only part of the changes leaves the tree different.
    writeFileSync(join(w.local, "b.txt"), "different\n");
    const partial = snapshotOf(w.local).patch;
    const patchUploadId = upload(w.uploads, "changes.patch", partial);
    await expect(
      w.moves.applySnapshot({ projectId: w.projectId, branch: "feat", head: snap.head, tree: snap.tree, patchUploadId }),
    ).rejects.toThrow(/do not match/);
    expect(existsSync(`${w.root}-worktrees/wt-feat`)).toBe(false);
  });

  it("restores a clean snapshot without a patch", async () => {
    const w = world();
    git(w.local, "checkout", "-q", "--", ".");
    rmSync(join(w.local, "b.txt"));
    rmSync(join(w.local, "bin.dat"));
    const snap = snapshotOf(w.local);
    const result = await w.moves.applySnapshot({ projectId: w.projectId, branch: "feat", ...snap });
    expect(result.patchApplied).toBe(false);
    expect(await worktreeTree(result.cwd)).toBe(snap.tree);
  });

  it("targets the project root on a new instance and rolls back a mismatch there", async () => {
    const w = world({ rootBranch: "feat" });
    const snap = snapshotOf(w.local);
    const patchUploadId = upload(w.uploads, "changes.patch", snap.patch);
    const wrong = snap.tree.replace(/^./, (c) => (c === "0" ? "1" : "0"));
    await expect(
      w.moves.applySnapshot({ projectId: w.projectId, instance: "new", branch: "feat", head: snap.head, tree: wrong, patchUploadId }),
    ).rejects.toThrow(/do not match/);
    expect(git(w.root, "status", "--porcelain")).toBe("");
    const ok = await w.moves.applySnapshot({
      projectId: w.projectId, instance: "new", branch: "feat", ...snap, patchUploadId,
    });
    expect(ok).toMatchObject({ cwd: w.root, createdWorktree: false, reused: false });
    expect(readFileSync(join(w.root, "a.txt"), "utf8")).toBe("one\ntwo\nthree\n");
    expect(existsSync(`${w.root}-worktrees`)).toBe(false);
  });

  it("is idempotent: a repeated call finds the snapshot already in place", async () => {
    for (const instance of ["shared", "new"] as const) {
      const w = world({ rootBranch: instance === "new" ? "feat" : undefined });
      const snap = snapshotOf(w.local);
      const patchUploadId = upload(w.uploads, "changes.patch", snap.patch);
      const args = { projectId: w.projectId, instance, branch: "feat", ...snap, patchUploadId };
      const first = await w.moves.applySnapshot(args);
      const second = await w.moves.applySnapshot(args);
      expect(second).toMatchObject({ cwd: first.cwd, reused: true, createdWorktree: false, patchApplied: false });
    }
  });

  it("fetches the pushed branch when the commit is missing, and not when it is there", async () => {
    const missing = world();
    const snap = snapshotOf(missing.local);
    expect(() => git(missing.root, "cat-file", "-e", `${snap.head}^{commit}`)).toThrow();
    const patchUploadId = upload(missing.uploads, "changes.patch", snap.patch);
    const result = await missing.moves.applySnapshot({ projectId: missing.projectId, branch: "feat", ...snap, patchUploadId });
    expect(git(missing.root, "rev-parse", "refs/remotes/origin/feat")).toBe(snap.head);
    expect(git(result.cwd, "rev-parse", "HEAD")).toBe(snap.head);
    // Never forced and never the default branch: origin/main is where it was.
    expect(git(missing.root, "rev-parse", "main")).toBe(git(missing.origin, "rev-parse", "main"));

    const present = world({ cloneRootAfterPush: true });
    const second = snapshotOf(present.local);
    expect(git(present.root, "cat-file", "-t", second.head)).toBe("commit");
    const secondPatch = upload(present.uploads, "changes.patch", second.patch);
    await expect(
      present.moves.applySnapshot({ projectId: present.projectId, branch: "feat", ...second, patchUploadId: secondPatch }),
    ).resolves.toMatchObject({ createdWorktree: true });
  });

  it("reports when the snapshot head is not on the pushed branch", async () => {
    const w = world();
    const snap = snapshotOf(w.local);
    const unpushed = "0".repeat(39) + "1";
    await expect(
      w.moves.applySnapshot({ projectId: w.projectId, branch: "feat", ...snap, head: unpushed }),
    ).rejects.toThrow(/not on the pushed branch/);
    expect(existsSync(`${w.root}-worktrees`)).toBe(false);
  });

  it("refuses the default branch of origin", async () => {
    const w = world();
    const snap = snapshotOf(w.local);
    await expect(
      w.moves.applySnapshot({ projectId: w.projectId, branch: "main", ...snap }),
    ).rejects.toThrow(/default branch/);
  });

  it("restores a force-added ignored file that is staged but uncommitted", async () => {
    const w = world();
    git(w.local, "add", "-f", "secret.env");
    const snap = snapshotOf(w.local, true);
    // The plain `add -A` tree (what the Host starts from) misses it, so the trees differ.
    expect(snap.tree).not.toBe(snapshotOf(w.local).tree);
    const patchUploadId = upload(w.uploads, "changes.patch", snap.patch);
    const result = await w.moves.applySnapshot({ projectId: w.projectId, branch: "feat", ...snap, patchUploadId });
    expect(readFileSync(join(result.cwd, "secret.env"), "utf8")).toBe("TOKEN=1\n");
    expect(await worktreeTree(result.cwd, ["secret.env"])).toBe(snap.tree);
  });

  describe("on a new instance", () => {
    const behind = (w: World) => {
      const tip = git(w.root, "rev-parse", "HEAD");
      git(w.root, "reset", "-q", "--hard", "HEAD~1");
      return { tip, old: git(w.root, "rev-parse", "HEAD") };
    };

    it("fast-forwards to the snapshot head and then applies the patch", async () => {
      const w = world({ rootBranch: "feat" });
      const { tip, old } = behind(w);
      expect(old).not.toBe(tip);
      const snap = snapshotOf(w.local);
      const patchUploadId = upload(w.uploads, "changes.patch", snap.patch);
      await w.moves.applySnapshot({ projectId: w.projectId, instance: "new", branch: "feat", ...snap, patchUploadId });
      expect(git(w.root, "rev-parse", "HEAD")).toBe(snap.head);
      expect(await worktreeTree(w.root)).toBe(snap.tree);
    });

    it("puts the branch back to its old tip when the tree check fails after the fast-forward", async () => {
      const w = world({ rootBranch: "feat" });
      const { old } = behind(w);
      const snap = snapshotOf(w.local);
      const patchUploadId = upload(w.uploads, "changes.patch", snap.patch);
      const wrong = snap.tree.replace(/^./, (c) => (c === "0" ? "1" : "0"));
      await expect(
        w.moves.applySnapshot({ projectId: w.projectId, instance: "new", branch: "feat", ...snap, tree: wrong, patchUploadId }),
      ).rejects.toThrow(/do not match/);
      expect(git(w.root, "rev-parse", "HEAD")).toBe(old);
      expect(git(w.root, "rev-parse", "feat")).toBe(old);
      expect(git(w.root, "status", "--porcelain")).toBe("");
      expect(existsSync(join(w.root, "b.txt"))).toBe(false);
    });

    it("puts the branch back when the patch itself fails to apply", async () => {
      const w = world({ rootBranch: "feat" });
      const { old } = behind(w);
      const snap = snapshotOf(w.local);
      const patchUploadId = upload(w.uploads, "bad.patch", snap.patch.toString().replace("two", "TWO"));
      await expect(
        w.moves.applySnapshot({ projectId: w.projectId, instance: "new", branch: "feat", ...snap, patchUploadId }),
      ).rejects.toThrow();
      expect(git(w.root, "rev-parse", "HEAD")).toBe(old);
      expect(git(w.root, "status", "--porcelain")).toBe("");
    });

    it("refuses a checkout that a session already uses", async () => {
      const w = world({ rootBranch: "feat" });
      behind(w);
      const snap = snapshotOf(w.local);
      const patchUploadId = upload(w.uploads, "changes.patch", snap.patch);
      const created = w.engine.command({
        type: "create", commandId: "c", projectId: w.projectId, harness: "claude",
        model: "claude:test", runtimeMode: "supervised",
      });
      w.engine.command({ type: "send", commandId: "s", sessionId: created.sessionId, text: "work" });
      await vi.waitFor(() => expect(w.turns).toHaveLength(1));
      w.turns[0].finish();
      await vi.waitFor(() => expect(w.store.session(created.sessionId).status).toBe("idle"));
      await expect(
        w.moves.applySnapshot({ projectId: w.projectId, instance: "new", branch: "feat", ...snap, patchUploadId }),
      ).rejects.toThrow(/already in use/);
      expect(git(w.root, "status", "--porcelain")).toBe("");
    });
  });

  describe("default branch protection", () => {
    /** The sender is on `main` at origin's tip with only uncommitted changes. */
    const onMain = (w: World) => {
      git(w.local, "checkout", "-q", "-f", "main");
      rmSync(join(w.local, "b.txt"), { force: true });
      rmSync(join(w.local, "bin.dat"), { force: true });
      writeFileSync(join(w.local, "a.txt"), "one\nlocal edit\n");
      writeFileSync(join(w.local, "c.txt"), "new\n");
    };

    it("accepts main at origin's tip with an uncommitted patch (new instance)", async () => {
      const w = world({ rootBranch: "main" });
      onMain(w);
      const snap = snapshotOf(w.local);
      expect(snap.head).toBe(git(w.origin, "rev-parse", "main"));
      const patchUploadId = upload(w.uploads, "changes.patch", snap.patch);
      const out = await w.moves.applySnapshot({ projectId: w.projectId, instance: "new", branch: "main", ...snap, patchUploadId });
      expect(out.cwd).toBe(w.root);
      expect(readFileSync(join(w.root, "c.txt"), "utf8")).toBe("new\n");
    });

    it("accepts main at origin's tip on a shared instance with a detached worktree", async () => {
      const w = world();
      onMain(w);
      const snap = snapshotOf(w.local);
      const patchUploadId = upload(w.uploads, "changes.patch", snap.patch);
      const out = await w.moves.applySnapshot({ projectId: w.projectId, branch: "main", ...snap, patchUploadId });
      expect(out).toMatchObject({ createdWorktree: true });
      expect(out.cwd).not.toBe(w.root);
      expect(git(out.cwd, "rev-parse", "HEAD")).toBe(snap.head);
      expect(() => git(out.cwd, "symbolic-ref", "-q", "HEAD")).toThrow();
      expect(git(w.root, "symbolic-ref", "--short", "HEAD")).toBe("main");
      expect(readFileSync(join(out.cwd, "c.txt"), "utf8")).toBe("new\n");
    });

    it("is idempotent for the detached worktree of a default-branch move (shared instance)", async () => {
      const w = world();
      onMain(w);
      const snap = snapshotOf(w.local);
      const patchUploadId = upload(w.uploads, "changes.patch", snap.patch);
      const args = { projectId: w.projectId, branch: "main", ...snap, patchUploadId };
      const first = await w.moves.applySnapshot(args);
      const second = await w.moves.applySnapshot(args);
      expect(second).toMatchObject({ cwd: first.cwd, createdWorktree: false, reused: true });
      // A worktree at that name that holds something else is still refused.
      writeFileSync(join(first.cwd, "c.txt"), "changed\n");
      await expect(w.moves.applySnapshot(args)).rejects.toThrow(/already exists/);
    });

    it("refuses main when it is ahead of origin/main", async () => {
      const w = world();
      onMain(w);
      git(w.local, "commit", "-q", "-am", "unpushed");
      const snap = snapshotOf(w.local);
      await expect(w.moves.applySnapshot({ projectId: w.projectId, branch: "main", ...snap })).rejects.toThrow(/default branch/);
      expect(existsSync(`${w.root}-worktrees`)).toBe(false);
    });

    it("still refuses main and master when origin/HEAD is unset and they are ahead", async () => {
      const w = world();
      git(w.local, "push", "-q", "origin", "feat:master");
      git(w.root, "fetch", "-q", "origin");
      git(w.root, "remote", "set-head", "origin", "-d");
      git(w.local, "commit", "-q", "--allow-empty", "-m", "ahead");
      const snap = snapshotOf(w.local);
      for (const branch of ["main", "master"])
        await expect(w.moves.applySnapshot({ projectId: w.projectId, branch, ...snap })).rejects.toThrow(/default branch/);
      expect(await defaultBranches(w.root)).toEqual(expect.arrayContaining(["main", "master"]));
    });

    it("treats the target of origin/HEAD as the default, such as develop", async () => {
      const w = world();
      git(w.local, "push", "-q", "origin", "feat:develop");
      git(w.root, "fetch", "-q", "origin");
      git(w.root, "remote", "set-head", "origin", "develop");
      git(w.local, "commit", "-q", "--allow-empty", "-m", "ahead");
      const snap = snapshotOf(w.local);
      expect(await defaultBranches(w.root)).toContain("develop");
      await expect(w.moves.applySnapshot({ projectId: w.projectId, branch: "develop", ...snap })).rejects.toThrow(/default branch/);
    });

    it("makes no network call", async () => {
      const w = world();
      git(w.root, "remote", "set-url", "origin", "/nonexistent/origin.git");
      const start = Date.now();
      await defaultBranches(w.root);
      expect(Date.now() - start).toBeLessThan(5_000);
    });
  });

  describe("fast-forward on a new instance", () => {
    it("refuses when it would overwrite an ignored file", async () => {
      const w = world({ rootBranch: "feat" });
      writeFileSync(join(w.local, "extra.txt"), "from the sender\n");
      git(w.local, "add", "extra.txt");
      git(w.local, "commit", "-q", "-m", "add extra");
      git(w.local, "push", "-q", "origin", "feat");
      git(w.root, "fetch", "-q", "origin");
      writeFileSync(join(w.root, ".git", "info", "exclude"), "extra.txt\n");
      writeFileSync(join(w.root, "extra.txt"), "precious local data\n");
      const before = git(w.root, "rev-parse", "HEAD");
      const snap = snapshotOf(w.local);
      await expect(
        w.moves.applySnapshot({ projectId: w.projectId, instance: "new", branch: "feat", ...snap }),
      ).rejects.toThrow(/would overwrite the ignored file extra\.txt/);
      expect(git(w.root, "rev-parse", "HEAD")).toBe(before);
      expect(readFileSync(join(w.root, "extra.txt"), "utf8")).toBe("precious local data\n");
    });

    it("removes what a patch left behind and restores the old tip", async () => {
      const w = world({ rootBranch: "feat" });
      const tip = git(w.root, "rev-parse", "HEAD");
      writeFileSync(join(w.root, "a.txt"), "changed\n");
      writeFileSync(join(w.root, "b.txt"), "created by the patch\n");
      mkdirSync(join(w.root, "dir"));
      writeFileSync(join(w.root, "dir", "c.txt"), "x\n");
      await removePatchLeftovers(w.root, tip, ["a.txt", "b.txt", "dir/c.txt"]);
      expect(git(w.root, "status", "--porcelain", "--untracked-files=all")).toBe("");
      expect(existsSync(join(w.root, "b.txt"))).toBe(false);
      expect(readFileSync(join(w.root, "a.txt"), "utf8")).toBe("one\ntwo\n");
    });
  });

  it("refuses an existing branch at a different commit and never moves it", async () => {
    const w = world();
    const snap = snapshotOf(w.local);
    git(w.root, "fetch", "-q", "origin");
    git(w.root, "branch", "feat", "origin/main");
    await expect(
      w.moves.applySnapshot({ projectId: w.projectId, branch: "feat", ...snap }),
    ).rejects.toThrow(/different commit/);
    expect(git(w.root, "rev-parse", "feat")).toBe(git(w.root, "rev-parse", "origin/main"));
  });

  it.each([
    ["head", { head: "xyz" }],
    ["head", { head: "ABC" }],
    ["tree", { tree: "1234" }],
    ["tree", { tree: 5 }],
    ["branch", { branch: "bad..name" }],
    ["branch", { branch: "-x" }],
    ["branch", { branch: "a b" }],
    ["branch", { branch: "@{-1}" }],
    ["branch", { branch: "refs/heads/x.lock" }],
    ["instance", { instance: "elsewhere" }],
    ["patch upload", { patchUploadId: "../../etc/passwd" }],
  ])("rejects an invalid %s", async (_label, bad) => {
    const w = world();
    const snap = snapshotOf(w.local);
    await expect(
      w.moves.applySnapshot({ projectId: w.projectId, branch: "feat", ...snap, ...bad }),
    ).rejects.toThrow();
    expect(existsSync(`${w.root}-worktrees`)).toBe(false);
  });

  it("refuses to run while a Host session is running", async () => {
    const w = world();
    const snap = snapshotOf(w.local);
    const created = w.engine.command({
      type: "create", commandId: "c", projectId: w.projectId, harness: "claude",
      model: "claude:test", runtimeMode: "supervised",
    });
    w.engine.command({ type: "send", commandId: "s", sessionId: created.sessionId, text: "work" });
    await expect(
      w.moves.applySnapshot({ projectId: w.projectId, branch: "feat", ...snap }),
    ).rejects.toThrow(/Wait for running host sessions/);
    w.turns[0]?.finish();
  });

  describe("the patch cannot touch .git/ or leave the target", () => {
    const patchFor = (path: string, from = path) =>
      `diff --git a/${from} b/${path}\nnew file mode 100644\n--- /dev/null\n+++ b/${path}\n@@ -0,0 +1 @@\n+x\n`;
    it.each([
      [".git/hooks/pre-commit", patchFor(".git/hooks/pre-commit")],
      ["sub/.git/config", patchFor("sub/.git/config")],
      ["a nested .GIT", patchFor("x/.GIT/config")],
      ["../escape.txt", patchFor("../escape.txt")],
      ["a/../../escape.txt", patchFor("a/../../escape.txt")],
      ["an absolute path", "diff --git a//tmp/abs.txt b//tmp/abs.txt\nnew file mode 100644\n--- /dev/null\n+++ b//tmp/abs.txt\n@@ -0,0 +1 @@\n+x\n"],
      ["a rename into .git", "diff --git a/a.txt b/.git/a.txt\nsimilarity index 100%\nrename from a.txt\nrename to .git/a.txt\n"],
    ])("refuses %s and creates nothing", async (_label, patch) => {
      const w = world();
      const snap = snapshotOf(w.local);
      const patchUploadId = upload(w.uploads, "evil.patch", patch);
      await expect(
        w.moves.applySnapshot({ projectId: w.projectId, branch: "feat", ...snap, patchUploadId }),
      ).rejects.toThrow();
      expect(existsSync(`${w.root}-worktrees`)).toBe(false);
      expect(git(w.root, "branch", "--list", "feat")).toBe("");
      expect(existsSync(join(w.dir, "escape.txt"))).toBe(false);
      expect(existsSync(join(w.root, ".git", "hooks", "pre-commit"))).toBe(false);
    });

    it("explains a .git path with our own message", async () => {
      const w = world();
      const snap = snapshotOf(w.local);
      const patchUploadId = upload(w.uploads, "evil.patch", patchFor("sub/.git/config"));
      await expect(
        w.moves.applySnapshot({ projectId: w.projectId, branch: "feat", ...snap, patchUploadId }),
      ).rejects.toThrow(/inside \.git/);
    });

    it("classifies paths", () => {
      for (const bad of ["", ".git", "a/.git/b", "A/.GIT/b", ".git./x", "git~1/x", "../x", "a/../x", "/etc/x", "\\x", "C:/x", "a\\..\\x"])
        expect(patchPathProblem(bad), bad).toBeTruthy();
      for (const good of ["a.txt", "dir/file", ".gitignore", ".github/workflows/a.yml", "my.git/x", "a..b/c"])
        expect(patchPathProblem(good), good).toBeUndefined();
    });
  });

  it("validates branch names with check-ref-format", async () => {
    const w = world();
    await expect(validateBranch(w.root, "feature/ok-1")).resolves.toBe("feature/ok-1");
    for (const bad of ["", "a..b", "x.lock", "a b", "-x", "@", "a~b", "a\0b", 7])
      await expect(validateBranch(w.root, bad), String(bad)).rejects.toThrow("Invalid branch name");
  });
});

describe("transcript install (shared Rust fixtures)", () => {
  const cases = (JSON.parse(fixture("cases.json")) as {
    cases: { name: string; existing: string | null; incoming: string; decision: string }[];
  }).cases;
  const dirOf = () => {
    const dir = mkdtempSync(join(tmpdir(), "move-install-"));
    cleanups.push(() => rmSync(dir, { recursive: true, force: true }));
    return dir;
  };

  it.each(cases.map((c) => [c.name, c] as const))("%s", (_name, c) => {
    const dir = dirOf();
    const id = "11111111-1111-4111-8111-111111111111";
    if (c.existing) writeFileSync(join(dir, `${id}.jsonl`), fixture(c.existing));
    const incoming = fixture(c.incoming);
    const out = installTranscript({ dir, id, text: incoming, newId: () => "22222222-2222-4222-8222-222222222222" });
    expect(out.decision).toBe(c.decision);
    const dest = join(dir, `${id}.jsonl`);
    if (c.decision === "install" || c.decision === "replace") {
      expect(readFileSync(dest, "utf8")).toBe(incoming);
      if (c.decision === "replace") {
        expect(readFileSync(join(dir, `${id}.jsonl.pre-cloud`), "utf8")).toBe(fixture(c.existing!));
      } else expect(existsSync(join(dir, `${id}.jsonl.pre-cloud`))).toBe(false);
    } else if (c.decision === "already_current") {
      expect(readFileSync(dest, "utf8")).toBe(fixture(c.existing!));
      expect(readdirSync(dir)).toEqual([`${id}.jsonl`]);
    } else {
      // Divergence is never overwritten: the existing file is byte for byte as before and
      // the incoming copy lives under the new id with its sessionId fields rewritten.
      expect(readFileSync(dest, "utf8")).toBe(fixture(c.existing!));
      expect(out.id).toBe("22222222-2222-4222-8222-222222222222");
      const forked = readFileSync(out.file, "utf8");
      expect(forked).not.toContain(id);
      expect(parseChain(forked)).toEqual(parseChain(incoming));
      expect(decide(readFileSync(dest), forked)).toBe("diverged");
    }
    // No temporary files are left behind.
    expect(readdirSync(dir).filter((name) => name.startsWith("."))).toEqual([]);
  });
});

describe("sessions.import", () => {
  const ID = "11111111-1111-4111-8111-111111111111";
  const imported = (w: World, uploadId: string, extra: Record<string, unknown> = {}) =>
    w.moves.importSession({
      projectId: w.projectId, providerSessionId: ID, uploadId, cwd: w.root,
      title: "Moved", model: "claude:test", fromCwds: ["/workspaces/app"], ...extra,
    });
  const fileOf = (w: World, id = ID, cwd = w.root) =>
    join(w.claudeDir, "projects", claudeProjectDirName(cwd), `${id}.jsonl`);

  it("installs under the encoded cwd dir, rewrites cwd, and creates a block-free bound session", async () => {
    const w = world();
    const uploadId = upload(w.uploads, "t.jsonl", fixture("base.jsonl"));
    const result = await imported(w, uploadId);
    expect(result).toMatchObject({ decision: "installed", providerSessionId: ID, reused: false });
    const file = fileOf(w);
    const text = readFileSync(file, "utf8");
    expect(text).not.toContain("/workspaces/app");
    expect(text).toContain(JSON.stringify(w.root).slice(1, -1));
    expect(parseChain(text)).toEqual(parseChain(fixture("base.jsonl")));
    const stored = w.store.session(result.sessionId);
    expect(stored.session.blocks).toHaveLength(0);
    expect(stored.session).toMatchObject({ harness: "claude", providerSessionId: ID, cwd: w.root, title: "Moved" });
    expect(w.provider.bind).toHaveBeenCalledWith(result.sessionId, ID, w.root);
  });

  it.each([
    ["missing", undefined],
    ["empty", []],
    ["relative", ["workspaces/app"]],
    ["not a list", "/workspaces/app"],
  ])("requires fromCwds (%s)", async (_label, fromCwds) => {
    const w = world();
    const uploadId = upload(w.uploads, "t.jsonl", fixture("base.jsonl"));
    await expect(imported(w, uploadId, { fromCwds })).rejects.toThrow(/fromCwds/);
    expect(existsSync(join(w.claudeDir, "projects"))).toBe(false);
  });

  it("extends: replaces an older copy, keeps it as .pre-cloud and reuses the untouched session", async () => {
    const w = world();
    const first = await imported(w, upload(w.uploads, "a.jsonl", fixture("base.jsonl")));
    const before = readFileSync(fileOf(w), "utf8");
    const second = await imported(w, upload(w.uploads, "b.jsonl", fixture("extended.jsonl")));
    expect(second).toMatchObject({ decision: "replaced", reused: true, sessionId: first.sessionId });
    expect(readFileSync(`${fileOf(w)}.pre-cloud`, "utf8")).toBe(before);
    expect(parseChain(readFileSync(fileOf(w)))).toEqual(parseChain(fixture("extended.jsonl")));
    expect(w.store.summaries(w.projectId)).toHaveLength(1);
  });

  it("is idempotent: a re-import changes nothing and creates no second session", async () => {
    const w = world();
    const uploadId = upload(w.uploads, "t.jsonl", fixture("base.jsonl"));
    const first = await imported(w, uploadId);
    const bytes = readFileSync(fileOf(w));
    const again = await imported(w, uploadId);
    expect(again).toMatchObject({ decision: "already_current", reused: true, sessionId: first.sessionId });
    expect(readFileSync(fileOf(w)).equals(bytes)).toBe(true);
    expect(readdirSync(join(w.claudeDir, "projects", claudeProjectDirName(w.root)))).toEqual([`${ID}.jsonl`]);
    expect(w.store.summaries(w.projectId)).toHaveLength(1);
  });

  it("forks a diverged copy to a new id and leaves the existing file alone", async () => {
    const w = world();
    mkdirSync(join(w.claudeDir, "projects", claudeProjectDirName(w.root)), { recursive: true });
    const local = rewrite(fixture("diverged.jsonl"), w.root);
    writeFileSync(fileOf(w), local);
    const result = await imported(w, upload(w.uploads, "t.jsonl", fixture("extended.jsonl")));
    expect(result.decision).toBe("forked");
    expect(result.forkedFrom).toBe(ID);
    expect(result.providerSessionId).not.toBe(ID);
    expect(result.notice).toContain(result.providerSessionId);
    expect(readFileSync(fileOf(w), "utf8")).toBe(local);
    const forked = readFileSync(fileOf(w, result.providerSessionId), "utf8");
    expect(forked).not.toContain(ID);
    expect(forked).toContain(result.providerSessionId);
    const stored = w.store.session(result.sessionId);
    expect(stored.session.providerSessionId).toBe(result.providerSessionId);
    expect(stored.session.blocks).toHaveLength(0);
    expect(w.provider.bind).toHaveBeenCalledWith(result.sessionId, result.providerSessionId, w.root);
  });

  it("rewrites only the named roots and keeps nested subpaths and outside paths", async () => {
    const entries = [
      { type: "user", uuid: "a1", sessionId: ID, cwd: "/workspaces/app", message: "x" },
      { type: "user", uuid: "a2", sessionId: ID, cwd: "/workspaces/app/pkg/src", file: "/workspaces/app/pkg/src/a.ts" },
      { type: "user", uuid: "a3", sessionId: ID, cwd: "/home/someone", other: "/tmp/x /workspaces/app-two" },
    ];
    const text = entries.map((entry) => JSON.stringify(entry)).join("\n") + "\n";
    for (const fromCwds of [["/workspaces/app"], ["/workspaces/app/"], ["", "/workspaces/app//", "/workspaces/app"]]) {
      const w = world();
      await imported(w, upload(w.uploads, "t.jsonl", text), { fromCwds });
      const out = readFileSync(fileOf(w), "utf8").trim().split("\n").map((line) => JSON.parse(line));
      expect(out[0].cwd).toBe(w.root);
      expect(out[1].cwd).toBe(`${w.root}/pkg/src`);
      expect(out[1].file).toBe(`${w.root}/pkg/src/a.ts`);
      expect(out[2].cwd).toBe("/home/someone");
      expect(out[2].other).toBe("/tmp/x /workspaces/app-two");
    }
  });

  it("forks to the same id and the same session on a retry", async () => {
    const w = world();
    mkdirSync(join(w.claudeDir, "projects", claudeProjectDirName(w.root)), { recursive: true });
    const local = rewrite(fixture("diverged.jsonl"), w.root);
    writeFileSync(fileOf(w), local);
    const uploadId = upload(w.uploads, "t.jsonl", fixture("extended.jsonl"));
    const first = await imported(w, uploadId);
    const again = await imported(w, uploadId);
    expect(first.decision).toBe("forked");
    expect(again).toMatchObject({ decision: "forked", reused: true, sessionId: first.sessionId, providerSessionId: first.providerSessionId });
    expect(readdirSync(join(w.claudeDir, "projects", claudeProjectDirName(w.root))).sort()).toEqual(
      [`${ID}.jsonl`, `${first.providerSessionId}.jsonl`].sort(),
    );
    expect(w.store.summaries(w.projectId)).toHaveLength(1);
    expect(forkSessionId(ID, fixture("extended.jsonl"))).toBe(first.providerSessionId);
    expect(first.providerSessionId).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);
  });

  it("rejects a transcript over 50 MB before reading it", async () => {
    const w = world();
    const uploadId = upload(w.uploads, "t.jsonl", fixture("base.jsonl"));
    truncateSync(w.uploads.committedPath(uploadId), MAX_JSONL_BYTES + 1);
    await expect(imported(w, uploadId)).rejects.toThrow(/larger than 50 MB/);
    expect(existsSync(join(w.claudeDir, "projects"))).toBe(false);
    const ok = upload(w.uploads, "u.jsonl", fixture("base.jsonl"));
    truncateSync(w.uploads.committedPath(ok), MAX_JSONL_BYTES);
    await expect(imported(w, ok)).resolves.toMatchObject({ decision: "installed" });
  });

  it("refuses when a running session binds the conversation, and writes nothing", async () => {
    const w = world();
    const first = await imported(w, upload(w.uploads, "a.jsonl", fixture("base.jsonl")));
    w.engine.command({ type: "send", commandId: "go", sessionId: first.sessionId, text: "hi" });
    const before = readFileSync(fileOf(w));
    await expect(
      imported(w, upload(w.uploads, "b.jsonl", fixture("extended.jsonl"))),
    ).rejects.toThrow(/already uses this Claude conversation/);
    expect(readFileSync(fileOf(w)).equals(before)).toBe(true);
    w.turns[0]?.finish();
  });

  it("refuses when an idle session that already has turns binds the conversation", async () => {
    const w = world();
    const first = await imported(w, upload(w.uploads, "a.jsonl", fixture("base.jsonl")));
    w.engine.command({ type: "send", commandId: "go", sessionId: first.sessionId, text: "hi" });
    await vi.waitFor(() => expect(w.turns).toHaveLength(1));
    w.turns[0].finish();
    await vi.waitFor(() => expect(w.store.session(first.sessionId).status).toBe("idle"));
    expect(w.store.session(first.sessionId).session.blocks.length).toBeGreaterThan(0);
    await expect(
      imported(w, upload(w.uploads, "b.jsonl", fixture("base.jsonl"))),
    ).rejects.toThrow(/already uses this Claude conversation/);
  });

  it.each([
    ["an id with a slash", { providerSessionId: "a/b" }],
    ["a dotted id", { providerSessionId: "../x" }],
    ["an empty id", { providerSessionId: "" }],
    ["a bad upload id", { uploadId: "nope" }],
    ["a cwd outside the project", { cwd: tmpdir() }],
    ["a relative cwd", { cwd: "relative/dir" }],
    ["a missing model", { model: "" }],
    ["a bad mode", { runtimeMode: "chaos" }],
  ])("refuses %s", async (_label, bad) => {
    const w = world();
    const uploadId = upload(w.uploads, "t.jsonl", fixture("base.jsonl"));
    await expect(imported(w, uploadId, bad)).rejects.toThrow();
    expect(existsSync(join(w.claudeDir, "projects"))).toBe(false);
    expect(w.store.summaries(w.projectId)).toHaveLength(0);
  });

  it("refuses an empty transcript, a non-UTF-8 one and an uncommitted upload", async () => {
    const w = world();
    await expect(imported(w, upload(w.uploads, "e.jsonl", fixture("empty.jsonl")))).rejects.toThrow(/no entries/);
    await expect(imported(w, upload(w.uploads, "b.jsonl", Buffer.from([0x7b, 0xff, 0x7d])))).rejects.toThrow(/UTF-8/);
    const bytes = Buffer.from("x");
    const { uploadId } = w.uploads.begin({ name: "p.jsonl", size: 1, sha256: createHash("sha256").update(bytes).digest("hex") });
    await expect(imported(w, uploadId)).rejects.toThrow(/not committed/);
    expect(existsSync(join(w.claudeDir, "projects"))).toBe(false);
  });

  it("uses a registered worktree as the cwd", async () => {
    const w = world();
    const snap = snapshotOf(w.local);
    const patchUploadId = upload(w.uploads, "changes.patch", snap.patch);
    const tree = await w.moves.applySnapshot({ projectId: w.projectId, branch: "feat", ...snap, patchUploadId });
    const result = await imported(w, upload(w.uploads, "t.jsonl", fixture("base.jsonl")), { cwd: tree.cwd });
    expect(existsSync(fileOf(w, ID, tree.cwd))).toBe(true);
    expect(w.store.session(result.sessionId).session.cwd).toBe(tree.cwd);
  });
});

/** The local copy of a cloud fixture, as the Host would have stored it. */
function rewrite(text: string, cwd: string): string {
  return text.split("/workspaces/app").join(cwd);
}

describe("worktreeTree stat reuse", () => {
  /** The old algorithm: a scratch index from a plain `read-tree HEAD`, then add -A and extras. */
  function reference(repo: string, extra: string[] = []) {
    const scratch = mkdtempSync(join(tmpdir(), "move-ref-index-"));
    const env = { ...process.env, GIT_INDEX_FILE: join(scratch, "index") };
    const run = (args: string[], input?: string) =>
      execFileSync("git", args, { cwd: repo, env, input, encoding: "utf8" }).trim();
    run(["read-tree", "HEAD"]);
    run(["add", "-A"]);
    if (extra.length) run(["update-index", "--add", "--remove", "-z", "--stdin"], extra.join("\0") + "\0");
    const tree = run(["write-tree"]);
    rmSync(scratch, { recursive: true, force: true });
    return tree;
  }

  function repo(setup: (dir: string) => void = () => {}) {
    const dir = realpathSync(mkdtempSync(join(tmpdir(), "move-stat-")));
    cleanups.push(() => rmSync(dir, { recursive: true, force: true }));
    git(dir, "init", "-q", "-b", "main");
    setup(dir);
    writeFileSync(join(dir, ".gitignore"), "ignored.txt\n");
    writeFileSync(join(dir, "a.txt"), "one\n");
    writeFileSync(join(dir, "b.txt"), "two\n");
    git(dir, "add", ".");
    git(dir, "commit", "-q", "-m", "init");
    return dir;
  }

  const same = async (dir: string, extra: string[] = []) => {
    const expected = reference(dir, extra);
    expect(await worktreeTree(dir, extra)).toBe(expected);
    return expected;
  };

  it("matches the old tree for a clean checkout", async () => {
    const dir = repo();
    expect(await same(dir)).toBe(git(dir, "rev-parse", "HEAD^{tree}"));
  });

  it("matches for a modified, a deleted, an untracked and an ignored file", async () => {
    const dir = repo();
    writeFileSync(join(dir, "a.txt"), "changed\n");
    rmSync(join(dir, "b.txt"));
    writeFileSync(join(dir, "new.txt"), "new\n");
    writeFileSync(join(dir, "ignored.txt"), "x\n");
    const tree = await same(dir);
    const files = git(dir, "ls-tree", "-r", "--name-only", tree).split("\n");
    expect(files).toContain("new.txt");
    expect(files).not.toContain("b.txt");
    expect(files).not.toContain("ignored.txt");
  });

  it("handles a file staged in the real index but not in HEAD like the old algorithm", async () => {
    const dir = repo();
    writeFileSync(join(dir, "staged.txt"), "s\n");
    git(dir, "add", "staged.txt");
    writeFileSync(join(dir, "ignored.txt"), "x\n");
    git(dir, "add", "-f", "ignored.txt");
    await same(dir);
    await same(dir, ["ignored.txt"]);
  });

  it("includes an ignored file passed in extra", async () => {
    const dir = repo();
    writeFileSync(join(dir, "ignored.txt"), "x\n");
    const tree = await worktreeTree(dir, ["ignored.txt"]);
    expect(tree).toBe(reference(dir, ["ignored.txt"]));
    expect(git(dir, "ls-tree", "-r", "--name-only", tree)).toContain("ignored.txt");
  });

  it("works in a linked worktree", async () => {
    const dir = repo();
    const linked = join(dir, "..", `${dir.split("/").pop()}-linked`);
    git(dir, "worktree", "add", "-q", "-b", "other", linked);
    cleanups.push(() => rmSync(linked, { recursive: true, force: true }));
    writeFileSync(join(linked, "a.txt"), "linked change\n");
    writeFileSync(join(linked, "n.txt"), "n\n");
    await same(linked);
  });

  it("falls back correctly with core.splitIndex", async () => {
    const dir = repo();
    git(dir, "config", "core.splitIndex", "true");
    git(dir, "update-index", "--split-index");
    writeFileSync(join(dir, "a.txt"), "changed\n");
    writeFileSync(join(dir, "n.txt"), "n\n");
    await same(dir);
  });

  it("re-hashes a racily clean file changed without a size change", async () => {
    const dir = repo();
    // ctime always moves on a write, so ignore it: only size and mtime can reveal the change.
    git(dir, "config", "core.trustctime", "false");
    const file = join(dir, "a.txt");
    const when = new Date(Math.floor(Date.now() / 1000) * 1000 - 5000);
    utimesSync(file, when, when);
    git(dir, "update-index", "--refresh");
    // The index is as new as the entry, so the entry is racily clean: git must re-hash it.
    const index = join(dir, ".git", "index");
    utimesSync(index, when, when);
    writeFileSync(file, "ONE\n");
    utimesSync(file, when, when);
    const tree = await same(dir);
    expect(git(dir, "show", `${tree}:a.txt`)).toBe("ONE");
  });

  for (const [flag, label] of [
    ["--assume-unchanged", "assume-unchanged"],
    ["--skip-worktree", "skip-worktree"],
  ] as const)
    for (const withExtra of [false, true])
      it(`matches the old tree for a modified ${label} file${withExtra ? " listed in extra" : ""}`, async () => {
        const dir = repo();
        git(dir, "update-index", flag, "a.txt");
        writeFileSync(join(dir, "a.txt"), "changed\n");
        writeFileSync(join(dir, "n.txt"), "n\n");
        const tree = await same(dir, withExtra ? ["a.txt"] : []);
        expect(git(dir, "ls-tree", "-r", "--name-only", tree)).toContain("n.txt");
        expect(git(dir, "show", `${tree}:a.txt`)).toBe("changed");
      });

  it("never changes the real index", async () => {
    const dir = repo();
    writeFileSync(join(dir, "a.txt"), "changed\n");
    const index = join(dir, ".git", "index");
    const bytes = readFileSync(index);
    const mtime = statSync(index).mtimeMs;
    await worktreeTree(dir, ["a.txt"]);
    expect(readFileSync(index).equals(bytes)).toBe(true);
    expect(statSync(index).mtimeMs).toBe(mtime);
    expect(existsSync(join(dir, ".git", "index.lock"))).toBe(false);
  });
});
