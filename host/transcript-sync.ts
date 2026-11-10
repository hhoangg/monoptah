/**
 * TypeScript mirror of crates/monocode-core/src/cloud_sessions/transcript_sync.rs and of
 * `rewrite_cwd` / `claude_project_dir_name` in convert.rs (design 4.9, rule D10).
 *
 * The same id can exist as two files on two machines. Transcripts are compared by the
 * chain of entry `uuid`s in file order, never by bytes, because the cwd rewrite changes
 * lines in both directions. The tests run this code against the SAME fixture files as the
 * Rust crate (crates/monocode-core/tests/fixtures/transcript_sync/).
 *
 * Only `uuid` and `sessionId` are interpreted; every other field passes through.
 */

export type Decision = "install" | "replace" | "already_current" | "diverged";

/** serde_json refuses to nest deeper than this many arrays/objects. */
const MAX_DEPTH = 127;

class Reject extends Error {}

type Span = { start: number; end: number };
/** What the scanner reports about one JSONL line. */
export interface Probe {
  /** The top-level `uuid`, only when it is a string. */
  uuid?: string;
  /** The top-level `sessionId`, only when it is a string. */
  sessionId?: string;
  /** Where the top-level `sessionId` value sits in the line. */
  sessionIdSpan?: Span;
}

/**
 * Parses one line like serde_json does for the Rust `Probe` struct: the whole line must be
 * one valid JSON object, a repeated top-level `uuid` or `sessionId` key rejects the line,
 * and nothing else is interpreted. Returns null for a line that contributes nothing.
 */
export function probeLine(line: string): Probe | null {
  let i = 0;
  const fail = (): never => {
    throw new Reject();
  };
  const ws = () => {
    while (i < line.length) {
      const c = line.charCodeAt(i);
      if (c === 0x20 || c === 0x09 || c === 0x0a || c === 0x0d) i++;
      else break;
    }
  };
  const hex4 = (at: number): number => {
    const part = line.slice(at, at + 4);
    if (!/^[0-9a-fA-F]{4}$/.test(part)) return fail();
    return parseInt(part, 16);
  };
  /** Scans a string token and returns its span, validating escapes like serde_json. */
  const string = (): Span => {
    const start = i;
    if (line[i] !== '"') return fail();
    i++;
    for (;;) {
      if (i >= line.length) return fail();
      const c = line.charCodeAt(i);
      if (c === 0x22) {
        i++;
        return { start, end: i };
      }
      if (c < 0x20) return fail();
      if (c === 0x5c) {
        const e = line[i + 1];
        if (e === undefined) return fail();
        if ('"\\/bfnrt'.includes(e)) {
          i += 2;
        } else if (e === "u") {
          const unit = hex4(i + 2);
          i += 6;
          if (unit >= 0xd800 && unit <= 0xdbff) {
            if (line[i] !== "\\" || line[i + 1] !== "u") return fail();
            const low = hex4(i + 2);
            if (low < 0xdc00 || low > 0xdfff) return fail();
            i += 6;
          } else if (unit >= 0xdc00 && unit <= 0xdfff) {
            return fail();
          }
        } else {
          return fail();
        }
      } else {
        i++;
      }
    }
  };
  const number = () => {
    const pattern = /-?(?:0|[1-9][0-9]*)(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?/y;
    pattern.lastIndex = i;
    const match = pattern.exec(line);
    if (!match) return fail();
    if (!Number.isFinite(Number(match[0]))) return fail();
    i += match[0].length;
  };
  const literal = (word: string) => {
    if (!line.startsWith(word, i)) return fail();
    i += word.length;
  };
  const value = (depth: number): Span => {
    ws();
    const start = i;
    const c = line[i];
    if (c === '"') string();
    else if (c === "{") {
      if (depth + 1 > MAX_DEPTH) return fail();
      object(depth + 1, false);
    } else if (c === "[") {
      if (depth + 1 > MAX_DEPTH) return fail();
      i++;
      ws();
      if (line[i] === "]") i++;
      else
        for (;;) {
          value(depth + 1);
          ws();
          if (line[i] === ",") i++;
          else if (line[i] === "]") {
            i++;
            break;
          } else return fail();
        }
    } else if (c === "t") literal("true");
    else if (c === "f") literal("false");
    else if (c === "n") literal("null");
    else number();
    return { start, end: i };
  };
  const probe: Probe = {};
  const seen = new Set<string>();
  const object = (depth: number, top: boolean) => {
    i++; // {
    ws();
    if (line[i] === "}") {
      i++;
      return;
    }
    for (;;) {
      ws();
      const key = string();
      ws();
      if (line[i] !== ":") return fail();
      i++;
      const name = top ? (JSON.parse(line.slice(key.start, key.end)) as string) : "";
      const span = value(depth);
      if (top && (name === "uuid" || name === "sessionId")) {
        if (seen.has(name)) return fail();
        seen.add(name);
        if (line[span.start] === '"') {
          const text = JSON.parse(line.slice(span.start, span.end)) as string;
          if (name === "uuid") probe.uuid = text;
          else {
            probe.sessionId = text;
            probe.sessionIdSpan = span;
          }
        }
      }
      ws();
      if (line[i] === ",") i++;
      else if (line[i] === "}") {
        i++;
        return;
      } else return fail();
    }
  };
  try {
    ws();
    if (line[i] !== "{") return null;
    object(1, true);
    ws();
    if (i !== line.length) return null;
    return probe;
  } catch (error) {
    if (error instanceof Reject) return null;
    throw error;
  }
}

const FATAL = new TextDecoder("utf-8", { fatal: true });

/** Lines split on LF, a trailing CR stripped, ASCII-whitespace-only lines dropped. */
function* lines(jsonl: Uint8Array): Generator<string> {
  let from = 0;
  for (let at = 0; at <= jsonl.length; at++) {
    if (at !== jsonl.length && jsonl[at] !== 0x0a) continue;
    let end = at;
    if (end > from && jsonl[end - 1] === 0x0d) end--;
    const bytes = jsonl.subarray(from, end);
    from = at + 1;
    if (bytes.every((b) => b === 0x20 || (b >= 0x09 && b <= 0x0d))) continue;
    try {
      yield FATAL.decode(bytes);
    } catch {
      // serde_json::from_slice rejects invalid UTF-8: the line is not an entry.
    }
  }
}

/** The entry `uuid`s in file order (see the Rust `parse_chain`). */
export function parseChain(jsonl: Uint8Array | string): string[] {
  const bytes = typeof jsonl === "string" ? Buffer.from(jsonl) : jsonl;
  const chain: string[] = [];
  for (const line of lines(bytes)) {
    const probe = probeLine(line);
    if (probe?.uuid !== undefined) chain.push(probe.uuid);
  }
  return chain;
}

/** Compares an incoming transcript with the one already installed under the same id. */
export function decide(
  existing: Uint8Array | string | null | undefined,
  incoming: Uint8Array | string,
): Decision {
  if (existing === null || existing === undefined) return "install";
  const old = parseChain(existing);
  const next = parseChain(incoming);
  if (old.length === next.length && old.every((id, at) => id === next[at]))
    return "already_current";
  if (next.length > old.length && old.every((id, at) => id === next[at]))
    return "replace";
  return "diverged";
}

/**
 * Rewrites every `sessionId` field of the entries to `newId`. Every other byte of a line is
 * kept; a line is only changed in its `sessionId` value.
 */
export function forkTranscript(jsonl: string, newId: string): string {
  let out = "";
  let from = 0;
  while (from < jsonl.length) {
    const nl = jsonl.indexOf("\n", from);
    const end = nl < 0 ? jsonl.length : nl + 1;
    const raw = jsonl.slice(from, end);
    const body = raw.endsWith("\n") ? raw.slice(0, -1) : raw;
    out += forkLine(body, newId) + (raw.endsWith("\n") ? "\n" : "");
    from = end;
  }
  return out;
}

function forkLine(line: string, newId: string): string {
  if (!line.includes('"sessionId"')) return line;
  const probe = probeLine(line);
  if (probe?.sessionId === undefined || !probe.sessionIdSpan) return line;
  if (probe.sessionId === newId) return line;
  const from = `"sessionId":${JSON.stringify(probe.sessionId)}`;
  if (line.includes(from))
    return line.split(from).join(`"sessionId":${JSON.stringify(newId)}`);
  // Spaced JSON: replace just the value, keeping every other byte.
  const { start, end } = probe.sessionIdSpan;
  return line.slice(0, start) + JSON.stringify(newId) + line.slice(end);
}

/** The text shown to the user when a transcript was forked. */
export function forkNotice(oldId: string, newId: string): string {
  return `The cloud and local conversations diverged. The cloud copy continues as a new Claude session (${newId}); the local one (${oldId}) was left untouched.`;
}

const alnum = (c: string) => /^[A-Za-z0-9]$/.test(c);

/**
 * Replaces every path in `from` (matched on boundaries) by `to` in a JSONL text. Longer
 * paths are tried first. A match must not be the tail of a longer path or the head of a
 * longer name (`/workspaces/app-two`, `/workspaces/app.git`).
 */
export function rewriteCwd(text: string, from: string[], to: string): string {
  const inner = (s: string) => JSON.stringify(s).slice(1, -1);
  const target = inner(to);
  const froms = [...new Set(from.filter((f) => f !== "").map(inner))].sort(
    (a, b) => b.length - a.length,
  );
  let current = text;
  for (const needle of froms) {
    let out = "";
    let rest = current;
    let before: string | undefined;
    for (;;) {
      const at = rest.indexOf(needle);
      if (at < 0) break;
      const prefix = rest.slice(0, at);
      const after = rest.slice(at + needle.length);
      const last = prefix.length ? prefix[prefix.length - 1] : before;
      const left = last === undefined || !(alnum(last) || "_-./~".includes(last));
      const first = after[0];
      let right: boolean;
      if (first === undefined) right = true;
      else if (alnum(first) || first === "-" || first === "_") right = false;
      else if (first === ".") {
        const second = after[1];
        right = !(second !== undefined && (alnum(second) || second === "-" || second === "_"));
      } else right = true;
      out += prefix;
      const replaced = left && right;
      const used = replaced ? target : needle;
      out += used;
      before = used.length ? used[used.length - 1] : undefined;
      rest = after;
    }
    current = out + rest;
  }
  return current;
}

/** Claude Code's folder name for a working directory: non-alphanumeric ASCII becomes `-`. */
export function claudeProjectDirName(cwd: string): string {
  return cwd.replace(/[^A-Za-z0-9]/gu, "-");
}

/** Claude Code shortens (and hashes) folder names above this length; we cannot reproduce it. */
export const CLAUDE_NAME_LIMIT = 200;
