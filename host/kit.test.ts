import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  existsSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  readlinkSync,
  renameSync,
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
  pinKitArgs,
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
    it(`refuses ${label} and leaves nothing behind`, async () => {
      const kit = kitFor();
      await expect(kit.apply({ hash: HASH_A, uploadId: stage(entries) })).rejects.toThrow();
      expect(existsSync(join(kit.root, HASH_A))).toBe(false);
      expect(existsSync(join(kit.root, "current"))).toBe(false);
      expect(existsSync(join(home, ".claude", "skills"))).toBe(false);
      expect(existsSync(join(home, "escape"))).toBe(false);
      expect(kit.current()).toBeUndefined();
    });
  }

  it("honours GNU long names but applies the same length caps", async () => {
    const long = (name: string): TarEntry[] => [
      { name: "././@LongLink", type: "L", data: `${name}\0` },
      { name, data: "x" },
    ];
    const kit = kitFor();
    const ok = `plugins/p/${"d".repeat(60)}/${"e".repeat(60)}/file.txt`;
    await kit.apply({ hash: HASH_A, uploadId: stage([...PLUGIN("p"), ...long(ok)]) });
    expect(readFileSync(join(kit.root, HASH_A, ok), "utf8")).toBe("x");
    await expect(kitFor().apply({ hash: HASH_B, uploadId: stage(long(`plugins/p/${"d/".repeat(300)}x`)) })).rejects.toThrow(/invalid path/);
    await expect(kitFor().apply({ hash: HASH_B, uploadId: stage(long(`plugins/p/${"n".repeat(300)}`)) })).rejects.toThrow(/invalid path/);
  });

  it("refuses compressed archives", async () => {
    const gz = Buffer.concat([Buffer.from([0x1f, 0x8b, 8, 0]), Buffer.alloc(40)]);
    await expect(kitFor().apply({ hash: HASH_A, uploadId: stage(gz) })).rejects.toThrow(/Compressed/);
  });

  it("refuses a truncated archive and a corrupt header", async () => {
    const whole = tar(SKILL("a"));
    await expect(kitFor().apply({ hash: HASH_A, uploadId: stage(whole.subarray(0, 700)) })).rejects.toThrow();
    const bad = Buffer.from(whole);
    bad[10] ^= 0xff;
    await expect(kitFor().apply({ hash: HASH_A, uploadId: stage(bad) })).rejects.toThrow(/Corrupt/);
  });

  it("refuses a header whose size runs past the archive", async () => {
    const lie = header({ name: "plugins/p/big" }, 10 * 1024 * 1024);
    const archive = Buffer.concat([lie, Buffer.alloc(1024)]);
    await expect(kitFor().apply({ hash: HASH_A, uploadId: stage(archive) })).rejects.toThrow(/size/);
  });

  it("validates the hash and the upload id", async () => {
    const kit = kitFor();
    const uploadId = stage(SKILL("a"));
    for (const hash of ["", "abc", "A".repeat(64), "g".repeat(64), `${"a".repeat(63)}\0`, 5, null, "../".repeat(22)])
      await expect(kit.apply({ hash, uploadId })).rejects.toThrow("Invalid kit hash");
    await expect(kit.apply({ hash: HASH_A, uploadId: "missing" })).rejects.toThrow();
  });

  it("enforces the entry count, per-file and total size caps", async () => {
    const entries = [...PLUGIN("p"), { name: "plugins/p/b", data: "bb" }, { name: "plugins/p/c", data: "cc" }];
    await expect(kitFor({ limits: { maxEntries: 3 } }).apply({ hash: HASH_A, uploadId: stage(entries) })).rejects.toThrow(/too many entries/);
    await expect(kitFor({ limits: { maxFileBytes: 5 } }).apply({ hash: HASH_A, uploadId: stage([{ name: "plugins/p/b", data: "123456" }]) })).rejects.toThrow(/size/);
    await expect(kitFor({ limits: { maxTotalBytes: 5 } }).apply({ hash: HASH_A, uploadId: stage([{ name: "plugins/p/a", data: "123" }, { name: "plugins/p/b", data: "456" }]) })).rejects.toThrow(/unpacked size cap/);
    await expect(kitFor({ limits: { maxDepth: 3 } }).apply({ hash: HASH_A, uploadId: stage([{ name: "plugins/p/a/b/c", data: "x" }]) })).rejects.toThrow(/too deep/);
    // The real defaults are generous but finite.
    expect(DEFAULT_KIT_LIMITS.maxTotalBytes).toBeLessThanOrEqual(512 * 1024 * 1024);
    expect(DEFAULT_KIT_LIMITS.maxEntries).toBeGreaterThan(0);
  });
});

describe("kit.apply: unpacking and install", () => {
  posix("creates a 0700 kit dir with 0600/0700 files and flips current", async () => {
    const kit = kitFor();
    const result = await kit.apply({
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

  it("never reuses or writes through an existing path (O_EXCL)", async () => {
    const dir = mkdtempSync(join(tmpdir(), "host-kit-excl-"));
    try {
      writeFileSync(join(dir, "mcp.json"), "{}");
      await expect(unpackKitTar(join(uploadRoot, stage([{ name: "mcp.json", data: "{}" }])), dir)).rejects.toThrow(/EEXIST/);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it("discards a half-written leftover of an earlier attempt and starts fresh", async () => {
    const kit = kitFor();
    mkdirSync(join(kit.root, HASH_A), { recursive: true });
    writeFileSync(join(kit.root, HASH_A, "stale"), "old");
    await kit.apply({ hash: HASH_A, uploadId: stage(SKILL("a")) });
    expect(existsSync(join(kit.root, HASH_A, "stale"))).toBe(false);
    expect(existsSync(join(kit.root, HASH_A, "skills", "a", "SKILL.md"))).toBe(true);
  });

  it("is idempotent: a second apply of the same archive changes nothing", async () => {
    const kit = kitFor();
    const entries = [...SKILL("a"), ...PLUGIN("p"), MCP];
    const first = await kit.apply({ hash: HASH_A, uploadId: stage(entries) });
    const files = () => readFileSync(join(kit.root, HASH_A, "mcp.json"), "utf8");
    const before = files();
    const second = await kit.apply({ hash: HASH_A, uploadId: stage(entries) });
    expect(second).toEqual(first);
    expect(files()).toBe(before);
    expect(JSON.parse(readFileSync(join(kit.root, "managed.json"), "utf8"))).toMatchObject({ hash: HASH_A, skills: ["a"] });
    expect(readdirSync(join(home, ".claude", "skills"))).toEqual(["a"]);
    expect(readdirSync(kit.root).filter((name) => name.startsWith("."))).toEqual([]);
    expect(readdirSync(kit.root).filter((name) => /^[0-9a-f]{64}$/.test(name))).toEqual([HASH_A]);
  });

  it("never trusts an existing <hash> dir: a tampered one is replaced from the upload", async () => {
    const kit = kitFor();
    const entries = [...PLUGIN("p"), MCP];
    await kit.apply({ hash: HASH_A, uploadId: stage(entries) });
    const mcp = join(kit.root, HASH_A, "mcp.json");
    const good = readFileSync(mcp, "utf8");
    writeFileSync(mcp, JSON.stringify({ mcpServers: { evil: { command: "sh" } } }));
    writeFileSync(join(kit.root, HASH_A, "plugins", "p", "extra.sh"), "planted");
    await kit.apply({ hash: HASH_A, uploadId: stage(entries) });
    expect(readFileSync(mcp, "utf8")).toBe(good);
    expect(existsSync(join(kit.root, HASH_A, "plugins", "p", "extra.sh"))).toBe(false);
    expect(readdirSync(kit.root).filter((name) => name.startsWith("."))).toEqual([]);
  });

  it("keeps the existing dir when the replacement upload is refused", async () => {
    const kit = kitFor();
    await kit.apply({ hash: HASH_A, uploadId: stage(SKILL("a")) });
    await expect(kit.apply({ hash: HASH_A, uploadId: stage([{ name: "../x", data: "x" }]) })).rejects.toThrow();
    expect(existsSync(join(kit.root, HASH_A, "skills", "a", "SKILL.md"))).toBe(true);
    expect(readdirSync(kit.root).filter((name) => name.startsWith("."))).toEqual([]);
  });

  it("yields to the event loop while unpacking", async () => {
    const kit = kitFor();
    const files: TarEntry[] = Array.from({ length: 30 }, (_, index) => ({ name: `plugins/p/f${index}`, data: "x".repeat(100) }));
    let ticks = 0;
    let running = true;
    const spin = () => {
      ticks++;
      if (running) setImmediate(spin);
    };
    setImmediate(spin);
    await kit.apply({ hash: HASH_A, uploadId: stage([...PLUGIN("p"), ...files]) });
    running = false;
    expect(ticks).toBeGreaterThanOrEqual(30);
  });

  it("uses realistic default caps", () => {
    expect(DEFAULT_KIT_LIMITS.maxTotalBytes).toBe(128 * 1024 * 1024);
    expect(DEFAULT_KIT_LIMITS.maxFileBytes).toBe(64 * 1024 * 1024);
  });

  it("rolls back created skill links and leaves state untouched when a link fails", async () => {
    const kit0 = kitFor();
    await kit0.apply({ hash: HASH_A, uploadId: stage(SKILL("old")) });
    const managed = readFileSync(join(kit0.root, "managed.json"), "utf8");
    let calls = 0;
    const kit = kitFor({
      symlink: ((target: string, path: string, type?: string) => {
        if (++calls === 2) throw new Error("symlink failed");
        symlinkSync(target, path, type as "junction" | undefined);
      }) as typeof symlinkSync,
    });
    await expect(kit.apply({ hash: HASH_B, uploadId: stage([...SKILL("n1"), ...SKILL("n2"), ...SKILL("n3")]) })).rejects.toThrow("symlink failed");
    expect(readdirSync(join(home, ".claude", "skills")).sort()).toEqual(["old"]);
    expect(readFileSync(join(kit.root, "managed.json"), "utf8")).toBe(managed);
    expect(readlinkSync(join(kit.root, "current"))).toBe(HASH_A);
  });

  it("keeps the previous kit dir for racing readers and prunes older ones", async () => {
    const kit = kitFor();
    const H = (c: string) => c.repeat(64);
    await kit.apply({ hash: H("1"), uploadId: stage(PLUGIN("p")) });
    const captured = kit.current()!.pluginDirs[0];
    await kit.apply({ hash: H("2"), uploadId: stage(PLUGIN("q")) });
    // A --plugin-dir captured before the apply still exists.
    expect(existsSync(join(captured, "run.sh"))).toBe(true);
    await kit.apply({ hash: H("3"), uploadId: stage(PLUGIN("q")) });
    expect(existsSync(captured)).toBe(false);
    expect(existsSync(join(kit.root, H("2")))).toBe(true);
  });

  it("never prunes a kit dir a running child still references", async () => {
    const kit = kitFor();
    const H = (c: string) => c.repeat(64);
    await kit.apply({ hash: H("1"), uploadId: stage(PLUGIN("p")) });
    const unpin = pinKitArgs(["--plugin-dir", kit.current()!.pluginDirs[0]]);
    await kit.apply({ hash: H("2"), uploadId: stage(PLUGIN("q")) });
    await kit.apply({ hash: H("3"), uploadId: stage(PLUGIN("q")) });
    expect(existsSync(join(kit.root, H("1"), "plugins", "p", "run.sh"))).toBe(true);
    unpin();
    await kit.apply({ hash: H("4"), uploadId: stage(PLUGIN("q")) });
    expect(existsSync(join(kit.root, H("1")))).toBe(false);
  });

  posix("keeps a real remote skill of the same name (collision)", async () => {
    const real = join(home, ".claude", "skills", "alpha");
    mkdirSync(real, { recursive: true });
    writeFileSync(join(real, "SKILL.md"), "remote copy");
    const foreign = join(home, ".claude", "skills", "beta");
    symlinkSync(join(home, "elsewhere"), foreign);
    const kit = kitFor();
    const result = await kit.apply({ hash: HASH_A, uploadId: stage([...SKILL("alpha"), ...SKILL("beta"), ...SKILL("gamma")]) });
    expect(result.skipped.sort()).toEqual(["alpha", "beta"]);
    expect(result.linked).toEqual(["gamma"]);
    expect(result.warnings).toHaveLength(2);
    expect(readFileSync(join(real, "SKILL.md"), "utf8")).toBe("remote copy");
    expect(lstatSync(real).isSymbolicLink()).toBe(false);
    expect(readlinkSync(foreign)).toBe(join(home, "elsewhere"));
    // The collision is not recorded as managed, so a later removal leaves it.
    await kit.apply({ hash: HASH_B, uploadId: stage([]) });
    expect(readFileSync(join(real, "SKILL.md"), "utf8")).toBe("remote copy");
    expect(readlinkSync(foreign)).toBe(join(home, "elsewhere"));
    expect(existsSync(join(home, ".claude", "skills", "gamma"))).toBe(false);
  });

  posix("removes the managed items an earlier kit created, and only those", async () => {
    const kit = kitFor();
    await kit.apply({ hash: HASH_A, uploadId: stage([...SKILL("a"), ...SKILL("b"), ...SKILL("c"), ...PLUGIN("p")]) });
    expect(readdirSync(join(home, ".claude", "skills")).sort()).toEqual(["a", "b", "c"]);
    // The user replaced c with a real skill after the kit was applied.
    const c = join(home, ".claude", "skills", "c");
    rmSync(c);
    mkdirSync(c);
    writeFileSync(join(c, "SKILL.md"), "mine");
    const result = await kit.apply({ hash: HASH_B, uploadId: stage(SKILL("a")) });
    expect(result.removed.sort()).toEqual(["b"]);
    expect(readdirSync(join(home, ".claude", "skills")).sort()).toEqual(["a", "c"]);
    expect(readFileSync(join(c, "SKILL.md"), "utf8")).toBe("mine");
    expect(readlinkSync(join(kit.root, "current"))).toBe(HASH_B);
    expect(kit.current()?.pluginDirs).toEqual([]);
    expect(JSON.parse(readFileSync(join(kit.root, "managed.json"), "utf8")).skills).toEqual(["a"]);
  });

  it("applies an empty kit", async () => {
    const kit = kitFor();
    expect(await kit.apply({ hash: HASH_A, uploadId: stage([]) })).toMatchObject({ skills: [], plugins: [], mcp: false });
    expect(kit.current()).toMatchObject({ hash: HASH_A, pluginDirs: [], env: {} });
    expect(kit.current()?.mcpConfig).toBeUndefined();
  });
});

describe("startup recovery", () => {
  it("restores a quarantined dir when the crash left no <hash> dir", async () => {
    const kit = kitFor();
    await kit.apply({ hash: HASH_A, uploadId: stage(PLUGIN("p")) });
    // Simulate a crash between "rename to .old" and "rename into place".
    renameSync(join(kit.root, HASH_A), join(kit.root, `.old-${HASH_A}-abc123`));
    mkdirSync(join(kit.root, ".unpack-deadbeef"));
    writeFileSync(join(kit.root, ".unpack-deadbeef", "half"), "x");
    mkdirSync(join(kit.root, `.old-${HASH_B}-abc123`));
    const restarted = kitFor();
    expect(existsSync(join(kit.root, HASH_A, "plugins", "p", "run.sh"))).toBe(true);
    expect(restarted.current()?.hash).toBe(HASH_A);
    expect(readdirSync(kit.root).filter((name) => name.startsWith("."))).toEqual([]);
    expect(existsSync(join(kit.root, HASH_B))).toBe(true);
  });

  it("drops a quarantined copy when the <hash> dir is already there", async () => {
    const kit = kitFor();
    await kit.apply({ hash: HASH_A, uploadId: stage(PLUGIN("p")) });
    mkdirSync(join(kit.root, `.old-${HASH_A}-abc123`));
    writeFileSync(join(kit.root, `.old-${HASH_A}-abc123`, "stale"), "x");
    kitFor();
    expect(readdirSync(kit.root).filter((name) => name.startsWith("."))).toEqual([]);
    expect(existsSync(join(kit.root, HASH_A, "plugins", "p", "run.sh"))).toBe(true);
  });
});

describe("rollback after the flip", () => {
  for (const step of ["flip", "unlink", "manifest"] as const) {
    posix(`restores current, links and manifest when ${step} fails`, async () => {
      const first = kitFor();
      await first.apply({ hash: HASH_A, uploadId: stage([...SKILL("keep"), ...SKILL("drop")]) });
      const managed = readFileSync(join(first.root, "managed.json"), "utf8");
      const before = first.current();
      const kit = kitFor({
        beforeStep: (name) => {
          if (name === step) throw new Error(`${step} failed`);
        },
      });
      await expect(kit.apply({ hash: HASH_B, uploadId: stage([...SKILL("keep"), ...SKILL("added")]) })).rejects.toThrow(`${step} failed`);
      expect(readlinkSync(join(kit.root, "current"))).toBe(HASH_A);
      expect(readdirSync(join(home, ".claude", "skills")).sort()).toEqual(["drop", "keep"]);
      expect(readlinkSync(join(home, ".claude", "skills", "drop"))).toBe(join(kit.root, "current", "skills", "drop"));
      expect(readFileSync(join(kit.root, "managed.json"), "utf8")).toBe(managed);
      expect({ ...kit.current(), lease: undefined }).toEqual({ ...before, lease: undefined });
      expect(kit.current()?.hash).toBe(HASH_A);
    });
  }

  it("rebuilds the cache after a failed apply", async () => {
    const kit = kitFor({
      beforeStep: (name) => {
        if (name === "manifest") throw new Error("boom");
      },
    });
    await expect(kit.apply({ hash: HASH_B, uploadId: stage(SKILL("a")) })).rejects.toThrow("boom");
    expect(kit.current()).toBeUndefined();
  });
});

describe("kit leases", () => {
  it("keeps the leased kit's files across two applies in the snapshot-to-spawn window", async () => {
    const kit = kitFor();
    const H = (c: string) => c.repeat(64);
    await kit.apply({ hash: H("1"), uploadId: stage(PLUGIN("p")) });
    const snapshot = kit.current()!;
    const release = snapshot.lease!();
    await kit.apply({ hash: H("2"), uploadId: stage(PLUGIN("q")) });
    await kit.apply({ hash: H("3"), uploadId: stage(PLUGIN("q")) });
    expect(existsSync(join(snapshot.pluginDirs[0], "run.sh"))).toBe(true);
    release();
    release();
    await kit.apply({ hash: H("4"), uploadId: stage(PLUGIN("q")) });
    expect(existsSync(join(kit.root, H("1")))).toBe(false);
  });
});

describe("kit.secrets", () => {
  const SECRET = "tok-9f8e7d6c5b4a-very-secret";

  posix("writes secrets.env with mode 0600 through a temp file and rename", async () => {
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

  it("puts secrets in the kit env, never in process.env", async () => {
    const before = JSON.stringify(process.env);
    const kit = kitFor();
    await kit.apply({ hash: HASH_A, uploadId: stage([...PLUGIN("p"), MCP]) });
    kit.secrets({ values: { MONOPTAH_KIT_CLICKUP_T: SECRET, MONOPTAH_KIT_MULTI: "a\nb" } });
    expect(kit.current()?.env).toEqual({ MONOPTAH_KIT_CLICKUP_T: SECRET, MONOPTAH_KIT_MULTI: "a\nb" });
    expect(process.env.MONOPTAH_KIT_CLICKUP_T).toBeUndefined();
    expect(JSON.stringify(process.env)).toBe(before);
    expect(JSON.stringify(process.env)).not.toContain(SECRET);
    // A fresh manager (a Host restart) reads the same file.
    expect(kitFor().current()?.env.MONOPTAH_KIT_CLICKUP_T).toBe(SECRET);
  });

  it("changes the secrets hash when the file changes", async () => {
    const kit = kitFor();
    await kit.apply({ hash: HASH_A, uploadId: stage(SKILL("a")) });
    const none = kit.current()!.secretsHash;
    kit.secrets({ values: { MONOPTAH_KIT_X: "value-one" } });
    const one = kit.current()!.secretsHash;
    kit.secrets({ values: { MONOPTAH_KIT_X: "value-two" } });
    const two = kit.current()!.secretsHash;
    expect(new Set([none, one, two]).size).toBe(3);
    kit.secrets({ values: { MONOPTAH_KIT_X: "value-two" } });
    expect(kit.current()!.secretsHash).toBe(two);
  });

  it("validates names and values, and error text never carries a value", async () => {
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

  it("scrubs secret values from Host logs and error text", async () => {
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

  it("scrubs multi-line and URL-encoded forms but not trivial values", async () => {
    const kit = kitFor();
    const pem = "-----BEGIN KEY-----\nMIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSj\nAbCdEfGhIjKlMnOpQrStUvWxYz0123456789==\n-----END KEY-----";
    await kit.secrets({ values: { MONOPTAH_KIT_PEM: pem, MONOPTAH_KIT_SHORT: "abc", MONOPTAH_KIT_URL: "p@ss word/1+2" } });
    expect(scrubSecrets("line: MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSj")).toBe("line: [redacted]");
    expect(scrubSecrets("x AbCdEfGhIjKlMnOpQrStUvWxYz0123456789== y")).toBe("x [redacted] y");
    expect(scrubSecrets(pem)).not.toContain("MIIEv");
    expect(scrubSecrets("-----BEGIN KEY-----")).toBe("-----BEGIN KEY-----");
    expect(scrubSecrets("q=p%40ss%20word%2F1%2B2")).toBe("q=[redacted]");
    expect(scrubSecrets("abc is an ordinary word")).toBe("abc is an ordinary word");
  });

  it("scrubs in one pass, caps line forms and leaves ordinary JSON structure alone", async () => {
    const kit = kitFor();
    const body = Array.from({ length: 900 }, (_, index) => `${String(index).padStart(5, "0")}AbCdEfGhIjKlMnOpQrStUvWxYz0123456789abcdefghijklmnopqrst`);
    const key = ["{", '  "type": "service_account",', '  "client_email": "svc@example.test",', ...body, "}"].join("\n");
    await kit.secrets({ values: { MONOPTAH_KIT_SA: key } });
    expect(scrubSecrets('{\n  "type": "service_account",\n}')).toBe('{\n  "type": "service_account",\n}');
    expect(scrubSecrets("  \"type\": \"service_account\",")).toBe("  \"type\": \"service_account\",");
    // The first 256 qualifying lines are scrubbed individually.
    expect(scrubSecrets(body[0])).toBe("[redacted]");
    expect(scrubSecrets(body[255])).toBe("[redacted]");
    // A large text is scrubbed in a bounded time.
    const text = `${"ordinary log line without secrets\n".repeat(60_000)}${body[3]}\n`;
    const started = performance.now();
    scrubSecrets(text);
    expect(performance.now() - started).toBeLessThan(1500);
    // Overlapping and adjacent hits become one redaction each.
    await kit.secrets({ values: { MONOPTAH_KIT_A: "abcdefgh", MONOPTAH_KIT_B: "defghijk" } });
    expect(scrubSecrets("x abcdefghijk y abcdefgh!")).toBe("x [redacted] y [redacted]!");
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
  it("reports only the commands found on the search path", async () => {
    const bin = join(home, "bin");
    mkdirSync(bin);
    writeFileSync(join(bin, "npx"), "#!/bin/sh\n", { mode: 0o755 });
    const kit = kitFor({ pathDirs: [bin] });
    expect(kit.probe({ commands: ["npx", "uvx", "docker"] })).toEqual({ present: ["npx"] });
  });

  it("validates its input", async () => {
    const kit = kitFor({ pathDirs: [] });
    for (const commands of ["npx", ["a b"], ["../npx"], ["x\0"], [5], Array(33).fill("npx")])
      expect(() => kit.probe({ commands })).toThrow();
  });
});

describe("kit.health", () => {
  it("runs a single `mcp list` against a throwaway config dir holding the kit servers", async () => {
    const calls: Array<{ args: string[]; cwd: string; env: NodeJS.ProcessEnv; config?: unknown; mode?: number }> = [];
    const run: KitRun = async (_cmd, args, options) => {
      const dir = options.env.CLAUDE_CONFIG_DIR!;
      calls.push({
        args,
        ...options,
        config: JSON.parse(readFileSync(join(dir, ".claude.json"), "utf8")),
        mode: statSync(join(dir, ".claude.json")).mode & 0o777,
      });
      return { code: 0, stdout: "clickup: x - Connected\n", stderr: "" };
    };
    const kit = kitFor({ resolveClaude: async () => "/fake/claude", run });
    await kit.apply({ hash: HASH_A, uploadId: stage([...PLUGIN("p"), MCP]) });
    await kit.secrets({ values: { MONOPTAH_KIT_CLICKUP_T: "value-1234" } });
    const out = await kit.health(home);
    expect(out).toMatchObject({ exitCode: 0, stdout: "clickup: x - Connected\n", stderr: "", kitServers: true, timedOut: false });
    expect(out.servers).toEqual([{ name: "clickup", status: "connected", signInNeeded: false, transport: "stdio", detail: "Connected" }]);
    expect(calls).toHaveLength(1);
    expect(calls[0].args).toEqual(["mcp", "list"]);
    expect(calls[0].cwd).toBe(home);
    expect(calls[0].env.MONOPTAH_KIT_CLICKUP_T).toBe("value-1234");
    expect(calls[0].env.CLAUDE_CONFIG_DIR).not.toBe(join(home, ".claude"));
    expect(calls[0].config).toEqual({ mcpServers: JSON.parse(MCP.data).mcpServers });
    if (process.platform !== "win32") expect(calls[0].mode).toBe(0o600);
    expect(process.env.MONOPTAH_KIT_CLICKUP_T).toBeUndefined();
    // The throwaway dir is gone afterwards.
    expect(existsSync(calls[0].env.CLAUDE_CONFIG_DIR!)).toBe(false);
  });

  it("runs the host's own `mcp list` when the kit has no MCP config", async () => {
    const seen: NodeJS.ProcessEnv[] = [];
    const run: KitRun = async (_cmd, _args, options) => {
      seen.push(options.env);
      return { code: 0, stdout: "", stderr: "" };
    };
    const kit = kitFor({ resolveClaude: async () => "/fake/claude", run });
    await kit.apply({ hash: HASH_A, uploadId: stage(SKILL("a")) });
    expect((await kit.health(home)).kitServers).toBe(false);
    expect(seen[0].CLAUDE_CONFIG_DIR).toBe(process.env.CLAUDE_CONFIG_DIR);
  });

  it("builds the real command line: one run, no retry on failure or timeout", async () => {
    // A stand-in `claude` script run by the default runner.
    const script = join(home, "claude.cjs");
    const log = join(home, "runs.log");
    writeFileSync(
      script,
      `const fs = require('node:fs');
fs.appendFileSync(${JSON.stringify(log)}, JSON.stringify({ argv: process.argv.slice(2), cwd: process.cwd(), cfg: fs.readFileSync(process.env.CLAUDE_CONFIG_DIR + '/.claude.json', 'utf8') }) + '\\n');
console.log('clickup: npx x - Connected');
console.error('token ' + process.env.MONOPTAH_KIT_CLICKUP_T);
process.exit(3);
`,
    );
    const kit = kitFor({ resolveClaude: async () => script });
    await kit.apply({ hash: HASH_A, uploadId: stage([MCP]) });
    await kit.secrets({ values: { MONOPTAH_KIT_CLICKUP_T: "real-secret-777" } });
    const out = await kit.health(home);
    expect(out.exitCode).toBe(3);
    expect(out.stdout).toContain("clickup: npx x - Connected");
    expect(out.stderr).toBe("token [redacted]\n");
    const runs = readFileSync(log, "utf8").trim().split("\n").map((line) => JSON.parse(line));
    expect(runs).toHaveLength(1);
    expect(runs[0].argv).toEqual(["mcp", "list"]);
    expect(Object.keys(JSON.parse(runs[0].cfg).mcpServers)).toEqual(["clickup"]);
  });

  it("classifies sign-in outcomes apart from failures and reports each transport", async () => {
    const mcp = {
      name: "mcp.json",
      data: JSON.stringify({
        mcpServers: {
          local: { command: "npx", args: ["x"] },
          linear: { type: "http", url: "https://example.test/mcp" },
          legacy: { type: "sse", url: "https://example.test/sse" },
          cu: { url: "https://example.test/mcp", headers: { Authorization: "Bearer ${MONOPTAH_KIT_CU_AUTH}" } },
          broken: { command: "nope" },
        },
      }),
    };
    const stdout = [
      "Checking MCP server health...",
      "",
      "local: npx x - \u2713 Connected",
      "linear: https://example.test/mcp (HTTP) - \u26a0 Needs authentication",
      "legacy: https://example.test/sse (SSE) - Authentication required",
      "cu: https://example.test/mcp (HTTP) - Pending approval (run `claude` to approve)",
      "broken: nope - \u2718 Failed to connect",
      "not a server line",
    ].join("\n");
    const run: KitRun = async () => ({ code: 0, stdout, stderr: "" });
    const kit = kitFor({ resolveClaude: async () => "/fake/claude", run });
    await kit.apply({ hash: HASH_A, uploadId: stage([mcp]) });
    const { servers } = await kit.health(home);
    const by = Object.fromEntries(servers.map((server) => [server.name, server]));
    expect(by.local).toMatchObject({ status: "connected", signInNeeded: false, transport: "stdio" });
    expect(by.linear).toMatchObject({ status: "signInNeeded", signInNeeded: true, transport: "http" });
    expect(by.legacy).toMatchObject({ status: "signInNeeded", signInNeeded: true, transport: "sse" });
    expect(by.cu).toMatchObject({ status: "signInNeeded", signInNeeded: true, transport: "http" });
    expect(by.broken).toMatchObject({ status: "failed", signInNeeded: false, transport: "stdio" });
    expect(servers).toHaveLength(5);
  });

  it("returns only the status of a line, never its command, arguments or URL", async () => {
    const stdout = [
      "tok: npx server --api-key=sk-live-123456 - \u2718 Failed to connect",
      "url: https://example.test/mcp/abcdef0123456789abcdef (HTTP) - \u2713 Connected",
      "bare: npx sk-live-123456",
    ].join("\n");
    const run: KitRun = async () => ({ code: 0, stdout, stderr: "" });
    const kit = kitFor({ resolveClaude: async () => "/fake/claude", run });
    await kit.apply({ hash: HASH_A, uploadId: stage([MCP]) });
    const { servers } = await kit.health(home);
    const text = JSON.stringify(servers);
    expect(text).not.toContain("sk-live-123456");
    expect(text).not.toContain("abcdef0123456789abcdef");
    const by = Object.fromEntries(servers.map((server) => [server.name, server]));
    expect(by.tok).toMatchObject({ status: "failed", detail: "\u2718 Failed to connect" });
    expect(by.url).toMatchObject({ status: "connected", detail: "\u2713 Connected" });
    expect(by.bare).toMatchObject({ status: "configured", detail: "" });
  });

  posix("returns within the timeout and leaves no process when a descendant holds the pipes", async () => {
    const pidFile = join(home, "grandchild.pid");
    const script = join(home, "claude-hang.cjs");
    writeFileSync(
      script,
      `const { spawn } = require('node:child_process');
const fs = require('node:fs');
// The grandchild inherits stdout and stderr and outlives the leader's patience.
const child = spawn(process.execPath, ['-e', 'setTimeout(() => {}, 25000)'], { stdio: ['ignore', 'inherit', 'inherit'] });
fs.writeFileSync(${JSON.stringify(pidFile)}, String(child.pid));
console.log('local: x - Connected');
setTimeout(() => {}, 25000);
`,
    );
    const kit = kitFor({ resolveClaude: async () => script, healthTimeoutMs: 400 });
    await kit.apply({ hash: HASH_A, uploadId: stage([MCP]) });
    const started = Date.now();
    const out = await kit.health(home);
    expect(Date.now() - started).toBeLessThan(8000);
    expect(out.timedOut).toBe(true);
    expect(out.stdout).toContain("local: x - Connected");
    const grandchild = Number(readFileSync(pidFile, "utf8"));
    await vi.waitFor(() => expect(() => process.kill(grandchild, 0)).toThrow(), { timeout: 5000 });
    // The throwaway config dir was removed even though the run timed out.
    expect(readdirSync(tmpdir()).filter((name) => name.startsWith("monoptah-kit-health-") && existsSync(join(tmpdir(), name, ".claude.json")) && readFileSync(join(tmpdir(), name, ".claude.json"), "utf8").includes("clickup"))).toEqual([]);
  }, 20_000);

  it("rejects a bad working directory", async () => {
    const kit = kitFor({ resolveClaude: async () => "/fake/claude" });
    await expect(kit.health(join(home, "missing"))).rejects.toThrow();
    await expect(kit.health("bad\0dir")).rejects.toThrow();
  });
});

describe("kit in the Claude spawn", () => {
  it("gives interactive spawns --plugin-dir and --mcp-config, isolated ones neither", async () => {
    const kit = kitFor();
    await kit.apply({ hash: HASH_A, uploadId: stage([...PLUGIN("p1"), ...PLUGIN("p2"), MCP]) });
    const current = kit.current()!;
    expect(current.pluginDirs).toEqual([join(kit.root, HASH_A, "plugins", "p1"), join(kit.root, HASH_A, "plugins", "p2")]);
    expect(current.mcpConfig).toBe(join(kit.root, HASH_A, "mcp.json"));
    const interactive = buildClaudeSpawnArgs({ kit: current });
    expect(interactive.filter((arg) => arg === "--plugin-dir")).toHaveLength(2);
    expect(interactive).toEqual(expect.arrayContaining(["--mcp-config", current.mcpConfig!]));
    const isolated = buildClaudeSpawnArgs({ isolated: true, kit: current });
    expect(isolated).not.toContain("--plugin-dir");
    expect(isolated).not.toContain(current.mcpConfig!);
  });

  it("changes the kit hash on a new apply so the process respawns", async () => {
    const kit = kitFor();
    await kit.apply({ hash: HASH_A, uploadId: stage(SKILL("a")) });
    const first = kit.key();
    await kit.apply({ hash: HASH_A, uploadId: stage(SKILL("a")) });
    expect(kit.key()).toBe(first);
    await kit.apply({ hash: HASH_B, uploadId: stage(SKILL("b")) });
    expect(kit.key()).not.toBe(first);
    kit.secrets({ values: { MONOPTAH_KIT_X: "value-1234" } });
    const withSecrets = kit.key();
    kit.secrets({ values: { MONOPTAH_KIT_X: "value-5678" } });
    expect(kit.key()).not.toBe(withSecrets);
  });
});

// ---------------------------------------------------------------------------
// User instructions (CLAUDE.md and what it imports).

const INSTR = (files: Record<string, string>, list = true): TarEntry[] => [
  ...Object.entries(files).map(([rel, data]) => ({ name: `instructions/${rel}`, data })),
  ...(list ? [{ name: "kit.json", data: JSON.stringify({ instructions: Object.keys(files).sort() }) }] : []),
];
const BASIC = { "CLAUDE.md": "@RTK.md\nme", "RTK.md": "rtk", "rules/a.md": "rule a" };
const claudeDir = () => join(home, ".claude");

describe("kit instructions: unpack", () => {
  it("accepts instructions with a matching kit.json list", async () => {
    const kit = kitFor();
    const result = await kit.apply({ hash: HASH_A, uploadId: stage(INSTR(BASIC)) });
    expect(result.instructions).toEqual({ installed: ["CLAUDE.md", "RTK.md", "rules/a.md"], skipped: [] });
    expect(readFileSync(join(kit.root, HASH_A, "instructions", "rules", "a.md"), "utf8")).toBe("rule a");
  });

  const many = Object.fromEntries(Array.from({ length: 65 }, (_, i) => [`n${i}.md`, "x"]));
  const refusals: Array<[string, TarEntry[], RegExp]> = [
    ["a parent traversal", [{ name: "instructions/../x", data: "x" }], /invalid path/],
    ["an absolute path", [{ name: "/instructions/CLAUDE.md", data: "x" }], /invalid path/],
    ["a path deeper than 8", INSTR({ "CLAUDE.md": "x", "a/b/c/d/e/f/g/h/i.md": "x" }), /too deep/],
    ["a .git segment", INSTR({ "CLAUDE.md": "x", ".git/config": "x" }), /invalid path/],
    ["too many files", INSTR({ "CLAUDE.md": "x", ...many }), /too many instruction files/],
    ["a file that is too big", INSTR({ "CLAUDE.md": "x".repeat(256 * 1024 + 1) }), /too large/],
    ["a total that is too big", INSTR(Object.fromEntries(Array.from({ length: 5 }, (_, i) => [i ? `f${i}.md` : "CLAUDE.md", "x".repeat(250 * 1024)]))), /too large/],
    ["a list that does not match", [...INSTR({ "CLAUDE.md": "x" }, false), { name: "kit.json", data: JSON.stringify({ instructions: ["CLAUDE.md", "RTK.md"] }) }], /do not match/],
    ["a list entry with no file", [{ name: "kit.json", data: JSON.stringify({ instructions: ["CLAUDE.md"] }) }], /do not match/],
    ["instructions without CLAUDE.md", INSTR({ "RTK.md": "x" }), /no CLAUDE.md/],
    ["instructions as a file", [{ name: "instructions", data: "x" }], /invalid path/],
  ];
  for (const [label, entries, message] of refusals) {
    it(`refuses ${label} and leaves nothing behind`, async () => {
      const kit = kitFor();
      await expect(kit.apply({ hash: HASH_A, uploadId: stage(entries) })).rejects.toThrow(message);
      expect(existsSync(join(kit.root, HASH_A))).toBe(false);
      expect(existsSync(claudeDir())).toBe(false);
    });
  }
});

describe("kit instructions: install", () => {
  posix("links each file at the hash dir and reads through the link", async () => {
    const kit = kitFor();
    await kit.apply({ hash: HASH_A, uploadId: stage(INSTR(BASIC)) });
    for (const [rel, data] of Object.entries(BASIC)) {
      const link = join(claudeDir(), ...rel.split("/"));
      expect(lstatSync(link).isSymbolicLink()).toBe(true);
      expect(readlinkSync(link)).toBe(join(kit.root, HASH_A, "instructions", ...rel.split("/")));
      expect(readFileSync(link, "utf8")).toBe(data);
    }
    expect(JSON.parse(readFileSync(join(kit.root, "managed.json"), "utf8")).instructions).toEqual(["CLAUDE.md", "RTK.md", "rules/a.md"]);
  });

  posix("never overwrites a file of the user's own and reports it", async () => {
    mkdirSync(claudeDir(), { recursive: true });
    writeFileSync(join(claudeDir(), "CLAUDE.md"), "mine");
    const kit = kitFor();
    const result = await kit.apply({ hash: HASH_A, uploadId: stage(INSTR(BASIC)) });
    expect(readFileSync(join(claudeDir(), "CLAUDE.md"), "utf8")).toBe("mine");
    expect(lstatSync(join(claudeDir(), "CLAUDE.md")).isSymbolicLink()).toBe(false);
    expect(result.instructions.installed).toEqual(["RTK.md", "rules/a.md"]);
    expect(result.instructions.skipped).toEqual([{ path: "CLAUDE.md", reason: "A file of your own is already there" }]);
    expect(JSON.parse(readFileSync(join(kit.root, "managed.json"), "utf8")).instructions).toEqual(["RTK.md", "rules/a.md"]);
  });

  posix("re-points managed links at the new hash and drops only files the kit lost", async () => {
    const kit = kitFor();
    await kit.apply({ hash: HASH_A, uploadId: stage(INSTR(BASIC)) });
    const result = await kit.apply({ hash: HASH_B, uploadId: stage(INSTR({ "CLAUDE.md": "v2", "rules/a.md": "rule a2" })) });
    expect(result.instructions.installed).toEqual(["CLAUDE.md", "rules/a.md"]);
    expect(readlinkSync(join(claudeDir(), "CLAUDE.md"))).toBe(join(kit.root, HASH_B, "instructions", "CLAUDE.md"));
    expect(readFileSync(join(claudeDir(), "CLAUDE.md"), "utf8")).toBe("v2");
    expect(existsSync(join(claudeDir(), "RTK.md"))).toBe(false);
    expect(lstatSync(join(claudeDir(), "rules", "a.md")).isSymbolicLink()).toBe(true);
  });

  posix("removes every managed link when the kit has no instructions, keeping the user's file", async () => {
    mkdirSync(join(claudeDir(), "rules"), { recursive: true });
    writeFileSync(join(claudeDir(), "rules", "a.md"), "mine");
    const kit = kitFor();
    await kit.apply({ hash: HASH_A, uploadId: stage(INSTR(BASIC)) });
    const result = await kit.apply({ hash: HASH_B, uploadId: stage(SKILL("s")) });
    expect(result.instructions).toEqual({ installed: [], skipped: [] });
    expect(existsSync(join(claudeDir(), "CLAUDE.md"))).toBe(false);
    expect(existsSync(join(claudeDir(), "RTK.md"))).toBe(false);
    expect(readFileSync(join(claudeDir(), "rules", "a.md"), "utf8")).toBe("mine");
    expect(JSON.parse(readFileSync(join(kit.root, "managed.json"), "utf8")).instructions).toEqual([]);
  });

  posix("does not remove a link the user replaced after we made it", async () => {
    const kit = kitFor();
    await kit.apply({ hash: HASH_A, uploadId: stage(INSTR(BASIC)) });
    rmSync(join(claudeDir(), "RTK.md"));
    writeFileSync(join(claudeDir(), "RTK.md"), "mine now");
    await kit.apply({ hash: HASH_B, uploadId: stage(INSTR({ "CLAUDE.md": "v2" })) });
    expect(readFileSync(join(claudeDir(), "RTK.md"), "utf8")).toBe("mine now");
  });

  posix("loads an old managed.json that has no instructions field", async () => {
    const kit = kitFor();
    await kit.apply({ hash: HASH_A, uploadId: stage(SKILL("s")) });
    const path = join(kit.root, "managed.json");
    const old = JSON.parse(readFileSync(path, "utf8"));
    delete old.instructions;
    writeFileSync(path, JSON.stringify(old));
    const result = await kitFor().apply({ hash: HASH_B, uploadId: stage([...SKILL("s"), ...INSTR(BASIC)]) });
    expect(result.instructions.installed).toEqual(["CLAUDE.md", "RTK.md", "rules/a.md"]);
  });

  posix("still applies an old kit without instructions", async () => {
    const result = await kitFor().apply({ hash: HASH_A, uploadId: stage([...SKILL("s"), { name: "kit.json", data: "{}" }]) });
    expect(result).toMatchObject({ skills: ["s"], instructions: { installed: [], skipped: [] } });
    expect(existsSync(join(claudeDir(), "CLAUDE.md"))).toBe(false);
  });

  for (const step of ["flip", "unlink", "manifest"] as const) {
    posix(`restores previous instruction links when ${step} fails`, async () => {
      const first = kitFor();
      await first.apply({ hash: HASH_A, uploadId: stage(INSTR(BASIC)) });
      const managed = readFileSync(join(first.root, "managed.json"), "utf8");
      const kit = kitFor({
        beforeStep: (name) => {
          if (name === step) throw new Error(`${step} failed`);
        },
      });
      await expect(kit.apply({ hash: HASH_B, uploadId: stage(INSTR({ "CLAUDE.md": "v2", "notes/new.md": "n" })) })).rejects.toThrow(`${step} failed`);
      for (const rel of Object.keys(BASIC))
        expect(readlinkSync(join(claudeDir(), ...rel.split("/")))).toBe(join(first.root, HASH_A, "instructions", ...rel.split("/")));
      expect(existsSync(join(claudeDir(), "notes", "new.md"))).toBe(false);
      expect(readFileSync(join(first.root, "managed.json"), "utf8")).toBe(managed);
    });
  }
});

describe("kit instructions: hardening", () => {
  it("accepts a path exactly 8 deep and refuses an instructions dir with no kit.json", async () => {
    const deep = "a/b/c/d/e/f/g/h.md";
    const result = await kitFor().apply({ hash: HASH_A, uploadId: stage(INSTR({ "CLAUDE.md": "x", [deep]: "x" })) });
    expect(result.instructions.installed).toContain(deep);
    await expect(
      kitFor().apply({ hash: HASH_B, uploadId: stage(INSTR({ "CLAUDE.md": "x" }, false)) }),
    ).rejects.toThrow(/must list the instructions/);
  });

  posix("unpacks instruction files read-only", async () => {
    const kit = kitFor();
    await kit.apply({ hash: HASH_A, uploadId: stage(INSTR(BASIC)) });
    expect(mode(join(kit.root, HASH_A, "instructions", "CLAUDE.md"))).toBe(0o400);
    // Root ignores file modes, so only the mode bits are checked there.
    if (process.getuid?.() !== 0)
      expect(() => writeFileSync(join(claudeDir(), "CLAUDE.md"), "edit")).toThrow();
  });

  posix("skips Claude's own folders and non-Markdown files", async () => {
    const result = await kitFor().apply({
      hash: HASH_A,
      uploadId: stage(INSTR({ "CLAUDE.md": "x", "skills/foo/SKILL.md": "x", "settings.json": "{}", "NOTES.MD": "n" })),
    });
    expect(result.instructions.installed).toEqual(["CLAUDE.md", "NOTES.MD"]);
    expect(result.instructions.skipped).toEqual([
      { path: "settings.json", reason: "Only Markdown files are copied" },
      { path: "skills/foo/SKILL.md", reason: "Claude's own folder" },
    ]);
    expect(existsSync(join(claudeDir(), "skills", "foo"))).toBe(false);
    expect(existsSync(join(claudeDir(), "settings.json"))).toBe(false);
  });

  posix("skips files under a linked folder and under a regular file", async () => {
    mkdirSync(claudeDir(), { recursive: true });
    const elsewhere = mkdtempSync(join(tmpdir(), "host-kit-else-"));
    symlinkSync(elsewhere, join(claudeDir(), "rules"));
    writeFileSync(join(claudeDir(), "notes"), "i am a file");
    const result = await kitFor().apply({
      hash: HASH_A,
      uploadId: stage(INSTR({ "CLAUDE.md": "x", "rules/a.md": "x", "notes/x.md": "x" })),
    });
    expect(result.instructions.installed).toEqual(["CLAUDE.md"]);
    expect(result.instructions.skipped).toEqual([
      { path: "notes/x.md", reason: "The folder could not be created" },
      { path: "rules/a.md", reason: "The folder is a link" },
    ]);
    expect(readdirSync(elsewhere)).toEqual([]);
    rmSync(elsewhere, { recursive: true, force: true });
  });

  posix("a link that cannot be made is skipped and the apply goes on", async () => {
    const kit = kitFor({
      symlink: ((target: string, path: string, type?: string) => {
        if (path.endsWith("RTK.md")) throw Object.assign(new Error("denied"), { code: "EPERM" });
        symlinkSync(target, path, type as "junction" | undefined);
      }) as typeof symlinkSync,
    });
    const result = await kit.apply({ hash: HASH_A, uploadId: stage(INSTR(BASIC)) });
    expect(result.instructions.installed).toEqual(["CLAUDE.md", "rules/a.md"]);
    expect(result.instructions.skipped).toEqual([{ path: "RTK.md", reason: "The link could not be created" }]);
    expect(readlinkSync(join(kit.root, "current"))).toBe(HASH_A);
  });

  posix("heals an orphan link left by a crash before the manifest was written", async () => {
    const kit = kitFor();
    await kit.apply({ hash: HASH_A, uploadId: stage(INSTR(BASIC)) });
    rmSync(join(kit.root, "managed.json"));
    const result = await kitFor().apply({ hash: HASH_B, uploadId: stage(INSTR(BASIC)) });
    expect(result.instructions).toEqual({ installed: ["CLAUDE.md", "RTK.md", "rules/a.md"], skipped: [] });
    expect(readlinkSync(join(claudeDir(), "CLAUDE.md"))).toBe(join(kit.root, HASH_B, "instructions", "CLAUDE.md"));
  });

  posix("removes only folders it created, and only when empty", async () => {
    const kit = kitFor();
    mkdirSync(join(claudeDir(), "mine"), { recursive: true });
    await kit.apply({ hash: HASH_A, uploadId: stage(INSTR({ "CLAUDE.md": "x", "rules/a.md": "x", "mine/b.md": "x" })) });
    expect(JSON.parse(readFileSync(join(kit.root, "managed.json"), "utf8")).instructionDirs).toEqual(["rules"]);
    writeFileSync(join(claudeDir(), "rules", "own.md"), "mine");
    await kit.apply({ hash: HASH_B, uploadId: stage(INSTR({ "CLAUDE.md": "x" })) });
    expect(existsSync(join(claudeDir(), "rules", "own.md"))).toBe(true);
    expect(existsSync(join(claudeDir(), "mine"))).toBe(true);
    rmSync(join(claudeDir(), "rules", "own.md"));
    await kit.apply({ hash: "c".repeat(64), uploadId: stage(INSTR({ "CLAUDE.md": "x" })) });
    expect(existsSync(join(claudeDir(), "rules"))).toBe(false);
    expect(existsSync(join(claudeDir(), "mine"))).toBe(true);
    expect(JSON.parse(readFileSync(join(kit.root, "managed.json"), "utf8")).instructionDirs).toEqual([]);
  });

  posix("rollback removes folders made by the failed apply and restores pruned ones", async () => {
    const first = kitFor();
    await first.apply({ hash: HASH_A, uploadId: stage(INSTR({ "CLAUDE.md": "x", "rules/a.md": "x" })) });
    const kit = kitFor({
      beforeStep: (name) => {
        if (name === "manifest") throw new Error("manifest failed");
      },
    });
    await expect(kit.apply({ hash: HASH_B, uploadId: stage(INSTR({ "CLAUDE.md": "x", "notes/n.md": "x" })) })).rejects.toThrow("manifest failed");
    expect(existsSync(join(claudeDir(), "notes"))).toBe(false);
    expect(readlinkSync(join(claudeDir(), "rules", "a.md"))).toBe(join(first.root, HASH_A, "instructions", "rules", "a.md"));
  });

  posix("re-applying the same hash is a no-op for the links", async () => {
    const kit = kitFor();
    await kit.apply({ hash: HASH_A, uploadId: stage(INSTR(BASIC)) });
    const managed = JSON.parse(readFileSync(join(kit.root, "managed.json"), "utf8"));
    const again = await kit.apply({ hash: HASH_A, uploadId: stage(INSTR(BASIC)) });
    expect(again.instructions).toEqual({ installed: ["CLAUDE.md", "RTK.md", "rules/a.md"], skipped: [] });
    expect(JSON.parse(readFileSync(join(kit.root, "managed.json"), "utf8")).instructions).toEqual(managed.instructions);
    expect(readFileSync(join(claudeDir(), "rules", "a.md"), "utf8")).toBe("rule a");
  });

  posix("startup removes a leftover temp link but not a file of the user's", async () => {
    const kit = kitFor();
    await kit.apply({ hash: HASH_A, uploadId: stage(INSTR(BASIC)) });
    const temp = join(claudeDir(), "CLAUDE.md.monoptah-0123456789ab");
    symlinkSync(join(kit.root, HASH_A, "instructions", "CLAUDE.md"), temp);
    const own = join(claudeDir(), "x.md.monoptah-0123456789ab");
    writeFileSync(own, "mine");
    kitFor();
    expect(existsSync(temp) || lstatSync(temp, { throwIfNoEntry: false }) !== undefined).toBe(false);
    expect(readFileSync(own, "utf8")).toBe("mine");
  });
});

describe("kit instructions: crash safety", () => {
  posix("a crash after linking leaves a record, and a kit without instructions cleans it up", async () => {
    const kit = kitFor();
    let recorded = "";
    const crashing = kitFor({
      beforeStep: (name) => {
        if (name === "flip") {
          recorded = readFileSync(join(kit.root, "managed.json"), "utf8");
          throw new Error("crash");
        }
      },
    });
    await expect(crashing.apply({ hash: HASH_A, uploadId: stage(INSTR({ "CLAUDE.md": "x", "notes/n.md": "x" })) })).rejects.toThrow("crash");
    // The write-ahead record named the links and the folder before they existed.
    expect(JSON.parse(recorded)).toMatchObject({ instructions: ["CLAUDE.md", "notes/n.md"], instructionDirs: ["notes"] });
    // A failed apply restores the manifest it found (none here) and the links.
    expect(existsSync(join(kit.root, "managed.json"))).toBe(false);
    expect(existsSync(join(claudeDir(), "notes"))).toBe(false);
    // Rebuild the state a real crash would leave: links, folder and record.
    mkdirSync(join(claudeDir(), "notes"), { recursive: true });
    for (const rel of ["CLAUDE.md", "notes/n.md"])
      symlinkSync(join(kit.root, HASH_A, "instructions", ...rel.split("/")), join(claudeDir(), ...rel.split("/")));
    writeFileSync(join(kit.root, "managed.json"), recorded);
    const result = await kitFor().apply({ hash: HASH_B, uploadId: stage(SKILL("s")) });
    expect(result.instructions).toEqual({ installed: [], skipped: [] });
    expect(existsSync(join(claudeDir(), "CLAUDE.md"))).toBe(false);
    expect(existsSync(join(claudeDir(), "notes"))).toBe(false);
  });

  posix("startup also removes a temp link in a folder the user made", async () => {
    const kit = kitFor();
    mkdirSync(join(claudeDir(), "rules"), { recursive: true });
    await kit.apply({ hash: HASH_A, uploadId: stage(INSTR(BASIC)) });
    const temp = join(claudeDir(), "rules", "a.md.monoptah-0123456789ab");
    symlinkSync(join(kit.root, HASH_A, "instructions", "rules", "a.md"), temp);
    kitFor();
    expect(lstatSync(temp, { throwIfNoEntry: false })).toBeUndefined();
    expect(lstatSync(join(claudeDir(), "rules", "a.md")).isSymbolicLink()).toBe(true);
  });
});

describe("kit instructions: write-ahead filter", () => {
  posix("does not record files or folders that linking would skip", async () => {
    const kit = kitFor();
    let recorded = "";
    const crashing = kitFor({
      beforeStep: (name) => {
        if (name === "flip") {
          recorded = readFileSync(join(kit.root, "managed.json"), "utf8");
          throw new Error("crash");
        }
      },
    });
    await expect(
      crashing.apply({ hash: HASH_A, uploadId: stage(INSTR({ "CLAUDE.md": "x", "agents/a.md": "x", "notes/s.json": "{}" })) }),
    ).rejects.toThrow("crash");
    expect(JSON.parse(recorded)).toMatchObject({ instructions: ["CLAUDE.md"], instructionDirs: [] });
  });
});
