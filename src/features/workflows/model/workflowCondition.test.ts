import { describe, expect, it } from "vitest";
import { evalCondition } from "./workflowCondition";

const scope = {
  inputs: { name: "release", n: 3, flag: true },
  dev: { tags: ["a", "b"], none: [], text: "", note: "hello world", obj: {} },
};

const ok = (condition: string, value: boolean) =>
  expect(evalCondition(condition, scope)).toEqual({ ok: true, value });

describe("workflowCondition", () => {
  it("== on strings", () => {
    ok('{{inputs.name}} == "release"', true);
    ok('{{inputs.name}} == "other"', false);
    ok('"a" == "a"', true);
  });

  it("!= on numbers", () => {
    ok("{{inputs.n}} != 4", true);
    ok("{{inputs.n}} != 3", false);
    ok("{{inputs.n}} != 3.5", true);
    ok("{{inputs.n}} == -3", false);
  });

  it("== on booleans and mixed types", () => {
    ok("{{inputs.flag}} == true", true);
    ok("{{inputs.flag}} == false", false);
    ok('{{inputs.n}} == "3"', false);
  });

  it("contains on string and list", () => {
    ok('{{dev.note}} contains "world"', true);
    ok('{{dev.note}} contains "nope"', false);
  });

  it("contains on list", () => {
    ok('{{dev.tags}} contains "a"', true);
    ok('{{dev.tags}} contains "z"', false);
  });

  it("contains with a missing left side is false, with a bad left side an error", () => {
    ok('{{dev.missing}} contains "x"', false);
    const result = evalCondition('{{inputs.n}} contains "x"', scope);
    expect(result.ok).toBe(false);
  });

  it("empty on [] and \"\"", () => {
    ok("{{dev.none}} empty", true);
    ok("{{dev.text}} empty", true);
    ok("{{dev.obj}} empty", true);
    ok("{{dev.missing}} empty", true);
    ok("{{dev.tags}} empty", false);
    ok("{{inputs.n}} empty", false);
  });

  it("not empty", () => {
    ok("{{dev.tags}} not empty", true);
    ok("{{dev.note}} not empty", true);
    ok("{{dev.none}} not empty", false);
  });

  it("tolerates surrounding whitespace and spaces inside braces", () => {
    ok('  {{ inputs.name }}   ==   "release"  ', true);
  });

  it("handles escaped quotes and operators inside strings", () => {
    ok('"a == b" == "a == b"', true);
    ok('"say \\"hi\\"" == "say \\"hi\\""', true);
  });

  it("does not read the prototype chain", () => {
    ok("{{dev.constructor}} empty", true);
    ok("{{inputs.toString}} empty", true);
  });

  it('parse error for "{{a}} >> 3"', () => {
    const result = evalCondition("{{a}} >> 3", { a: 1 });
    expect(result.ok).toBe(false);
    if (!result.ok) expect(result.error).toContain("{{a}} >> 3");
  });

  it("rejects and/or, trailing text, empty input and unterminated strings", () => {
    for (const bad of [
      '{{a}} == 1 and {{b}} == 2',
      "{{a}} == 1 or {{b}} == 2",
      "{{a}} empty extra",
      "{{a}} ==",
      "",
      '"abc == "abc',
      '{{a}} == "abc',
      "{{a}",
      "yes == yes",
    ]) {
      expect(evalCondition(bad, { a: 1, b: 2 }).ok, bad).toBe(false);
    }
  });
});
