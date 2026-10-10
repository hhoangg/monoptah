// @vitest-environment happy-dom
import { beforeEach, describe, expect, it } from "vitest";
import { saveProviderSurface } from "../../providers/model/providerSurface";
import {
  moveHighlight,
  newSessionProviders,
  shouldPromptForProvider,
} from "./newSessionProviderPrompt";
import {
  setProjectDefaultProvider,
  setProjectProviderHidden,
} from "./projectProviders";
import { HARNESSES } from "./session";

beforeEach(() => {
  localStorage.clear();
  setProjectDefaultProvider("/repo", "claude", "claude:opus-5");
  setProjectDefaultProvider("remote://env/repo", "claude", "claude:opus-5");
});

describe("shouldPromptForProvider", () => {
  it("does not prompt when the default provider opens as a chat", () => {
    saveProviderSurface("claude", "chat");
    expect(shouldPromptForProvider("/repo")).toBe(false);
  });

  it("prompts when the default provider opens as a TUI", () => {
    saveProviderSurface("claude", "tui");
    expect(shouldPromptForProvider("/repo")).toBe(true);
  });

  it("only looks at the default provider's surface", () => {
    saveProviderSurface("codex", "tui");
    expect(shouldPromptForProvider("/repo")).toBe(false);
  });

  it("does not prompt in a remote project, where a TUI becomes a chat", () => {
    saveProviderSurface("claude", "tui");
    expect(shouldPromptForProvider("remote://env/repo")).toBe(false);
  });
});

describe("shouldPromptForProvider with few providers", () => {
  beforeEach(() => saveProviderSurface("claude", "tui"));

  it("does not prompt when no provider is installed", () => {
    expect(shouldPromptForProvider("/repo", () => false, true)).toBe(false);
  });

  it("does not prompt when only one provider is installed", () => {
    expect(
      shouldPromptForProvider("/repo", (id) => id === "claude", true),
    ).toBe(false);
  });

  it("prompts when two providers are installed", () => {
    expect(
      shouldPromptForProvider(
        "/repo",
        (id) => id === "claude" || id === "codex",
        true,
      ),
    ).toBe(true);
  });
});

describe("newSessionProviders", () => {
  it("lists every provider before availability is probed", () => {
    expect(newSessionProviders("/repo", () => false, false)).toEqual([
      ...HARNESSES,
    ]);
  });

  it("keeps only installed providers once probed", () => {
    expect(newSessionProviders("/repo", (id) => id === "codex", true)).toEqual([
      "codex",
    ]);
  });

  it("excludes a provider hidden for the project", () => {
    setProjectProviderHidden("/repo", "codex", true);
    expect(newSessionProviders("/repo", () => true, true)).not.toContain(
      "codex",
    );
  });
});

describe("moveHighlight", () => {
  it("moves down and up", () => {
    expect(moveHighlight(0, 1, 3)).toBe(1);
    expect(moveHighlight(2, -1, 3)).toBe(1);
  });

  it("wraps at both ends", () => {
    expect(moveHighlight(2, 1, 3)).toBe(0);
    expect(moveHighlight(0, -1, 3)).toBe(2);
  });

  it("stays at 0 for an empty list", () => {
    expect(moveHighlight(0, 1, 0)).toBe(0);
  });
});
