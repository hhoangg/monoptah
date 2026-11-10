import { execFileSync } from "node:child_process";
import {
  mkdirSync,
  mkdtempSync,
  realpathSync,
  renameSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { realpath, readdir, stat } from "node:fs/promises";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import {
  BATCH_MS,
  HEAL_INTERVAL_MS,
  LEASE_MS,
  MAX_DIRS,
  MAX_WATCHERS,
  WAIT_MS,
  WATCH_UNKNOWN,
  HEAL_TIMEOUT_MS,
  WorkspaceWatchHost,
  type FsOps,
  type WatchFn,
} from "./workspace-watch";

type Fake = {
  path: string;
  options: { persistent: boolean };
  closed: boolean;
  emit: (event: string, name: string | null) => void;
  fail: (error: Error) => void;
};

const cleanups: Array<() => void> = [];
const tmp = () => {
  const dir = realpathSync.native(mkdtempSync(join(tmpdir(), "monoptah-watch-")));
  cleanups.push(() => rmSync(dir, { recursive: true, force: true }));
  return dir;
};
const repo = () => {
  const cwd = tmp();
  execFileSync("git", ["init", "-q", "-b", "main"], { cwd });
  mkdirSync(join(cwd, "src"));
  mkdirSync(join(cwd, "other"));
  return cwd;
};

function fakes() {
  const all: Fake[] = [];
  let failNext: Error | undefined;
  const failing = new Set<string>();
  const watchFn: WatchFn = (path, options, listener) => {
    if (failing.has(path)) throw Object.assign(new Error("ENOSPC"), { code: "ENOSPC" });
    if (failNext) {
      const error = failNext;
      failNext = undefined;
      throw error;
    }
    let onError: (error: Error) => void = () => undefined;
    const fake: Fake = {
      path,
      options,
      closed: false,
      emit: (event, name) => {
        if (!fake.closed) listener(event, name);
      },
      fail: (error) => onError(error),
    };
    all.push(fake);
    return {
      close: () => {
        fake.closed = true;
      },
      on: (_event, handler) => {
        onError = handler;
      },
    };
  };
  return {
    all,
    watchFn,
    open: () => all.filter((fake) => !fake.closed),
    at: (path: string) => all.find((fake) => fake.path === path && !fake.closed)!,
    failPath: (path: string, fail = true) => {
      if (fail) failing.add(path);
      else failing.delete(path);
    },
    failNext: (error: Error) => {
      failNext = error;
    },
  };
}

beforeEach(() => vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] }));
afterEach(() => {
  vi.useRealTimers();
  cleanups.splice(0).reverse().forEach((cleanup) => cleanup());
});

const dir = (cwd: string, rel: string) => ({ rel, path: rel ? join(cwd, rel) : cwd });
const settle = () => vi.advanceTimersByTimeAsync(BATCH_MS);
/** Waits for real file system work without moving the fake clock. */
const until = async (check: () => boolean) => {
  const end = Date.now() + 3000;
  while (!check() && Date.now() < end) await new Promise((resolve) => setImmediate(resolve));
  expect(check()).toBe(true);
};
const real = async (ms: number) => {
  const end = Date.now() + ms;
  while (Date.now() < end) await new Promise((resolve) => setImmediate(resolve));
};

it("reports a git metadata change as git: true after the batch window", async () => {
  const cwd = repo();
  const f = fakes();
  const host = new WorkspaceWatchHost(f.watchFn);
  const id = await host.start(cwd, []);
  const pending = host.wait(id);
  f.at(join(cwd, ".git")).emit("change", "HEAD");
  await settle();
  expect(await pending).toEqual({ git: true, dirs: [], overflow: false });
  host.close();
});

it("ignores lock files and unrelated git internals", async () => {
  const cwd = repo();
  const f = fakes();
  const host = new WorkspaceWatchHost(f.watchFn);
  const id = await host.start(cwd, []);
  const pending = host.wait(id);
  const git = f.at(join(cwd, ".git"));
  git.emit("change", "index.lock");
  git.emit("change", "COMMIT_EDITMSG");
  git.emit("change", "objects");
  await vi.advanceTimersByTimeAsync(WAIT_MS);
  expect(await pending).toEqual({ git: false, dirs: [], overflow: false });
  host.close();
});

it("watches refs sub-directories and reports ref changes", async () => {
  const cwd = repo();
  mkdirSync(join(cwd, ".git", "refs", "heads", "feature"), { recursive: true });
  const f = fakes();
  const host = new WorkspaceWatchHost(f.watchFn);
  const id = await host.start(cwd, []);
  expect(f.open().map((fake) => fake.path)).toContain(
    join(cwd, ".git", "refs", "heads", "feature"),
  );
  const pending = host.wait(id);
  f.at(join(cwd, ".git", "refs", "heads", "feature")).emit("rename", "x");
  await settle();
  expect((await pending).git).toBe(true);
  host.close();
});

it("reports a file in a watched dir and nothing for an unwatched dir", async () => {
  const cwd = repo();
  const f = fakes();
  const host = new WorkspaceWatchHost(f.watchFn);
  const id = await host.start(cwd, [dir(cwd, "src")]);
  expect(f.open().some((fake) => fake.path === join(cwd, "other"))).toBe(false);
  const pending = host.wait(id);
  f.at(join(cwd, "src")).emit("rename", "new.ts");
  await settle();
  expect(await pending).toEqual({ git: false, dirs: ["src"], overflow: false });

  const quiet = host.wait(id);
  // No watcher exists for "other", so nothing can be delivered for it.
  await vi.advanceTimersByTimeAsync(WAIT_MS);
  expect(await quiet).toEqual({ git: false, dirs: [], overflow: false });
  host.close();
});

it("batches a burst into one response and keeps events that arrive before a wait", async () => {
  const cwd = repo();
  const f = fakes();
  const host = new WorkspaceWatchHost(f.watchFn);
  const id = await host.start(cwd, [dir(cwd, "src"), dir(cwd, "other")]);
  f.at(join(cwd, "src")).emit("change", "a");
  f.at(join(cwd, "other")).emit("change", "b");
  f.at(join(cwd, "src")).emit("change", "c");
  await settle();
  expect(await host.wait(id)).toEqual({ git: false, dirs: ["other", "src"], overflow: false });
  host.close();
});

it("never watches recursively and never keeps the process alive", async () => {
  const cwd = repo();
  const f = fakes();
  const host = new WorkspaceWatchHost(f.watchFn);
  await host.start(cwd, [dir(cwd, ""), dir(cwd, "src")]);
  expect(f.all.length).toBeGreaterThan(2);
  for (const fake of f.all) {
    expect(fake.options).toEqual({ persistent: false });
    expect(fake.options).not.toHaveProperty("recursive");
  }
  host.close();
});

it("update_watch swaps directories and closes the removed ones", async () => {
  const cwd = repo();
  const f = fakes();
  const host = new WorkspaceWatchHost(f.watchFn);
  const id = await host.start(cwd, [dir(cwd, "src")]);
  const src = f.at(join(cwd, "src"));
  host.update(id, [dir(cwd, "other")]);
  expect(src.closed).toBe(true);
  expect(f.at(join(cwd, "other"))).toBeDefined();
  host.close();
});

it("long-poll returns empty after WAIT_MS", async () => {
  const cwd = repo();
  const f = fakes();
  const host = new WorkspaceWatchHost(f.watchFn);
  const id = await host.start(cwd, []);
  let done = false;
  const pending = host.wait(id).then((value) => {
    done = true;
    return value;
  });
  await vi.advanceTimersByTimeAsync(WAIT_MS - 1);
  expect(done).toBe(false);
  await vi.advanceTimersByTimeAsync(1);
  expect(await pending).toEqual({ git: false, dirs: [], overflow: false });
  expect(WAIT_MS).toBe(15_000);
  host.close();
});

it("expires an idle lease, renews on wait, and tells the client to re-subscribe", async () => {
  const cwd = repo();
  const f = fakes();
  const host = new WorkspaceWatchHost(f.watchFn);
  const id = await host.start(cwd, [dir(cwd, "src")]);
  await vi.advanceTimersByTimeAsync(LEASE_MS - 1_000);
  const pending = host.wait(id); // renews
  await vi.advanceTimersByTimeAsync(WAIT_MS);
  await pending;
  await vi.advanceTimersByTimeAsync(LEASE_MS - 1_000);
  expect(host.size).toBe(1);
  await vi.advanceTimersByTimeAsync(1_000);
  expect(host.size).toBe(0);
  expect(f.open()).toHaveLength(0);
  expect(vi.getTimerCount()).toBe(0);
  expect(() => host.wait(id)).toThrow(WATCH_UNKNOWN);
  expect(() => host.update(id, [])).toThrow(WATCH_UNKNOWN);
  expect(() => host.wait("nope")).toThrow(WATCH_UNKNOWN);
});

it("caps the number of watchers", async () => {
  const cwd = tmp();
  const f = fakes();
  const host = new WorkspaceWatchHost(f.watchFn);
  const ids: string[] = [];
  for (let i = 0; i < MAX_WATCHERS; i++) ids.push(await host.start(cwd, []));
  await expect(host.start(cwd, [])).rejects.toThrow("Too many watchers");
  host.stop(ids[0]);
  await host.start(cwd, []);
  host.close();
  expect(host.size).toBe(0);
});

it("caps watched directories", async () => {
  const cwd = tmp();
  const host = new WorkspaceWatchHost(fakes().watchFn);
  const many = Array.from({ length: MAX_DIRS + 1 }, (_, i) => dir(cwd, `d${i}`));
  await expect(host.start(cwd, many)).rejects.toThrow("Too many");
  expect(host.size).toBe(0);
});

it("degrades fs.watch failures to overflow instead of crashing", async () => {
  const cwd = repo();
  const f = fakes();
  const host = new WorkspaceWatchHost(f.watchFn);
  const id = await host.start(cwd, [dir(cwd, "src")]);

  // A runtime watcher error (for example EMFILE).
  const pending = host.wait(id);
  f.at(join(cwd, "src")).fail(Object.assign(new Error("EMFILE"), { code: "EMFILE" }));
  await settle();
  expect((await pending).overflow).toBe(true);

  // A watch that cannot be created (ENOSPC) on update.
  f.failNext(Object.assign(new Error("ENOSPC"), { code: "ENOSPC" }));
  host.update(id, [dir(cwd, "other")]);
  await settle();
  expect(await host.wait(id)).toMatchObject({ overflow: true });

  // A watched directory that disappears.
  host.update(id, [dir(cwd, "src")]);
  const src = f.at(join(cwd, "src"));
  rmSync(join(cwd, "src"), { recursive: true });
  await vi.advanceTimersByTimeAsync(HEAL_INTERVAL_MS);
  src.emit("rename", "src");
  await until(() => src.closed);
  await settle();
  expect(await host.wait(id)).toMatchObject({ overflow: true });
  host.close();
});

it("retries a client directory whose first watch could not be created", async () => {
  const cwd = repo();
  const f = fakes();
  const host = new WorkspaceWatchHost(f.watchFn);
  const id = await host.start(cwd, []);
  f.failNext(Object.assign(new Error("ENOSPC"), { code: "ENOSPC" }));
  host.update(id, [dir(cwd, "src")]);
  await settle();
  expect(await host.wait(id)).toMatchObject({ overflow: true });
  expect(f.open().some((fake) => fake.path === join(cwd, "src"))).toBe(false);

  // The same update again must try again, not skip the directory.
  host.update(id, [dir(cwd, "src")]);
  const pending = host.wait(id);
  f.at(join(cwd, "src")).emit("change", "a.ts");
  await settle();
  expect(await pending).toMatchObject({ dirs: ["src"] });

  // A handle that errors later is released and can be watched again.
  const pendingError = host.wait(id);
  f.at(join(cwd, "src")).fail(new Error("EMFILE"));
  await settle();
  expect(await pendingError).toMatchObject({ overflow: true });
  host.update(id, [dir(cwd, "src")]);
  expect(f.at(join(cwd, "src"))).toBeDefined();
  host.close();
});

it("re-watches a ref directory that was deleted and recreated", async () => {
  const cwd = repo();
  const heads = join(cwd, ".git", "refs", "heads");
  const feature = join(heads, "feature");
  mkdirSync(feature, { recursive: true });
  const f = fakes();
  const host = new WorkspaceWatchHost(f.watchFn);
  const id = await host.start(cwd, []);
  const first = f.at(feature);
  rmSync(feature, { recursive: true });
  f.at(heads).emit("rename", "feature");
  await settle();
  await vi.waitFor(() => expect(first.closed).toBe(true));
  mkdirSync(feature);
  f.at(heads).emit("rename", "feature");
  await settle();
  await vi.waitFor(() => expect(f.open().some((fake) => fake.path === feature)).toBe(true));
  expect(f.at(feature)).not.toBe(first);
  const pending = host.wait(id);
  f.at(feature).emit("rename", "x");
  await settle();
  expect((await pending).git).toBe(true);
  host.close();
});

it("re-watches a directory replaced by a new one of the same name", async () => {
  const cwd = repo();
  const f = fakes();
  const host = new WorkspaceWatchHost(f.watchFn);
  const id = await host.start(cwd, [dir(cwd, "src")]);
  const old = f.at(join(cwd, "src"));
  renameSync(join(cwd, "src"), join(cwd, "src.old"));
  mkdirSync(join(cwd, "src"));
  host.update(id, [dir(cwd, "src")]);
  expect(old.closed).toBe(true);
  const fresh = f.at(join(cwd, "src"));
  expect(fresh).not.toBe(old);
  const replaced = host.wait(id);
  await settle();
  expect(await replaced).toMatchObject({ dirs: ["src"] });

  // Without an update, a rename-self event heals it too.
  await vi.advanceTimersByTimeAsync(HEAL_INTERVAL_MS);
  renameSync(join(cwd, "src"), join(cwd, "src.old2"));
  mkdirSync(join(cwd, "src"));
  fresh.emit("rename", null);
  await until(() => fresh.closed);
  await until(() => f.open().some((fake) => fake.path === join(cwd, "src")));
  const healed = f.at(join(cwd, "src"));
  expect(healed).not.toBe(fresh);

  // So does the next poll.
  await vi.advanceTimersByTimeAsync(HEAL_INTERVAL_MS);
  renameSync(join(cwd, "src"), join(cwd, "src.old3"));
  mkdirSync(join(cwd, "src"));
  const polled = host.wait(id);
  await until(() => healed.closed);
  await settle();
  expect(await polled).toMatchObject({ dirs: ["src"] });
  expect(f.at(join(cwd, "src"))).not.toBe(healed);
  host.close();
});

it("re-watches a wanted directory that was removed and recreated, without a new update", async () => {
  const cwd = repo();
  mkdirSync(join(cwd, "dist"));
  const f = fakes();
  const host = new WorkspaceWatchHost(f.watchFn);
  const id = await host.start(cwd, [dir(cwd, "dist")]);
  const first = f.at(join(cwd, "dist"));
  rmSync(join(cwd, "dist"), { recursive: true });
  first.emit("rename", "dist");
  await until(() => first.closed);
  mkdirSync(join(cwd, "dist"));
  await vi.advanceTimersByTimeAsync(HEAL_INTERVAL_MS);
  const pending = host.wait(id);
  await until(() => f.open().some((fake) => fake.path === join(cwd, "dist")));
  await settle();
  // The earlier drop reported overflow; the restored watch reports the dir.
  expect(await pending).toMatchObject({ dirs: ["dist"] });
  host.close();
});

it("recovers a wanted directory after ENOSPC, silently", async () => {
  const cwd = repo();
  const f = fakes();
  const host = new WorkspaceWatchHost(f.watchFn);
  const id = await host.start(cwd, [dir(cwd, "src")]);
  const src = () => f.open().some((fake) => fake.path === join(cwd, "src"));
  f.at(join(cwd, "src")).fail(new Error("EMFILE"));
  f.failNext(Object.assign(new Error("ENOSPC"), { code: "ENOSPC" }));
  // The drop reports overflow; the first retry hits ENOSPC and adds no more noise.
  const first = host.wait(id);
  await real(100);
  await settle();
  expect(await first).toEqual({ git: false, dirs: [], overflow: true });
  expect(src()).toBe(false);
  await vi.advanceTimersByTimeAsync(HEAL_INTERVAL_MS);
  const second = host.wait(id);
  await until(src);
  await settle();
  expect(await second).toEqual({ git: false, dirs: ["src"], overflow: false });
  host.close();
});

it("never re-creates a watch through a swapped symlink", async () => {
  const cwd = repo();
  const secret = tmp();
  const f = fakes();
  const host = new WorkspaceWatchHost(f.watchFn);
  const id = await host.start(cwd, [dir(cwd, "src")]);
  const first = f.at(join(cwd, "src"));
  rmSync(join(cwd, "src"), { recursive: true });
  symlinkSync(secret, join(cwd, "src"));
  first.emit("rename", null);
  await until(() => first.closed);
  await vi.advanceTimersByTimeAsync(HEAL_INTERVAL_MS);
  const pending = host.wait(id);
  await settle();
  expect(await pending).toMatchObject({ overflow: true });
  await vi.advanceTimersByTimeAsync(HEAL_INTERVAL_MS);
  void host.wait(id);
  await real(100);
  for (const fake of f.all) {
    expect(fake.path).not.toBe(secret);
    expect(fake.path.startsWith(secret)).toBe(false);
  }
  expect(f.all.filter((fake) => fake.path === join(cwd, "src"))).toHaveLength(1);
  host.close();
});

it("re-creates a git watch after its handle errored", async () => {
  const cwd = repo();
  const f = fakes();
  const host = new WorkspaceWatchHost(f.watchFn);
  const id = await host.start(cwd, []);
  const git = f.at(join(cwd, ".git"));
  git.fail(new Error("EMFILE"));
  expect(git.closed).toBe(true);
  await settle();
  const first = host.wait(id);
  await until(() => f.open().some((fake) => fake.path === join(cwd, ".git")));
  await settle();
  expect(await first).toMatchObject({ overflow: true, git: true });
  const again = host.wait(id);
  f.at(join(cwd, ".git")).emit("change", "HEAD");
  await settle();
  expect((await again).git).toBe(true);
  host.close();
});

it("releases vanished ref dirs before counting toward the cap", async () => {
  const cwd = repo();
  const heads = join(cwd, ".git", "refs", "heads");
  for (let i = 0; i < 60; i++) mkdirSync(join(heads, `old${i}`), { recursive: true });
  const f = fakes();
  const host = new WorkspaceWatchHost(f.watchFn);
  const id = await host.start(cwd, []);
  await settle();
  for (let i = 0; i < 60; i++) rmSync(join(heads, `old${i}`), { recursive: true });
  for (let i = 0; i < 40; i++) mkdirSync(join(heads, `new${i}`), { recursive: true });
  const pending = host.wait(id);
  f.at(heads).emit("rename", "x");
  await settle();
  const news = () => f.open().filter((fake) => fake.path.includes("new")).length;
  await until(() => news() === 40);
  await settle();
  // 40 new dirs fit only because the 60 stale ones were released first.
  expect(news()).toBe(40);
  expect(await pending).toMatchObject({ overflow: false });
  host.close();
});

it("rate limits healing passes", async () => {
  const cwd = repo();
  const f = fakes();
  const host = new WorkspaceWatchHost(f.watchFn);
  const id = await host.start(cwd, [dir(cwd, "src")]);
  const lose = () => f.at(join(cwd, "src")).fail(new Error("EMFILE"));
  const count = () => f.all.filter((fake) => fake.path === join(cwd, "src")).length;
  lose();
  void host.wait(id); // first pass re-watches src
  await until(() => count() === 2);
  lose();
  void host.wait(id);
  void host.wait(id);
  await real(100);
  expect(count()).toBe(2); // still cooling down
  await vi.advanceTimersByTimeAsync(HEAL_INTERVAL_MS);
  void host.wait(id);
  await until(() => count() === 3);
  host.close();
});

it("reports a persistent ref watch failure once, not on every heal cycle", async () => {
  const cwd = repo();
  const feature = join(cwd, ".git", "refs", "heads", "feature");
  mkdirSync(feature);
  const f = fakes();
  f.failPath(feature);
  const host = new WorkspaceWatchHost(f.watchFn);
  const id = await host.start(cwd, []);
  await settle();
  let overflows = 0;
  for (let i = 0; i < 6; i++) {
    const pending = host.wait(id);
    await real(80);
    await vi.advanceTimersByTimeAsync(WAIT_MS);
    if ((await pending).overflow) overflows++;
  }
  expect(overflows).toBe(1);
  // Once the failure clears, the dir is watched and a later failure is a new episode.
  f.failPath(feature, false);
  const pending = host.wait(id);
  await real(80);
  await vi.advanceTimersByTimeAsync(WAIT_MS);
  await pending;
  await until(() => f.open().some((fake) => fake.path === feature));
  host.close();
});

function hangingOps() {
  const calls = { realpath: 0 };
  let hung: ((value: string) => void) | undefined;
  let hang: ((error: Error) => void) | undefined;
  let hangNext = false;
  const ops: FsOps = {
    realpath: (path) => {
      calls.realpath++;
      if (hangNext) {
        hangNext = false;
        return new Promise<string>((resolve, reject) => {
          hung = resolve;
          hang = reject;
        });
      }
      return realpath(path);
    },
    stat: (path) => stat(path),
    readdir: (path) => readdir(path, { withFileTypes: true }),
  };
  return {
    ops,
    calls,
    hangNext: () => {
      hangNext = true;
    },
    settleHung: (value?: string) =>
      value === undefined ? hang?.(Object.assign(new Error("ENOENT"), { code: "ENOENT" })) : hung?.(value),
  };
}

it("abandons a hung file system call and lets a stale pass change nothing", async () => {
  const cwd = repo();
  const f = fakes();
  const h = hangingOps();
  const host = new WorkspaceWatchHost(f.watchFn, h.ops);
  const id = await host.start(cwd, [dir(cwd, "src")]);
  const src = f.at(join(cwd, "src"));
  h.hangNext();
  src.emit("rename", null); // pass 1 hangs in realpath
  await until(() => h.calls.realpath === 1);
  await vi.advanceTimersByTimeAsync(HEAL_TIMEOUT_MS);
  // The next request runs a new pass instead of being wedged behind the hung one.
  void host.wait(id);
  await until(() => h.calls.realpath === 2);
  // The hung call finally fails: it must not drop the live watch.
  h.settleHung();
  await real(80);
  expect(src.closed).toBe(false);
  expect(await Promise.race([host.wait(id), Promise.resolve("pending")])).toBe("pending");
  host.close();
});

it("does not add watches from a pass that is still running when the watcher closes", async () => {
  const cwd = repo();
  const f = fakes();
  const h = hangingOps();
  const host = new WorkspaceWatchHost(f.watchFn, h.ops);
  const id = await host.start(cwd, [dir(cwd, "src")]);
  f.at(join(cwd, "src")).fail(new Error("EMFILE"));
  h.hangNext();
  void host.wait(id);
  await until(() => h.calls.realpath === 1);
  const before = f.all.length;
  host.close();
  h.settleHung(join(cwd, "src"));
  await real(80);
  expect(f.all.length).toBe(before);
  expect(f.open()).toHaveLength(0);
});

it("refuses a watched directory that is a link by the time it is adopted", async () => {
  const cwd = repo();
  const secret = tmp();
  const f = fakes();
  const host = new WorkspaceWatchHost(f.watchFn);
  const id = await host.start(cwd, []);
  rmSync(join(cwd, "src"), { recursive: true });
  symlinkSync(secret, join(cwd, "src"));
  // The caller validated "<cwd>/src" as a real directory before the swap.
  host.update(id, [dir(cwd, "src"), dir(cwd, "other")]);
  expect(f.all.some((fake) => fake.path === join(cwd, "src"))).toBe(false);
  expect(f.all.some((fake) => fake.path === secret)).toBe(false);
  expect(f.at(join(cwd, "other"))).toBeDefined();
  await settle();
  expect(await host.wait(id)).toMatchObject({ overflow: true });
  // Healing never adopts it either.
  await vi.advanceTimersByTimeAsync(HEAL_INTERVAL_MS);
  void host.wait(id);
  await real(100);
  expect(f.all.some((fake) => fake.path === join(cwd, "src"))).toBe(false);
  host.close();
});

it("reports ref overflow once per episode and immediately", async () => {
  const cwd = repo();
  const heads = join(cwd, ".git", "refs", "heads");
  for (let i = 0; i < 70; i++) mkdirSync(join(heads, `b${i}`), { recursive: true });
  mkdirSync(join(heads, "d", "e", "f", "g", "h"), { recursive: true });
  const f = fakes();
  const host = new WorkspaceWatchHost(f.watchFn);
  const id = await host.start(cwd, []);
  await settle();
  expect(await host.wait(id)).toMatchObject({ overflow: true });
  // Later ref events resync, but the same degraded state is not reported again.
  const pending = host.wait(id);
  f.at(heads).emit("rename", "x");
  await settle();
  await settle();
  expect(await pending).toEqual({ git: true, dirs: [], overflow: false });
  host.close();
});

it("reports overflow when the git directory cannot be found", async () => {
  const cwd = tmp();
  const f = fakes();
  const host = new WorkspaceWatchHost(f.watchFn);
  const id = await host.start(cwd, []);
  await settle();
  expect(await host.wait(id)).toMatchObject({ overflow: true });
  host.close();
});

it("releases every handle and timer on unwatch and close", async () => {
  const cwd = repo();
  const f = fakes();
  const host = new WorkspaceWatchHost(f.watchFn);
  const a = await host.start(cwd, [dir(cwd, "src")]);
  await host.start(cwd, [dir(cwd, "other")]);
  const pending = host.wait(a);
  host.stop(a);
  expect(await pending).toEqual({ git: false, dirs: [], overflow: false });
  expect(() => host.wait(a)).toThrow(WATCH_UNKNOWN);
  host.close();
  expect(host.size).toBe(0);
  expect(host.openHandles).toBe(0);
  expect(f.open()).toHaveLength(0);
  expect(vi.getTimerCount()).toBe(0);
});

it("sees real file system changes with real fs.watch", async () => {
  vi.useRealTimers();
  const cwd = repo();
  const host = new WorkspaceWatchHost();
  const id = await host.start(cwd, [dir(cwd, "src")]);
  // fs.watch registers its native stream asynchronously; a write made right
  // away can be missed, so let the watches settle first.
  const settle = () => new Promise((resolve) => setTimeout(resolve, 300));
  await settle();
  // FSEvents on macOS can replay events from just before the watch started, so
  // collect responses until the expected change has arrived.
  const collect = async (done: (seen: { git: boolean; dirs: Set<string> }) => boolean) => {
    const seen = { git: false, dirs: new Set<string>() };
    for (let i = 0; i < 5 && !done(seen); i++) {
      const changes = await host.wait(id);
      seen.git ||= changes.git;
      changes.dirs.forEach((value) => seen.dirs.add(value));
    }
    return seen;
  };
  try {
    const first = collect((seen) => seen.dirs.has("src"));
    writeFileSync(join(cwd, "src", "a.ts"), "x");
    expect((await first).dirs.has("src")).toBe(true);

    await settle();
    writeFileSync(join(cwd, "other", "b.ts"), "x");
    execFileSync("git", ["checkout", "-q", "-b", "topic"], { cwd });
    const second = await collect((seen) => seen.git);
    expect(second.git).toBe(true);
    expect(second.dirs.has("other")).toBe(false);
  } finally {
    host.close();
  }
  expect(host.openHandles).toBe(0);
});
