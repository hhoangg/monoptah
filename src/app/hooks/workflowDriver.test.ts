import { describe, expect, it } from "vitest";
import { isDriverWindow } from "./workflowDriver";

describe("isDriverWindow", () => {
  it("is true only for the main window", () => {
    expect(isDriverWindow("main")).toBe(true);
    for (const label of ["window-2", "quick-session-abc", "mono-chat", ""])
      expect(isDriverWindow(label)).toBe(false);
    expect(isDriverWindow(undefined)).toBe(false);
  });
});
