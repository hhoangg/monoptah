// Prompt and command templates for workflows: `{{a.b.c}}` lookups and `{{#if x}}...{{/if}}`.

export type TemplateScope = Record<string, unknown>; // { inputs, run, item?, [nodeId]: output }

const PATH = "[^{}\\s]+";
const IF_BLOCK = new RegExp(`\\{\\{#if\\s+(${PATH})\\s*\\}\\}([\\s\\S]*?)\\{\\{/if\\}\\}`, "g");
const VAR = new RegExp(`\\{\\{\\s*(${PATH})\\s*\\}\\}`, "g");
// Matches either an `#if` opener (capture 1) or a plain variable (capture 2), in text order.
const ANY_VAR = new RegExp(`\\{\\{#if\\s+(${PATH})\\s*\\}\\}|\\{\\{\\s*(${PATH})\\s*\\}\\}`, "g");

/** POSIX single-quote quoting: the value always reaches the shell as one literal word. */
export function shellQuote(value: string): string {
  return `'${value.replace(/'/g, `'\\''`)}'`;
}

/** Every variable path used by the template (including `#if` conditions), once, in order. */
export function templateVars(text: string): string[] {
  const seen = new Set<string>();
  for (const m of text.matchAll(ANY_VAR)) {
    const path = m[1] ?? m[2];
    if (path !== undefined && !path.startsWith("/")) seen.add(path);
  }
  return [...seen];
}

const VAR_AT = new RegExp(`\\{\\{\\s*(${PATH})\\s*\\}\\}`, "y");
const IF_OPEN_AT = new RegExp(`\\{\\{#if\\s+${PATH}\\s*\\}\\}`, "y");

const QUOTE_NAMES: Record<string, string> = {
  "'": "single quotes",
  '"': "double quotes",
};

const UNSAFE_MESSAGE =
  "the command uses shell syntax that cannot be quoted safely around variables; " +
  "move the variable to a simple argument or pass it differently";

/** True when the literal text uses shell syntax this check cannot reason about. */
function usesUnquotableSyntax(text: string): boolean {
  // Backslash right before a variable would escape the quote the app adds.
  if (/\\\{\{/.test(text)) return true;
  // A quote or backslash inside an #if body can change quote state only when it renders.
  for (const m of text.matchAll(IF_BLOCK)) {
    if (/["'`\\]/.test(m[2])) return true;
  }
  // Check the remaining literal text with variables and #if tags removed.
  const literal = text.replace(IF_BLOCK, "$2").replace(VAR, "X").replace(/\{\{[#/]if[^}]*\}\}/g, "");
  if (literal.includes("`") || literal.includes("$(") || literal.includes("$'")) return true;
  if (literal.includes("<<")) return true;
  // An unquoted-looking `#` at the start of a word may start a comment that swallows quotes.
  return /(^|[\s;&|(])#/.test(literal);
}

/**
 * Problems with using `text` as a shell command template: one message per `{{var}}` that sits
 * inside quotes in the literal text. The app quotes every value itself, so a variable written
 * inside quotes would be wrapped twice and its value could break out of the author's quotes.
 *
 * Deliberately conservative: this is a simple quote tracker, not a shell parser. A variable is
 * accepted only in a clearly bare context. If the text uses syntax the tracker cannot follow
 * (backticks, `$(`, `$'`, heredocs, comments, a backslash before a variable, quotes inside an
 * #if body), every variable is reported instead.
 */
export function shellTemplateProblems(text: string): string[] {
  if (usesUnquotableSyntax(text)) {
    return [...text.matchAll(VAR)]
      .filter((m) => !m[1].startsWith("/"))
      .map((m) => `{{${m[1]}}}: ${UNSAFE_MESSAGE}`);
  }
  const problems: string[] = [];
  let quote: string | null = null;
  let i = 0;
  while (i < text.length) {
    if (text.startsWith("{{", i)) {
      IF_OPEN_AT.lastIndex = i;
      if (IF_OPEN_AT.test(text)) {
        i = IF_OPEN_AT.lastIndex;
        continue;
      }
      VAR_AT.lastIndex = i;
      const m = VAR_AT.exec(text);
      if (m && !m[1].startsWith("/")) {
        if (quote) {
          problems.push(
            `{{${m[1]}}} is inside ${QUOTE_NAMES[quote]}; write it bare, the app quotes it for the shell`,
          );
        }
        i = VAR_AT.lastIndex;
        continue;
      }
    }
    const ch = text[i];
    if (ch === "\\" && quote !== "'") {
      // Never let the escape consume the start of a `{{` token.
      i += text.startsWith("{{", i + 1) ? 1 : 2;
      continue;
    }
    if (quote === null) {
      if (ch === "'" || ch === '"') quote = ch;
    } else if (ch === quote) {
      quote = null;
    }
    i++;
  }
  return problems;
}

function lookup(scope: TemplateScope, path: string): unknown {
  let cur: unknown = scope;
  for (const key of path.split(".")) {
    if (cur === null || typeof cur !== "object" || !Object.prototype.hasOwnProperty.call(cur, key)) return undefined;
    cur = (cur as Record<string, unknown>)[key];
  }
  return cur;
}

function stringify(value: unknown): string {
  if (value === undefined || value === null) return "";
  if (typeof value === "string") return value;
  if (typeof value === "number" || typeof value === "boolean") return String(value);
  // JSON.stringify yields undefined for functions and symbols.
  return JSON.stringify(value, null, 2) ?? "";
}

function isTruthy(value: unknown): boolean {
  if (Array.isArray(value)) return value.length > 0;
  return Boolean(value);
}

/**
 * Renders `text` against `scope`. Missing values render as an empty string. With
 * `shell: true` every interpolated value is single-quoted; the template's own text is not.
 */
export function renderTemplate(
  text: string,
  scope: TemplateScope,
  opts?: { shell?: boolean },
): string {
  const withoutIfs = text.replace(IF_BLOCK, (_m, path: string, body: string) =>
    isTruthy(lookup(scope, path)) ? body : "",
  );
  return withoutIfs.replace(VAR, (_m, path: string) => {
    const rendered = stringify(lookup(scope, path));
    return opts?.shell ? shellQuote(rendered) : rendered;
  });
}
