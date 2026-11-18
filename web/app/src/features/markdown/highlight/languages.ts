import type { LanguageInput } from "shiki/core";

// The curated language set. Each entry is its own lazily loaded chunk, so a language
// costs bytes only when a reply uses it. Anything else renders as plain text.
type Loader = () => Promise<{ default: LanguageInput }>;

const LOADERS = {
  typescript: () => import("shiki/langs/typescript.mjs"),
  tsx: () => import("shiki/langs/tsx.mjs"),
  javascript: () => import("shiki/langs/javascript.mjs"),
  jsx: () => import("shiki/langs/jsx.mjs"),
  json: () => import("shiki/langs/json.mjs"),
  jsonc: () => import("shiki/langs/jsonc.mjs"),
  shellscript: () => import("shiki/langs/shellscript.mjs"),
  python: () => import("shiki/langs/python.mjs"),
  rust: () => import("shiki/langs/rust.mjs"),
  go: () => import("shiki/langs/go.mjs"),
  css: () => import("shiki/langs/css.mjs"),
  scss: () => import("shiki/langs/scss.mjs"),
  html: () => import("shiki/langs/html.mjs"),
  xml: () => import("shiki/langs/xml.mjs"),
  markdown: () => import("shiki/langs/markdown.mjs"),
  yaml: () => import("shiki/langs/yaml.mjs"),
  toml: () => import("shiki/langs/toml.mjs"),
  diff: () => import("shiki/langs/diff.mjs"),
  sql: () => import("shiki/langs/sql.mjs"),
  kotlin: () => import("shiki/langs/kotlin.mjs"),
  java: () => import("shiki/langs/java.mjs"),
  c: () => import("shiki/langs/c.mjs"),
  dockerfile: () => import("shiki/langs/dockerfile.mjs"),
  ini: () => import("shiki/langs/ini.mjs"),
} satisfies Record<string, Loader>;

export type LanguageId = keyof typeof LOADERS;

const ALIASES: Record<string, LanguageId> = {
  ts: "typescript",
  mts: "typescript",
  cts: "typescript",
  js: "javascript",
  mjs: "javascript",
  cjs: "javascript",
  node: "javascript",
  sh: "shellscript",
  bash: "shellscript",
  shell: "shellscript",
  zsh: "shellscript",
  console: "shellscript",
  py: "python",
  python3: "python",
  rs: "rust",
  golang: "go",
  md: "markdown",
  yml: "yaml",
  patch: "diff",
  kt: "kotlin",
  kts: "kotlin",
  docker: "dockerfile",
  htm: "html",
  svg: "xml",
  jsonl: "json",
  conf: "ini",
  properties: "ini",
};

/** The curated language id for a fence tag, or undefined for plain text. */
export function normalizeLanguage(language: string): LanguageId | undefined {
  const lower = language.trim().toLowerCase();
  if (Object.hasOwn(LOADERS, lower)) return lower as LanguageId;
  return Object.hasOwn(ALIASES, lower) ? ALIASES[lower] : undefined;
}

export function loadLanguage(id: LanguageId): Promise<{ default: LanguageInput }> {
  return LOADERS[id]();
}

export function supportedLanguages(): LanguageId[] {
  return Object.keys(LOADERS) as LanguageId[];
}
