// @vitest-environment happy-dom
import { beforeEach, describe, expect, it } from "vitest";
import { saveProviderSurface } from "../../providers/model/providerSurface";
import { automationRunSurface, tuiRunError } from "./automationSession";

beforeEach(() => {
  localStorage.clear();
});

describe("automationRunSurface", () => {
  it("is chat when the provider has no preference", () => {
    expect(automationRunSurface("claude", "/repo")).toBe("chat");
  });

  it("is a terminal when the provider prefers one and takes a first prompt", () => {
    saveProviderSurface("claude", "tui");
    expect(automationRunSurface("claude", "/repo")).toBe("tui");
  });

  it("stays chat when the CLI cannot take a first prompt", () => {
    saveProviderSurface("codex", "tui");
    expect(automationRunSurface("codex", "/repo")).toBe("chat");
  });

  it("stays chat for a project on another machine", () => {
    saveProviderSurface("claude", "tui");
    expect(automationRunSurface("claude", "remote://env/repo")).toBe("chat");
  });
});

describe("tuiRunError", () => {
  it("reports a CLI that could not start", () => {
    expect(
      tuiRunError("claude", {
        code: null,
        error: "binary not found",
        early: true,
      }),
    ).toBe("Could not start Claude Code. binary not found");
  });

  it("explains an early exit with its code", () => {
    const text = tuiRunError("claude", { code: 1, early: true });
    expect(text.startsWith("Process exited (code 1). ")).toBe(true);
    expect(text).toContain("stopped right after it started");
  });
});
