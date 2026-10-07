// @vitest-environment happy-dom
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  loadProviderSurface,
  saveProviderSurface,
  subscribeProviderSurface,
} from "./providerSurface";

const key = "monocode.providerSurface.v1";

beforeEach(() => {
  localStorage.clear();
});

describe("provider surface", () => {
  it("defaults every provider to chat when nothing is stored", () => {
    expect(loadProviderSurface("claude")).toBe("chat");
    expect(loadProviderSurface("antigravity")).toBe("chat");
  });

  it("round-trips a saved surface without touching other providers", () => {
    expect(saveProviderSurface("codex", "tui")).toBe(true);
    expect(loadProviderSurface("codex")).toBe("tui");
    expect(loadProviderSurface("claude")).toBe("chat");
    expect(saveProviderSurface("codex", "chat")).toBe(true);
    expect(loadProviderSurface("codex")).toBe("chat");
  });

  it("falls back to chat for malformed or invalid storage", () => {
    localStorage.setItem(key, "not json");
    expect(loadProviderSurface("claude")).toBe("chat");
    localStorage.setItem(key, JSON.stringify(["tui"]));
    expect(loadProviderSurface("claude")).toBe("chat");
    localStorage.setItem(key, JSON.stringify({ claude: "pty", codex: "tui" }));
    expect(loadProviderSurface("claude")).toBe("chat");
    expect(loadProviderSurface("codex")).toBe("tui");
  });

  it("repairs malformed storage when saving", () => {
    localStorage.setItem(key, "not json");
    expect(saveProviderSurface("claude", "tui")).toBe(true);
    expect(loadProviderSurface("claude")).toBe("tui");
  });

  it("notifies subscribers on save and stops after unsubscribe", () => {
    const listener = vi.fn();
    const unsubscribe = subscribeProviderSurface(listener);
    saveProviderSurface("claude", "tui");
    expect(listener).toHaveBeenCalledTimes(1);
    unsubscribe();
    saveProviderSurface("claude", "chat");
    expect(listener).toHaveBeenCalledTimes(1);
  });

  it("notifies subscribers when another window changes the key", () => {
    const listener = vi.fn();
    const unsubscribe = subscribeProviderSurface(listener);
    window.dispatchEvent(new StorageEvent("storage", { key }));
    window.dispatchEvent(new StorageEvent("storage", { key: "other" }));
    expect(listener).toHaveBeenCalledTimes(1);
    unsubscribe();
  });
});
