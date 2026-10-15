import type { GitIdentity } from "./types";

const escapeChar = (ch: string) => ch.replace(/[\\^$.*+?()[\]{}|]/g, "\\$&");

// Rescoring tests every file and identity against the same few patterns, so
// compiled expressions are memoized. Neither flavor uses the `g` flag, which
// keeps a shared instance stateless.
const CACHE_LIMIT = 500;

function memoized(cache: Map<string, RegExp>, key: string, make: () => RegExp) {
  let re = cache.get(key);
  if (!re) {
    if (cache.size >= CACHE_LIMIT) cache.clear();
    re = make();
    cache.set(key, re);
  }
  return re;
}

const patternCache = new Map<string, RegExp>();
const globCache = new Map<string, RegExp>();

/** Identity pattern: case-insensitive, `*` is the only wildcard, must match the whole string. */
export function matchPattern(pattern: string, value: string): boolean {
  const p = pattern.trim();
  if (!p) return false;
  const re = memoized(patternCache, p, () => {
    const source = p
      .split("*")
      .map((part) => Array.from(part).map(escapeChar).join(""))
      .join("[\\s\\S]*");
    return new RegExp(`^${source}$`, "iu");
  });
  return re.test(value);
}

/**
 * Checks email, name, login and the `gh:<login>` key form. Empty fields are
 * skipped so a bare `*` never matches "nothing".
 */
export function matchesAnyIdentity(
  patterns: string[],
  identity: GitIdentity,
): boolean {
  const login = identity.login?.trim();
  const values = [
    identity.email,
    identity.name,
    login,
    login ? `gh:${login}` : "",
  ].filter((v): v is string => typeof v === "string" && v.trim() !== "");
  return patterns.some((pattern) =>
    values.some((value) => matchPattern(pattern, value)),
  );
}

/**
 * Git-ish glob to RegExp. A glob without `/` matches the basename at any depth;
 * otherwise it is anchored at the repo root. `*` stops at `/`, `**` does not.
 * Character classes like `[abc]` are not supported; brackets match literally.
 */
export function globToRegExp(glob: string): RegExp {
  return memoized(globCache, glob, () => compileGlob(glob));
}

function compileGlob(glob: string): RegExp {
  let g = glob.trim().replace(/\*{3,}/g, "**");
  let anchored = g.includes("/");
  if (g.startsWith("/")) {
    g = g.slice(1);
    anchored = true;
  }
  if (g.endsWith("/")) g += "**";

  let out = "";
  for (let i = 0; i < g.length; i++) {
    const ch = g[i];
    if (ch === "*") {
      if (g[i + 1] === "*") {
        const atSegmentStart = i === 0 || g[i - 1] === "/";
        if (atSegmentStart && g[i + 2] === "/") {
          out += "(?:.*/)?";
          i += 2;
        } else {
          out += ".*";
          i += 1;
        }
      } else {
        out += "[^/]*";
      }
    } else if (ch === "?") {
      out += "[^/]";
    } else {
      out += escapeChar(ch);
    }
  }
  return new RegExp(`^${anchored ? "" : "(?:.*/)?"}${out}$`, "su");
}

const normalizePath = (path: string) =>
  path.replace(/\\/g, "/").replace(/^\.\//, "");

export function matchGlob(glob: string, path: string): boolean {
  if (!glob.trim()) return false;
  return globToRegExp(glob).test(normalizePath(path));
}

export function isIgnoredPath(globs: string[], path: string): boolean {
  const normalized = normalizePath(path);
  return globs.some(
    (glob) => glob.trim() !== "" && globToRegExp(glob).test(normalized),
  );
}
