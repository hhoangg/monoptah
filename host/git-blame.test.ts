import { execFileSync } from "node:child_process";
import { mkdtempSync, rmSync, unlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, expect, it, vi } from "vitest";
import { hostGitBlame, parseGitBlamePorcelain } from "./git-blame";

const A = "a".repeat(40);
const B = "b".repeat(40);
const ZERO = "0".repeat(40);

const dirs: string[] = [];
afterEach(() => {
  vi.unstubAllEnvs();
  for (const dir of dirs.splice(0)) rmSync(dir, { recursive: true, force: true });
});

function repo(initial = true) {
  const cwd = mkdtempSync(join(tmpdir(), "monocode-host-blame-"));
  dirs.push(cwd);
  const git = (...args: string[]) =>
    execFileSync("git", args, { cwd, encoding: "utf8" }).trim();
  git("init", "-q", "-b", "main");
  git("config", "user.name", "Test");
  git("config", "user.email", "test@example.com");
  git("config", "commit.gpgsign", "false");
  git("config", "core.autocrlf", "false");
  const write = (file: string, content: string | Buffer) =>
    writeFileSync(join(cwd, file), content);
  if (initial) {
    write("a.txt", "one\ntwo\nthree\n");
    git("add", ".");
    git("commit", "-q", "-m", "init");
  }
  return { cwd, git, write };
}

const SAMPLE = [
  `${A} 1 1 2`,
  "author Alice",
  "author-mail <alice@example.com>",
  "author-time 1700000000",
  "summary first commit",
  "filename f.txt",
  "\tone",
  `${A} 2 2`,
  `\t${B} 9 9 9`,
  `${B} 7 3 2`,
  "author Bob",
  "author-time 1700000500",
  "summary second: fix",
  "filename f.txt",
  "\tauthor Mallory",
  `${B} 8 4`,
  "\tsummary nope",
  `${A} 5 5 1`,
  "\tfive",
  `${ZERO} 6 6 1`,
  "author Not Committed Yet",
  "author-time 1700001000",
  "summary Version of f.txt from f.txt",
  "filename f.txt",
  "\tsix",
  "",
].join("\n");

it("parses porcelain output, reusing metadata and merging adjacent groups", () => {
  const hunks = parseGitBlamePorcelain(SAMPLE);
  expect(hunks.map((h) => [h.sha, h.startLine, h.lineCount])).toEqual([
    [A, 1, 2],
    [B, 3, 2],
    [A, 5, 1],
    [ZERO, 6, 1],
  ]);
  expect(hunks[0]).toMatchObject({
    author: "Alice",
    authorTime: 1_700_000_000,
    summary: "first commit",
    shortSha: "aaaaaaa",
    uncommitted: false,
  });
  expect(hunks[1]).toMatchObject({ author: "Bob", summary: "second: fix" });
  // Metadata was only printed with A's first line; later groups reuse it.
  expect(hunks[2]).toMatchObject({ author: "Alice", authorTime: 1_700_000_000 });
  expect(hunks[3]).toMatchObject({ uncommitted: true, shortSha: "0000000" });
  expect(parseGitBlamePorcelain("")).toEqual([]);
});

it("strips carriage returns from porcelain metadata lines", () => {
  const text = `${A} 1 1 1\r\nauthor Alice\r\nauthor-time 5\r\nsummary crlf subject\r\nfilename f.txt\r\n\tone\r\n`;
  expect(parseGitBlamePorcelain(text)).toMatchObject([
    { author: "Alice", authorTime: 5, summary: "crlf subject" },
  ]);
});

it("blames commits and uncommitted edits in the working tree", async () => {
  const { cwd, git, write } = repo();
  write("a.txt", "one\ntwo\nthree\nfour\n");
  git("commit", "-q", "-am", "add four");
  write("a.txt", "one\nTWO\nthree\nfour\n");
  const second = git("rev-parse", "HEAD");
  const first = git("rev-parse", "HEAD~1");

  const { hunks } = await hostGitBlame(cwd, "a.txt", 1024 * 1024);
  expect(hunks.map((h) => [h.sha, h.startLine, h.lineCount, h.uncommitted])).toEqual([
    [first, 1, 1, false],
    [ZERO, 2, 1, true],
    [first, 3, 1, false],
    [second, 4, 1, false],
  ]);
  expect(hunks[0]).toMatchObject({ author: "Test", summary: "init" });
  expect(hunks[3].summary).toBe("add four");
});

it("returns no hunks for untracked, binary and oversized files", async () => {
  const { cwd, write } = repo();
  write("new.txt", "fresh\n");
  expect(await hostGitBlame(cwd, "new.txt", 1024)).toEqual({ hunks: [] });
  write("big.txt", "x".repeat(2048));
  expect(await hostGitBlame(cwd, "big.txt", 1024)).toEqual({ hunks: [] });
  write("bin.dat", Buffer.from([97, 0, 98, 10]));
  expect(await hostGitBlame(cwd, "bin.dat", 1024)).toEqual({ hunks: [] });
  const unborn = repo(false);
  unborn.write("a.txt", "one\n");
  expect(await hostGitBlame(unborn.cwd, "a.txt", 1024)).toEqual({ hunks: [] });
});

it("treats untracked files as empty under a non-English locale", async () => {
  vi.stubEnv("LC_ALL", "de_DE.UTF-8");
  vi.stubEnv("LANGUAGE", "de");
  const { cwd, write } = repo();
  write("new.txt", "fresh\n");
  expect(await hostGitBlame(cwd, "new.txt", 1024)).toEqual({ hunks: [] });
});

it("rejects a directory that is not a git work tree and surfaces git errors", async () => {
  const plain = mkdtempSync(join(tmpdir(), "monocode-host-blame-plain-"));
  dirs.push(plain);
  await expect(hostGitBlame(plain, "a.txt", 1024)).rejects.toThrow("Not a git repository");
  const { cwd } = repo();
  unlinkSync(join(cwd, "a.txt"));
  await expect(hostGitBlame(cwd, "a.txt", 1024)).rejects.toThrow(/Cannot lstat/);
});
