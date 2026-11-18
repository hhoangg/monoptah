import { afterEach, describe, expect, it } from "vitest";
import type { ModelSelection } from "@/features/models/modelSettings";
import { clearNewSessionDraft, readNewSessionDraft, resetNewSessionDrafts, writeNewSessionDraft } from "./newSessionDraft";

afterEach(resetNewSessionDrafts);

describe("new session draft store", () => {
  it("keeps text and the model pick per desktop and project", () => {
    const pick = { harness: "h", model: "m" } as unknown as ModelSelection;
    writeNewSessionDraft("d1", "p1", { text: "hello" });
    writeNewSessionDraft("d1", "p1", { picked: pick });
    expect(readNewSessionDraft("d1", "p1")).toEqual({ text: "hello", picked: pick });
    expect(readNewSessionDraft("d1", "p2")).toEqual({ text: "", picked: null });
    expect(readNewSessionDraft("d2", "p1")).toEqual({ text: "", picked: null });
  });

  it("forgets an emptied draft and a cleared one", () => {
    writeNewSessionDraft("d1", "p1", { text: "x" });
    writeNewSessionDraft("d1", "p1", { text: "" });
    expect(readNewSessionDraft("d1", "p1")).toEqual({ text: "", picked: null });
    writeNewSessionDraft("d1", "p1", { text: "y" });
    clearNewSessionDraft("d1", "p1");
    expect(readNewSessionDraft("d1", "p1").text).toBe("");
  });
});
