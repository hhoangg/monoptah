import { execFile } from "node:child_process";
import { open, stat } from "node:fs/promises";
import { join } from "node:path";
import { promisify } from "node:util";
import type { GitBlame, GitBlameHunk } from "../src/platform/tauri/fs";

const exec = promisify(execFile);

// `<sha> <original line> <final line> [<group size>]`
const HEADER = /^([0-9a-f]{40}|[0-9a-f]{64}) \d+ (\d+)(?: \d+)?$/i;

/** Parse `git blame --porcelain`. Commit metadata is printed only the first
 * time a sha appears, so it is cached by sha; consecutive lines from the same
 * commit are merged into one hunk. */
export function parseGitBlamePorcelain(text: string): GitBlameHunk[] {
  const metas = new Map<string, { author: string; authorTime: number; summary: string }>();
  const lines: Array<[string, number]> = [];
  let current: { author: string; authorTime: number; summary: string } | undefined;
  for (const line of text.split(/\r?\n/)) {
    // File content is prefixed with a tab and may look like anything.
    if (line.startsWith("\t")) continue;
    const header = HEADER.exec(line);
    if (header) {
      const sha = header[1].toLowerCase();
      let meta = metas.get(sha);
      if (!meta) metas.set(sha, (meta = { author: "", authorTime: 0, summary: "" }));
      lines.push([sha, Number(header[2])]);
      current = meta;
    } else if (current) {
      if (line.startsWith("author ")) current.author = line.slice(7);
      else if (line.startsWith("author-time ")) current.authorTime = Number(line.slice(12)) || 0;
      else if (line.startsWith("summary ")) current.summary = line.slice(8);
    }
  }
  lines.sort((a, b) => a[1] - b[1]);
  const hunks: GitBlameHunk[] = [];
  for (const [sha, line] of lines) {
    const last = hunks[hunks.length - 1];
    if (last && last.sha === sha && last.startLine + last.lineCount === line) {
      last.lineCount += 1;
      continue;
    }
    const meta = metas.get(sha)!;
    hunks.push({
      sha,
      shortSha: sha.slice(0, 7),
      author: meta.author,
      authorTime: meta.authorTime,
      summary: meta.summary,
      uncommitted: /^0+$/.test(sha),
      startLine: line,
      lineCount: 1,
    });
  }
  return hunks;
}

/** True when the file is small enough and has no NUL byte in its first 8 KiB.
 * A file that cannot be inspected is left to git, which reports why. */
async function blameCandidate(path: string, maxBytes: number): Promise<boolean> {
  try {
    if ((await stat(path)).size > maxBytes) return false;
    const file = await open(path, "r");
    try {
      const head = Buffer.alloc(8192);
      const { bytesRead } = await file.read(head, 0, head.length, 0);
      return !head.subarray(0, bytesRead).includes(0);
    } finally {
      await file.close();
    }
  } catch {
    return true;
  }
}

/** Blame the working-tree version of `relative` (a clean path inside `root`).
 * Untracked, binary and oversized files give no hunks. */
export async function hostGitBlame(
  root: string,
  relative: string,
  maxBytes: number,
): Promise<GitBlame> {
  const run = (args: string[]) =>
    exec("git", ["-c", "core.pager=cat", ...args], {
      cwd: root,
      timeout: 30_000,
      maxBuffer: 64 * 1024 * 1024,
      encoding: "utf8",
      // The untracked check below reads git's English error text.
      env: { ...process.env, LC_ALL: "C", GIT_TERMINAL_PROMPT: "0", GIT_OPTIONAL_LOCKS: "0" },
    });
  const inside = await run(["rev-parse", "--is-inside-work-tree"]).then(
    ({ stdout }) => stdout.trim() === "true",
    () => false,
  );
  if (!inside) throw new Error("Not a git repository");
  if (!(await blameCandidate(join(root, relative), maxBytes))) return { hunks: [] };
  try {
    const { stdout } = await run(["blame", "--porcelain", "--", relative]);
    return { hunks: parseGitBlamePorcelain(stdout) };
  } catch (reason) {
    const stderr = String((reason as { stderr?: unknown }).stderr ?? "").trim();
    // A file git has never seen (untracked, or any file before the first
    // commit) has nothing to blame.
    if (/no such path|no such ref: HEAD/.test(stderr)) return { hunks: [] };
    throw new Error(stderr || "git blame failed");
  }
}
