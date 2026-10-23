import { describe, expect, it } from "vitest";
import type { OutputField } from "./workflow";
import { OUTPUT_FENCE, outputInstruction, outputReask, parseWorkflowOutput } from "./workflowOutput";

const fields: Record<string, OutputField> = {
  summary: { type: "string", description: "One line summary" },
  count: { type: "number" },
  done: { type: "boolean" },
  items: { type: "list" },
  meta: { type: "object" },
};

const block = (json: string, info = OUTPUT_FENCE) => "```" + info + "\n" + json + "\n```";

describe("workflowOutput", () => {
  it("parses last block", () => {
    const first = block('{"summary":"old","count":1,"done":false,"items":[],"meta":{}}');
    const last = block('{"summary":"new","count":2,"done":true,"items":["a"],"meta":{"k":1},"extra":7}');
    const result = parseWorkflowOutput(`Intro\n${first}\nmore text\n${last}\nbye`, fields);
    expect(result).toEqual({
      ok: true,
      value: { summary: "new", count: 2, done: true, items: ["a"], meta: { k: 1 }, extra: 7 },
    });
  });

  it("ignores blocks with a different info string", () => {
    const json = '{"summary":"x"}';
    const reply = block(json, "json") + "\n" + block(json, "workflow-output-extra");
    const result = parseWorkflowOutput(reply, { summary: { type: "string" } });
    expect(result.ok).toBe(false);
  });

  it("fails when block missing", () => {
    const result = parseWorkflowOutput("no block here", fields);
    expect(result.ok).toBe(false);
    if (!result.ok) expect(result.error).toContain("No workflow-output block");
  });

  it("fails on wrong type: error names field and expected type", () => {
    const reply = block('{"summary":"x","count":"3","done":true,"items":[],"meta":{}}');
    const result = parseWorkflowOutput(reply, fields);
    expect(result.ok).toBe(false);
    if (!result.ok) {
      expect(result.error).toContain("count");
      expect(result.error).toContain("number");
    }
  });

  it("fails on missing field and on list or object mismatches", () => {
    const missing = parseWorkflowOutput(block('{"summary":"x"}'), fields);
    expect(missing.ok).toBe(false);
    if (!missing.ok) expect(missing.error).toContain("count");

    const listAsObject = parseWorkflowOutput(block('{"items":{}}'), { items: { type: "list" } });
    expect(listAsObject.ok).toBe(false);
    const objectAsList = parseWorkflowOutput(block('{"meta":[]}'), { meta: { type: "object" } });
    expect(objectAsList.ok).toBe(false);
    const objectAsNull = parseWorkflowOutput(block('{"meta":null}'), { meta: { type: "object" } });
    expect(objectAsNull.ok).toBe(false);
  });

  it("does not satisfy a field from the prototype chain", () => {
    const result = parseWorkflowOutput(block("{}"), { constructor: { type: "object" } });
    expect(result.ok).toBe(false);
  });

  it("fails on invalid JSON", () => {
    const result = parseWorkflowOutput(block("{not json"), fields);
    expect(result.ok).toBe(false);
    if (!result.ok) expect(result.error).toContain("not valid JSON");
  });

  it("fails when the JSON is not an object", () => {
    const result = parseWorkflowOutput(block("[1,2]"), fields);
    expect(result.ok).toBe(false);
    if (!result.ok) expect(result.error).toContain("JSON object");
  });

  it("zero fields accepts missing block", () => {
    expect(parseWorkflowOutput("just prose", {})).toEqual({ ok: true, value: {} });
  });

  it("zero fields still validates a present block", () => {
    expect(parseWorkflowOutput(block('{"a":1}'), {})).toEqual({ ok: true, value: { a: 1 } });
    expect(parseWorkflowOutput(block("{bad"), {}).ok).toBe(false);
  });

  it("instruction lists each field name and type", () => {
    const text = outputInstruction(fields);
    expect(text).toContain(OUTPUT_FENCE);
    for (const [name, field] of Object.entries(fields)) {
      expect(text).toContain(name);
      expect(text).toContain(field.type);
    }
    expect(text).toContain("One line summary");
  });

  it("reask carries the problem and the contract", () => {
    const text = outputReask(fields, "field count must be a number");
    expect(text).toContain("field count must be a number");
    expect(text).toContain(OUTPUT_FENCE);
    expect(text).toContain("summary");
  });
});
