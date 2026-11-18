// Copied from src/features/files/model/fileName.ts at ea83dd1f
// Copied from src/features/files/ui/FileTypeIcon.tsx at ea83dd1f
// (leafName and its helpers come from the first file; the name -> icon
// resolution is now a pure name -> kind mapping, the Material icon pack and its
// raw SVG strings are gone).
/**
 * Pure file naming helpers, no DOM and no React.
 *
 * API:
 * - `leafName(path)` -> last path segment ("" for an empty path). Accepts / and \.
 * - `pathSegments(path)` -> non-empty segments.
 * - `fileKind(name)` -> a `FileKind` from the file name or extension (compound
 *   suffixes and well-known names such as `Dockerfile` or `package.json` win
 *   over the plain extension). Used by `FileTypeIcon` (`@/features/files`).
 * - `languageId(name)` -> a shiki language id for the name, or `null` when the
 *   language is not known cheaply (render as plain text then).
 */

export type FileKind =
  | "code"
  | "markup"
  | "style"
  | "data"
  | "config"
  | "markdown"
  | "text"
  | "image"
  | "media"
  | "archive"
  | "shell"
  | "lock"
  | "secret"
  | "binary"
  | "file";

export function wellFormedFileName(filename: string): string {
  if (!filename) return filename;
  return filename.replace(/^\t+|\t+$/g, "").replace(/[/\\]+$/, "");
}

export function pathSegments(name: string): string[] {
  return wellFormedFileName(name).split(/[/\\]/).filter(Boolean);
}

export function leafName(raw: string): string {
  const names = pathSegments(raw);
  return names[names.length - 1] ?? "";
}

const KIND_BY_NAME: Record<string, FileKind> = {
  dockerfile: "config",
  makefile: "config",
  "package.json": "config",
  "tsconfig.json": "config",
  "cargo.toml": "config",
  ".gitignore": "config",
  ".gitattributes": "config",
  ".editorconfig": "config",
  ".npmrc": "config",
  ".prettierrc": "config",
  readme: "markdown",
  license: "text",
  "package-lock.json": "lock",
  "pnpm-lock.yaml": "lock",
  "yarn.lock": "lock",
  "cargo.lock": "lock",
  ".env": "secret",
};

const KIND_BY_EXT: Record<string, FileKind> = {};
function addExts(kind: FileKind, exts: string): void {
  for (const ext of exts.split(" ")) KIND_BY_EXT[ext] = kind;
}
addExts("code", "ts tsx js jsx mjs cjs mts cts rs py rb go java kt kts swift c h cc cpp hpp cs php lua dart scala sql vue svelte zig ex exs clj hs");
addExts("markup", "html htm xml svg xhtml");
addExts("style", "css scss sass less");
addExts("data", "json jsonc json5 csv tsv ndjson jsonl");
addExts("config", "toml yaml yml ini conf cfg properties plist");
addExts("markdown", "md mdx markdown");
addExts("text", "txt log rst");
addExts("image", "png jpg jpeg gif webp avif bmp ico icns tiff heic");
addExts("media", "mp3 wav ogg flac m4a mp4 mov webm mkv avi");
addExts("archive", "zip tar gz tgz bz2 xz 7z rar zst");
addExts("shell", "sh bash zsh fish ps1 bat cmd");
addExts("lock", "lock");
addExts("secret", "pem key p12 pfx");
addExts("binary", "exe dll so dylib bin wasm o a class jar pdf ttf otf woff woff2 sqlite db");

const LANG_BY_EXT: Record<string, string> = {
  ts: "typescript", mts: "typescript", cts: "typescript", tsx: "tsx",
  js: "javascript", mjs: "javascript", cjs: "javascript", jsx: "jsx",
  rs: "rust", py: "python", rb: "ruby", go: "go", java: "java", kt: "kotlin", kts: "kotlin",
  swift: "swift", c: "c", h: "c", cc: "cpp", cpp: "cpp", hpp: "cpp", cs: "csharp", php: "php",
  lua: "lua", dart: "dart", scala: "scala", sql: "sql", vue: "vue", svelte: "svelte", zig: "zig",
  ex: "elixir", exs: "elixir", clj: "clojure", hs: "haskell",
  html: "html", htm: "html", xhtml: "html", xml: "xml", svg: "xml",
  css: "css", scss: "scss", sass: "sass", less: "less",
  json: "json", jsonc: "jsonc", json5: "json5", jsonl: "json", ndjson: "json",
  toml: "toml", yaml: "yaml", yml: "yaml", ini: "ini", conf: "ini", properties: "properties",
  md: "markdown", markdown: "markdown", mdx: "mdx",
  sh: "bash", bash: "bash", zsh: "bash", fish: "fish", ps1: "powershell", bat: "bat", cmd: "bat",
  csv: "csv", tsv: "tsv", rst: "rst",
};

const LANG_BY_NAME: Record<string, string> = {
  dockerfile: "docker",
  makefile: "make",
  "cargo.lock": "toml",
  "pnpm-lock.yaml": "yaml",
  ".gitignore": "ini",
  ".env": "ini",
};

/** Dotted suffixes of `name`, longest first: `a.d.ts` -> `["d.ts", "ts"]`. */
function compoundExtensions(fileName: string): string[] {
  const parts = fileName.split(".");
  const start = parts[0] === "" ? 1 : 0;
  const exts: string[] = [];
  for (let i = start + 1; i < parts.length; i++) {
    exts.push(parts.slice(i).join("."));
  }
  return exts;
}

function lookup<T>(table: Record<string, T>, name: string): T | undefined {
  const key = leafName(name).toLowerCase();
  if (Object.hasOwn(table, key)) return table[key];
  for (const ext of compoundExtensions(key)) {
    if (Object.hasOwn(table, ext)) return table[ext];
  }
  return undefined;
}

export function fileKind(name: string): FileKind {
  const key = leafName(name).toLowerCase();
  if (key.startsWith(".env.") && !key.endsWith(".example") && !key.endsWith(".sample")) return "secret";
  return lookup(KIND_BY_NAME, name) ?? lookup(KIND_BY_EXT, name) ?? "file";
}

export function languageId(name: string): string | null {
  const key = leafName(name).toLowerCase();
  if (key.startsWith(".env.")) return "ini";
  return lookup(LANG_BY_NAME, name) ?? lookup(LANG_BY_EXT, name) ?? null;
}
