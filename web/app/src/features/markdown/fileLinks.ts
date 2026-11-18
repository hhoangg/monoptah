// Adapted from src/features/files/model/markdownFileLinks.ts at ca73ac2b (file reference rules)
// and mobile/src/features/sessions/Markdown.tsx `inlineFileName`. Works on AST text, not on
// a remark tree. A minimal local path helper: swap for `@/lib/paths` when that lands.

export interface FileReference {
  path: string;
  line?: number;
  column?: number;
}

const LINE_SUFFIX = /(?::(\d{1,9})(?::(\d{1,9}))?|#L(\d{1,9})(?:C(\d{1,9}))?(?:-L?\d{1,9}(?:C\d{1,9})?)?)$/;
const KNOWN_NAMES = /^(dockerfile|makefile|gemfile|license|readme)$/i;

function hasControl(text: string): boolean {
  for (let i = 0; i < text.length; i++) {
    if (text.charCodeAt(i) < 32) return true;
  }
  return false;
}

function decode(text: string): string {
  try {
    return decodeURIComponent(text);
  } catch {
    return text;
  }
}

function looksLikeName(name: string): boolean {
  if (!name || !/^[\w%@+().~-]+$/.test(name)) return false;
  if (KNOWN_NAMES.test(name)) return true;
  const dot = name.lastIndexOf(".");
  if (dot <= 0 && !(dot === 0 && name.length > 1)) return false;
  return /^[a-z][a-z0-9+-]{0,11}$/i.test(name.slice(dot + 1));
}

/**
 * A path (optionally with `:line[:column]` or `#L12`) naming a file, or null. Pure text
 * analysis: nothing is read and nothing is opened here.
 */
export function resolveFileReference(target: string): FileReference | null {
  let text = target.trim();
  if (!text || text.length > 1000 || /\s/.test(text) || hasControl(text)) return null;
  if (/^file:\/\//i.test(text)) text = decode(text.slice("file://".length).replace(/^localhost(?=\/)/i, ""));
  else if (/^[a-z][a-z0-9+.-]+:/i.test(text) && !/^[a-z]:[\\/]/i.test(text)) {
    // Another scheme (http:, mailto:, ...) or a bare `name:12` pair; the latter has a line suffix.
    if (!/^[^:/]+:\d+(?::\d+)?$/.test(text)) return null;
  } else text = decode(text);
  if (text.startsWith("//")) return null;
  let line: number | undefined;
  let column: number | undefined;
  const suffix = LINE_SUFFIX.exec(text);
  if (suffix) {
    line = Number(suffix[1] ?? suffix[3]);
    const col = suffix[2] ?? suffix[4];
    if (col) column = Number(col);
    text = text.slice(0, suffix.index);
  }
  const name = text.split(/[/\\]/).filter(Boolean).pop();
  if (!name || !looksLikeName(name)) return null;
  if (text.split(/[/\\]/).includes("..")) return null;
  const result: FileReference = { path: text };
  if (line !== undefined) result.line = line;
  if (column !== undefined) result.column = column;
  return result;
}

/** The file a code span names (one token ending in a plausible file name), or null. */
export function inlineFileReference(value: string): FileReference | null {
  const text = value.trim();
  if (!text || text.length > 240 || /\s/.test(text)) return null;
  if (/^[a-z][a-z0-9+.-]+:\/\//i.test(text)) return null;
  return resolveFileReference(text);
}
