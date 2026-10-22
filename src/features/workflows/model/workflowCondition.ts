// Conditions for If nodes and loops: one comparison over {{path}}, string, number and boolean operands.
import { lookup, type TemplateScope } from "./workflowTemplate";

export type ConditionResult = { ok: true; value: boolean } | { ok: false; error: string };

type Operand = { value: unknown };
type Read = { operand: Operand; end: number } | { error: string };

const PATH_OPERAND = /\{\{\s*([^{}\s]+)\s*\}\}/y;
const NUMBER_OPERAND = /-?\d+(?:\.\d+)?(?=\s|$)/y;
const WORD_OPERAND = /(true|false)(?=\s|$)/y;
const OPERATOR = /(==|!=|contains|not\s+empty|empty)(?=\s|$)/y;

function matchAt(re: RegExp, text: string, pos: number): RegExpExecArray | null {
  re.lastIndex = pos;
  return re.exec(text);
}

function skipSpace(text: string, pos: number): number {
  while (pos < text.length && /\s/.test(text[pos])) pos++;
  return pos;
}

/** Reads a double-quoted string starting at `pos`, honouring backslash escapes. */
function readString(text: string, pos: number): Read {
  let i = pos + 1;
  while (i < text.length && text[i] !== '"') i += text[i] === "\\" ? 2 : 1;
  if (i >= text.length) return { error: "unterminated string" };
  try {
    return { operand: { value: JSON.parse(text.slice(pos, i + 1)) as string }, end: i + 1 };
  } catch {
    return { error: "invalid string escape" };
  }
}

function readOperand(text: string, pos: number, scope: TemplateScope): Read {
  if (text[pos] === '"') return readString(text, pos);
  const path = matchAt(PATH_OPERAND, text, pos);
  if (path) return { operand: { value: lookup(scope, path[1]) }, end: PATH_OPERAND.lastIndex };
  const num = matchAt(NUMBER_OPERAND, text, pos);
  if (num) return { operand: { value: Number(num[0]) }, end: NUMBER_OPERAND.lastIndex };
  const word = matchAt(WORD_OPERAND, text, pos);
  if (word) return { operand: { value: word[1] === "true" }, end: WORD_OPERAND.lastIndex };
  return { error: "expected {{path}}, a double-quoted string, a number, true or false" };
}

function sameValue(a: unknown, b: unknown): boolean {
  if (a !== null && b !== null && typeof a === "object" && typeof b === "object") {
    return JSON.stringify(a) === JSON.stringify(b);
  }
  return a === b;
}

function isEmpty(value: unknown): boolean {
  if (value === undefined || value === null || value === "") return true;
  if (Array.isArray(value)) return value.length === 0;
  if (typeof value === "object") return Object.keys(value).length === 0;
  return false;
}

/** Evaluates a single-comparison condition against `scope`. */
export function evalCondition(condition: string, scope: TemplateScope): ConditionResult {
  const fail = (reason: string): ConditionResult => ({
    ok: false,
    error: `Cannot parse condition "${condition}": ${reason}`,
  });
  const text = condition.trim();
  if (text === "") return fail("the condition is empty");

  const left = readOperand(text, 0, scope);
  if ("error" in left) return fail(left.error);
  let pos = skipSpace(text, left.end);

  const op = matchAt(OPERATOR, text, pos);
  if (!op) return fail("expected ==, !=, contains, empty or not empty");
  const operator = op[1].replace(/\s+/g, " ");
  pos = skipSpace(text, OPERATOR.lastIndex);

  if (operator === "empty" || operator === "not empty") {
    if (pos < text.length) return fail("unexpected text after the operator (and/or are not supported)");
    const empty = isEmpty(left.operand.value);
    return { ok: true, value: operator === "empty" ? empty : !empty };
  }

  const right = readOperand(text, pos, scope);
  if ("error" in right) return fail(right.error);
  if (skipSpace(text, right.end) < text.length) {
    return fail("unexpected text after the second operand (and/or are not supported)");
  }

  const a = left.operand.value;
  const b = right.operand.value;
  switch (operator) {
    case "==":
      return { ok: true, value: sameValue(a, b) };
    case "!=":
      return { ok: true, value: !sameValue(a, b) };
    default: {
      if (a === undefined || a === null) return { ok: true, value: false };
      if (typeof a === "string") {
        if (typeof b !== "string") return fail("contains on a string needs a string on the right");
        return { ok: true, value: a.includes(b) };
      }
      if (Array.isArray(a)) return { ok: true, value: a.some((item) => sameValue(item, b)) };
      return fail("contains needs a string or a list on the left");
    }
  }
}
