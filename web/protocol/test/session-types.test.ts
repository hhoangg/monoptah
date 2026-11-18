import { describe, expect, it } from "vitest";
import { MAX_ATTACHMENTS } from "../src/limits.ts";
import { ModelCatalogSchema } from "../src/methods.ts";
import { QuestionReplySchema } from "../src/question.ts";
import {
  DEFAULT_RUNTIME_MODE,
  HARNESS_IDS,
  REPORTED_RUNTIME_MODES,
  RUNTIME_MODE_HINT,
  RUNTIME_MODE_LABEL,
  SENDABLE_RUNTIME_MODES,
  AttachmentsSchema,
  isHarnessId,
  isSendableRuntimeMode,
  toSendableRuntimeMode,
} from "../src/session.ts";

describe("session type unions", () => {
  it("has the 11 harness ids and rejects others", () => {
    expect(HARNESS_IDS).toHaveLength(11);
    expect(isHarnessId("claude")).toBe(true);
    expect(isHarnessId("mystery")).toBe(false);
  });

  it("caps attachments at the protocol limit", () => {
    const one = { name: "a.png", mimeType: "image/png", data: "AAAA" };
    expect(AttachmentsSchema.safeParse(Array(MAX_ATTACHMENTS).fill(one)).success).toBe(true);
    expect(AttachmentsSchema.safeParse(Array(MAX_ATTACHMENTS + 1).fill(one)).success).toBe(false);
  });
});

describe("full-access is display only", () => {
  it("is reported but never sendable", () => {
    expect(REPORTED_RUNTIME_MODES).toContain("full-access");
    expect(SENDABLE_RUNTIME_MODES).not.toContain("full-access");
    expect(SENDABLE_RUNTIME_MODES).toEqual(["supervised", "auto-accept-edits", "auto"]);
    expect(isSendableRuntimeMode("full-access")).toBe(false);
    expect(DEFAULT_RUNTIME_MODE).not.toBe("full-access");
  });

  it("maps full-access and unknown values to no sendable mode", () => {
    expect(toSendableRuntimeMode("full-access")).toBeNull();
    expect(toSendableRuntimeMode("root")).toBeNull();
    expect(toSendableRuntimeMode(undefined)).toBeNull();
    for (const mode of SENDABLE_RUNTIME_MODES) expect(toSendableRuntimeMode(mode)).toBe(mode);
  });

  it("has a label and a hint for every reported mode", () => {
    for (const mode of REPORTED_RUNTIME_MODES) {
      expect(RUNTIME_MODE_LABEL[mode]).toBeTruthy();
      expect(RUNTIME_MODE_HINT[mode]).toBeTruthy();
    }
  });

  it("the catalog may list full-access ids but the helper never offers it", () => {
    const catalog = ModelCatalogSchema.parse({
      harnesses: [],
      models: [],
      runtimeModes: [{ id: "full-access", label: "Full access", hint: "x" }],
      current: { harness: "claude", model: "m", modelSettings: {}, runtimeMode: "full-access" },
    });
    expect(catalog.runtimeModes.map((mode) => toSendableRuntimeMode(mode.id))).toEqual([null]);
  });
});

describe("question reply shape", () => {
  it("accepts answered and skipped, as mobile QuestionPrompt sends them", () => {
    expect(QuestionReplySchema.safeParse({ kind: "skipped" }).success).toBe(true);
    expect(QuestionReplySchema.safeParse({ kind: "answered", answers: { q: ["a"] }, custom: { q: "x" } }).success).toBe(true);
    expect(QuestionReplySchema.safeParse({ kind: "answered" }).success).toBe(false);
  });
});
