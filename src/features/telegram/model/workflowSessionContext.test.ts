import { describe, expect, it } from "vitest";
import {
  clearWorkflowSessionContext,
  setWorkflowSessionContext,
  workflowItemInfo,
  workflowSessionContext,
} from "./workflowSessionContext";

describe("workflowSessionContext", () => {
  it("workflow context set and cleared", () => {
    expect(workflowSessionContext("s1")).toBeUndefined();
    setWorkflowSessionContext("s1", {
      workflowName: "Triage",
      nodeTitle: "Fix",
      itemId: "ABC-1",
      round: 2,
    });
    expect(workflowSessionContext("s1")).toEqual({
      workflowName: "Triage",
      nodeTitle: "Fix",
      itemId: "ABC-1",
      round: 2,
    });
    expect(workflowSessionContext("s2")).toBeUndefined();
    clearWorkflowSessionContext("s1");
    expect(workflowSessionContext("s1")).toBeUndefined();
  });

  it("reads an item label and id from a loop item", () => {
    expect(workflowItemInfo({ key: "ABC-1", title: "Crash on start" })).toEqual(
      {
        itemId: "ABC-1",
        itemLabel: "Crash on start",
      },
    );
    expect(workflowItemInfo({ number: 12 })).toEqual({ itemId: "12" });
    expect(workflowItemInfo("plain")).toEqual({ itemLabel: "plain" });
    expect(workflowItemInfo(undefined)).toEqual({});
  });
});
