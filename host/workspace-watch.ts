import { watch as fsWatch } from "node:fs";
import { readdir } from "node:fs/promises";
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

class Watch {
  private handles = new Map<string, WatchHandle>();
  private dirKeys = new Map<string, WatchDir>();
  private refDirs = new Set<string>();
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
    for (const dir of await gitDirs(cwd)) {
      this.add(`git:${dir}`, dir, (name) => {
        if (name === null) return this.gitChanged();
        if (name.endsWith(".lock")) return;
        if (GIT_FILES.has(name) || name === "refs") this.gitChanged();
      });
      await this.syncRefs(join(dir, "refs"), 0);
    }
    this.setDirs(dirs);
  }

  private add(
    key: string,
    path: string,
    onEvent: (name: string | null) => void,
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
      this.overflow = true;
      this.schedule();
      return false;
    }
  }

  private drop(key: string, overflow: boolean) {
    const handle = this.handles.get(key);
    this.handles.delete(key);
    try {
      handle?.close();
    } catch {
      // Already closed.
    }
    if (overflow && !this.closed) {
      this.overflow = true;
      this.schedule();
    }
  }

  /** Watches `dir` and its sub-directories of refs, one non-recursive watch each. */
  private async syncRefs(dir: string, depth: number): Promise<void> {
    if (this.closed || depth > MAX_REF_DEPTH) return;
    if (!this.refDirs.has(dir)) {
      if (this.refDirs.size >= MAX_REF_DIRS) {
        this.overflow = true;
        return;
      }
      const added = this.add(`ref:${dir}`, dir, () => {
        this.gitChanged();
        void this.syncRefs(dir, depth).catch(() => undefined);
      });
      if (!added) return;
      this.refDirs.add(dir);
    }
    const entries = await readdir(dir, { withFileTypes: true }).catch(() => []);
    for (const entry of entries)
      if (entry.isDirectory()) await this.syncRefs(join(dir, entry.name), depth + 1);
  }

  /** Replaces the watched client directories. */
  setDirs(dirs: WatchDir[]) {
    this.renew();
    const wanted = new Map(dirs.map((dir) => [dir.rel, dir]));
    for (const [rel] of this.dirKeys)
      if (!wanted.has(rel)) {
        this.drop(`dir:${rel}`, false);
        this.dirKeys.delete(rel);
      }
    for (const [rel, dir] of wanted) {
      if (this.dirKeys.get(rel)?.path === dir.path) continue;
      this.drop(`dir:${rel}`, false);
      this.dirKeys.set(rel, dir);
      this.add(`dir:${rel}`, dir.path, (name) => {
        if (name === ".git") return;
        this.pendingDirs.add(rel);
        this.schedule();
        // A watched directory that is deleted or renamed away degrades to a
        // full refresh; the dead watch is released.
        if (name === null || name === dir.path.split(sep).pop())
          void readdir(dir.path).catch(() => {
            this.drop(`dir:${rel}`, true);
            this.dirKeys.delete(rel);
          });
      });
    }
  }

  private gitChanged() {
    this.pendingGit = true;
    this.schedule();
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
