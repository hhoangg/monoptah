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

function lookup(scope: TemplateScope, path: string): unknown {
  let cur: unknown = scope;
  for (const key of path.split(".")) {
    if (cur === null || typeof cur !== "object") return undefined;
    cur = (cur as Record<string, unknown>)[key];
  }
  return cur;
}

function stringify(value: unknown): string {
  if (value === undefined || value === null) return "";
  if (typeof value === "string") return value;
  if (typeof value === "number" || typeof value === "boolean") return String(value);
  return JSON.stringify(value, null, 2);
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
