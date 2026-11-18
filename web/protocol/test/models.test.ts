// Copied from src/features/sessions/model/models.test.ts at ca73ac2b
// Effort helper tests, adapted to the catalog types (no static table, no storage access).
import { describe, expect, it } from "vitest";
import {
  MODEL_STORAGE_KEYS,
  defaultModelSettings,
  fastSetting,
  isEffortSettingId,
  mergeModelSettings,
  modelEffortLabel,
  modelEffortSetting,
  otherSettings,
  parseStringRecord,
  preferredModelSettings,
  rememberModelSettings,
  withModel,
  type AgentModel,
} from "../src/models.ts";

const toggle = (id: string, label: string, value = "false") => ({
  id,
  label,
  kind: "toggle" as const,
  value,
  options: [
    { value: "true", label: "On" },
    { value: "false", label: "Off" },
  ],
});

const opus: AgentModel = {
  id: "claude:opus-5",
  harness: "claude",
  name: "Opus 5",
  settings: [
    {
      id: "effort",
      label: "Reasoning",
      kind: "select",
      value: "high",
      options: [
        { value: "high", label: "High" },
        { value: "xhigh", label: "Extra High" },
        { value: "max", label: "Max" },
      ],
    },
    toggle("fast", "Fast"),
  ],
};

const haiku: AgentModel = {
  id: "claude:haiku-4.5",
  harness: "claude",
  name: "Haiku 4.5",
  settings: [toggle("thinking", "Thinking")],
};

describe("model settings", () => {
  it("keeps valid current values when merging onto a model", () => {
    expect(mergeModelSettings(opus, { effort: "xhigh", fast: "true" })).toEqual({ effort: "xhigh", fast: "true" });
  });

  it("drops values the new model does not support", () => {
    expect(mergeModelSettings(haiku, { effort: "xhigh", fast: "true" })).toEqual({ thinking: "false" });
  });

  it("maps extra-high onto Claude's xhigh", () => {
    expect(mergeModelSettings(opus, { effort: "extra-high" })).toEqual({ effort: "xhigh", fast: "false" });
  });

  it("uses defaults when nothing is current", () => {
    expect(mergeModelSettings(opus)).toEqual(defaultModelSettings(opus));
    expect(defaultModelSettings(opus)).toEqual({ effort: "high", fast: "false" });
  });

  it("remembers extra-high and fast across models that support them", () => {
    const remembered = rememberModelSettings({}, { effort: "xhigh", fast: "true" });
    expect(preferredModelSettings(opus, undefined, remembered)).toEqual({ effort: "xhigh", fast: "true" });
    expect(preferredModelSettings(haiku, undefined, remembered)).toEqual({ thinking: "false" });
  });

  it("merges newly remembered settings into previous ones", () => {
    const first = rememberModelSettings({}, { effort: "xhigh", fast: "true" });
    expect(rememberModelSettings(first, { thinking: "true" })).toEqual({ effort: "xhigh", fast: "true", thinking: "true" });
  });

  it("applies remembered preferences over a session's current values", () => {
    expect(preferredModelSettings(opus, { effort: "high", fast: "false" }, { effort: "xhigh", fast: "true" })).toEqual({
      effort: "xhigh",
      fast: "true",
    });
  });

  it("fill mode keeps what is remembered and records only new values", () => {
    const stored = { effort: "xhigh", fast: "true" };
    expect(rememberModelSettings(stored, { effort: "high", fast: "false" }, "fill")).toEqual(stored);
    expect(rememberModelSettings({}, { effort: "xhigh" }, "fill")).toEqual({ effort: "xhigh" });
  });

  it("uses the current session when nothing has been remembered yet", () => {
    expect(preferredModelSettings(opus, { effort: "xhigh", fast: "true" })).toEqual({ effort: "xhigh", fast: "true" });
  });

  it("treats OpenCode variant as the effort setting", () => {
    const model: AgentModel = {
      id: "opencode:some-cloud/spark-1",
      harness: "opencode",
      name: "Spark 1",
      settings: [
        {
          id: "variant",
          label: "Variant",
          kind: "select",
          value: "medium",
          options: ["minimal", "low", "medium", "high", "xhigh"].map((value) => ({ value, label: value })),
        },
      ],
    };
    expect(modelEffortSetting(model)?.id).toBe("variant");
    expect(mergeModelSettings(model, { variant: "high" })).toEqual({ variant: "high" });
  });
});

describe("effort helpers", () => {
  it("knows the effort setting ids", () => {
    for (const id of ["effort", "reasoning", "reasoningEffort", "thinking", "variant"]) {
      expect(isEffortSettingId(id)).toBe(true);
    }
    expect(isEffortSettingId("fast")).toBe(false);
  });

  it("labels the current effort from the choice, then the default", () => {
    expect(modelEffortLabel(opus)).toBe("High");
    expect(modelEffortLabel(opus, { effort: "xhigh" })).toBe("Extra High");
    expect(modelEffortLabel(opus, { effort: "weird" })).toBe("weird");
    expect(modelEffortLabel(haiku)).toBeUndefined();
  });

  it("finds fast mode and the other settings", () => {
    expect(fastSetting(opus)?.on).toBe("true");
    expect(fastSetting(haiku)).toBeUndefined();
    expect(otherSettings(opus)).toEqual([]);
  });

  it("carries settings over when the choice changes model", () => {
    const choice = { harness: "claude", model: haiku.id, modelSettings: { thinking: "true" } };
    expect(withModel(choice, opus)).toEqual({
      harness: "claude",
      model: opus.id,
      modelSettings: { effort: "high", fast: "false" },
    });
  });
});

describe("storage keys and parsing", () => {
  it("uses only the monoptah. prefix", () => {
    for (const key of Object.values(MODEL_STORAGE_KEYS)) expect(key.startsWith("monoptah.")).toBe(true);
    expect(JSON.stringify(MODEL_STORAGE_KEYS)).not.toMatch(/mono[c]ode/);
  });

  it("keeps only string values from a stored record", () => {
    expect(parseStringRecord({ a: "x", b: 1, c: null })).toEqual({ a: "x" });
    expect(parseStringRecord(["a"])).toEqual({});
    expect(parseStringRecord(null)).toEqual({});
  });
});
