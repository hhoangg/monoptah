import { createHash } from "node:crypto";
import {
  chmodSync,
  existsSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  realpathSync,
  rmSync,
  statSync,
  symlinkSync,
  utimesSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, expect, it, vi } from "vitest";
import { HostStore } from "./store";
import { WorkspaceCommands } from "./workspace-commands";

const hooks = vi.hoisted(() => ({
  rename: undefined as undefined | (() => Promise<void>),
  afterOpen: undefined as undefined | (() => void),
}));
vi.mock("node:fs/promises", async (original) => {
  const actual = await original<typeof import("node:fs/promises")>();
  return {
    ...actual,
    rename: (...args: Parameters<typeof actual.rename>) =>
      hooks.rename ? hooks.rename() : actual.rename(...args),
    open: async (...args: Parameters<typeof actual.open>) => {
      const handle = await actual.open(...args);
      hooks.afterOpen?.();
      return handle;
    },
  };
});

const cleanups: Array<() => void> = [];
afterEach(() => {
  vi.restoreAllMocks();
  hooks.rename = undefined;
  hooks.afterOpen = undefined;
  cleanups.splice(0).reverse().forEach((cleanup) => cleanup());
});

const sha = (text: string) => createHash("sha256").update(text).digest("hex");

function fixture() {
  const home = realpathSync.native(mkdtempSync(join(tmpdir(), "monoptah-write-")));
  const store = new HostStore(join(home, "host.db"));
  cleanups.push(() => {
    store.close();
    rmSync(home, { recursive: true, force: true });
  });
  const commands = new WorkspaceCommands(store, async (_id, action) => action());
  const project = join(home, "project");
  mkdirSync(project);
  store.addProject(project, "project");
  commands.invalidateRoots();
  const write = (path: string, content: string, expectedSha256?: string) =>
    commands.run("write_text_file", { path, content, expectedSha256 });
  return { home, project, write, store };
}

const posix = process.platform !== "win32";

it("writes when the hash matches and leaves no temp file", async () => {
  const { project, write } = fixture();
  const file = join(project, "a.txt");
  writeFileSync(file, "before\n");
  await write(file, "after\n", sha("before\n"));
  expect(readFileSync(file, "utf8")).toBe("after\n");
  expect(readdirSync(project)).toEqual(["a.txt"]);
});

it("fails with File changed on the host on a hash mismatch and keeps the file", async () => {
  const { project, write } = fixture();
  const file = join(project, "a.txt");
  writeFileSync(file, "someone else\n");
  await expect(write(file, "mine\n", sha("before\n"))).rejects.toThrow(
    "File changed on the host",
  );
  expect(readFileSync(file, "utf8")).toBe("someone else\n");
  expect(readdirSync(project)).toEqual(["a.txt"]);
});

it("fails when the file no longer exists", async () => {
  const { project, write } = fixture();
  await expect(write(join(project, "gone.txt"), "x", sha("x"))).rejects.toThrow(
    "File changed on the host",
  );
});

it("rejects a malformed expected hash", async () => {
  const { project, write } = fixture();
  const file = join(project, "a.txt");
  writeFileSync(file, "x");
  await expect(write(file, "y", "nope")).rejects.toThrow("Invalid expected file hash");
});

it.runIf(posix)("preserves the file mode even when the umask would strip it", async () => {
  const { project, write } = fixture();
  const file = join(project, "run.sh");
  writeFileSync(file, "#!/bin/sh\n");
  chmodSync(file, 0o666);
  const before = process.umask(0o077);
  try {
    await write(file, "#!/bin/sh\necho hi\n", sha("#!/bin/sh\n"));
  } finally {
    process.umask(before);
  }
  expect(statSync(file).mode & 0o7777).toBe(0o666);
  expect(readFileSync(file, "utf8")).toBe("#!/bin/sh\necho hi\n");
});

it("treats a retry of a write that already landed as success", async () => {
  const { project, write } = fixture();
  const file = join(project, "a.txt");
  writeFileSync(file, "before\n");
  await write(file, "after\n", sha("before\n"));
  await write(file, "after\n", sha("before\n"));
  expect(readFileSync(file, "utf8")).toBe("after\n");
  expect(readdirSync(project)).toEqual(["a.txt"]);
});

it("treats the new content appearing during the write as success", async () => {
  const { project, write } = fixture();
  const file = join(project, "a.txt");
  writeFileSync(file, "before\n");
  hooks.afterOpen = () => writeFileSync(file, "after\n");
  await write(file, "after\n", sha("before\n"));
  expect(readFileSync(file, "utf8")).toBe("after\n");
  expect(readdirSync(project)).toEqual(["a.txt"]);
});

it("lets only one of two interleaved writes with the same hash succeed", async () => {
  const { project, write } = fixture();
  const file = join(project, "a.txt");
  writeFileSync(file, "base\n");
  const results = await Promise.allSettled([
    write(file, "one\n", sha("base\n")),
    write(file, "two\n", sha("base\n")),
  ]);
  expect(results.map((r) => r.status).sort()).toEqual(["fulfilled", "rejected"]);
  const rejected = results.find((r) => r.status === "rejected") as PromiseRejectedResult;
  expect(String(rejected.reason)).toContain("File changed on the host");
  expect(["one\n", "two\n"]).toContain(readFileSync(file, "utf8"));
  expect(readdirSync(project)).toEqual(["a.txt"]);
});

it("sweeps stale temp files of the same name and keeps fresh and unrelated ones", async () => {
  const { project, write } = fixture();
  const file = join(project, "a.txt");
  writeFileSync(file, "x");
  const stale = join(project, ".a.txt.monoptah-1-aa.tmp");
  const fresh = join(project, ".a.txt.monoptah-2-bb.tmp");
  const other = join(project, ".b.txt.monoptah-1-cc.tmp");
  for (const f of [stale, fresh, other]) writeFileSync(f, "t");
  const old = new Date(Date.now() - 2 * 60 * 60 * 1000);
  utimesSync(stale, old, old);
  utimesSync(other, old, old);
  await write(file, "y", sha("x"));
  expect(existsSync(stale)).toBe(false);
  expect(existsSync(fresh)).toBe(true);
  expect(existsSync(other)).toBe(true);
});

it.runIf(posix)("writes through a symlink whose target is inside the workspace", async () => {
  const { project, write } = fixture();
  mkdirSync(join(project, "real"));
  const target = join(project, "real", "t.txt");
  const link = join(project, "link.txt");
  writeFileSync(target, "old\n");
  symlinkSync(target, link);
  await write(link, "new\n", sha("old\n"));
  expect(lstatSync(link).isSymbolicLink()).toBe(true);
  expect(readFileSync(target, "utf8")).toBe("new\n");
  expect(readdirSync(join(project, "real"))).toEqual(["t.txt"]);
});

it.runIf(posix)("refuses a symlink that leaves the workspace", async () => {
  const { home, project, write } = fixture();
  const outside = join(home, "outside.txt");
  writeFileSync(outside, "secret\n");
  const link = join(project, "escape.txt");
  symlinkSync(outside, link);
  await expect(write(link, "x", sha("secret\n"))).rejects.toThrow();
  expect(readFileSync(outside, "utf8")).toBe("secret\n");
  expect(lstatSync(link).isSymbolicLink()).toBe(true);
});

it.runIf(posix)("refuses a symlink into another project", async () => {
  const { home, project, write, store } = fixture();
  const other = join(home, "other");
  mkdirSync(other);
  store.addProject(other, "other");
  writeFileSync(join(other, "f.txt"), "o\n");
  const link = join(project, "cross.txt");
  symlinkSync(join(other, "f.txt"), link);
  await expect(write(link, "x", sha("o\n"))).rejects.toThrow(
    "Path is outside the workspace",
  );
  expect(readFileSync(join(other, "f.txt"), "utf8")).toBe("o\n");
});

it.runIf(posix)("refuses a linked directory that leads into another project", async () => {
  const { home, project, write, store } = fixture();
  const other = join(home, "other");
  mkdirSync(join(other, "dir"), { recursive: true });
  store.addProject(other, "other");
  writeFileSync(join(other, "dir", "f.txt"), "o\n");
  symlinkSync(join(other, "dir"), join(project, "shared"));
  await expect(
    write(join(project, "shared", "f.txt"), "x", sha("o\n")),
  ).rejects.toThrow("Path is outside the workspace");
  expect(readFileSync(join(other, "dir", "f.txt"), "utf8")).toBe("o\n");
});

it("refuses content over 1 MB", async () => {
  const { project, write } = fixture();
  const file = join(project, "a.txt");
  writeFileSync(file, "x");
  await expect(write(file, "a".repeat(1024 * 1024 + 1), sha("x"))).rejects.toThrow(
    "too large",
  );
  expect(readFileSync(file, "utf8")).toBe("x");
});

it("removes the temp file and keeps the target when rename fails", async () => {
  const { project, write } = fixture();
  const file = join(project, "a.txt");
  writeFileSync(file, "keep\n");
  hooks.rename = () => Promise.reject(new Error("rename failed"));
  await expect(write(file, "new\n", sha("keep\n"))).rejects.toThrow("rename failed");
  expect(readFileSync(file, "utf8")).toBe("keep\n");
  expect(readdirSync(project)).toEqual(["a.txt"]);
});

it("removes the temp file when the file changes during the write", async () => {
  const { project, write } = fixture();
  const file = join(project, "a.txt");
  writeFileSync(file, "keep\n");
  hooks.afterOpen = () => writeFileSync(file, "raced\n");
  await expect(write(file, "new\n", sha("keep\n"))).rejects.toThrow(
    "File changed on the host",
  );
  expect(readFileSync(file, "utf8")).toBe("raced\n");
  expect(readdirSync(project)).toEqual(["a.txt"]);
});

it("leaves the old behaviour without a hash", async () => {
  const { project, write } = fixture();
  const file = join(project, "a.txt");
  writeFileSync(file, "x");
  await write(file, "y");
  expect(readFileSync(file, "utf8")).toBe("y");
  await write(join(project, "new.txt"), "z");
  expect(readFileSync(join(project, "new.txt"), "utf8")).toBe("z");
  await expect(write(file, "a".repeat(1024 * 1024 + 1))).rejects.toThrow("too large");
});
