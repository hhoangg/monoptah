#!/usr/bin/env node
// Fork-local (Monoptah). W-20: keeps web/ free of any reference to the old Tauri app.
//
// Fails when anything under web/ (except web/docs, node_modules, dist, .wrangler and the
// lockfile) points into the repo-root `src/` or `src-tauri/`, carries a malformed
// provenance comment, copies a block from `src/` without one, or reintroduces upstream
// identity (`monocode.` storage keys, `open-monocode`, user-visible "MonoCode").
//
// Usage: node scripts/check-web-isolation.mjs [repoRoot]   (default: the repo this file is in)
// Output: one `path:line: rule: detail` per violation. Exit 1 on any, 0 when clean.
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const SCRIPT_DIR = path.dirname(fileURLToPath(import.meta.url));
const SKIP_DIRS = new Set(["node_modules", "dist", ".wrangler"]);
const SKIP_FILES = new Set(["pnpm-lock.yaml"]);
const CODE_EXT = new Set([".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs", ".mts", ".cts"]);
const SIMILARITY_EXT = new Set([...CODE_EXT, ".css"]);
const YAML_EXT = new Set([".yaml", ".yml"]);
const WINDOW = 12;
const JS_PROVENANCE = /^\/\/ Copied from src\/[^ ]+ at [0-9a-f]{7,40}$/;
const CSS_PROVENANCE = /^\/\* Copied from src\/[^ ]+ at [0-9a-f]{7,40} \*\/$/;
const SINKS = /dangerouslySetInnerHTML|\.innerHTML|outerHTML|insertAdjacentHTML/;

function walk(dir, skipTop, out = []) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    if (entry.isDirectory()) {
      if (SKIP_DIRS.has(entry.name) || skipTop?.has(entry.name)) continue;
      walk(path.join(dir, entry.name), null, out);
    } else if (entry.isFile() && !SKIP_FILES.has(entry.name)) {
      out.push(path.join(dir, entry.name));
    }
  }
  return out;
}

function readText(file) {
  const buf = readFileSync(file);
  if (buf.subarray(0, 8000).includes(0)) return null;
  return buf.toString("utf8");
}

const lineOf = (text, index) => text.slice(0, index).split("\n").length;

/** Candidate relative paths in a file: quoted strings, CSS url(), YAML scalars. */
function relativeCandidates(text, ext) {
  const found = [];
  const quoted = /(["'`])(\.{1,2}(?:\/[^"'`\n]*)?)\1/g;
  for (const m of text.matchAll(quoted)) found.push({ spec: m[2], index: m.index });
  for (const m of text.matchAll(/url\(\s*([^)'"\s]+)\s*\)/g)) {
    if (m[1].startsWith(".")) found.push({ spec: m[1], index: m.index });
  }
  if (YAML_EXT.has(ext)) {
    let offset = 0;
    for (const raw of text.split("\n")) {
      const line = raw.replace(/\s#.*$/, "");
      for (const token of line.split(/[\s'",]+/)) {
        const spec = token.replace(/^[-!]+/, "");
        if (/^\.{1,2}(\/|$)/.test(spec)) found.push({ spec, index: offset });
      }
      offset += raw.length + 1;
    }
  }
  return found;
}

/** Strip a template expression and everything from the first glob character. */
function literalPrefix(spec) {
  let s = spec.split("${")[0].replace(/^!/, "");
  const g = s.search(/[*?[{]/);
  if (g !== -1) s = s.slice(0, g);
  return s;
}

function intoLegacy(resolved, root) {
  for (const name of ["src", "src-tauri"]) {
    const base = path.join(root, name);
    if (resolved === base || resolved.startsWith(base + path.sep)) return name;
  }
  return null;
}

function significantLines(text) {
  const lines = [];
  for (const raw of text.split("\n")) {
    const line = raw.trim().replace(/\s+/g, " ");
    if (!line) continue;
    if (/^[\s{}()[\];,.<>/]*$/.test(line)) continue;
    if (/^import\b/.test(line) || /^(export .*)?\} from /.test(line) || /^export \* from /.test(line)) continue;
    lines.push(line);
  }
  return lines;
}

function windowHashes(lines) {
  const hashes = [];
  for (let i = 0; i + WINDOW <= lines.length; i++) {
    hashes.push(createHash("sha1").update(lines.slice(i, i + WINDOW).join("\n")).digest("hex"));
  }
  return hashes;
}

function buildSrcIndex(srcDir) {
  const index = new Map();
  for (const file of walk(srcDir, null)) {
    if (!SIMILARITY_EXT.has(path.extname(file))) continue;
    const text = readText(file);
    if (text === null) continue;
    for (const h of windowHashes(significantLines(text))) if (!index.has(h)) index.set(h, file);
  }
  return index;
}

function brandingHits(root) {
  const script = path.join(SCRIPT_DIR, "check-fork-branding.sh");
  if (!existsSync(script)) return [];
  const result = spawnSync("bash", [script], {
    env: { ...process.env, BRANDING_ROOT: root },
    encoding: "utf8",
  });
  const hits = [];
  for (const line of (result.stdout ?? "").split("\n")) {
    const m = line.match(/^(web\/[^:]+):(\d+):(.*)$/);
    if (m) hits.push({ file: m[1], line: Number(m[2]), detail: m[3].trim() });
  }
  return hits;
}

export function check(root) {
  const webDir = path.join(root, "web");
  const violations = [];
  const notes = [];
  const add = (file, line, rule, detail) =>
    violations.push(`${path.relative(root, file).split(path.sep).join("/")}:${line}: ${rule}: ${detail}`);
  if (!existsSync(webDir)) return { violations, notes };

  const srcDir = path.join(root, "src");
  const srcIndex = existsSync(srcDir) && statSync(srcDir).isDirectory() ? buildSrcIndex(srcDir) : null;
  if (!srcIndex) notes.push("src/ not present, similarity check skipped");

  for (const file of walk(webDir, new Set(["docs"]))) {
    const text = readText(file);
    if (text === null) continue;
    const ext = path.extname(file);
    const dir = path.dirname(file);

    for (const m of text.matchAll(/src-tauri/g)) {
      add(file, lineOf(text, m.index), "legacy-reference", "literal `src-tauri`");
    }

    for (const { spec, index } of relativeCandidates(text, ext)) {
      const prefix = literalPrefix(spec);
      if (!prefix) continue;
      const hit = intoLegacy(path.resolve(dir, prefix), root);
      if (hit) add(file, lineOf(text, index), "legacy-reference", `\`${spec}\` resolves into repo-root ${hit}/`);
    }

    for (const m of text.matchAll(/(["'`])([^"'`\n]*)\1/g)) {
      const abs = m[2].split("\\").join("/");
      const rootPosix = root.split(path.sep).join("/");
      if (path.isAbsolute(m[2]) || abs.startsWith(rootPosix)) {
        const hit = intoLegacy(path.resolve(m[2]), root);
        if (hit) add(file, lineOf(text, m.index), "legacy-reference", `absolute path into repo-root ${hit}/`);
      }
    }

    let copied = false;
    const lines = text.split("\n");
    lines.forEach((raw, i) => {
      const line = raw.trim().replace(/\r$/, "");
      if (!/Copied from\s+src/i.test(line)) return;
      copied = true;
      if (!JS_PROVENANCE.test(line) && !CSS_PROVENANCE.test(line)) {
        add(file, i + 1, "provenance", `malformed comment, expected \`// Copied from src/<path> at <commit>\`: ${line}`);
      }
    });

    if (copied) {
      lines.forEach((raw, i) => {
        if (SINKS.test(raw) && !/Copied from/.test(raw)) add(file, i + 1, "copied-html-sink", "raw HTML sink in a copied file");
      });
    } else if (srcIndex && SIMILARITY_EXT.has(ext)) {
      for (const h of windowHashes(significantLines(text))) {
        const from = srcIndex.get(h);
        if (from) {
          add(file, 1, "missing-provenance", `shares ${WINDOW}+ lines with ${path.relative(root, from)} but has no "Copied from" comment`);
          break;
        }
      }
    }

    lines.forEach((raw, i) => {
      if (/(["'`])monocode\./.test(raw)) add(file, i + 1, "monocode-key", "`monocode.` storage key, use the `monoptah.` prefix");
      if (/open-monocode/.test(raw)) add(file, i + 1, "open-monocode", "`open-monocode` id must not be carried into web/");
    });
  }

  for (const hit of brandingHits(root)) {
    add(path.join(root, hit.file), hit.line, "branding", hit.detail);
  }
  return { violations, notes };
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const root = path.resolve(process.argv[2] ?? path.join(SCRIPT_DIR, ".."));
  const { violations, notes } = check(root);
  if (violations.length > 0) {
    console.log(violations.join("\n"));
    console.error(`web isolation guard: ${violations.length} violation(s)`);
    process.exit(1);
  }
  console.log(`web isolation guard: OK${notes.length ? ` (${notes.join("; ")})` : ""}`);
}
