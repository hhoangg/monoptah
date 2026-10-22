// Prompt and command templates for workflows: `{{a.b.c}}` lookups and `{{#if x}}...{{/if}}`.

export type TemplateScope = Record<string, unknown>; // { inputs, run, item?, [nodeId]: output }

const PATH = "[^{}\\s]+";
const IF_BLOCK = new RegExp(`\\{\\{#if\\s+(${PATH})\\s*\\}\\}([\\s\\S]*?)\\{\\{/if\\}\\}`, "g");
const VAR = new RegExp(`\\{\\{\\s*(${PATH})\\s*\\}\\}`, "g");
// Matches either an `#if` opener (capture 1) or a plain variable (capture 2), in text order.
const ANY_VAR = new RegExp(`\\{\\{#if\\s+(${PATH})\\s*\\}\\}|\\{\\{\\s*(${PATH})\\s*\\}\\}`, "g");

/**
 * POSIX single-quote quoting: the value always reaches the shell as one literal word.
 * Quoting only protects against word splitting, expansion and operators. A value that a
 * command later feeds to eval, bash -c, (( )), [[ ]], let, declare -i or an array subscript is
 * still interpreted by the shell, so command authors must not pass variables to those; templates
 * that use the syntax forms of these are rejected (see shellTemplateProblems). The same holds
 * for declare -a / typeset -a (a quoted '(...)' value is re-parsed as a compound assignment) and
 * for zsh glob qualifiers such as `ls /tmp({{x}})` (an `e:cmd:` qualifier runs even when the
 * value was quoted). Quoting also does not stop a value that starts with `-` from being read as
 * an option; shellDashProblems covers that.
 */
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

const QUOTE_NAMES: Record<string, string> = {
  "'": "single quotes",
  '"': "double quotes",
};

const UNSAFE_MESSAGE =
  "the command uses shell syntax that cannot be quoted safely around variables; " +
  "move the variable to a simple argument or pass it differently";

const IF_MESSAGE = "conditionals ({{#if}}) are not supported in command templates";

// `let`, `declare`, `typeset` and `eval` as whole words, not inside `evaluate`, `outlet` or `eval-all`.
const RE_PARSING_WORD = /(^|[^\w./-])(let|declare|typeset|eval)(?![\w./-])/;

/** True when the literal text uses shell syntax this check cannot reason about. */
function usesUnquotableSyntax(text: string): boolean {
  // A backslash would escape the quote the app adds; a `$` would turn it into `$'...'`.
  if (/\\\{\{/.test(text)) return true;
  if (/\$\s*\{\{/.test(text)) return true;
  // Bash deletes backslash-newline before tokenising, which can join `$` and `'`, `<` and `<`, etc.
  if (/\\\r?\n/.test(text)) return true;
  // Parameter expansion nests its own quoting (`"${y:-"}"}"`), which the tracker does not follow.
  if (text.includes("${")) return true;
  const literal = text.replace(VAR, "X");
  if (literal.includes("`") || literal.includes("$(") || literal.includes("$'")) return true;
  if (literal.includes("$[") || literal.includes("<<")) return true;
  // zsh and ksh do not treat quotes inside (( )) as quotes, which desyncs the tracker.
  if (literal.includes("((")) return true;
  // These builtins and operators parse a value a second time, even one that was quoted:
  // `[[ 'a[$(cmd)]' -eq 0 ]]` and `let`/`declare -i` evaluate it as arithmetic, `eval` runs it,
  // and a zsh glob qualifier (`*(e:cmd:)`) runs it.
  if (literal.includes("[[") || literal.includes("*(") || literal.includes("?(")) return true;
  if (RE_PARSING_WORD.test(literal)) return true;
  // A `#` at the start of a word starts a comment that can swallow quotes.
  return /(^|[\s;&|()])#/.test(literal);
}

/**
 * Problems with using `text` as a shell command template, as readable messages.
 *
 * Covers quoting context only. The app single-quotes every value, so a variable written
 * inside the author's own quotes would be wrapped twice and could break out of them. Quoting
 * does not help when the shell parses a value a second time, so a template that uses any of
 * these is rejected for every variable: `eval`, `(( ))`, `[[ ]]`, `let`, `declare`, `typeset` and
 * zsh glob qualifiers such as `ls /tmp/*(e:cmd:)` (`*(` and `?(`). `bash -c` and array
 * subscripts cannot be told apart from ordinary arguments, so command authors must not feed
 * variables to those. A value that starts with `-` is checked separately, against its rendered
 * value (see shellDashProblems).
 *
 * Deliberately conservative: this is a simple quote tracker, not a shell parser. A variable is
 * accepted only in a clearly bare context. If the text uses syntax the tracker cannot follow
 * (backticks, `$(`, `$'`, `$[`, `${`, `((`, `[[`, heredocs, comments, line continuations, `$` or a
 * backslash before a variable), every variable is reported instead. Conditionals are rejected outright: whether an #if body
 * renders changes the surrounding quote state, so no static check can cover both branches.
 */
export function shellTemplateProblems(text: string): string[] {
  const everyVar = (message: string) =>
    [...text.matchAll(VAR)].filter((m) => !m[1].startsWith("/")).map((m) => `{{${m[1]}}}: ${message}`);
  if (text.includes("{{#if") || text.includes("{{/if}}")) {
    return [IF_MESSAGE, ...everyVar(IF_MESSAGE)];
  }
  if (usesUnquotableSyntax(text)) return everyVar(UNSAFE_MESSAGE);

  const problems: string[] = [];
  let quote: string | null = null;
  let i = 0;
  while (i < text.length) {
    if (text.startsWith("{{", i)) {
      VAR_AT.lastIndex = i;
      const m = VAR_AT.exec(text);
      if (m) {
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

/** Own-property path lookup (`a.b.c`); never reads the prototype chain. */
export function lookup(scope: TemplateScope, path: string): unknown {
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

/**
 * Problems with rendered values that start with `-`: a command that gets such a value as a word
 * of its own reads it as an option (`ssh {{host}}` with `-oProxyCommand=...`). A value is fine
 * after a standalone `--` earlier in the same simple command. A variable that is only part of a
 * word (`--flag={{x}}`) cannot become an option, so it is not reported.
 */
export function shellDashProblems(text: string, scope: TemplateScope): string[] {
  const problems: string[] = [];
  for (const m of text.matchAll(VAR)) {
    if (m[1].startsWith("/") || !stringify(lookup(scope, m[1])).startsWith("-")) continue;
    const before = text.slice(0, m.index);
    if (before !== "" && !/[\s;&|(]$/.test(before)) continue;
    const command = before.split(/[;&|\n]/).pop() ?? "";
    if (/(^|\s)--(\s|$)/.test(command)) continue;
    problems.push(
      `{{${m[1]}}}: the value starts with "-" and would be read as an option; put "--" before it`,
    );
  }
  return problems;
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
