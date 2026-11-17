// Fork-local (Monoptah). Self-test for scripts/check-web-isolation.mjs (W-20): a clean tree
// passes, each seeded violation fails with its rule name, and a legit `../src` inside a
// package does not.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { after, test } from "node:test";
import { fileURLToPath } from "node:url";

const GUARD = path.join(path.dirname(fileURLToPath(import.meta.url)), "check-web-isolation.mjs");
const made = [];
after(() => {
  for (const dir of made) rmSync(dir, { recursive: true, force: true });
});

const CLEAN = {
  "web/pnpm-workspace.yaml": "packages:\n  - protocol\n  - app\n",
  "web/tsconfig.base.json": '{"compilerOptions":{"strict":true}}\n',
  "web/app/tsconfig.json": '{"extends":"../tsconfig.base.json","include":["src","test"]}\n',
  "web/app/vite.config.ts":
    'import path from "node:path";\nexport default { resolve: { alias: { "@": path.resolve(import.meta.dirname, "src") } } };\n',
  "web/app/src/main.ts": 'export const answer = 42;\n',
  "web/app/styles.css": '@import "tailwindcss";\n@source "./src";\n',
  "web/protocol/src/a.ts": "export const a = 1;\n",
  "web/protocol/test/a.test.ts": 'import { a } from "../src/a";\nexport const b = a;\n',
};

/** Builds a temp repo tree from CLEAN plus overrides (a null value deletes the file). */
function tree(extra = {}) {
  const root = mkdtempSync(path.join(tmpdir(), "web-iso-"));
  made.push(root);
  for (const [rel, content] of Object.entries({ ...CLEAN, ...extra })) {
    if (content === null) continue;
    const file = path.join(root, rel);
    mkdirSync(path.dirname(file), { recursive: true });
    writeFileSync(file, content);
  }
  return root;
}

function run(root) {
  const r = spawnSync("node", [GUARD, root], { encoding: "utf8" });
  return { status: r.status, out: r.stdout + r.stderr };
}

function expectViolation(extra, rule, label = rule) {
  const { status, out } = run(tree(extra));
  assert.equal(status, 1, `${label}: expected exit 1, got ${status}\n${out}`);
  assert.match(out, new RegExp(`^web/[^:]+:\\d+: ${rule}: `, "m"), `${label}: missing rule ${rule}\n${out}`);
}

// A block of 14 distinct significant lines, used as the "copied" body.
const BLOCK = Array.from({ length: 14 }, (_, i) => `const value${i} = compute(${i}, "x${i}");`).join("\n") + "\n";

test("clean tree passes, including the legit ../src inside a package", () => {
  const { status, out } = run(tree());
  assert.equal(status, 0, out);
  assert.match(out, /OK/);
});

test("a tree without web/ passes", () => {
  const root = mkdtempSync(path.join(tmpdir(), "web-iso-"));
  made.push(root);
  assert.equal(run(root).status, 0);
});

test("relative import into repo-root src", () => {
  expectViolation({ "web/app/src/x.ts": 'import { y } from "../../../src/y";\nexport const z = y;\n' }, "legacy-reference");
});

test("dynamic import and require into src", () => {
  expectViolation({ "web/app/src/x.ts": 'export const m = () => import("../../../src/y");\n' }, "legacy-reference", "dynamic import");
  expectViolation({ "web/app/src/x.cjs": 'module.exports = require("../../../src/y");\n' }, "legacy-reference", "require");
});

test("tsconfig paths into src", () => {
  expectViolation(
    { "web/app/tsconfig.json": '{"compilerOptions":{"paths":{"@old/*":["../../src/*"]}}}\n' },
    "legacy-reference",
  );
});

test("tsconfig references and include to ../src-tauri", () => {
  expectViolation({ "web/app/tsconfig.json": '{"references":[{"path":"../../src-tauri"}]}\n' }, "legacy-reference", "references");
  expectViolation({ "web/app/tsconfig.json": '{"include":["../../src/**/*"]}\n' }, "legacy-reference", "include glob");
});

test("tsconfig extends into src", () => {
  expectViolation({ "web/app/tsconfig.json": '{"extends":"../../src/tsconfig.json"}\n' }, "legacy-reference");
});

test("vite config alias to ../../src", () => {
  expectViolation(
    {
      "web/app/vite.config.ts":
        'import path from "node:path";\nexport default { resolve: { alias: { old: path.resolve(import.meta.dirname, "../../src") } } };\n',
    },
    "legacy-reference",
  );
});

test("css @import into src", () => {
  expectViolation({ "web/app/styles.css": '@import "../../src/x.css";\n' }, "legacy-reference", "css import");
  expectViolation({ "web/app/styles.css": "@import url(../../src/x.css);\n" }, "legacy-reference", "css url()");
});

test("tailwind @source into src", () => {
  expectViolation({ "web/app/styles.css": '@source "../../src";\n' }, "legacy-reference");
});

test("pnpm-workspace glob into src", () => {
  expectViolation({ "web/pnpm-workspace.yaml": "packages:\n  - app\n  - ../src\n" }, "legacy-reference", "plain");
  expectViolation({ "web/pnpm-workspace.yaml": 'packages:\n  - "../src/*"\n' }, "legacy-reference", "quoted glob");
});

test("eslint config path into src", () => {
  expectViolation(
    { "web/app/eslint.config.js": 'import base from "../../src/eslint.shared.js";\nexport default [base];\n' },
    "legacy-reference",
  );
});

test("absolute path into repo-root src", () => {
  const root = tree();
  writeFileSync(path.join(root, "web/app/src/abs.ts"), `export const p = ${JSON.stringify(path.join(root, "src", "x"))};\n`);
  const { status, out } = run(root);
  assert.equal(status, 1, out);
  assert.match(out, /legacy-reference: absolute path/);
});

test("literal src-tauri anywhere scanned", () => {
  expectViolation({ "web/app/notes.txt": "see src-tauri/src/lib.rs\n" }, "legacy-reference");
});

test("web/docs is not scanned", () => {
  const { status, out } = run(tree({ "web/docs/a.md": "Reuse src-tauri and ../../src and monocode.x here.\n" }));
  assert.equal(status, 0, out);
});

test("bad provenance comment", () => {
  expectViolation({ "web/app/src/c.ts": "// Copied from src/a/b.ts\nexport const c = 1;\n" }, "provenance", "no commit");
  expectViolation({ "web/app/src/c.ts": "// Copied from src/a/b.ts at HEAD\nexport const c = 1;\n" }, "provenance", "non-hex commit");
});

test("good provenance comments pass (js and css forms)", () => {
  const { status, out } = run(
    tree({
      "web/app/src/c.ts": "// Copied from src/a/b.ts at 32738239\nexport const c = 1;\n",
      "web/app/src/c.css": "/* Copied from src/styles/x.css at 32738239 */\n.a { color: red; }\n",
    }),
  );
  assert.equal(status, 0, out);
});

test("copied block without provenance when src/ has it", () => {
  expectViolation({ "src/lib/orig.ts": BLOCK, "web/app/src/c.ts": "export function f() {\n" + BLOCK + "}\n" }, "missing-provenance");
});

test("copied block with provenance passes; similarity skipped without src/", () => {
  const withComment = "// Copied from src/lib/orig.ts at 32738239\nexport function f() {\n" + BLOCK + "}\n";
  assert.equal(run(tree({ "src/lib/orig.ts": BLOCK, "web/app/src/c.ts": withComment })).status, 0);
  const noSrc = run(tree({ "web/app/src/c.ts": "export function f() {\n" + BLOCK + "}\n" }));
  assert.equal(noSrc.status, 0, noSrc.out);
  assert.match(noSrc.out, /similarity check skipped/);
});

test("a short shared snippet is not a copy", () => {
  const short = Array.from({ length: 5 }, (_, i) => `const v${i} = f(${i});`).join("\n") + "\n";
  assert.equal(run(tree({ "src/lib/orig.ts": short, "web/app/src/c.ts": short })).status, 0);
});

test("copied file with an HTML sink", () => {
  for (const sink of ["el.innerHTML = x;", "el.outerHTML = x;", "el.insertAdjacentHTML('beforeend', x);", "<div dangerouslySetInnerHTML={h} />"]) {
    expectViolation(
      { "web/app/src/c.tsx": `// Copied from src/a/b.tsx at 32738239\nexport const c = () => { ${sink} };\n` },
      "copied-html-sink",
      sink,
    );
  }
});

test("monocode. storage key", () => {
  expectViolation({ "web/app/src/k.ts": 'export const KEY = "monocode.theme";\n' }, "monocode-key");
});

test("open-monocode id", () => {
  expectViolation({ "web/app/src/m.ts": 'export const ID = "open-monocode";\n' }, "open-monocode");
});

test("user-visible MonoCode text", () => {
  expectViolation({ "web/app/src/t.ts": 'export const TITLE = "Welcome to MonoCode";\n' }, "branding");
});

test("identifiers containing MonoCode do not trip branding", () => {
  const { status, out } = run(tree({ "web/app/src/t.ts": "export const MonoCodeMark = 1;\n" }));
  assert.equal(status, 0, out);
});

// --- Wave 1 fixes (review I6, M1, M2, M3, M4, M5) ---

test("symlink into repo-root src is reported", () => {
  const root = tree({ "src/x.ts": "export const x = 1;\n" });
  symlinkSync(path.join(root, "src"), path.join(root, "web/app/src/legacy"));
  const { status, out } = run(root);
  assert.equal(status, 1, out);
  assert.match(out, /^web\/app\/src\/legacy:\d+: legacy-reference: symlink points into repo-root src\//m);
});

test("a symlink that stays inside web/ passes", () => {
  const root = tree();
  symlinkSync(path.join(root, "web/protocol/src"), path.join(root, "web/app/src/shared"));
  const { status, out } = run(root);
  assert.equal(status, 0, out);
});

test("file: dependency on the repo root or src", () => {
  expectViolation({ "web/app/package.json": '{"dependencies":{"monocode-desktop":"file:../.."}}\n' }, "legacy-reference", "file root");
  expectViolation({ "web/app/package.json": '{"devDependencies":{"old":"link:../../src"}}\n' }, "legacy-reference", "link src");
  const { status, out } = run(tree({ "web/app/package.json": '{"dependencies":{"@monoptah/protocol":"workspace:*","local":"file:../protocol"}}\n' }));
  assert.equal(status, 0, out);
});

test("import of the root package monocode-desktop", () => {
  expectViolation({ "web/app/src/p.ts": 'import { x } from "monocode-desktop/src/x";\nexport { x };\n' }, "legacy-reference");
});

test("vitest config seeds", () => {
  expectViolation({ "web/app/vitest.config.ts": 'export default { test: { root: "../../src" } };\n' }, "legacy-reference", "root");
  expectViolation({ "web/app/vitest.config.ts": 'export default { test: { include: ["../../src/**/*.test.ts"] } };\n' }, "legacy-reference", "include");
});

test("tsconfig references that resolve exactly into src", () => {
  expectViolation({ "web/app/tsconfig.json": '{"references":[{"path":"../../src"}]}\n' }, "legacy-reference", "references src");
});

test("paths assembled from literal pieces (best effort)", () => {
  expectViolation({ "web/app/vite.config.ts": 'import path from "node:path";\nexport default { root: path.join(__dirname, "..", "..", "src") };\n' }, "legacy-reference", "join with ..");
  expectViolation({ "web/app/vite.config.ts": 'import path from "node:path";\nconst repoRoot = "/x";\nexport default { root: path.join(repoRoot, "src-tauri") };\n' }, "legacy-reference", "join root-ish");
});

test("legit paths assembled from pieces inside web/ pass", () => {
  const { status, out } = run(
    tree({
      "web/app/vite.config.ts":
        'import path from "node:path";\nconst repoRoot = "/x";\nexport default { a: path.resolve(__dirname, "..", "protocol", "src"), b: path.join(repoRoot, "web", "app", "src"), c: path.join(__dirname, "src") };\n',
    }),
  );
  assert.equal(status, 0, out);
});

test("branding scan covers the manifest and the other source types", () => {
  expectViolation({ "web/app/public/manifest.webmanifest": '{"name": "MonoCode", "short_name": "MonoCode"}\n' }, "branding", "webmanifest");
  expectViolation({ "web/worker/wrangler.jsonc": '{ "name": "MonoCode API" }\n' }, "branding", "jsonc");
  expectViolation({ "web/app/src/a.cjs": 'module.exports = "Welcome to MonoCode";\n' }, "branding", "cjs");
  expectViolation({ "web/app/src/a.jsx": 'export const A = () => "Welcome to MonoCode";\n' }, "branding", "jsx");
  expectViolation({ "web/app/ci.yml": 'name: "MonoCode build"\n' }, "branding", "yml");
  expectViolation({ "web/app/public/logo.svg": '<svg><title>"MonoCode"</title></svg>\n' }, "branding", "svg");
});

test("branding: only web/docs is excluded, not a docs folder deeper down", () => {
  expectViolation({ "web/app/src/features/docs/t.ts": 'export const T = "Welcome to MonoCode";\n' }, "branding", "nested docs");
  const { status, out } = run(tree({ "web/docs/t.ts": 'export const T = "Welcome to MonoCode";\n' }));
  assert.equal(status, 0, out);
});
