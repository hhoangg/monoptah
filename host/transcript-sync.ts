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

/** serde_json refuses to nest deeper than this many arrays/objects in a parsed `Value`. */
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

const FATAL = new TextDecoder("utf-8", { fatal: true });

/**
 * Parses one line like serde_json does for the Rust `Probe` struct. The whole line must be
 * one syntactically valid JSON object, and a repeated top-level `uuid` or `sessionId` key
 * rejects the line. Strictness is limited to what serde interprets: top-level KEY strings
 * and the VALUES of `uuid` / `sessionId` (valid escapes, paired surrogates, valid UTF-8,
 * finite numbers, depth). Every other value only needs correct JSON syntax, so a lone
 * surrogate, invalid UTF-8, `1e400` or deep nesting there does not drop the line. Returns
 * null for a line that contributes nothing.
 *
 * `binary` means the line holds one char per byte (latin1), which is how the file reader
 * keeps invalid UTF-8 visible; strict strings are then UTF-8 checked and decoded.
 */
export function probeLine(line: string, binary = false): Probe | null {
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
  /** Scans a string token; `strict` also checks surrogate pairing and UTF-8. */
  const string = (strict: boolean): Span => {
    const start = i;
    if (line[i] !== '"') return fail();
    i++;
    for (;;) {
      if (i >= line.length) return fail();
      const c = line.charCodeAt(i);
      if (c === 0x22) {
        i++;
        if (strict && binary) {
          try {
            FATAL.decode(Buffer.from(line.slice(start, i), "latin1"));
          } catch {
            return fail();
          }
        }
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
          if (strict && unit >= 0xd800 && unit <= 0xdbff) {
            if (line[i] !== "\\" || line[i + 1] !== "u") return fail();
            const low = hex4(i + 2);
            if (low < 0xdc00 || low > 0xdfff) return fail();
            i += 6;
          } else if (strict && unit >= 0xdc00 && unit <= 0xdfff) {
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
  /** The decoded text of a string token that `string(true)` accepted. */
  const decoded = (span: Span): string => {
    const token = line.slice(span.start, span.end);
    return JSON.parse(binary ? FATAL.decode(Buffer.from(token, "latin1")) : token) as string;
  };
  const number = (strict: boolean) => {
    const pattern = /-?(?:0|[1-9][0-9]*)(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?/y;
    pattern.lastIndex = i;
    const match = pattern.exec(line);
    if (!match) return fail();
    if (strict && !Number.isFinite(Number(match[0]))) return fail();
    i += match[0].length;
  };
  const literal = (word: string) => {
    if (!line.startsWith(word, i)) return fail();
    i += word.length;
  };
  /** A fully checked value, as serde_json parses `Option<Value>`. */
  const strictValue = (depth: number): Span => {
    ws();
    const start = i;
    const c = line[i];
    if (c === '"') string(true);
    else if (c === "{") {
      if (depth + 1 > MAX_DEPTH) return fail();
      i++;
      ws();
      if (line[i] === "}") i++;
      else
        for (;;) {
          ws();
          string(true);
          ws();
          if (line[i] !== ":") return fail();
          i++;
          strictValue(depth + 1);
          ws();
          if (line[i] === ",") i++;
          else if (line[i] === "}") {
            i++;
            break;
          } else return fail();
        }
    } else if (c === "[") {
      if (depth + 1 > MAX_DEPTH) return fail();
      i++;
      ws();
      if (line[i] === "]") i++;
      else
        for (;;) {
          strictValue(depth + 1);
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
    else number(true);
    return { start, end: i };
  };
  /** An ignored value: correct JSON syntax only, no depth limit (iterative on purpose). */
  const skipValue = () => {
    const stack: string[] = [];
    for (;;) {
      ws();
      const c = line[i];
      if (c === "{") {
        i++;
        ws();
        if (line[i] === "}") i++;
        else {
          stack.push("{");
          string(false);
          ws();
          if (line[i] !== ":") return fail();
          i++;
          continue;
        }
      } else if (c === "[") {
        i++;
        ws();
        if (line[i] === "]") i++;
        else {
          stack.push("[");
          continue;
        }
      } else if (c === '"') string(false);
      else if (c === "t") literal("true");
      else if (c === "f") literal("false");
      else if (c === "n") literal("null");
      else number(false);
      // A value is complete: close containers or move to the next member.
      for (;;) {
        const top = stack[stack.length - 1];
        if (top === undefined) return;
        ws();
        if (line[i] === ",") {
          i++;
          if (top === "{") {
            ws();
            string(false);
            ws();
            if (line[i] !== ":") return fail();
            i++;
          }
          break;
        }
        if ((top === "{" && line[i] === "}") || (top === "[" && line[i] === "]")) {
          i++;
          stack.pop();
          continue;
        }
        return fail();
      }
    }
  };
  const probe: Probe = {};
  const seen = new Set<string>();
  const topObject = () => {
    i++; // {
    ws();
    if (line[i] === "}") {
      i++;
      return;
    }
    for (;;) {
      ws();
      const name = decoded(string(true));
      ws();
      if (line[i] !== ":") return fail();
      i++;
      if (name === "uuid" || name === "sessionId") {
        const span = strictValue(1);
        if (seen.has(name)) return fail();
        seen.add(name);
        if (line[span.start] === '"') {
          const text = decoded(span);
          if (name === "uuid") probe.uuid = text;
          else {
            probe.sessionId = text;
            probe.sessionIdSpan = span;
          }
        }
      } else {
        skipValue();
      }
      ws();
      if (line[i] === ",") i++;
      else if (line[i] === "}") {
        i++;
        return;
      } else return fail();
    }
  };
  /** serde reads a struct from a two-element array too: uuid first, sessionId second. */
  const topArray = () => {
    i++; // [
    const values: Span[] = [];
    ws();
    if (line[i] === "]") i++;
    else
      for (;;) {
        values.push(strictValue(1));
        ws();
        if (line[i] === ",") i++;
        else if (line[i] === "]") {
          i++;
          break;
        } else return fail();
      }
    if (values.length !== 2) return fail();
    if (line[values[0].start] === '"') probe.uuid = decoded(values[0]);
    if (line[values[1].start] === '"') probe.sessionId = decoded(values[1]);
  };
  try {
    ws();
    if (line[i] === "[") topArray();
    else if (line[i] === "{") topObject();
    else return null;
    ws();
    if (i !== line.length) return null;
    return probe;
  } catch (error) {
    if (error instanceof Reject) return null;
    throw error;
  }
}

/** Lines split on LF, a trailing CR stripped, ASCII-whitespace-only lines dropped. Each
 * line is returned one char per byte (latin1), so invalid UTF-8 stays visible. */
function* lines(jsonl: Uint8Array): Generator<string> {
  let from = 0;
  for (let at = 0; at <= jsonl.length; at++) {
    if (at !== jsonl.length && jsonl[at] !== 0x0a) continue;
    let end = at;
    if (end > from && jsonl[end - 1] === 0x0d) end--;
    const bytes = jsonl.subarray(from, end);
    from = at + 1;
    if (bytes.every((b) => b === 0x20 || (b >= 0x09 && b <= 0x0d))) continue;
    yield Buffer.from(bytes).toString("latin1");
  }
}

/** The entry `uuid`s in file order (see the Rust `parse_chain`). */
export function parseChain(jsonl: Uint8Array | string): string[] {
  const bytes = typeof jsonl === "string" ? Buffer.from(jsonl) : jsonl;
  const chain: string[] = [];
  for (const line of lines(bytes)) {
    const probe = probeLine(line, true);
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
  if (probe?.sessionId === undefined) return line;
  if (probe.sessionId === newId) return line;
  const from = `"sessionId":${JSON.stringify(probe.sessionId)}`;
  if (line.includes(from))
    return line.split(from).join(`"sessionId":${JSON.stringify(newId)}`);
  if (!probe.sessionIdSpan) return line;
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
