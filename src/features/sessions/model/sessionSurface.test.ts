// @vitest-environment happy-dom
import { beforeEach, describe, expect, it } from "vitest";
import { saveProviderSurface } from "../../providers/model/providerSurface";
import { newDefaultSession, newSession } from "./session";

beforeEach(() => {
  localStorage.clear();
});

describe("new session surface", () => {
  it("is chat when the provider has no preference", () => {
    expect(newSession("claude", "/repo").surface).toBe("chat");
    expect(newDefaultSession("/repo").surface).toBe("chat");
  });

  it("inherits the provider's preference when the caller does not pin one", () => {
    saveProviderSurface("claude", "tui");
    expect(newSession("claude", "/repo").surface).toBe("tui");
    expect(newSession("codex", "/repo").surface).toBe("chat");
  });

  it("lets a caller pin chat over a terminal preference", () => {
    saveProviderSurface("claude", "tui");
    expect(
      newSession("claude", "/repo", undefined, undefined, undefined, {
        surface: "chat",
      }).surface,
    ).toBe("chat");
  });

  it("carries the pinned surface through newDefaultSession", () => {
    saveProviderSurface(newDefaultSession("/repo").harness, "tui");
    const pinned = newDefaultSession("/repo", undefined, { surface: "chat" });
    expect(pinned.surface).toBe("chat");
    expect(newDefaultSession("/repo").surface).toBe("tui");
  });

  it("never starts a terminal for a project on another machine", () => {
    saveProviderSurface("claude", "tui");
    expect(newSession("claude", "remote://env/repo").surface).toBe("chat");
  });
});
