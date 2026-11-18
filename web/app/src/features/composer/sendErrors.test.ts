import { describe, expect, it } from "vitest";
import { DESKTOP_ERROR_CODES } from "@monoptah/protocol";
import { SEND_FAILED_MESSAGE, sendErrorMessage } from "./sendErrors";

describe("sendErrorMessage", () => {
  it("has a specific message for each refusal reason", () => {
    for (const code of ["starting", "auto_stopping", "busy", "read_only", "not_selected", "path_denied", "wake_required"]) {
      const message = sendErrorMessage(code);
      expect(message).not.toBe(SEND_FAILED_MESSAGE);
      expect(message).not.toMatch(/_/);
    }
  });

  it("covers every desktop error code", () => {
    for (const code of DESKTOP_ERROR_CODES) expect(sendErrorMessage(code)).not.toBe(SEND_FAILED_MESSAGE);
  });

  it("falls back to the generic line for an unknown code, a transport text or nothing", () => {
    expect(sendErrorMessage("brand_new_code")).toBe(SEND_FAILED_MESSAGE);
    expect(sendErrorMessage("toString")).toBe(SEND_FAILED_MESSAGE);
    expect(sendErrorMessage(undefined)).toBe(SEND_FAILED_MESSAGE);
  });
});
