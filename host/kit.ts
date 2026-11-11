import { createHash, randomBytes } from "node:crypto";
import {
  chmodSync,
  closeSync,
  existsSync,
  fsyncSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  openSync,
  readFileSync,
  readSync,
  readdirSync,
  readlinkSync,
  renameSync,
  rmSync,
  statSync,
  symlinkSync,
  unlinkSync,
  writeFileSync,
  writeSync,
} from "node:fs";
import { execFile, spawn } from "node:child_process";
import { homedir, tmpdir } from "node:os";
import { delimiter, join, resolve, sep } from "node:path";
import { inspect } from "node:util";
import {
  claudeKitKey,
  type ClaudeKit,
} from "../src/integrations/harness/providers/claude/claudeKit";
import { providerLaunch } from "./process";

/**
 * The Claude kit on the Host: skills, plugins and MCP servers copied once from
 * the user's machine. See docs/cloud-session-improvements.md section 3.2.3.
 *
 * Everything lives under `~/.monoptah-kit`:
 *   <hash>/        one unpacked kit, created fresh with mkdir (O_EXCL semantics)
 *   current        symlink to the active <hash>, flipped atomically
 *   managed.json   skill links this module created in ~/.claude/skills
 *   secrets.env    kit secrets, mode 0600, loaded into the Claude child env only
 */

const HASH = /^[0-9a-f]{64}$/;
const SKILL_NAME = /^[a-z0-9]+(?:-[a-z0-9]+)*$/;
const PLUGIN_ID = /^[A-Za-z0-9@][A-Za-z0-9._@-]{0,127}$/;
const SERVER_NAME = /^[A-Za-z0-9._@-]{1,128}$/;
const SECRET_NAME = /^MONOPTAH_KIT_[A-Z0-9_]{1,128}$/;
const COMMAND_NAME = /^[A-Za-z0-9._+-]{1,64}$/;
const BLOCK = 512;
const COMPLETE = ".complete";
const TOP_LEVEL = new Set(["skills", "plugins", "mcp.json", "kit.json"]);
/** Values shorter than this are not scrubbed: they would mangle ordinary text. */
const MIN_SCRUB_LENGTH = 4;
const MAX_SECRET_VALUE_BYTES = 64 * 1024;
const MAX_SECRET_ENTRIES = 256;
const MAX_SECRETS_TOTAL_BYTES = 1024 * 1024;
const MAX_JSON_BYTES = 1024 * 1024;
const HEALTH_TIMEOUT_MS = 60_000;
const HEALTH_MAX_BUFFER = 1024 * 1024;

export interface KitLimits {
  maxEntries: number;
  /** Sum of every regular file's size. */
  maxTotalBytes: number;
  maxFileBytes: number;
  maxPathBytes: number;
  maxSegmentBytes: number;
  maxDepth: number;
}

export const DEFAULT_KIT_LIMITS: KitLimits = {
  maxEntries: 50_000,
  maxTotalBytes: 128 * 1024 * 1024,
  maxFileBytes: 64 * 1024 * 1024,
  maxPathBytes: 512,
  maxSegmentBytes: 255,
  maxDepth: 24,
};

// ---------------------------------------------------------------------------
// Secret scrubbing for Host logs and error text.

let secretValues: string[] = [];

/** Shorter forms would mangle ordinary text, so line and URL forms need more. */
const MIN_DERIVED_SCRUB_LENGTH = 6;
const MIN_LINE_SCRUB_LENGTH = 8;
const MAX_LINE_FORMS = 256;

/** True for a line of a multi-line secret that is worth scrubbing on its own. */
function secretLike(line: string): boolean {
  if (line.length < MIN_LINE_SCRUB_LENGTH) return false;
  // Armor lines (-----BEGIN ...-----) are the same for every key.
  if (line.startsWith("-----")) return false;
  if (!/[A-Za-z0-9]/.test(line)) return false;
  // Short lines with spaces are structure or prose (`"type": "service_account",`).
  if (/\s/.test(line) && line.length < 40) return false;
  return true;
}

function setScrubValues(values: Iterable<string>): void {
  const forms = new Set<string>();
  const add = (form: string, min: number) => {
    if (form.length >= min) forms.add(form);
  };
  for (const value of values) {
    add(value, MIN_SCRUB_LENGTH);
    // The JSON-escaped form is how an object dump would show it.
    add(JSON.stringify(value).slice(1, -1), MIN_SCRUB_LENGTH);
    add(encodeURIComponent(value), MIN_DERIVED_SCRUB_LENGTH);
    if (/[\r\n]/.test(value)) {
      // Output is read line by line, so each line of a multi-line value
      // (a PEM key, for example) is scrubbed on its own as well, within a cap.
      add(value.replace(/\r\n/g, "\n"), MIN_SCRUB_LENGTH);
      let lineForms = 0;
      for (const line of value.split(/\r?\n/)) {
        const trimmed = line.trim();
        if (lineForms >= MAX_LINE_FORMS) break;
        if (!secretLike(trimmed) || forms.has(trimmed)) continue;
        forms.add(trimmed);
        lineForms++;
      }
    }
  }
  automaton = buildAutomaton([...forms]);
  secretValues = [...forms];
}

// Aho-Corasick over UTF-16 code units: one pass, O(length) per text.
interface Automaton {
  next: Array<Map<number, number>>;
  fail: Int32Array;
  longest: Int32Array;
}

function buildAutomaton(patterns: string[]): Automaton | undefined {
  if (patterns.length === 0) return undefined;
  const next: Array<Map<number, number>> = [new Map()];
  const longest: number[] = [0];
  for (const pattern of patterns) {
    let node = 0;
    for (let index = 0; index < pattern.length; index++) {
      const code = pattern.charCodeAt(index);
      let child = next[node].get(code);
      if (child === undefined) {
        child = next.length;
        next.push(new Map());
        longest.push(0);
        next[node].set(code, child);
      }
      node = child;
    }
    longest[node] = Math.max(longest[node], pattern.length);
  }
  const fail = new Int32Array(next.length);
  const queue: number[] = [];
  for (const child of next[0].values()) queue.push(child);
  for (let head = 0; head < queue.length; head++) {
    const node = queue[head];
    for (const [code, child] of next[node]) {
      let f = fail[node];
      while (f !== 0 && !next[f].has(code)) f = fail[f];
      fail[child] = next[f].get(code) ?? 0;
      if (fail[child] === child) fail[child] = 0;
      longest[child] = Math.max(longest[child], longest[fail[child]]);
      queue.push(child);
    }
  }
  return { next, fail, longest: Int32Array.from(longest) };
}

let automaton: Automaton | undefined;

/** Replaces every known kit secret value in `text`. */
export function scrubSecrets(text: string): string {
  const machine = automaton;
  if (!machine || text.length === 0) return text;
  // covered[i] is the end (exclusive) of the longest hit that starts at i.
  let hits: Map<number, number> | undefined;
  let node = 0;
  for (let index = 0; index < text.length; index++) {
    const code = text.charCodeAt(index);
    while (node !== 0 && !machine.next[node].has(code)) node = machine.fail[node];
    node = machine.next[node].get(code) ?? 0;
    const length = machine.longest[node];
    if (length > 0) {
      hits ??= new Map();
      const start = index + 1 - length;
      hits.set(start, Math.max(hits.get(start) ?? 0, index + 1));
    }
  }
  if (!hits) return text;
  let out = "";
  let at = 0;
  for (const start of [...hits.keys()].sort((x, y) => x - y)) {
    const end = hits.get(start)!;
    if (end <= at) continue;
    // An overlapping hit extends the redaction that is already written.
    if (start >= at) out += text.slice(at, start) + "[redacted]";
    at = end;
  }
  return out + text.slice(at);
}

type ConsoleMethod = "log" | "info" | "warn" | "error" | "debug";
const CONSOLE_METHODS: ConsoleMethod[] = [
  "log",
  "info",
  "warn",
  "error",
  "debug",
];
let logScrubberInstalled = false;

/**
 * Wraps the console so no logged value can carry a kit secret. Idempotent.
 * Returns a function that restores the original console (used by tests).
 */
export function installLogScrubber(): () => void {
  if (logScrubberInstalled) return () => {};
  logScrubberInstalled = true;
  const originals = new Map<ConsoleMethod, (...args: unknown[]) => void>();
  for (const method of CONSOLE_METHODS) {
    const original = console[method].bind(console) as (
      ...args: unknown[]
    ) => void;
    originals.set(method, console[method] as (...args: unknown[]) => void);
    console[method] = (...args: unknown[]) => {
      if (secretValues.length === 0) return original(...args);
      original(
        ...args.map((arg) =>
          typeof arg === "string"
            ? scrubSecrets(arg)
            : arg instanceof Error
              ? scrubSecrets(arg.stack ?? arg.message)
              : scrubSecrets(inspect(arg, { depth: 6 })),
        ),
      );
    };
  }
  return () => {
    for (const [method, original] of originals) console[method] = original;
    logScrubberInstalled = false;
  };
}

// ---------------------------------------------------------------------------
// Kit dirs a running child still references are never pruned.

const pinned = new Map<string, number>();
const KIT_HASH_IN_PATH = /\.monoptah-kit[\\/]([0-9a-f]{64})[\\/]/;

/** Pins the kit dirs named in a child's argv. Call the result when it exits. */
export function pinKitArgs(args: readonly string[]): () => void {
  const hashes = new Set<string>();
  for (const arg of args) {
    const match = KIT_HASH_IN_PATH.exec(arg);
    if (match) hashes.add(match[1]);
  }
  return pinHashes(hashes);
}

/** A lease expires on its own so a forgotten release cannot pin a dir forever. */
const LEASE_MS = 2 * 60 * 1000;

function pinHashes(hashes: Set<string>, expireMs?: number): () => void {
  for (const hash of hashes) pinned.set(hash, (pinned.get(hash) ?? 0) + 1);
  let released = false;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const release = () => {
    if (released) return;
    released = true;
    if (timer) clearTimeout(timer);
    for (const hash of hashes) {
      const left = (pinned.get(hash) ?? 1) - 1;
      if (left <= 0) pinned.delete(hash);
      else pinned.set(hash, left);
    }
  };
  if (expireMs) {
    timer = setTimeout(release, expireMs);
    timer.unref();
  }
  return release;
}

// ---------------------------------------------------------------------------
// Validation.

function validateHash(value: unknown): string {
  if (typeof value !== "string" || !HASH.test(value))
    throw new Error("Invalid kit hash");
  return value;
}

export function validateSkillName(value: string): boolean {
  return value.length > 0 && value.length <= 64 && SKILL_NAME.test(value);
}

function hasControl(value: string): boolean {
  // eslint-disable-next-line no-control-regex
  return /[\u0000-\u001f\u007f]/.test(value);
}

// ---------------------------------------------------------------------------
// Tar reader. Regular files and directories only.

function octal(field: Buffer): number {
  // A set high bit means GNU base-256 sizes, which no kit needs.
  if (field[0] & 0x80) throw new Error("Unsupported tar size encoding");
  const text = field.toString("latin1").replace(/\0.*$/s, "").trim();
  if (text === "") return 0;
  if (!/^[0-7]+$/.test(text)) throw new Error("Corrupt tar header");
  return parseInt(text, 8);
}

function cstring(field: Buffer): string {
  const end = field.indexOf(0);
  return field.subarray(0, end < 0 ? field.length : end).toString("utf8");
}

function checksumOk(header: Buffer): boolean {
  let sum = 0;
  for (let index = 0; index < BLOCK; index++)
    sum += index >= 148 && index < 156 ? 0x20 : header[index];
  return sum === octal(header.subarray(148, 156));
}

interface Entry {
  segments: string[];
  directory: boolean;
  executable: boolean;
  size: number;
  /** Offset of the first data byte in the archive. */
  offset: number;
}

function entrySegments(name: string, limits: KitLimits): string[] {
  if (
    name === "" ||
    Buffer.byteLength(name) > limits.maxPathBytes ||
    hasControl(name) ||
    name.includes("\\") ||
    name.startsWith("/") ||
    /^[A-Za-z]:/.test(name)
  )
    throw new Error("Refused tar entry: invalid path");
  const segments = name.replace(/\/$/, "").split("/");
  if (segments.length > limits.maxDepth)
    throw new Error("Refused tar entry: path too deep");
  for (const segment of segments)
    if (
      segment === "" ||
      segment === "." ||
      segment === ".." ||
      Buffer.byteLength(segment) > limits.maxSegmentBytes
    )
      throw new Error("Refused tar entry: invalid path");
  return segments;
}

/** Only `skills/<name>/..`, `plugins/<id>/..`, `mcp.json` and `kit.json`. */
function checkLayout(segments: string[], directory: boolean): void {
  const [top, second] = segments;
  if (!TOP_LEVEL.has(top)) throw new Error("Refused tar entry: unknown path");
  if (top === "mcp.json" || top === "kit.json") {
    if (segments.length !== 1 || directory)
      throw new Error("Refused tar entry: invalid path");
    return;
  }
  if (segments.length === 1) {
    if (!directory) throw new Error("Refused tar entry: invalid path");
    return;
  }
  const valid =
    top === "skills" ? validateSkillName(second) : PLUGIN_ID.test(second);
  if (!valid) throw new Error("Refused tar entry: invalid item name");
  if (segments.length === 2 && !directory)
    throw new Error("Refused tar entry: invalid path");
}

function* readTar(
  fd: number,
  archiveSize: number,
  limits: KitLimits,
): Generator<Entry> {
  const header = Buffer.alloc(BLOCK);
  const read = (position: number, into: Buffer): boolean => {
    let done = 0;
    while (done < into.length) {
      const n = readSync(fd, into, done, into.length - done, position + done);
      if (n === 0) return false;
      done += n;
    }
    return true;
  };
  let position = 0;
  let count = 0;
  let total = 0;
  let longName: string | undefined;
  let paxPath: string | undefined;
  for (;;) {
    // A valid archive ends with a zero block; running out of bytes is a cut.
    if (position + BLOCK > archiveSize) throw new Error("Truncated tar archive");
    if (!read(position, header)) throw new Error("Truncated tar archive");
    if (header.every((byte) => byte === 0)) return;
    if (!checksumOk(header)) throw new Error("Corrupt tar header");
    const type = String.fromCharCode(header[156] || 0x30);
    const size = octal(header.subarray(124, 136));
    const data = position + BLOCK;
    const padded = Math.ceil(size / BLOCK) * BLOCK;
    if (data + padded > archiveSize || size > limits.maxFileBytes)
      throw new Error("Refused tar entry: size exceeds the archive or the cap");
    position = data + padded;

    if (type === "L" || type === "x") {
      // GNU long name and pax path. Both are small text records.
      if (size > 64 * 1024) throw new Error("Refused tar entry: invalid header");
      const record = Buffer.alloc(size);
      if (!read(data, record)) throw new Error("Truncated tar archive");
      if (type === "L") {
        longName = cstring(record);
      } else {
        const text = record.toString("utf8");
        for (const line of text.split("\n")) {
          if (!line) continue;
          const match = /^\d+ ([^=]+)=(.*)$/s.exec(line);
          if (!match) throw new Error("Refused tar entry: invalid header");
          if (match[1] === "path") paxPath = match[2];
          else if (!/^(mtime|atime|ctime)$/.test(match[1]))
            throw new Error("Refused tar entry: unsupported extended header");
        }
      }
      continue;
    }
    if (type !== "0" && type !== "5")
      // Hard and symbolic links, devices, fifos, sparse files and the rest.
      throw new Error(
        `Refused tar entry: type '${type === "\0" ? "NUL" : type}' is not a regular file or directory`,
      );

    let name = cstring(header.subarray(0, 100));
    const magic = header.subarray(257, 263).toString("latin1");
    const prefix = magic === "ustar\0" ? cstring(header.subarray(345, 500)) : "";
    if (prefix) name = `${prefix}/${name}`;
    if (paxPath !== undefined) name = paxPath;
    else if (longName !== undefined) name = longName;
    longName = undefined;
    paxPath = undefined;

    const directory = type === "5";
    if (!directory && name.endsWith("/"))
      throw new Error("Refused tar entry: invalid path");
    if (directory && size !== 0)
      throw new Error("Refused tar entry: invalid directory");
    const segments = entrySegments(name, limits);
    checkLayout(segments, directory);
    if (++count > limits.maxEntries)
      throw new Error("Kit has too many entries");
    if (!directory) total += size;
    if (total > limits.maxTotalBytes)
      throw new Error("Kit is larger than the unpacked size cap");
    const mode = octal(header.subarray(100, 108));
    yield {
      segments,
      directory,
      executable: !directory && (mode & 0o111) !== 0,
      size,
      offset: data,
    };
  }
}

function checkMagic(fd: number): void {
  const head = Buffer.alloc(6);
  const n = readSync(fd, head, 0, 6, 0);
  const b = head.subarray(0, n);
  if (
    (b[0] === 0x1f && b[1] === 0x8b) || // gzip
    (b[0] === 0x42 && b[1] === 0x5a && b[2] === 0x68) || // bzip2
    (b[0] === 0xfd && b[1] === 0x37 && b[2] === 0x7a) || // xz
    (b[0] === 0x28 && b[1] === 0xb5 && b[2] === 0x2f && b[3] === 0xfd) || // zstd
    (b[0] === 0x50 && b[1] === 0x4b) // zip
  )
    throw new Error("Compressed archives are not accepted; send a plain tar");
}

const YIELD_BYTES = 4 * 1024 * 1024;
const yieldToLoop = () => new Promise<void>((done) => setImmediate(done));

/** Unpacks into `dir`, which must be a fresh directory. */
export async function unpackKitTar(
  archive: string,
  dir: string,
  limits: KitLimits = DEFAULT_KIT_LIMITS,
): Promise<void> {
  const fd = openSync(archive, "r");
  try {
    checkMagic(fd);
    const archiveSize = statSync(archive).size;
    const seen = new Set<string>();
    const root = resolve(dir) + sep;
    const buffer = Buffer.alloc(64 * 1024);
    for (const entry of readTar(fd, archiveSize, limits)) {
      // Case-folded so a case-insensitive filesystem cannot merge two entries.
      const key = entry.segments.join("/").normalize("NFC").toLowerCase();
      if (seen.has(key)) throw new Error("Refused tar entry: duplicate path");
      seen.add(key);
      const target = resolve(dir, ...entry.segments);
      if (!(target + sep).startsWith(root))
        throw new Error("Refused tar entry: invalid path");
      if (entry.directory) {
        mkdirSync(target, { recursive: true, mode: 0o700 });
        continue;
      }
      // Let other Host streams run between files and every few megabytes.
      await yieldToLoop();
      mkdirSync(join(target, ".."), { recursive: true, mode: 0o700 });
      // "wx" is O_CREAT|O_EXCL: it never follows a link or replaces a file.
      const out = openSync(target, "wx", entry.executable ? 0o700 : 0o600);
      try {
        let done = 0;
        while (done < entry.size) {
          const want = Math.min(buffer.length, entry.size - done);
          const n = readSync(fd, buffer, 0, want, entry.offset + done);
          if (n <= 0) throw new Error("Truncated tar archive");
          let written = 0;
          while (written < n)
            written += writeSync(out, buffer, written, n - written);
          done += n;
          if ((done & (YIELD_BYTES - 1)) < n) await yieldToLoop();
        }
      } finally {
        closeSync(out);
      }
    }
  } finally {
    closeSync(fd);
  }
}

// ---------------------------------------------------------------------------
// The kit manager.

interface Managed {
  version: 1;
  hash: string;
  /** The kit applied before this one, kept on disk for racing readers. */
  previous?: string;
  skills: string[];
  plugins: string[];
}

export interface KitApplyResult {
  hash: string;
  skills: string[];
  plugins: string[];
  mcp: boolean;
  /** Skill links created or kept. */
  linked: string[];
  /** Skills skipped because a real skill of that name exists. */
  skipped: string[];
  /** Managed links removed because the new kit no longer has them. */
  removed: string[];
  warnings: string[];
}

export interface KitRun {
  (
    command: string,
    args: string[],
    options: { cwd: string; env: NodeJS.ProcessEnv; timeoutMs?: number },
  ): Promise<{
    code: number;
    stdout: string;
    stderr: string;
    timedOut?: boolean;
  }>;
}

export interface HostKitOptions {
  home?: string;
  limits?: Partial<KitLimits>;
  /** Directories searched by `kit.probe`. */
  pathDirs?: string[];
  resolveClaude?: () => Promise<string>;
  run?: KitRun;
  /** Test seam for failure injection. */
  symlink?: typeof symlinkSync;
  /** Test seam: called before each step that follows the unpack. */
  beforeStep?: (step: "flip" | "unlink" | "manifest") => void;
  /** Hard limit for `kit.health`; the default is 60 s. */
  healthTimeoutMs?: number;
}

const KILL_GRACE_MS = 2000;

/** Ends a child and everything it started (its own process group). */
function killTree(pid: number | undefined, signal: NodeJS.Signals): void {
  if (!pid) return;
  try {
    if (process.platform === "win32") {
      execFile("taskkill", ["/pid", String(pid), "/T", "/F"], () => {});
    } else {
      process.kill(-pid, signal);
    }
  } catch {
    /* already gone */
  }
}

/**
 * Runs a command with a hard deadline. The child leads its own process group,
 * so a timeout reaches the MCP servers it started too, and the promise settles
 * on a timer even when a descendant keeps the output pipes open.
 */
const defaultRun: KitRun = async (command, args, options) => {
  const launch = await providerLaunch(command, args);
  const timeoutMs = options.timeoutMs ?? HEALTH_TIMEOUT_MS;
  return new Promise((resolveRun) => {
    const child = spawn(launch.command, launch.args, {
      cwd: options.cwd,
      env: options.env,
      stdio: ["ignore", "pipe", "pipe"],
      detached: process.platform !== "win32",
      windowsHide: true,
    });
    let stdout = "";
    let stderr = "";
    let settled = false;
    let timedOut = false;
    const timers: ReturnType<typeof setTimeout>[] = [];
    const finish = (code: number) => {
      if (settled) return;
      settled = true;
      for (const timer of timers) clearTimeout(timer);
      child.stdout?.destroy();
      child.stderr?.destroy();
      resolveRun({ code, stdout, stderr, ...(timedOut ? { timedOut } : {}) });
    };
    const collect = (stream: "stdout" | "stderr") => (data: Buffer) => {
      const text = data.toString("utf8");
      if (stream === "stdout") stdout = (stdout + text).slice(0, HEALTH_MAX_BUFFER);
      else stderr = (stderr + text).slice(0, HEALTH_MAX_BUFFER);
    };
    child.stdout?.on("data", collect("stdout"));
    child.stderr?.on("data", collect("stderr"));
    child.once("error", () => finish(1));
    child.once("close", (code) => {
      // The leader has exited; stray descendants must not outlive the call.
      killTree(child.pid, "SIGKILL");
      finish(code ?? 1);
    });
    timers.push(
      setTimeout(() => {
        timedOut = true;
        killTree(child.pid, "SIGTERM");
        timers.push(
          setTimeout(() => {
            killTree(child.pid, "SIGKILL");
            // Settle even if a descendant still holds the pipes open.
            finish(1);
          }, KILL_GRACE_MS),
        );
      }, timeoutMs),
    );
  });
};

/** What `claude mcp list` says about one server. */
export type KitServerStatus =
  | "connected"
  | "failed"
  | "signInNeeded"
  | "disabled"
  | "configured";

export interface KitServerHealth {
  name: string;
  status: KitServerStatus;
  /**
   * True for "needs authentication", "auth required" and "pending approval"
   * outcomes. This is NOT a failure. Health cannot observe whether the user
   * signed in on this host (D12: sign-ins are never copied or read), so the
   * app must not treat it as an error for OAuth-style servers.
   */
  signInNeeded: boolean;
  /** From the kit's mcp.json; "unknown" when the server is not in the kit. */
  transport: "stdio" | "http" | "sse" | "unknown";
  /** The status after the last " - " of the line (never the command or URL), scrubbed. */
  detail: string;
}

const SERVER_LINE_NAME = /^[A-Za-z0-9._@-]+$/;

export function classifyHealth(detail: string): KitServerStatus {
  const lower = detail.toLowerCase();
  if (
    lower.includes("authentication") ||
    lower.includes("needs auth") ||
    lower.includes("auth required") ||
    lower.includes("authenticate") ||
    lower.includes("pending approval") ||
    lower.includes("sign in")
  )
    return "signInNeeded";
  if (lower.includes("failed")) return "failed";
  if (lower.includes("disabled")) return "disabled";
  if (lower.includes("connected")) return "connected";
  return "configured";
}

function transportOf(config: unknown): KitServerHealth["transport"] {
  if (typeof config !== "object" || config === null) return "unknown";
  const record = config as Record<string, unknown>;
  const type = typeof record.type === "string" ? record.type.toLowerCase() : "";
  if (type === "stdio" || (!type && typeof record.command === "string"))
    return "stdio";
  if (type === "sse") return "sse";
  if (type === "http" || type === "streamable-http" || typeof record.url === "string")
    return "http";
  return "unknown";
}

export function parseHealth(
  text: string,
  servers: Record<string, unknown> = {},
): KitServerHealth[] {
  const out: KitServerHealth[] = [];
  for (const line of text.split("\n")) {
    const at = line.indexOf(": ");
    if (at <= 0) continue;
    const name = line.slice(0, at).trim();
    if (!SERVER_LINE_NAME.test(name)) continue;
    // Only the STATUS after the last " - ": what precedes it is the server's command,
    // arguments or URL, which can hold a credential and must not leave the Host.
    const rest = line.slice(at + 2).trim();
    const cut = rest.lastIndexOf(" - ");
    const detail = cut < 0 ? "" : rest.slice(cut + 3).trim();
    const status = classifyHealth(detail);
    out.push({
      name,
      status,
      signInNeeded: status === "signInNeeded",
      transport: transportOf(servers[name]),
      detail,
    });
  }
  return out;
}

export class HostKit {
  readonly root: string;
  private readonly skillsDir: string;
  private readonly limits: KitLimits;
  private cache: { kit: ClaudeKit | undefined } | undefined;
  private queue: Promise<unknown> = Promise.resolve();

  constructor(
    private readonly uploads: { committedPath(uploadId: unknown): string },
    private readonly options: HostKitOptions = {},
  ) {
    const home = options.home ?? homedir();
    this.root = join(home, ".monoptah-kit");
    this.skillsDir = join(home, ".claude", "skills");
    this.limits = { ...DEFAULT_KIT_LIMITS, ...options.limits };
    // Host restarts must keep scrubbing secrets stored earlier.
    this.refreshScrub();
    this.recover();
  }

  /**
   * A crash inside an apply can leave a quarantined `.old-<hash>-*` dir with
   * no `<hash>` dir, or half-built `.unpack-*` dirs. Put the first back and
   * drop the rest.
   */
  private recover(): void {
    let entries: string[];
    try {
      entries = readdirSync(this.root);
    } catch {
      return;
    }
    for (const entry of entries) {
      const path = join(this.root, entry);
      try {
        const old = /^\.old-([0-9a-f]{64})-[0-9a-f]+$/.exec(entry);
        if (old && !existsSync(join(this.root, old[1]))) {
          renameSync(path, join(this.root, old[1]));
        } else if (/^\.(old|unpack|current|tmp)-/.test(entry)) {
          rmSync(path, { recursive: true, force: true });
        }
      } catch {
        /* best effort; the next apply cleans up */
      }
    }
  }

  private ensureRoot(): void {
    mkdirSync(this.root, { recursive: true, mode: 0o700 });
    chmodSync(this.root, 0o700);
  }

  private secretsPath(): string {
    return join(this.root, "secrets.env");
  }

  private managedPath(): string {
    return join(this.root, "managed.json");
  }

  private skillTarget(name: string): string {
    return join(this.root, "current", "skills", name);
  }

  scrub(text: string): string {
    return scrubSecrets(text);
  }

  private readSecrets(): Record<string, string> {
    let text: string;
    try {
      text = readFileSync(this.secretsPath(), "utf8");
    } catch {
      return {};
    }
    const out: Record<string, string> = {};
    for (const line of text.split("\n")) {
      const at = line.indexOf("=");
      if (at <= 0) continue;
      const name = line.slice(0, at);
      if (!SECRET_NAME.test(name)) continue;
      try {
        const value: unknown = JSON.parse(line.slice(at + 1));
        if (typeof value === "string") out[name] = value;
      } catch {
        /* ignore a damaged line */
      }
    }
    return out;
  }

  private refreshScrub(): void {
    this.cache = undefined;
    setScrubValues(Object.values(this.readSecrets()));
  }

  private readManaged(): Managed | undefined {
    try {
      const value = JSON.parse(readFileSync(this.managedPath(), "utf8"));
      if (
        value &&
        value.version === 1 &&
        Array.isArray(value.skills) &&
        value.skills.every(
          (name: unknown) => typeof name === "string" && validateSkillName(name),
        )
      )
        return value as Managed;
    } catch {
      /* none yet */
    }
    return undefined;
  }

  private writeAtomic(path: string, data: string): void {
    const temp = join(this.root, `.tmp-${randomBytes(8).toString("hex")}`);
    const fd = openSync(temp, "wx", 0o600);
    try {
      let written = 0;
      const buffer = Buffer.from(data);
      while (written < buffer.length)
        written += writeSync(fd, buffer, written, buffer.length - written);
      fsyncSync(fd);
    } catch (error) {
      closeSync(fd);
      rmSync(temp, { force: true });
      throw error;
    }
    closeSync(fd);
    try {
      chmodSync(temp, 0o600);
      renameSync(temp, path);
    } catch (error) {
      rmSync(temp, { force: true });
      throw error;
    }
  }

  private flipCurrent(hash: string): void {
    const link = join(this.root, "current");
    const temp = join(this.root, `.current-${randomBytes(8).toString("hex")}`);
    const windows = process.platform === "win32";
    symlinkSync(
      windows ? join(this.root, hash) : hash,
      temp,
      windows ? "junction" : undefined,
    );
    try {
      // rename over an existing symlink is atomic on POSIX.
      renameSync(temp, link);
    } catch (error) {
      if (!windows) {
        rmSync(temp, { force: true });
        throw error;
      }
      // Windows cannot rename over an existing junction.
      rmSync(link, { force: true, recursive: true });
      renameSync(temp, link);
    }
  }

  /** The hash `current` points at, if any. */
  private currentTarget(): string | undefined {
    try {
      const target = readlinkSync(join(this.root, "current"));
      const hash = target.split(/[\\/]/).filter(Boolean).pop() ?? "";
      return HASH.test(hash) ? hash : undefined;
    } catch {
      return undefined;
    }
  }

  private list(dir: string): string[] {
    try {
      return readdirSync(dir, { withFileTypes: true })
        .filter((entry) => entry.isDirectory())
        .map((entry) => entry.name)
        .sort();
    } catch {
      return [];
    }
  }

  /** Applies run one at a time so two of them never race on `current`. */
  apply(params: Record<string, unknown>): Promise<KitApplyResult> {
    const run = this.queue.then(() => this.applyNow(params));
    this.queue = run.then(
      () => undefined,
      () => undefined,
    );
    return run;
  }

  private async applyNow(
    params: Record<string, unknown>,
  ): Promise<KitApplyResult> {
    const hash = validateHash(params.hash);
    const archive = this.uploads.committedPath(params.uploadId);
    this.ensureRoot();
    const dir = join(this.root, hash);
    // Always unpack the uploaded archive into a fresh dir: an existing
    // <hash> dir is never trusted, so "Update Claude setup" repairs a
    // tampered one.
    const fresh = join(this.root, `.unpack-${randomBytes(8).toString("hex")}`);
    // mkdir without `recursive` fails if the path exists: O_EXCL semantics.
    mkdirSync(fresh, { mode: 0o700 });
    try {
      await unpackKitTar(archive, fresh, this.limits);
      this.checkJson(fresh);
      closeSync(openSync(join(fresh, COMPLETE), "wx", 0o600));
    } catch (error) {
      rmSync(fresh, { recursive: true, force: true });
      throw error;
    }
    let quarantined: string | undefined;
    try {
      if (existsSync(dir)) {
        quarantined = join(
          this.root,
          `.old-${hash}-${randomBytes(8).toString("hex")}`,
        );
        renameSync(dir, quarantined);
      }
      renameSync(fresh, dir);
    } catch (error) {
      if (quarantined && !existsSync(dir)) renameSync(quarantined, dir);
      rmSync(fresh, { recursive: true, force: true });
      throw error;
    }
    if (quarantined) rmSync(quarantined, { recursive: true, force: true });

    const skills = this.list(join(dir, "skills"));
    const plugins = this.list(join(dir, "plugins"));
    const mcp = existsSync(join(dir, "mcp.json"));
    const before = this.readManaged();
    const previous = before && before.hash !== hash ? before.hash : before?.previous;
    const links = this.linkSkills(skills);
    const removed: string[] = [];
    const previousTarget = this.currentTarget();
    const step = this.options.beforeStep ?? (() => {});
    try {
      step("flip");
      this.flipCurrent(hash);
      step("unlink");
      this.unlinkDropped(before?.skills ?? [], skills, removed);
      step("manifest");
      this.writeAtomic(
        this.managedPath(),
        JSON.stringify({
          version: 1,
          hash,
          previous,
          skills: links.linked,
          plugins,
        } satisfies Managed),
      );
    } catch (error) {
      // Undo in reverse: restore the old current, re-create unlinked links,
      // then remove the links this apply created.
      try {
        if (previousTarget) this.flipCurrent(previousTarget);
        else rmSync(join(this.root, "current"), { force: true, recursive: true });
      } catch {
        /* keep undoing the rest */
      }
      for (const name of removed) {
        try {
          symlinkSync(
            this.skillTarget(name),
            join(this.skillsDir, name),
            process.platform === "win32" ? "junction" : undefined,
          );
        } catch {
          /* best effort */
        }
      }
      this.rollbackLinks(links.created);
      this.cache = undefined;
      throw error;
    }
    // Keep this kit and the one before it: a Claude start that read the old
    // paths just before this apply must still find them. Dirs a running child
    // references are never removed either.
    const keep = new Set([hash, previous]);
    for (const entry of readdirSync(this.root)) {
      if (HASH.test(entry) && !keep.has(entry) && !pinned.has(entry))
        rmSync(join(this.root, entry), { recursive: true, force: true });
      else if (/^\.(old|unpack|current)-/.test(entry) && entry !== "current")
        rmSync(join(this.root, entry), { recursive: true, force: true });
    }
    this.cache = undefined;
    return {
      hash,
      skills,
      plugins,
      mcp,
      linked: links.linked,
      skipped: links.skipped,
      removed,
      warnings: links.skipped.map(
        (name) =>
          `Skill "${name}" already exists on this host; the remote copy was kept`,
      ),
    };
  }

  private checkJson(dir: string): void {
    const read = (name: string): unknown => {
      const file = join(dir, name);
      if (!existsSync(file)) return undefined;
      if (statSync(file).size > MAX_JSON_BYTES)
        throw new Error(`${name} is too large`);
      try {
        return JSON.parse(readFileSync(file, "utf8"));
      } catch {
        throw new Error(`${name} is not valid JSON`);
      }
    };
    const isObject = (value: unknown): value is Record<string, unknown> =>
      typeof value === "object" && value !== null && !Array.isArray(value);
    const mcp = read("mcp.json");
    if (mcp !== undefined) {
      if (!isObject(mcp) || !isObject(mcp.mcpServers))
        throw new Error("mcp.json must hold an mcpServers object");
      for (const name of Object.keys(mcp.mcpServers))
        if (!SERVER_NAME.test(name))
          throw new Error("mcp.json has an invalid server name");
    }
    const manifest = read("kit.json");
    if (manifest !== undefined && !isObject(manifest))
      throw new Error("kit.json must be an object");
  }

  /**
   * Links the kit's skills. Only links created here are in `created`, so a
   * failure can undo exactly those and nothing else.
   */
  private linkSkills(skills: string[]): {
    linked: string[];
    skipped: string[];
    created: string[];
  } {
    mkdirSync(this.skillsDir, { recursive: true });
    const linked: string[] = [];
    const skipped: string[] = [];
    const created: string[] = [];
    const symlink = this.options.symlink ?? symlinkSync;
    try {
      for (const name of skills) {
        const link = join(this.skillsDir, name);
        let exists = true;
        try {
          lstatSync(link);
        } catch {
          exists = false;
        }
        if (!exists) {
          symlink(
            this.skillTarget(name),
            link,
            process.platform === "win32" ? "junction" : undefined,
          );
          created.push(link);
          linked.push(name);
        } else if (this.isOurLink(link, name)) {
          linked.push(name);
        } else {
          // A real skill, a directory or a foreign link: the remote one wins.
          skipped.push(name);
        }
      }
    } catch (error) {
      this.rollbackLinks(created);
      throw error;
    }
    return { linked, skipped, created };
  }

  private rollbackLinks(created: string[]): void {
    for (const link of created) rmSync(link, { force: true });
    created.length = 0;
  }

  /** Removes managed links the new kit no longer has. */
  private unlinkDropped(
    previous: string[],
    skills: string[],
    removed: string[],
  ): void {
    for (const name of previous) {
      if (skills.includes(name)) continue;
      const link = join(this.skillsDir, name);
      if (this.isOurLink(link, name)) {
        unlinkSync(link);
        removed.push(name);
      }
    }
  }

  private isOurLink(link: string, name: string): boolean {
    try {
      if (!lstatSync(link).isSymbolicLink()) return false;
      return readlinkSync(link) === this.skillTarget(name);
    } catch {
      return false;
    }
  }

  /** Replaces the whole secrets file. Errors never carry a value. */
  secrets(params: Record<string, unknown>): { count: number; hash: string } {
    const values = params.values;
    if (typeof values !== "object" || values === null || Array.isArray(values))
      throw new Error("Invalid kit secrets");
    const entries = Object.entries(values as Record<string, unknown>);
    if (entries.length > MAX_SECRET_ENTRIES)
      throw new Error("Too many kit secrets");
    let total = 0;
    const lines: string[] = [];
    for (const [name, value] of entries.sort(([a], [b]) => a.localeCompare(b))) {
      if (!SECRET_NAME.test(name)) throw new Error("Invalid kit secret name");
      if (typeof value !== "string")
        throw new Error(`Invalid value for kit secret ${name}`);
      if (value.includes("\0"))
        throw new Error(`Invalid value for kit secret ${name}`);
      const bytes = Buffer.byteLength(value);
      if (bytes > MAX_SECRET_VALUE_BYTES)
        throw new Error(`Kit secret ${name} is too large`);
      total += bytes;
      if (total > MAX_SECRETS_TOTAL_BYTES)
        throw new Error("Kit secrets are too large");
      lines.push(`${name}=${JSON.stringify(value)}`);
    }
    this.ensureRoot();
    const data = lines.length ? `${lines.join("\n")}\n` : "";
    this.writeAtomic(this.secretsPath(), data);
    this.refreshScrub();
    return { count: lines.length, hash: this.hashOf(data) };
  }

  private hashOf(data: string): string {
    return data === ""
      ? ""
      : createHash("sha256").update(data).digest("hex");
  }

  /** The kit for the next interactive Claude spawn, or undefined. */
  current(): ClaudeKit | undefined {
    if (this.cache) return this.cache.kit;
    const kit = this.load();
    this.cache = { kit };
    return kit;
  }

  private load(): ClaudeKit | undefined {
    let hash: string;
    try {
      const target = readlinkSync(join(this.root, "current"));
      hash = target.split(/[\\/]/).filter(Boolean).pop() ?? "";
    } catch {
      return undefined;
    }
    if (!HASH.test(hash) || !existsSync(join(this.root, hash, COMPLETE)))
      return undefined;
    let secretsData = "";
    try {
      secretsData = readFileSync(this.secretsPath(), "utf8");
    } catch {
      /* no secrets */
    }
    // Hash paths, not `current`: a started Claude keeps seeing the kit it was
    // launched with until it respawns.
    const base = join(this.root, hash);
    const mcp = join(base, "mcp.json");
    return {
      hash,
      secretsHash: this.hashOf(secretsData),
      pluginDirs: this.list(join(this.root, hash, "plugins")).map((id) =>
        join(base, "plugins", id),
      ),
      mcpConfig: existsSync(join(this.root, hash, "mcp.json"))
        ? mcp
        : undefined,
      env: this.readSecrets(),
      lease: () => pinHashes(new Set([hash]), LEASE_MS),
    };
  }

  probe(params: Record<string, unknown>): { present: string[] } {
    const commands = params.commands;
    if (!Array.isArray(commands) || commands.length > 32)
      throw new Error("Invalid probe commands");
    const present: string[] = [];
    for (const command of commands) {
      if (typeof command !== "string" || !COMMAND_NAME.test(command))
        throw new Error("Invalid probe command");
      if (this.which(command)) present.push(command);
    }
    return { present };
  }

  private which(command: string): boolean {
    const home = this.options.home ?? homedir();
    const dirs =
      this.options.pathDirs ??
      [
        ...(process.env.PATH ?? "").split(delimiter),
        join(home, ".local", "bin"),
        join(home, ".bun", "bin"),
        join(home, ".cargo", "bin"),
        "/usr/local/bin",
        "/opt/homebrew/bin",
      ];
    const extensions =
      process.platform === "win32" ? ["", ".exe", ".cmd", ".bat", ".com"] : [""];
    for (const dir of new Set(dirs)) {
      if (!dir) continue;
      for (const extension of extensions) {
        try {
          const file = join(dir.replace(/^"|"$/g, ""), command + extension);
          if (statSync(file).isFile()) return true;
        } catch {
          /* keep looking */
        }
      }
    }
    return false;
  }

  /**
   * Runs `claude mcp list` once in `cwd`. Claude Code has no flag that adds an
   * MCP config to `mcp list` (`--mcp-config` is rejected there), so the kit's
   * servers are supplied as the user-scope config of a throwaway
   * CLAUDE_CONFIG_DIR. That run therefore checks the kit's servers, with the
   * kit secrets in its env. With no kit MCP config it checks the host's own.
   * Returns the raw output (scrubbed); the app parses it.
   */
  async health(cwd: string): Promise<{
    exitCode: number;
    stdout: string;
    stderr: string;
    kitServers: boolean;
    timedOut: boolean;
    /** Per-server result. See KitServerHealth.signInNeeded and D12. */
    servers: KitServerHealth[];
  }> {
    if (
      typeof cwd !== "string" ||
      cwd.includes("\0") ||
      !statSync(cwd).isDirectory()
    )
      throw new Error("Invalid working directory");
    const claude = await (
      this.options.resolveClaude ??
      (async () => {
        throw new Error("Claude is not available on this host");
      })
    )();
    const run = this.options.run ?? defaultRun;
    const kit = this.current();
    const env: NodeJS.ProcessEnv = { ...process.env, ...(kit?.env ?? {}) };
    let configDir: string | undefined;
    let configured: Record<string, unknown> = {};
    try {
      if (kit?.mcpConfig) {
        const mcp = JSON.parse(readFileSync(kit.mcpConfig, "utf8")) as {
          mcpServers: Record<string, unknown>;
        };
        configured = mcp.mcpServers;
        configDir = mkdtempSync(join(tmpdir(), "monoptah-kit-health-"));
        chmodSync(configDir, 0o700);
        writeFileSync(
          join(configDir, ".claude.json"),
          JSON.stringify({ mcpServers: mcp.mcpServers }),
          { mode: 0o600 },
        );
        env.CLAUDE_CONFIG_DIR = configDir;
      }
      const result = await run(claude, ["mcp", "list"], {
        cwd,
        env,
        timeoutMs: this.options.healthTimeoutMs,
      });
      const stdout = this.scrub(result.stdout);
      return {
        exitCode: result.code,
        stdout,
        stderr: this.scrub(result.stderr),
        kitServers: configDir !== undefined,
        timedOut: result.timedOut === true,
        servers: parseHealth(stdout, configured),
      };
    } finally {
      if (configDir) rmSync(configDir, { recursive: true, force: true });
    }
  }

  /** Hash pair used by tests and by callers that need the settings key. */
  key(): string {
    return claudeKitKey(this.current());
  }
}
