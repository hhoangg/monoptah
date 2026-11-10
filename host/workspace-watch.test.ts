import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import {
  BATCH_MS,
  LEASE_MS,
  MAX_DIRS,
  MAX_WATCHERS,
  WAIT_MS,
  WATCH_UNKNOWN,
  WorkspaceWatchHost,
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
  const watchFn: WatchFn = (path, options, listener) => {
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
  src.emit("rename", "src");
  await vi.waitFor(() => expect(src.closed).toBe(true));
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
