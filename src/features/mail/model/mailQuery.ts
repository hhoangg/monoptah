import type {
  MatchMode,
  Rule,
  RuleField,
  RuleOp,
  RuleValue,
} from "./mailTypes";

/**
 * One grammar, one rule type. The search box and the filter chips both build
 * rules through `buildRule`, so a chip set and the equivalent search string
 * cannot drift into two different rules (a saved folder would then disagree
 * with the search that created it).
 */

/** Typing in the box must not hit the FTS index on every keystroke. */
export const SEARCH_DEBOUNCE_MS = 300;

/**
 * `contains_prefix` walks a slice of the FTS term index that widens fast: one
 * or two characters match a large part of it. Rust enforces no minimum, so
 * this is the only guard.
 */
export const MIN_PREFIX_CHARS = 3;

/**
 * The operators the backend accepts per field. Mirrors `FIELD_OPS` in
 * src-tauri/src/mail_store.rs; mailQuery.test.ts reads that file and fails if
 * the two ever differ, so a pair the backend rejects cannot reach the menu.
 */
export const FIELD_OPS: Record<RuleField, readonly RuleOp[]> = {
  from: [
    "is",
    "contains",
    "contains_any",
    "starts_with",
    "ends_with",
    "matches",
  ],
  to: ["is", "contains", "contains_any", "starts_with", "ends_with", "matches"],
  cc: ["is", "contains", "contains_any", "starts_with", "ends_with", "matches"],
  recipient: [
    "is",
    "contains",
    "contains_any",
    "starts_with",
    "ends_with",
    "matches",
  ],
  subject: [
    "is",
    "contains",
    "contains_prefix",
    "contains_any",
    "starts_with",
    "ends_with",
    "matches",
  ],
  body: ["contains", "contains_prefix", "contains_any"],
  mailbox: ["is", "contains_any"],
  domain: ["is", "contains", "contains_any"],
  account: ["is", "contains", "contains_any"],
  has_attachment: ["true"],
  is_unread: ["true"],
  is_starred: ["true"],
  date: ["before", "after", "within_days"],
};

/**
 * Operators whose answer depends on the clock. Mirrors `CLOCK_RELATIVE_OPS` in
 * mail_store.rs (a test reads it): a folder with one never hides its threads
 * from the mailbox views, so the "hide" option cannot apply to it.
 */
export const CLOCK_RELATIVE_OPS: readonly RuleOp[] = ["within_days"];
export const hasClockRelativeRule = (rules: Rule[]) =>
  rules.some((r) => CLOCK_RELATIVE_OPS.includes(r.op));

/** Backend caps (`MAX_RULES`, `MAX_LIST_VALUES`), stopped at the control. */
export const MAX_RULES = 50;
export const MAX_LIST_VALUES = 100;

/**
 * `is` is an exact, case-insensitive match, so a fresh condition starts at
 * `contains`: typing a name into `From is ...` would otherwise match nothing.
 */
export const defaultOp = (field: RuleField): RuleOp => {
  const ops = editorOps(field);
  return ops.includes("contains") ? "contains" : ops[0]!;
};

export const RULE_FIELDS = Object.keys(FIELD_OPS) as RuleField[];

export const FIELD_LABELS: Record<RuleField, string> = {
  from: "From",
  to: "To",
  cc: "Cc",
  recipient: "To or Cc",
  subject: "Subject",
  body: "Body",
  mailbox: "Mailbox",
  domain: "Domain",
  account: "Source",
  has_attachment: "Has attachment",
  is_unread: "Is unread",
  is_starred: "Is starred",
  date: "Date",
};

const FLAG_FIELDS: readonly RuleField[] = [
  "has_attachment",
  "is_unread",
  "is_starred",
];
export const isFlagField = (field: RuleField) => FLAG_FIELDS.includes(field);

/** Fields whose `contains` is a full-text match rather than a substring. */
const FTS_FIELDS: readonly RuleField[] = ["subject", "body"];
const isFts = (field: RuleField, op: RuleOp) =>
  FTS_FIELDS.includes(field) && (op === "contains" || op === "contains_prefix");

/**
 * On these fields `contains_any` is exact-any (like `is`, over a list) while
 * `contains` is a substring, so calling it "contains any of" would claim it is
 * the list form of `contains`. The backend keeps the semantics because
 * changing them would silently redefine every saved rule; the label is the fix.
 */
const EXACT_ANY_FIELDS: readonly RuleField[] = ["mailbox", "domain", "account"];

export function opLabel(field: RuleField, op: RuleOp): string {
  switch (op) {
    case "is":
      return "is";
    case "contains":
      return "contains";
    case "contains_prefix":
      return "contains a word starting with";
    case "contains_any":
      return EXACT_ANY_FIELDS.includes(field) ? "is any of" : "contains any of";
    case "starts_with":
      return "starts with";
    case "ends_with":
      return "ends with";
    case "matches":
      return "matches (* and ? wildcards)";
    case "before":
      return "is before";
    case "after":
      return "is on or after";
    case "within_days":
      return "is within the last (days)";
    case "true":
      return "";
  }
}

/**
 * `contains_prefix` is only meant for the last word of the search box while
 * it is being typed. Chips and saved rules keep exact phrases.
 */
export const editorOps = (field: RuleField): RuleOp[] =>
  FIELD_OPS[field].filter((op) => op !== "contains_prefix");

// ── values ─────────────────────────────────────────────────────────

/** The scalar text of a value. A number is its decimal text. */
export function valueText(value: RuleValue): string {
  if (value === null) return "";
  if (Array.isArray(value)) return value[0] ?? "";
  return String(value);
}

/**
 * The items of a `contains_any` value. Rust stores the list as JSON text and
 * accepts a string holding an array, so a string is tried as one first.
 */
export function valueList(value: RuleValue): string[] {
  if (Array.isArray(value)) return value.map(String);
  if (value === null) return [];
  const text = String(value);
  try {
    const parsed: unknown = JSON.parse(text);
    if (Array.isArray(parsed)) return parsed.map(String);
  } catch {
    // Not a list: a single value.
  }
  return text === "" ? [] : [text];
}

/** A rule as the editor holds it: text everywhere, a list for `contains_any`. */
export function normaliseRule(rule: Rule): Rule {
  return {
    ...rule,
    value:
      rule.op === "contains_any"
        ? valueList(rule.value)
        : valueText(rule.value),
  };
}

/**
 * Whether the index could hold the text. FTS5 folds punctuation away, so a bare
 * `%` is no word at all: Rust compiles that to "matches nothing", and a
 * negated one (`-%`) to "everything".
 */
export const hasIndexableText = (text: string) =>
  /[\p{Alphabetic}\p{N}]/u.test(text);

const DATE_RE = /^(\d{4})-(\d{2})-(\d{2})$/;
export function isIsoDate(text: string): boolean {
  const m = DATE_RE.exec(text);
  if (!m) return false;
  const [y, mo, d] = [Number(m[1]), Number(m[2]), Number(m[3])];
  const date = new Date(Date.UTC(y, mo - 1, d));
  return (
    date.getUTCFullYear() === y &&
    date.getUTCMonth() === mo - 1 &&
    date.getUTCDate() === d
  );
}

const MAX_WITHIN_DAYS = 36_500;

/** Why the rule cannot be saved or run, or null. Mirrors what Rust refuses. */
export function ruleProblem(rule: Rule): string | null {
  if (!FIELD_OPS[rule.field]?.includes(rule.op)) {
    return "This condition is not supported.";
  }
  if (rule.op === "true") return null;
  if (rule.op === "contains_any") {
    const items = valueList(rule.value).filter((v) => v.trim() !== "");
    if (items.length === 0) return "Add at least one value.";
    if (isFts(rule.field, "contains") && !items.every(hasIndexableText)) {
      return "Use words, not only symbols.";
    }
    return null;
  }
  const text = valueText(rule.value).trim();
  if (text === "") return "Enter a value.";
  if (isFts(rule.field, rule.op) && !hasIndexableText(text)) {
    return "Use a word, not only symbols.";
  }
  if (rule.field === "date") {
    if (rule.op === "within_days") {
      const days = Number(text);
      return Number.isInteger(days) && days >= 1 && days <= MAX_WITHIN_DAYS
        ? null
        : `Enter a whole number of days from 1 to ${MAX_WITHIN_DAYS}.`;
    }
    return isIsoDate(text) ? null : "Use a date like 2025-01-31.";
  }
  return null;
}

// ── building rules ─────────────────────────────────────────────────

/** Search-box keys and the rule each maps to. */
export type SearchKey =
  | "from"
  | "to"
  | "cc"
  | "subject"
  | "body"
  | "has"
  | "is"
  | "in"
  | "domain"
  | "source"
  | "after"
  | "before";
const KEYS: readonly string[] = [
  "from",
  "to",
  "cc",
  "subject",
  "body",
  "has",
  "is",
  "in",
  "domain",
  "source",
  "after",
  "before",
];

export type Built = { rule: Rule } | { error: string };

/**
 * The one place a key and a value become a rule. `key` null is a bare word,
 * which searches the body. `prefix` turns a full-text `contains` into
 * `contains_prefix`; only the search box's last token asks for it.
 */
export function buildRule(
  key: SearchKey | null,
  value: string,
  negate: boolean,
  prefix = false,
): Built {
  const rule = (field: RuleField, op: RuleOp, v: string): Built => ({
    rule: { field, op, value: v, negate },
  });
  const word = (field: "from" | "to" | "cc" | "subject" | "body") => {
    if ((field === "subject" || field === "body") && !hasIndexableText(value)) {
      return { error: `Search for a word, not only symbols: ${value}` };
    }
    const op: RuleOp =
      prefix && !negate && (field === "subject" || field === "body")
        ? "contains_prefix"
        : "contains";
    return rule(field, op, value);
  };
  switch (key) {
    case null:
    case "body":
      return word("body");
    case "from":
    case "to":
    case "cc":
    case "subject":
      return word(key);
    case "has":
      return value.toLowerCase() === "attachment"
        ? rule("has_attachment", "true", "")
        : { error: `Unknown filter has:${value}` };
    case "is":
      return value.toLowerCase() === "unread"
        ? rule("is_unread", "true", "")
        : value.toLowerCase() === "starred"
          ? rule("is_starred", "true", "")
          : { error: `Unknown filter is:${value}` };
    case "in":
      return rule("mailbox", "is", value);
    case "domain":
      return rule("domain", "is", value);
    case "source":
      return rule("account", "is", value);
    case "after":
    case "before":
      return isIsoDate(value)
        ? rule("date", key, value)
        : { error: `Use a date like 2025-01-31 for ${key}:` };
  }
}

/** The filter chips. Each is a phrase, never a prefix. */
export type ChipFilters = { from: string; to: string; content: string };
export const NO_CHIPS: ChipFilters = { from: "", to: "", content: "" };

export function chipRules(chips: ChipFilters): {
  rules: Rule[];
  errors: string[];
} {
  const rules: Rule[] = [];
  const errors: string[] = [];
  const entries: [SearchKey, string][] = [
    ["from", chips.from],
    ["to", chips.to],
    ["body", chips.content],
  ];
  for (const [key, raw] of entries) {
    const value = raw.trim();
    if (value === "") continue;
    const built = buildRule(key, value, false);
    if ("rule" in built) rules.push(built.rule);
    else errors.push(built.error);
  }
  return { rules, errors };
}

// ── parsing ────────────────────────────────────────────────────────

type Token = {
  key: SearchKey | null;
  value: string;
  quoted: boolean;
  negate: boolean;
};

function tokenise(text: string): Token[] {
  const tokens: Token[] = [];
  let i = 0;
  const n = text.length;
  while (i < n) {
    if (/\s/.test(text[i]!)) {
      i += 1;
      continue;
    }
    let negate = false;
    if (text[i] === "-" && i + 1 < n && !/\s/.test(text[i + 1]!)) {
      negate = true;
      i += 1;
    } else if (text[i] === "-") {
      i += 1;
      continue;
    }
    let key: SearchKey | null = null;
    const head = /^([A-Za-z]+):/.exec(text.slice(i));
    if (head && KEYS.includes(head[1]!.toLowerCase())) {
      key = head[1]!.toLowerCase() as SearchKey;
      i += head[0].length;
    }
    let value = "";
    let quoted = false;
    if (text[i] === '"') {
      quoted = true;
      i += 1;
      while (i < n && text[i] !== '"') {
        const next = text[i + 1];
        if (text[i] === "\\" && (next === '"' || next === "\\")) {
          value += next;
          i += 2;
        } else {
          value += text[i];
          i += 1;
        }
      }
      i += 1; // The closing quote; absent while the user is still typing.
    } else {
      while (i < n && !/\s/.test(text[i]!)) {
        value += text[i];
        i += 1;
      }
    }
    tokens.push({ key, value, quoted, negate });
  }
  return tokens;
}

export type ParsedQuery = { rules: Rule[]; errors: string[] };

/**
 * Parses a search string. With `prefixLast`, the last token also matches words
 * that merely start with it, so results follow the typing; it applies only to
 * a positive, unquoted full-text token of at least MIN_PREFIX_CHARS letters
 * and digits that the cursor has not left (no trailing space). Saved folders
 * must parse without it.
 */
export function parseQuery(
  text: string,
  options: { prefixLast?: boolean } = {},
): ParsedQuery {
  const tokens = tokenise(text);
  const rules: Rule[] = [];
  const errors: string[] = [];
  const typing = options.prefixLast === true && !/\s$/.test(text);
  tokens.forEach((token, index) => {
    // `from:` with nothing after it yet is unfinished, not an error.
    if (token.value.trim() === "") return;
    const isLast = index === tokens.length - 1;
    const alnum = [...token.value].filter((c) =>
      /[\p{Alphabetic}\p{N}]/u.test(c),
    ).length;
    const prefix =
      typing && isLast && !token.quoted && alnum >= MIN_PREFIX_CHARS;
    const built = buildRule(token.key, token.value, token.negate, prefix);
    if ("rule" in built) rules.push(built.rule);
    else errors.push(built.error);
  });
  return { rules, errors };
}

// ── serialising ────────────────────────────────────────────────────

const startsWithKnownKey = (value: string) => {
  const head = /^([A-Za-z]+):/.exec(value);
  return head !== null && KEYS.includes(head[1]!.toLowerCase());
};

/** A bare word that would be read as a negation, a quote or a key needs quotes. */
function quoteIfNeeded(value: string, keyed: boolean): string {
  const ambiguous =
    !keyed && (/^[-"]/.test(value) || startsWithKnownKey(value));
  if (value === "" || /[\s"]/.test(value) || ambiguous) {
    return `"${value.replace(/[\\"]/g, "\\$&")}"`;
  }
  return value;
}

function serialiseRule(rule: Rule): string | null {
  const v = valueText(rule.value);
  const text = (key: string | null) =>
    key === null ? quoteIfNeeded(v, false) : `${key}:${quoteIfNeeded(v, true)}`;
  let out: string | null = null;
  const { field, op } = rule;
  if (op === "contains" || op === "contains_prefix") {
    if (field === "from" || field === "to" || field === "cc") out = text(field);
    else if (field === "subject") out = text("subject");
    else if (field === "body") out = text(null);
  } else if (op === "true") {
    if (field === "has_attachment") out = "has:attachment";
    else if (field === "is_unread") out = "is:unread";
    else if (field === "is_starred") out = "is:starred";
  } else if (op === "is") {
    if (field === "mailbox") out = text("in");
    else if (field === "domain") out = text("domain");
    else if (field === "account") out = text("source");
  } else if (field === "date" && (op === "before" || op === "after")) {
    out = isIsoDate(v) ? `${op}:${v}` : null;
  }
  if (out === null) return null;
  return rule.negate ? `-${out}` : out;
}

/**
 * The search string for rules, or null when any rule has no spelling in the
 * grammar (`starts_with`, a list, a relative date): the box cannot show it and
 * the editor is the only way to see it. A `contains_prefix` serialises as a
 * plain word, since the prefix is how the box is typed, not what is saved.
 */
export function serialiseRules(rules: Rule[]): string | null {
  const parts: string[] = [];
  for (const rule of rules) {
    const part = serialiseRule(rule);
    if (part === null) return null;
    parts.push(part);
  }
  return parts.join(" ");
}

/** The match mode a search always uses: every token must hold. */
export const QUERY_MATCH_MODE: MatchMode = "all";
