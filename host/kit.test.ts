import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  existsSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  readlinkSync,
  rmSync,
  statSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  HostKit,
  installLogScrubber,
  scrubSecrets,
  unpackKitTar,
  DEFAULT_KIT_LIMITS,
  type KitRun,
} from "./kit";
import { buildClaudeSpawnArgs } from "../src/integrations/harness/providers/claude/claudeProtocol";

// ---------------------------------------------------------------------------
// A minimal tar writer, so tests can build hostile archives on purpose.

interface TarEntry {
  name: string;
  type?: string; // default "0"
  data?: string | Buffer;
  mode?: number;
  linkname?: string;
}

function header(entry: TarEntry, size: number): Buffer {
  const block = Buffer.alloc(512);
  block.write(entry.name, 0, 100, "utf8");
  block.write((entry.mode ?? 0o644).toString(8).padStart(7, "0") + "\0", 100);
  block.write("0000000\0", 108);
  block.write("0000000\0", 116);
  block.write(size.toString(8).padStart(11, "0") + "\0", 124);
  block.write("00000000000\0", 136);
  block.write("        ", 148);
  block.write(entry.type ?? "0", 156);
  if (entry.linkname) block.write(entry.linkname, 157, 100, "utf8");
  block.write("ustar\0" + "00", 257);
  let sum = 0;
  for (const byte of block) sum += byte;
  block.write(sum.toString(8).padStart(6, "0") + "\0 ", 148);
  return block;
}

function tar(entries: TarEntry[]): Buffer {
  const parts: Buffer[] = [];
  for (const entry of entries) {
    const data = Buffer.from(entry.data ?? "");
    parts.push(header(entry, data.length));
    if (data.length) {
      parts.push(data);
      parts.push(Buffer.alloc((512 - (data.length % 512)) % 512));
    }
  }
  parts.push(Buffer.alloc(1024));
  return Buffer.concat(parts);
}

const HASH_A = "a".repeat(64);
const HASH_B = "b".repeat(64);
const SKILL = (name: string): TarEntry[] => [
  { name: `skills/${name}/`, type: "5", mode: 0o755 },
  { name: `skills/${name}/SKILL.md`, data: `# ${name}` },
];
const PLUGIN = (id: string): TarEntry[] => [
  { name: `plugins/${id}/`, type: "5", mode: 0o755 },
  { name: `plugins/${id}/run.sh`, data: "#!/bin/sh\n", mode: 0o755 },
];
const MCP = {
  name: "mcp.json",
  data: JSON.stringify({
    mcpServers: { clickup: { command: "npx", env: { T: "${MONOPTAH_KIT_CLICKUP_T}" } } },
  }),
};

let home: string;
let uploadRoot: string;
let counter = 0;
const uploads = {
  committedPath(id: unknown) {
    return join(uploadRoot, String(id));
  },
};
function stage(entries: TarEntry[] | Buffer): string {
  const id = `up${counter++}`;
  writeFileSync(join(uploadRoot, id), Buffer.isBuffer(entries) ? entries : tar(entries));
  return id;
}
function kitFor(options: ConstructorParameters<typeof HostKit>[1] = {}) {
  return new HostKit(uploads, { home, ...options });
}

beforeEach(() => {
  home = mkdtempSync(join(tmpdir(), "host-kit-test-"));
  uploadRoot = mkdtempSync(join(tmpdir(), "host-kit-uploads-"));
  scrubSecrets("");
});
afterEach(() => {
  rmSync(home, { recursive: true, force: true });
  rmSync(uploadRoot, { recursive: true, force: true });
});

const mode = (path: string) => statSync(path).mode & 0o777;
const posix = process.platform === "win32" ? it.skip : it;

describe("kit.apply: what is refused", () => {
  const refused: Array<[string, TarEntry[]]> = [
    ["a parent traversal", [{ name: "skills/a/../../escape", data: "x" }]],
    ["a leading parent traversal", [{ name: "../escape", data: "x" }]],
    ["an absolute path", [{ name: "/etc/passwd", data: "x" }]],
    ["a backslash path", [{ name: "skills\\a\\x", data: "x" }]],
    ["a drive path", [{ name: "C:/x", data: "x" }]],
    ["a dot segment", [{ name: "skills/./a/x", data: "x" }]],
    ["an empty segment", [{ name: "skills//a/x", data: "x" }]],
    ["a control character", [{ name: "skills/a/x\u0001y", data: "x" }]],
    ["a symlink", [...SKILL("a"), { name: "skills/a/link", type: "2", linkname: "/etc" }]],
    ["a symlink to a parent", [...SKILL("a"), { name: "skills/a/up", type: "2", linkname: ".." }]],
    ["a hard link", [...SKILL("a"), { name: "skills/a/hard", type: "1", linkname: "skills/a/SKILL.md" }]],
    ["a character device", [{ name: "plugins/p/dev", type: "3" }]],
    ["a block device", [{ name: "plugins/p/dev", type: "4" }]],
    ["a fifo", [{ name: "plugins/p/fifo", type: "6" }]],
    ["a sparse file", [{ name: "plugins/p/sparse", type: "S" }]],
    ["a pax global header", [{ name: "pax", type: "g", data: "" }]],
    ["a duplicate file", [...SKILL("a"), { name: "skills/a/SKILL.md", data: "again" }]],
    ["a case-folded duplicate", [...SKILL("a"), { name: "skills/a/skill.md", data: "again" }]],
    ["a duplicate directory", [...SKILL("a"), { name: "skills/a/", type: "5" }]],
    ["a file under a file", [...SKILL("a"), { name: "skills/a/SKILL.md/x", data: "x" }]],
    ["an unknown top-level entry", [{ name: "other/x", data: "x" }]],
    ["a file named like a skill", [{ name: "skills/a", data: "x" }]],
    ["an invalid skill name", [{ name: "skills/Bad_Name/x", data: "x" }]],
    ["an invalid plugin id", [{ name: "plugins/.hidden/x", data: "x" }]],
    ["a path over the length cap", [{ name: `plugins/p/${"d/".repeat(100)}x`, data: "x" }]],
    ["mcp.json that is not an mcpServers object", [{ name: "mcp.json", data: "[]" }]],
    ["mcp.json with an invalid server name", [{ name: "mcp.json", data: '{"mcpServers":{"a b":{}}}' }]],
  ];

  for (const [label, entries] of refused) {
    it(`refuses ${label} and leaves nothing behind`, () => {
      const kit = kitFor();
      expect(() => kit.apply({ hash: HASH_A, uploadId: stage(entries) })).toThrow();
      expect(existsSync(join(kit.root, HASH_A))).toBe(false);
      expect(existsSync(join(kit.root, "current"))).toBe(false);
      expect(existsSync(join(home, ".claude", "skills"))).toBe(false);
      expect(existsSync(join(home, "escape"))).toBe(false);
      expect(kit.current()).toBeUndefined();
    });
  }

  it("honours GNU long names but applies the same length caps", () => {
    const long = (name: string): TarEntry[] => [
      { name: "././@LongLink", type: "L", data: `${name}\0` },
      { name, data: "x" },
    ];
    const kit = kitFor();
    const ok = `plugins/p/${"d".repeat(60)}/${"e".repeat(60)}/file.txt`;
    kit.apply({ hash: HASH_A, uploadId: stage([...PLUGIN("p"), ...long(ok)]) });
    expect(readFileSync(join(kit.root, HASH_A, ok), "utf8")).toBe("x");
    expect(() => kitFor().apply({ hash: HASH_B, uploadId: stage(long(`plugins/p/${"d/".repeat(300)}x`)) })).toThrow(/invalid path/);
    expect(() => kitFor().apply({ hash: HASH_B, uploadId: stage(long(`plugins/p/${"n".repeat(300)}`)) })).toThrow(/invalid path/);
  });

  it("refuses compressed archives", () => {
    const gz = Buffer.concat([Buffer.from([0x1f, 0x8b, 8, 0]), Buffer.alloc(40)]);
    expect(() => kitFor().apply({ hash: HASH_A, uploadId: stage(gz) })).toThrow(/Compressed/);
  });

  it("refuses a truncated archive and a corrupt header", () => {
    const whole = tar(SKILL("a"));
    expect(() => kitFor().apply({ hash: HASH_A, uploadId: stage(whole.subarray(0, 700)) })).toThrow();
    const bad = Buffer.from(whole);
    bad[10] ^= 0xff;
    expect(() => kitFor().apply({ hash: HASH_A, uploadId: stage(bad) })).toThrow(/Corrupt/);
  });

  it("refuses a header whose size runs past the archive", () => {
    const lie = header({ name: "plugins/p/big" }, 10 * 1024 * 1024);
    const archive = Buffer.concat([lie, Buffer.alloc(1024)]);
    expect(() => kitFor().apply({ hash: HASH_A, uploadId: stage(archive) })).toThrow(/size/);
  });

  it("validates the hash and the upload id", () => {
    const kit = kitFor();
    const uploadId = stage(SKILL("a"));
    for (const hash of ["", "abc", "A".repeat(64), "g".repeat(64), `${"a".repeat(63)}\0`, 5, null, "../".repeat(22)])
      expect(() => kit.apply({ hash, uploadId })).toThrow("Invalid kit hash");
    expect(() => kit.apply({ hash: HASH_A, uploadId: "missing" })).toThrow();
  });

  it("enforces the entry count, per-file and total size caps", () => {
    const entries = [...PLUGIN("p"), { name: "plugins/p/b", data: "bb" }, { name: "plugins/p/c", data: "cc" }];
    expect(() => kitFor({ limits: { maxEntries: 3 } }).apply({ hash: HASH_A, uploadId: stage(entries) })).toThrow(/too many entries/);
    expect(() => kitFor({ limits: { maxFileBytes: 5 } }).apply({ hash: HASH_A, uploadId: stage([{ name: "plugins/p/b", data: "123456" }]) })).toThrow(/size/);
    expect(() => kitFor({ limits: { maxTotalBytes: 5 } }).apply({ hash: HASH_A, uploadId: stage([{ name: "plugins/p/a", data: "123" }, { name: "plugins/p/b", data: "456" }]) })).toThrow(/unpacked size cap/);
    expect(() => kitFor({ limits: { maxDepth: 3 } }).apply({ hash: HASH_A, uploadId: stage([{ name: "plugins/p/a/b/c", data: "x" }]) })).toThrow(/too deep/);
    // The real defaults are generous but finite.
    expect(DEFAULT_KIT_LIMITS.maxTotalBytes).toBeLessThanOrEqual(512 * 1024 * 1024);
    expect(DEFAULT_KIT_LIMITS.maxEntries).toBeGreaterThan(0);
  });
});

describe("kit.apply: unpacking and install", () => {
  posix("creates a 0700 kit dir with 0600/0700 files and flips current", () => {
    const kit = kitFor();
    const result = kit.apply({
      hash: HASH_A,
      uploadId: stage([...SKILL("alpha"), ...PLUGIN("p1"), MCP, { name: "kit.json", data: "{}" }]),
    });
    expect(result).toMatchObject({ hash: HASH_A, skills: ["alpha"], plugins: ["p1"], mcp: true });
    const dir = join(kit.root, HASH_A);
    expect(mode(kit.root)).toBe(0o700);
    expect(mode(dir)).toBe(0o700);
    expect(mode(join(dir, "skills", "alpha", "SKILL.md"))).toBe(0o600);
    expect(mode(join(dir, "plugins", "p1", "run.sh"))).toBe(0o700);
    expect(readlinkSync(join(kit.root, "current"))).toBe(HASH_A);
    expect(readFileSync(join(kit.root, "current", "skills", "alpha", "SKILL.md"), "utf8")).toBe("# alpha");
    const link = join(home, ".claude", "skills", "alpha");
    expect(lstatSync(link).isSymbolicLink()).toBe(true);
    expect(readlinkSync(link)).toBe(join(kit.root, "current", "skills", "alpha"));
    expect(JSON.parse(readFileSync(join(kit.root, "managed.json"), "utf8"))).toMatchObject({
      hash: HASH_A,
      skills: ["alpha"],
      plugins: ["p1"],
    });
  });

  it("never reuses or writes through an existing path (O_EXCL)", () => {
    const dir = mkdtempSync(join(tmpdir(), "host-kit-excl-"));
    try {
      writeFileSync(join(dir, "mcp.json"), "{}");
      expect(() => unpackKitTar(join(uploadRoot, stage([{ name: "mcp.json", data: "{}" }])), dir)).toThrow(/EEXIST/);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it("discards a half-written leftover of an earlier attempt and starts fresh", () => {
    const kit = kitFor();
    mkdirSync(join(kit.root, HASH_A), { recursive: true });
    writeFileSync(join(kit.root, HASH_A, "stale"), "old");
    kit.apply({ hash: HASH_A, uploadId: stage(SKILL("a")) });
    expect(existsSync(join(kit.root, HASH_A, "stale"))).toBe(false);
    expect(existsSync(join(kit.root, HASH_A, "skills", "a", "SKILL.md"))).toBe(true);
  });

  it("is idempotent: a second apply changes nothing", () => {
    const kit = kitFor();
    const entries = [...SKILL("a"), ...PLUGIN("p"), MCP];
    const first = kit.apply({ hash: HASH_A, uploadId: stage(entries) });
    const inode = statSync(join(kit.root, HASH_A, "skills", "a", "SKILL.md")).ino;
    const managed = readFileSync(join(kit.root, "managed.json"), "utf8");
    const second = kit.apply({ hash: HASH_A, uploadId: stage(entries) });
    expect(second).toEqual(first);
    expect(statSync(join(kit.root, HASH_A, "skills", "a", "SKILL.md")).ino).toBe(inode);
    expect(readFileSync(join(kit.root, "managed.json"), "utf8")).toBe(managed);
    expect(readdirSync(join(home, ".claude", "skills"))).toEqual(["a"]);
    expect(readdirSync(kit.root).filter((name) => name.startsWith("."))).toEqual([]);
  });

  posix("keeps a real remote skill of the same name (collision)", () => {
    const real = join(home, ".claude", "skills", "alpha");
    mkdirSync(real, { recursive: true });
    writeFileSync(join(real, "SKILL.md"), "remote copy");
    const foreign = join(home, ".claude", "skills", "beta");
    symlinkSync(join(home, "elsewhere"), foreign);
    const kit = kitFor();
    const result = kit.apply({ hash: HASH_A, uploadId: stage([...SKILL("alpha"), ...SKILL("beta"), ...SKILL("gamma")]) });
    expect(result.skipped.sort()).toEqual(["alpha", "beta"]);
    expect(result.linked).toEqual(["gamma"]);
    expect(result.warnings).toHaveLength(2);
    expect(readFileSync(join(real, "SKILL.md"), "utf8")).toBe("remote copy");
    expect(lstatSync(real).isSymbolicLink()).toBe(false);
    expect(readlinkSync(foreign)).toBe(join(home, "elsewhere"));
    // The collision is not recorded as managed, so a later removal leaves it.
    kit.apply({ hash: HASH_B, uploadId: stage([]) });
    expect(readFileSync(join(real, "SKILL.md"), "utf8")).toBe("remote copy");
    expect(readlinkSync(foreign)).toBe(join(home, "elsewhere"));
    expect(existsSync(join(home, ".claude", "skills", "gamma"))).toBe(false);
  });

  posix("removes the managed items an earlier kit created, and only those", () => {
    const kit = kitFor();
    kit.apply({ hash: HASH_A, uploadId: stage([...SKILL("a"), ...SKILL("b"), ...SKILL("c"), ...PLUGIN("p")]) });
    expect(readdirSync(join(home, ".claude", "skills")).sort()).toEqual(["a", "b", "c"]);
    // The user replaced c with a real skill after the kit was applied.
    const c = join(home, ".claude", "skills", "c");
    rmSync(c);
    mkdirSync(c);
    writeFileSync(join(c, "SKILL.md"), "mine");
    const result = kit.apply({ hash: HASH_B, uploadId: stage(SKILL("a")) });
    expect(result.removed.sort()).toEqual(["b"]);
    expect(readdirSync(join(home, ".claude", "skills")).sort()).toEqual(["a", "c"]);
    expect(readFileSync(join(c, "SKILL.md"), "utf8")).toBe("mine");
    expect(existsSync(join(kit.root, HASH_A))).toBe(false);
    expect(readlinkSync(join(kit.root, "current"))).toBe(HASH_B);
    expect(kit.current()?.pluginDirs).toEqual([]);
    expect(JSON.parse(readFileSync(join(kit.root, "managed.json"), "utf8")).skills).toEqual(["a"]);
  });

  it("applies an empty kit", () => {
    const kit = kitFor();
    expect(kit.apply({ hash: HASH_A, uploadId: stage([]) })).toMatchObject({ skills: [], plugins: [], mcp: false });
    expect(kit.current()).toMatchObject({ hash: HASH_A, pluginDirs: [], env: {} });
    expect(kit.current()?.mcpConfig).toBeUndefined();
  });
});

describe("kit.secrets", () => {
  const SECRET = "tok-9f8e7d6c5b4a-very-secret";

  posix("writes secrets.env with mode 0600 through a temp file and rename", () => {
    const kit = kitFor();
    const result = kit.secrets({ values: { MONOPTAH_KIT_CLICKUP_T: SECRET, MONOPTAH_KIT_B: "line1\nline2" } });
    expect(result.count).toBe(2);
    expect(result.hash).toMatch(/^[0-9a-f]{64}$/);
    const file = join(kit.root, "secrets.env");
    expect(mode(file)).toBe(0o600);
    expect(mode(kit.root)).toBe(0o700);
    expect(readdirSync(kit.root).filter((name) => name.startsWith(".tmp-"))).toEqual([]);
    // A rewrite replaces the file wholesale and keeps the mode.
    kit.secrets({ values: { MONOPTAH_KIT_ONLY: "value-1234" } });
    expect(mode(file)).toBe(0o600);
    expect(readFileSync(file, "utf8")).not.toContain(SECRET);
  });

  it("puts secrets in the kit env, never in process.env", () => {
    const before = JSON.stringify(process.env);
    const kit = kitFor();
    kit.apply({ hash: HASH_A, uploadId: stage([...PLUGIN("p"), MCP]) });
    kit.secrets({ values: { MONOPTAH_KIT_CLICKUP_T: SECRET, MONOPTAH_KIT_MULTI: "a\nb" } });
    expect(kit.current()?.env).toEqual({ MONOPTAH_KIT_CLICKUP_T: SECRET, MONOPTAH_KIT_MULTI: "a\nb" });
    expect(process.env.MONOPTAH_KIT_CLICKUP_T).toBeUndefined();
    expect(JSON.stringify(process.env)).toBe(before);
    expect(JSON.stringify(process.env)).not.toContain(SECRET);
    // A fresh manager (a Host restart) reads the same file.
    expect(kitFor().current()?.env.MONOPTAH_KIT_CLICKUP_T).toBe(SECRET);
  });

  it("changes the secrets hash when the file changes", () => {
    const kit = kitFor();
    kit.apply({ hash: HASH_A, uploadId: stage(SKILL("a")) });
    const none = kit.current()!.secretsHash;
    kit.secrets({ values: { MONOPTAH_KIT_X: "value-one" } });
    const one = kit.current()!.secretsHash;
    kit.secrets({ values: { MONOPTAH_KIT_X: "value-two" } });
    const two = kit.current()!.secretsHash;
    expect(new Set([none, one, two]).size).toBe(3);
    kit.secrets({ values: { MONOPTAH_KIT_X: "value-two" } });
    expect(kit.current()!.secretsHash).toBe(two);
  });

  it("validates names and values, and error text never carries a value", () => {
    const kit = kitFor();
    for (const values of [
      { lowercase: SECRET },
      { MONOPTAH_KIT_: SECRET },
      { OTHER_X: SECRET },
      { MONOPTAH_KIT_X: 5 },
      { MONOPTAH_KIT_X: `${SECRET}\0` },
      { MONOPTAH_KIT_X: "x".repeat(70 * 1024) },
      [],
      "string",
    ]) {
      let message = "";
      try {
        kit.secrets({ values });
      } catch (error) {
        message = (error as Error).message;
      }
      expect(message).not.toBe("");
      expect(message).not.toContain(SECRET);
    }
    expect(existsSync(join(kit.root, "secrets.env"))).toBe(false);
  });

  it("scrubs secret values from Host logs and error text", () => {
    const kit = kitFor();
    kit.secrets({ values: { MONOPTAH_KIT_T: SECRET } });
    expect(scrubSecrets(`token=${SECRET}!`)).toBe("token=[redacted]!");
    expect(scrubSecrets(JSON.stringify({ t: SECRET }))).not.toContain(SECRET);
    expect(kit.scrub(`bad ${SECRET}`)).toBe("bad [redacted]");
    const lines: unknown[][] = [];
    const spy = vi.spyOn(console, "error").mockImplementation((...args: unknown[]) => {
      lines.push(args);
    });
    const restore = installLogScrubber();
    try {
      console.error("leaked", SECRET);
      console.error(new Error(`boom ${SECRET}`));
      console.error({ nested: { token: SECRET } });
      expect(JSON.stringify(lines)).not.toContain(SECRET);
      expect(JSON.stringify(lines)).toContain("[redacted]");
    } finally {
      restore();
      spy.mockRestore();
    }
  });

  it("scrubs health output", async () => {
    const run: KitRun = async () => ({ code: 0, stdout: `clickup: failed ${SECRET}\n`, stderr: `warn ${SECRET}` });
    const kit = kitFor({ resolveClaude: async () => "/fake/claude", run });
    kit.secrets({ values: { MONOPTAH_KIT_T: SECRET } });
    const out = await kit.health(home);
    expect(JSON.stringify(out)).not.toContain(SECRET);
    expect(out.stdout).toContain("[redacted]");
  });
});

describe("kit.probe", () => {
  it("reports only the commands found on the search path", () => {
    const bin = join(home, "bin");
    mkdirSync(bin);
    writeFileSync(join(bin, "npx"), "#!/bin/sh\n", { mode: 0o755 });
    const kit = kitFor({ pathDirs: [bin] });
    expect(kit.probe({ commands: ["npx", "uvx", "docker"] })).toEqual({ present: ["npx"] });
  });

  it("validates its input", () => {
    const kit = kitFor({ pathDirs: [] });
    for (const commands of ["npx", ["a b"], ["../npx"], ["x\0"], [5], Array(33).fill("npx")])
      expect(() => kit.probe({ commands })).toThrow();
  });
});

describe("kit.health", () => {
  it("runs `mcp list` with the kit MCP config and the secrets env", async () => {
    const calls: Array<{ args: string[]; cwd: string; env: NodeJS.ProcessEnv }> = [];
    const run: KitRun = async (_cmd, args, options) => {
      calls.push({ args, ...options });
      return { code: 0, stdout: "clickup: x - Connected\n", stderr: "" };
    };
    const kit = kitFor({ resolveClaude: async () => "/fake/claude", run });
    kit.apply({ hash: HASH_A, uploadId: stage([...PLUGIN("p"), MCP]) });
    kit.secrets({ values: { MONOPTAH_KIT_CLICKUP_T: "value-1234" } });
    const out = await kit.health(home);
    expect(out).toEqual({ exitCode: 0, stdout: "clickup: x - Connected\n", stderr: "", kitFlags: true });
    expect(calls[0].args).toEqual(["mcp", "list", "--mcp-config", join(kit.root, "current", "mcp.json")]);
    expect(calls[0].cwd).toBe(home);
    expect(calls[0].env.MONOPTAH_KIT_CLICKUP_T).toBe("value-1234");
    expect(process.env.MONOPTAH_KIT_CLICKUP_T).toBeUndefined();
  });

  it("falls back to a plain `mcp list` when the flag is rejected", async () => {
    const run: KitRun = async (_cmd, args) =>
      args.includes("--mcp-config") ? { code: 1, stdout: "", stderr: "unknown option" } : { code: 0, stdout: "ok", stderr: "" };
    const kit = kitFor({ resolveClaude: async () => "/fake/claude", run });
    kit.apply({ hash: HASH_A, uploadId: stage([MCP]) });
    expect(await kit.health(home)).toMatchObject({ exitCode: 0, stdout: "ok", kitFlags: false });
  });

  it("rejects a bad working directory", async () => {
    const kit = kitFor({ resolveClaude: async () => "/fake/claude" });
    await expect(kit.health(join(home, "missing"))).rejects.toThrow();
    await expect(kit.health("bad\0dir")).rejects.toThrow();
  });
});

describe("kit in the Claude spawn", () => {
  it("gives interactive spawns --plugin-dir and --mcp-config, isolated ones neither", () => {
    const kit = kitFor();
    kit.apply({ hash: HASH_A, uploadId: stage([...PLUGIN("p1"), ...PLUGIN("p2"), MCP]) });
    const current = kit.current()!;
    expect(current.pluginDirs).toEqual([join(kit.root, "current", "plugins", "p1"), join(kit.root, "current", "plugins", "p2")]);
    expect(current.mcpConfig).toBe(join(kit.root, "current", "mcp.json"));
    const interactive = buildClaudeSpawnArgs({ kit: current });
    expect(interactive.filter((arg) => arg === "--plugin-dir")).toHaveLength(2);
    expect(interactive).toEqual(expect.arrayContaining(["--mcp-config", current.mcpConfig!]));
    const isolated = buildClaudeSpawnArgs({ isolated: true, kit: current });
    expect(isolated).not.toContain("--plugin-dir");
    expect(isolated).not.toContain(current.mcpConfig!);
  });

  it("changes the kit hash on a new apply so the process respawns", () => {
    const kit = kitFor();
    kit.apply({ hash: HASH_A, uploadId: stage(SKILL("a")) });
    const first = kit.key();
    kit.apply({ hash: HASH_A, uploadId: stage(SKILL("a")) });
    expect(kit.key()).toBe(first);
    kit.apply({ hash: HASH_B, uploadId: stage(SKILL("b")) });
    expect(kit.key()).not.toBe(first);
    kit.secrets({ values: { MONOPTAH_KIT_X: "value-1234" } });
    const withSecrets = kit.key();
    kit.secrets({ values: { MONOPTAH_KIT_X: "value-5678" } });
    expect(kit.key()).not.toBe(withSecrets);
  });
});
