import { createHash, randomBytes } from "node:crypto";
import {
  chmodSync,
  closeSync,
  existsSync,
  fsyncSync,
  lstatSync,
  mkdirSync,
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
  writeSync,
} from "node:fs";
import { execFile } from "node:child_process";
import { homedir } from "node:os";
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
  maxTotalBytes: 512 * 1024 * 1024,
  maxFileBytes: 256 * 1024 * 1024,
  maxPathBytes: 512,
  maxSegmentBytes: 255,
  maxDepth: 24,
};

// ---------------------------------------------------------------------------
// Secret scrubbing for Host logs and error text.

let secretValues: string[] = [];

function setScrubValues(values: Iterable<string>): void {
  const forms = new Set<string>();
  for (const value of values) {
    if (value.length < MIN_SCRUB_LENGTH) continue;
    forms.add(value);
    // Also the JSON-escaped form, which is how an object dump would show it.
    forms.add(JSON.stringify(value).slice(1, -1));
  }
  secretValues = [...forms]
    .filter((value) => value.length >= MIN_SCRUB_LENGTH)
    .sort((a, b) => b.length - a.length);
}

/** Replaces every known kit secret value in `text`. */
export function scrubSecrets(text: string): string {
  let out = text;
  for (const value of secretValues) out = out.split(value).join("[redacted]");
  return out;
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

/** Unpacks into `dir`, which must be a fresh directory. */
export function unpackKitTar(
  archive: string,
  dir: string,
  limits: KitLimits = DEFAULT_KIT_LIMITS,
): void {
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
    options: { cwd: string; env: NodeJS.ProcessEnv },
  ): Promise<{ code: number; stdout: string; stderr: string }>;
}

export interface HostKitOptions {
  home?: string;
  limits?: Partial<KitLimits>;
  /** Directories searched by `kit.probe`. */
  pathDirs?: string[];
  resolveClaude?: () => Promise<string>;
  run?: KitRun;
}

const defaultRun: KitRun = async (command, args, options) => {
  const launch = await providerLaunch(command, args);
  return new Promise((resolveRun) => {
    execFile(
      launch.command,
      launch.args,
      {
        cwd: options.cwd,
        env: options.env,
        timeout: HEALTH_TIMEOUT_MS,
        maxBuffer: HEALTH_MAX_BUFFER,
        windowsHide: true,
      },
      (error, stdout, stderr) => {
        const code =
          error == null
            ? 0
            : typeof (error as NodeJS.ErrnoException).code === "number"
              ? ((error as NodeJS.ErrnoException).code as unknown as number)
              : 1;
        resolveRun({ code, stdout: String(stdout), stderr: String(stderr) });
      },
    );
  });
};

export class HostKit {
  readonly root: string;
  private readonly skillsDir: string;
  private readonly limits: KitLimits;
  private cache: { kit: ClaudeKit | undefined } | undefined;

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

  apply(params: Record<string, unknown>): KitApplyResult {
    const hash = validateHash(params.hash);
    const archive = this.uploads.committedPath(params.uploadId);
    this.ensureRoot();
    const dir = join(this.root, hash);
    let complete = false;
    try {
      complete = lstatSync(dir).isDirectory() && existsSync(join(dir, COMPLETE));
    } catch {
      /* not there yet */
    }
    if (!complete) {
      // A leftover without the marker is a failed earlier attempt of ours.
      rmSync(dir, { recursive: true, force: true });
      // mkdir without `recursive` fails if the path exists: O_EXCL semantics.
      mkdirSync(dir, { mode: 0o700 });
      try {
        unpackKitTar(archive, dir, this.limits);
        this.checkJson(dir);
        closeSync(openSync(join(dir, COMPLETE), "wx", 0o600));
      } catch (error) {
        rmSync(dir, { recursive: true, force: true });
        throw error;
      }
    }

    const skills = this.list(join(dir, "skills"));
    const plugins = this.list(join(dir, "plugins"));
    const mcp = existsSync(join(dir, "mcp.json"));
    this.flipCurrent(hash);
    const links = this.reconcileSkills(skills);
    this.writeAtomic(
      this.managedPath(),
      JSON.stringify({
        version: 1,
        hash,
        skills: links.linked,
        plugins,
      } satisfies Managed),
    );
    // Older unpacked kits are managed dirs this module created earlier.
    for (const entry of readdirSync(this.root))
      if (HASH.test(entry) && entry !== hash)
        rmSync(join(this.root, entry), { recursive: true, force: true });
    this.cache = undefined;
    return {
      hash,
      skills,
      plugins,
      mcp,
      linked: links.linked,
      skipped: links.skipped,
      removed: links.removed,
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

  /** Links new skills, removes managed links the new kit dropped. */
  private reconcileSkills(skills: string[]): {
    linked: string[];
    skipped: string[];
    removed: string[];
  } {
    const previous = this.readManaged()?.skills ?? [];
    const removed: string[] = [];
    for (const name of previous) {
      if (skills.includes(name)) continue;
      const link = join(this.skillsDir, name);
      if (this.isOurLink(link, name)) {
        unlinkSync(link);
        removed.push(name);
      }
    }
    mkdirSync(this.skillsDir, { recursive: true });
    const linked: string[] = [];
    const skipped: string[] = [];
    for (const name of skills) {
      const link = join(this.skillsDir, name);
      let exists = true;
      try {
        lstatSync(link);
      } catch {
        exists = false;
      }
      if (!exists) {
        symlinkSync(
          this.skillTarget(name),
          link,
          process.platform === "win32" ? "junction" : undefined,
        );
        linked.push(name);
      } else if (this.isOurLink(link, name)) {
        linked.push(name);
      } else {
        // A real skill, a directory or a foreign link: the remote one wins.
        skipped.push(name);
      }
    }
    return { linked, skipped, removed };
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
    const current = join(this.root, "current");
    const mcp = join(current, "mcp.json");
    return {
      hash,
      secretsHash: this.hashOf(secretsData),
      pluginDirs: this.list(join(this.root, hash, "plugins")).map((id) =>
        join(current, "plugins", id),
      ),
      mcpConfig: existsSync(join(this.root, hash, "mcp.json"))
        ? mcp
        : undefined,
      env: this.readSecrets(),
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
   * Runs `claude mcp list` in `cwd` with the kit's MCP config and secrets.
   * Returns the raw output (scrubbed); the app parses it.
   */
  async health(cwd: string): Promise<{
    exitCode: number;
    stdout: string;
    stderr: string;
    kitFlags: boolean;
  }> {
    if (typeof cwd !== "string" || cwd.includes("\0") || !statSync(cwd).isDirectory())
      throw new Error("Invalid working directory");
    const claude = await (this.options.resolveClaude ?? (async () => {
      throw new Error("Claude is not available on this host");
    }))();
    const run = this.options.run ?? defaultRun;
    const kit = this.current();
    const env = { ...process.env, ...(kit?.env ?? {}) };
    const withKit = kit?.mcpConfig
      ? ["mcp", "list", "--mcp-config", kit.mcpConfig]
      : undefined;
    let kitFlags = withKit !== undefined;
    let result = await run(claude, withKit ?? ["mcp", "list"], { cwd, env });
    if (withKit && result.code !== 0) {
      // Older Claude builds may not take the flag on `mcp list`.
      kitFlags = false;
      result = await run(claude, ["mcp", "list"], { cwd, env });
    }
    return {
      exitCode: result.code,
      stdout: this.scrub(result.stdout),
      stderr: this.scrub(result.stderr),
      kitFlags,
    };
  }

  /** Hash pair used by tests and by callers that need the settings key. */
  key(): string {
    return claudeKitKey(this.current());
  }
}
