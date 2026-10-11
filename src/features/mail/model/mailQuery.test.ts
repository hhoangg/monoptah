import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import {
  CLOCK_RELATIVE_OPS,
  FIELD_OPS,
  MAX_LIST_VALUES,
  MAX_RULES,
  defaultOp,
  hasClockRelativeRule,
  MIN_PREFIX_CHARS,
  RULE_FIELDS,
  chipRules,
  editorOps,
  hasIndexableText,
  opLabel,
  parseQuery,
  ruleProblem,
  serialiseRules,
  valueList,
  valueText,
} from "./mailQuery";
import type { Rule, RuleField } from "./mailTypes";

const rule = (
  field: Rule["field"],
  op: Rule["op"],
  value: Rule["value"] = "",
  negate = false,
): Rule => ({ field, op, value, negate });

describe("round trip", () => {
  // Canonical spellings only: keys lowercase, one space, quotes only where a
  // value needs them. `body:x` is read but written back as a bare word.
  it.each([
    "from:bob",
    "from:bob to:ann cc:lee",
    "subject:invoice",
    "invoice",
    '"order refund"',
    'subject:"order refund"',
    "-from:noreply",
    "-spam",
    "has:attachment is:unread is:starred",
    "-is:unread",
    "in:owner@byptah.com",
    "domain:byptah.com source:byptah-mail",
    "after:2025-01-31 before:2026-02-01",
    '"-5"',
    '"from:bob"',
    '"say \\"hi\\""',
    "from:bob invoice -refund has:attachment",
    "http://example.com",
  ])("parse then serialise returns %s", (text) => {
    const { rules, errors } = parseQuery(text);
    expect(errors).toEqual([]);
    expect(serialiseRules(rules)).toBe(text);
  });

  it("serialise then parse returns the same rules", () => {
    const rules = [
      rule("from", "contains", "bob smith"),
      rule("body", "contains", "-odd"),
      rule("subject", "contains", "from:x", true),
      rule("date", "after", "2025-03-04"),
      rule("is_unread", "true"),
      rule("mailbox", "is", "owner@byptah.com", true),
    ];
    const text = serialiseRules(rules)!;
    expect(parseQuery(text).rules).toEqual(rules);
  });

  it("has no spelling for rules the box cannot express", () => {
    expect(serialiseRules([rule("from", "starts_with", "a")])).toBeNull();
    expect(serialiseRules([rule("date", "within_days", "30")])).toBeNull();
    expect(
      serialiseRules([rule("domain", "contains_any", ["a.com"])]),
    ).toBeNull();
    expect(serialiseRules([rule("date", "after", "soon")])).toBeNull();
  });

  it("writes a prefix word as a plain word", () => {
    const { rules } = parseQuery("invoi", { prefixLast: true });
    expect(rules[0]!.op).toBe("contains_prefix");
    expect(serialiseRules(rules)).toBe("invoi");
  });
});

describe("same rules as the chip editor", () => {
  it("builds identical rules for the equivalent chips", () => {
    const fromBox = parseQuery('from:bob to:"ann lee" "hello world"').rules;
    const fromChips = chipRules({
      from: "bob",
      to: "ann lee",
      content: "hello world",
    }).rules;
    expect(fromChips).toEqual(fromBox);
    expect(fromChips).toHaveLength(3);
  });

  it("agrees on one chip and on none", () => {
    expect(chipRules({ from: "", to: "", content: "" }).rules).toEqual(
      parseQuery("").rules,
    );
    expect(chipRules({ from: " x ", to: "", content: "" }).rules).toEqual(
      parseQuery("from:x").rules,
    );
  });

  it("keeps a chip exact where the same word in the box becomes a prefix", () => {
    const chip = chipRules({ from: "", to: "", content: "invoice" }).rules;
    const typed = parseQuery("invoice", { prefixLast: true }).rules;
    expect(chip[0]!.op).toBe("contains");
    expect(typed[0]!.op).toBe("contains_prefix");
    // Saved from the box, the rule is the chip's rule.
    expect(parseQuery("invoice").rules).toEqual(chip);
  });
});

describe("contains_prefix only for the last token being typed", () => {
  const ops = (text: string) =>
    parseQuery(text, { prefixLast: true }).rules.map((r) => r.op);

  it("applies to the last bare word and nothing earlier", () => {
    expect(ops("from:bob invoi")).toEqual(["contains", "contains_prefix"]);
    expect(ops("invoice refu")).toEqual(["contains", "contains_prefix"]);
  });

  it("applies to a subject token and never to people or flags", () => {
    expect(ops("subject:invo")).toEqual(["contains_prefix"]);
    expect(ops("from:bob")).toEqual(["contains"]);
  });

  it(`needs ${MIN_PREFIX_CHARS} letters or digits`, () => {
    expect(ops("in")).toEqual(["contains"]);
    expect(ops("inv")).toEqual(["contains_prefix"]);
    expect(ops("i-n")).toEqual(["contains"]);
  });

  it("stops once the word is finished, quoted or negated", () => {
    expect(ops("invoice ")).toEqual(["contains"]);
    expect(ops('"invoice')).toEqual(["contains"]);
    expect(ops("-invoice")).toEqual(["contains"]);
  });

  it("is never applied without the option", () => {
    expect(parseQuery("invoice").rules[0]!.op).toBe("contains");
  });
});

describe("a bare wildcard", () => {
  it.each(["%", "-%", "subject:%", "-subject:*", "body:_", '"%"', "-?!"])(
    "is refused in %s",
    (text) => {
      const { rules, errors } = parseQuery(text);
      expect(rules).toEqual([]);
      expect(errors).toHaveLength(1);
    },
  );

  it("is refused by the editor's check as well", () => {
    expect(ruleProblem(rule("subject", "contains", "%", true))).not.toBeNull();
    expect(
      ruleProblem(rule("body", "contains_any", ["ok", "%"])),
    ).not.toBeNull();
  });

  it("is fine where it is a literal match", () => {
    expect(parseQuery("from:%").errors).toEqual([]);
    expect(ruleProblem(rule("from", "contains", "100%"))).toBeNull();
    expect(hasIndexableText("100%")).toBe(true);
    expect(hasIndexableText("café")).toBe(true);
  });
});

describe("parsing", () => {
  it("treats an unknown key as a plain word", () => {
    expect(parseQuery("foo:bar").rules).toEqual([
      rule("body", "contains", "foo:bar"),
    ]);
  });

  it("is case-insensitive on keys and keeps the value as typed", () => {
    expect(parseQuery("FROM:Bob").rules).toEqual([
      rule("from", "contains", "Bob"),
    ]);
  });

  it("skips a key with no value yet", () => {
    expect(parseQuery("from: invoice")).toEqual({
      rules: [rule("body", "contains", "invoice")],
      errors: [],
    });
    expect(parseQuery('from:""').rules).toEqual([]);
  });

  it("reports an unknown flag or a bad date and drops the token", () => {
    const flag = parseQuery("is:unred");
    expect(flag.rules).toEqual([]);
    expect(flag.errors[0]).toContain("is:unred");
    expect(parseQuery("after:2025-02-30").errors).toHaveLength(1);
    expect(parseQuery("before:yesterday").errors).toHaveLength(1);
  });

  it("reads an unterminated quote as the phrase so far", () => {
    expect(parseQuery('"order ref').rules).toEqual([
      rule("body", "contains", "order ref"),
    ]);
  });

  it("ignores a lone dash", () => {
    expect(parseQuery("- invoice").rules).toEqual([
      rule("body", "contains", "invoice"),
    ]);
  });

  it("maps every key to the field the spec lists", () => {
    expect(parseQuery("in:a@b.c").rules[0]).toEqual(
      rule("mailbox", "is", "a@b.c"),
    );
    expect(parseQuery("domain:b.c").rules[0]).toEqual(
      rule("domain", "is", "b.c"),
    );
    expect(parseQuery("source:lab").rules[0]).toEqual(
      rule("account", "is", "lab"),
    );
    expect(parseQuery("has:attachment").rules[0]).toEqual(
      rule("has_attachment", "true"),
    );
  });
});

describe("values read back from a saved folder", () => {
  it("accepts a number that came back as text", () => {
    expect(valueText("30")).toBe("30");
    expect(valueText(30)).toBe("30");
    expect(ruleProblem(rule("date", "within_days", "30"))).toBeNull();
    expect(ruleProblem(rule("date", "within_days", 30))).toBeNull();
    expect(ruleProblem(rule("date", "within_days", "0"))).not.toBeNull();
    expect(ruleProblem(rule("date", "within_days", "36501"))).not.toBeNull();
  });

  it("reads a list whether it is an array or a JSON string", () => {
    expect(valueList(["a", "b"])).toEqual(["a", "b"]);
    expect(valueList('["a","b"]')).toEqual(["a", "b"]);
    expect(valueList("solo")).toEqual(["solo"]);
    expect(valueList("")).toEqual([]);
  });
});

describe("operator labels", () => {
  it("calls contains_any on mailbox, domain and account 'is any of'", () => {
    for (const field of ["mailbox", "domain", "account"] as const) {
      expect(opLabel(field, "contains_any")).toBe("is any of");
    }
    expect(opLabel("from", "contains_any")).toBe("contains any of");
    expect(opLabel("subject", "contains_any")).toBe("contains any of");
  });

  it("never offers contains_prefix in the editor", () => {
    for (const field of RULE_FIELDS) {
      expect(editorOps(field)).not.toContain("contains_prefix");
      expect(editorOps(field).length).toBeGreaterThan(0);
    }
  });
});

// The contract is src-tauri/src/mail_store.rs. Read it instead of copying it
// a second time, so a pair the backend rejects cannot reach the menu.
describe("operator table", () => {
  const source = readFileSync(
    resolve(__dirname, "../../../../src-tauri/src/mail_store.rs"),
    "utf8",
  );
  const sets = new Map<string, string[]>();
  for (const m of source.matchAll(
    /const (\w+_OPS): &\[&str\] = &\[([^\]]*)\];/g,
  )) {
    sets.set(
      m[1]!,
      [...m[2]!.matchAll(/"(\w+)"/g)].map((x) => x[1]!),
    );
  }
  const table = source.slice(source.indexOf("pub(crate) const FIELD_OPS"));
  const rust: Record<string, string[]> = {};
  for (const m of table
    .slice(0, table.indexOf("];"))
    .matchAll(/\("(\w+)", (\w+_OPS)\)/g)) {
    rust[m[1]!] = sets.get(m[2]!)!;
  }

  it("found the table", () => {
    expect(Object.keys(rust).length).toBeGreaterThan(0);
  });

  it("agrees on the clock-relative operators and the caps", () => {
    const relative =
      /const CLOCK_RELATIVE_OPS: &\[&str\] = &\[([^\]]*)\];/.exec(source)!;
    expect([...relative[1]!.matchAll(/"(\w+)"/g)].map((m) => m[1])).toEqual([
      ...CLOCK_RELATIVE_OPS,
    ]);
    expect(Number(/const MAX_RULES: usize = (\d+);/.exec(source)![1])).toBe(
      MAX_RULES,
    );
    expect(
      Number(/const MAX_LIST_VALUES: usize = (\d+);/.exec(source)![1]),
    ).toBe(MAX_LIST_VALUES);
  });

  it("matches the backend field by field", () => {
    for (const field of RULE_FIELDS) {
      expect([...FIELD_OPS[field as RuleField]].sort()).toEqual(
        [...(rust[field] ?? ["<missing>"])].sort(),
      );
    }
    expect(Object.keys(rust).sort()).toEqual([...RULE_FIELDS].sort());
  });
});

describe("new conditions", () => {
  it("start as contains where the field has it, so typing matches", () => {
    for (const field of [
      "from",
      "to",
      "cc",
      "recipient",
      "subject",
      "body",
      "domain",
      "account",
    ] as const) {
      expect(defaultOp(field)).toBe("contains");
    }
    expect(defaultOp("mailbox")).toBe("is");
    expect(defaultOp("date")).toBe("before");
    expect(defaultOp("is_unread")).toBe("true");
  });

  it("spots a clock-relative rule", () => {
    expect(hasClockRelativeRule([rule("date", "within_days", "7")])).toBe(true);
    expect(hasClockRelativeRule([rule("date", "after", "2025-01-01")])).toBe(
      false,
    );
  });
});
