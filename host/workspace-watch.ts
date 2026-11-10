import { realpathSync, statSync, watch as fsWatch } from "node:fs";
import { readdir, realpath, stat } from "node:fs/promises";
import { execFile } from "node:child_process";
import { randomUUID } from "node:crypto";
import { join, sep } from "node:path";
import { promisify } from "node:util";

const exec = promisify(execFile);

export const BATCH_MS = 100;
export const WAIT_MS = 15_000;
export const LEASE_MS = 60_000;
export const MAX_WATCHERS = 128;
export const MAX_DIRS = 256;
/** Minimum gap between self-healing passes, and the cap on one pass. */
export const HEAL_INTERVAL_MS = 2_000;
export const HEAL_TIMEOUT_MS = 5_000;
/** Ref directories are watched one by one (no recursion). */
const MAX_REF_DIRS = 64;
const MAX_REF_DEPTH = 4;

/** Clients re-subscribe when they see this prefix. */
export const WATCH_UNKNOWN = "watch_unknown";

/** Metadata files inside a git dir that change what `git status` shows. */
const GIT_FILES = new Set(["HEAD", "index", "packed-refs"]);

export type WatchHandle = {
  close(): void;
  on(event: "error", listener: (error: Error) => void): unknown;
};
export type WatchFn = (
  path: string,
  options: { persistent: boolean },
  listener: (event: string, filename: string | Buffer | null) => void,
) => WatchHandle;

export type WatchChanges = { git: boolean; dirs: string[]; overflow: boolean };

/** A directory the client wants watched. `rel` is slash-separated and relative
 * to the watch root ("" is the root); `path` is its validated absolute path. */
export type WatchDir = { rel: string; path: string };

async function gitDirs(cwd: string): Promise<string[]> {
  try {
    const { stdout } = await exec(
      "git",
      ["-C", cwd, "rev-parse", "--path-format=absolute", "--git-dir", "--git-common-dir"],
      {
        env: { ...process.env, GIT_OPTIONAL_LOCKS: "0", GIT_TERMINAL_PROMPT: "0" },
        timeout: 10_000,
      },
    );
    return [...new Set(stdout.split("\n").map((line) => line.trim()).filter(Boolean))];
  } catch {
    return [];
  }
}

const identity = (path: string): string | undefined => {
  try {
    const { dev, ino } = statSync(path);
    return `${dev}:${ino}`;
  } catch {
    return undefined;
  }
};

const identityAsync = async (path: string): Promise<string | undefined> => {
  try {
    const { dev, ino } = await stat(path);
    return `${dev}:${ino}`;
  } catch {
    return undefined;
  }
};

type Wanted = WatchDir & { real?: string };

class Watch {
  private handles = new Map<string, WatchHandle>();
  /** Live client-dir watches with the inode they were created on. */
  private dirKeys = new Map<string, WatchDir & { id?: string }>();
  /** What the client last asked for, kept even when a handle is lost. */
  private wanted = new Map<string, Wanted>();
  private gitWanted: string[] = [];
  private healing = false;
  private cooling = false;
  private healAgain = false;
  private cooldown?: ReturnType<typeof setTimeout>;
  private healTimer?: ReturnType<typeof setTimeout>;
  private refDirs = new Map<string, string | undefined>();
  private refRoots: string[] = [];
  private degraded = new Set<string>();
  private refChain: Promise<void> = Promise.resolve();
  private refQueued = false;
  private pendingGit = false;
  private pendingDirs = new Set<string>();
  private overflow = false;
  private batch?: ReturnType<typeof setTimeout>;
  private lease?: ReturnType<typeof setTimeout>;
  private waiter?: {
    resolve: (changes: WatchChanges) => void;
    timer: ReturnType<typeof setTimeout>;
  };
  private closed = false;

  constructor(
    readonly cwd: string,
    private readonly watchFn: WatchFn,
    private readonly expire: () => void,
  ) {}

  async start(dirs: WatchDir[]) {
    this.renew();
    const cwd = this.cwd;
    const gitDirList = await gitDirs(cwd);
    // Nothing to watch for git: tell the client to fall back to refreshing.
    if (!gitDirList.length) this.degrade();
    for (const dir of gitDirList) {
      this.gitWanted.push(dir);
      this.addGit(dir, false);
      this.refRoots.push(join(dir, "refs"));
    }
    await this.syncAllRefs();
    this.setDirs(dirs);
  }

  private addGit(dir: string, quiet: boolean): boolean {
    return this.add(
      `git:${dir}`,
      dir,
      (name) => {
        if (name === null) return this.gitChanged();
        if (name.endsWith(".lock")) return;
        if (GIT_FILES.has(name) || name === "refs") this.gitChanged();
      },
      quiet,
    );
  }

  private degrade() {
    this.overflow = true;
    this.schedule();
  }

  private add(
    key: string,
    path: string,
    onEvent: (name: string | null) => void,
    quiet = false,
  ): boolean {
    if (this.closed || this.handles.has(key)) return false;
    try {
      const handle = this.watchFn(path, { persistent: false }, (_event, name) => {
        if (!this.closed) onEvent(name === null ? null : name.toString());
      });
      handle.on("error", () => this.drop(key, true));
      this.handles.set(key, handle);
      return true;
    } catch {
      // ENOSPC, EMFILE or a directory that vanished: the client refreshes.
      // Quiet retries stay silent so a persistent failure is reported once.
      if (!quiet) this.degrade();
      return false;
    }
  }

  private drop(key: string, overflow: boolean) {
    const handle = this.handles.get(key);
    this.handles.delete(key);
    // A dead handle must not block re-watching the same path later.
    if (key.startsWith("dir:")) this.dirKeys.delete(key.slice(4));
    else if (key.startsWith("ref:")) this.refDirs.delete(key.slice(4));
    try {
      handle?.close();
    } catch {
      // Already closed.
    }
    if (overflow && !this.closed) this.degrade();
  }

  private syncAllRefs(): Promise<void> {
    if (this.refQueued) return this.refChain;
    this.refQueued = true;
    this.refChain = this.refChain
      .then(async () => {
        this.refQueued = false;
        if (this.closed) return;
        const hits = new Set<string>();
        const found: string[] = [];
        for (const root of this.refRoots) await this.walkRefs(root, 0, hits, found);
        if (this.closed) return;
        // Phase 1: release watches on directories that are gone or were
        // replaced, so stale entries never count toward the cap.
        for (const [known, id] of [...this.refDirs]) {
          if (!found.includes(known) || id !== (await identityAsync(known)))
            this.drop(`ref:${known}`, false);
        }
        // Phase 2: watch what is missing, up to the cap.
        for (const dir of found) {
          if (this.closed || this.refDirs.has(dir)) continue;
          if (this.refDirs.size >= MAX_REF_DIRS) {
            hits.add("cap");
            break;
          }
          const id = await identityAsync(dir);
          if (this.add(`ref:${dir}`, dir, () => this.gitChanged())) this.refDirs.set(dir, id);
        }
        // Report each degraded state once per episode, not on every sync.
        let fresh = false;
        for (const hit of hits) if (!this.degraded.has(hit)) fresh = true;
        this.degraded = hits;
        if (fresh && !this.closed) this.degrade();
      })
      .catch(() => undefined);
    return this.refChain;
  }

  /** Lists `dir` and its ref sub-directories (depth first, bounded depth). */
  private async walkRefs(
    dir: string,
    depth: number,
    hits: Set<string>,
    found: string[],
  ): Promise<void> {
    if (this.closed) return;
    if (depth > MAX_REF_DEPTH) {
      hits.add("depth");
      return;
    }
    const entries = await readdir(dir, { withFileTypes: true }).catch(() => undefined);
    if (!entries) return;
    found.push(dir);
    for (const entry of entries)
      if (entry.isDirectory()) await this.walkRefs(join(dir, entry.name), depth + 1, hits, found);
  }

  /** Replaces the watched client directories. A directory whose inode changed
   * since it was watched (removed and recreated) is watched again. */
  setDirs(dirs: WatchDir[]) {
    this.renew();
    this.wanted = new Map(
      dirs.map((dir) => {
        let real: string | undefined;
        try {
          real = realpathSync.native(dir.path);
        } catch {
          // Healing keeps retrying while it does not resolve.
        }
        return [dir.rel, { ...dir, real }] as const;
      }),
    );
    for (const [rel] of [...this.dirKeys])
      if (!this.wanted.has(rel)) this.drop(`dir:${rel}`, false);
    for (const [rel, dir] of this.wanted) this.watchDir(rel, dir, false);
  }

  private watchDir(rel: string, dir: WatchDir, quiet: boolean) {
    const id = identity(dir.path);
    const current = this.dirKeys.get(rel);
    if (current && current.path === dir.path && current.id === id && this.handles.has(`dir:${rel}`))
      return;
    const replaced = current !== undefined && current.id !== id;
    this.drop(`dir:${rel}`, false);
    const added = this.add(
      `dir:${rel}`,
      dir.path,
      (name) => {
        if (name === ".git") return;
        this.pendingDirs.add(rel);
        this.schedule();
        // A watched directory that was deleted, renamed away or replaced is
        // re-checked by a healing pass.
        if (name === null || name === dir.path.split(sep).pop()) this.requestHeal();
      },
      quiet,
    );
    // Remember the directory only while a live handle exists.
    if (added) this.dirKeys.set(rel, { ...dir, id });
    if (replaced || (added && quiet)) {
      this.pendingDirs.add(rel);
      this.schedule();
    }
  }

  /** Runs a healing pass now, or right after the current cool-down. */
  private requestHeal() {
    if (this.closed) return;
    if (this.cooling || this.healing) {
      this.healAgain = true;
      return;
    }
    void this.heal();
  }

  private healIfAsked() {
    if (!this.healAgain || this.cooling || this.healing) return;
    this.healAgain = false;
    this.requestHeal();
  }

  private async heal() {
    if (this.closed) return;
    this.healing = true;
    this.cooling = true;
    this.cooldown = setTimeout(() => {
      this.cooling = false;
      this.cooldown = undefined;
      this.healIfAsked();
    }, HEAL_INTERVAL_MS);
    this.cooldown.unref?.();
    const run = this.healPass()
      .catch(() => undefined)
      .finally(() => {
        this.healing = false;
        this.healIfAsked();
      });
    try {
      await Promise.race([
        run,
        new Promise<void>((resolve) => {
          this.healTimer = setTimeout(resolve, HEAL_TIMEOUT_MS);
          this.healTimer.unref?.();
        }),
      ]);
    } finally {
      clearTimeout(this.healTimer);
    }
  }

  /** Restores watches that were lost and re-checks the live ones, without
   * blocking the event loop. Every re-created watch is confined again. */
  private async healPass() {
    for (const [rel, dir] of [...this.wanted]) {
      if (this.closed) return;
      const live = this.dirKeys.get(rel);
      const real = await realpath(dir.path).catch(() => undefined);
      if (this.closed || this.wanted.get(rel) !== dir) continue;
      if (real === undefined || real !== (dir.real ?? dir.path)) {
        // Gone, or swapped for a link that leads elsewhere: never watch it.
        if (live) this.drop(`dir:${rel}`, true);
        continue;
      }
      const id = await identityAsync(real);
      if (this.closed || this.wanted.get(rel) !== dir) continue;
      if (id === undefined) {
        if (live) this.drop(`dir:${rel}`, true);
      } else if (!live || live.id !== id || !this.handles.has(`dir:${rel}`)) {
        this.watchDir(rel, dir, true);
      }
    }
    if (this.closed) return;
    for (const dir of this.gitWanted)
      if (!this.handles.has(`git:${dir}`) && this.addGit(dir, true)) this.gitChanged();
    await this.syncAllRefs();
  }

  private gitChanged() {
    this.pendingGit = true;
    this.schedule();
    void this.syncAllRefs();
  }

  private schedule() {
    if (this.batch || this.closed) return;
    this.batch = setTimeout(() => {
      this.batch = undefined;
      this.deliver();
    }, BATCH_MS);
    this.batch.unref?.();
  }

  private take(): WatchChanges | undefined {
    if (!this.pendingGit && !this.pendingDirs.size && !this.overflow) return undefined;
    const changes = {
      git: this.pendingGit,
      dirs: [...this.pendingDirs].sort(),
      overflow: this.overflow,
    };
    this.pendingGit = false;
    this.pendingDirs.clear();
    this.overflow = false;
    return changes;
  }

  private deliver() {
    if (!this.waiter) return;
    const changes = this.take();
    if (changes) this.finish(changes);
  }

  private finish(changes: WatchChanges) {
    const waiter = this.waiter;
    this.waiter = undefined;
    if (!waiter) return;
    clearTimeout(waiter.timer);
    waiter.resolve(changes);
    this.renew();
  }

  private renew() {
    if (this.closed) return;
    clearTimeout(this.lease);
    this.lease = setTimeout(this.expire, LEASE_MS);
    this.lease.unref?.();
  }

  wait(): Promise<WatchChanges> {
    // A newer request supersedes a pending one (a retry after a lost response).
    this.finish({ git: false, dirs: [], overflow: false });
    this.renew();
    // Heal lost or replaced watches (git checkout, mv + mkdir), rate limited.
    this.requestHeal();
    // While a batch window is open, wait for it so one burst is one response.
    if (!this.batch) {
      const ready = this.take();
      if (ready) return Promise.resolve(ready);
    }
    return new Promise((resolve) => {
      const timer = setTimeout(
        () => this.finish({ git: false, dirs: [], overflow: false }),
        WAIT_MS,
      );
      timer.unref?.();
      this.waiter = { resolve, timer };
    });
  }

  close() {
    if (this.closed) return;
    this.closed = true;
    clearTimeout(this.batch);
    clearTimeout(this.lease);
    clearTimeout(this.cooldown);
    clearTimeout(this.healTimer);
    this.cooldown = undefined;
    this.batch = undefined;
    const waiter = this.waiter;
    this.waiter = undefined;
    if (waiter) {
      clearTimeout(waiter.timer);
      waiter.resolve({ git: false, dirs: [], overflow: false });
    }
    for (const key of [...this.handles.keys()]) this.drop(key, false);
  }

  get openHandles() {
    return this.handles.size;
  }
}

/** Non-recursive change watches with a long-poll. Callers pass directories that
 * are already confined to the workspace; this class only watches them. Leases
 * release watchers whose client went away without unsubscribing. */
export class WorkspaceWatchHost {
  private watches = new Map<string, Watch>();

  constructor(private readonly watchFn: WatchFn = fsWatch as unknown as WatchFn) {}

  get size() {
    return this.watches.size;
  }

  /** Total open fs.watch handles, for leak checks. */
  get openHandles() {
    let total = 0;
    for (const watch of this.watches.values()) total += watch.openHandles;
    return total;
  }

  async start(cwd: string, dirs: WatchDir[]): Promise<string> {
    if (dirs.length > MAX_DIRS) throw new Error(`Too many watched directories (maximum ${MAX_DIRS})`);
    if (this.watches.size >= MAX_WATCHERS) throw new Error("Too many watchers");
    const id = randomUUID();
    const watch = new Watch(cwd, this.watchFn, () => this.stop(id));
    // Reserve the slot first so concurrent starts respect the cap.
    this.watches.set(id, watch);
    try {
      await watch.start(dirs);
    } catch (error) {
      this.stop(id);
      throw error;
    }
    return id;
  }

  private get(id: unknown): Watch {
    const watch = typeof id === "string" ? this.watches.get(id) : undefined;
    if (!watch) throw new Error(`${WATCH_UNKNOWN}: watcher is unknown or expired; subscribe again`);
    return watch;
  }

  /** The workspace root a watcher was created for. */
  root(id: unknown): string {
    return this.get(id).cwd;
  }

  update(id: unknown, dirs: WatchDir[]) {
    if (dirs.length > MAX_DIRS) throw new Error(`Too many watched directories (maximum ${MAX_DIRS})`);
    this.get(id).setDirs(dirs);
  }

  wait(id: unknown): Promise<WatchChanges> {
    return this.get(id).wait();
  }

  stop(id: unknown) {
    const watch = typeof id === "string" ? this.watches.get(id) : undefined;
    if (typeof id === "string") this.watches.delete(id);
    watch?.close();
  }

  close() {
    for (const id of [...this.watches.keys()]) this.stop(id);
  }
}
