// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as settings from "./settings";
import * as appearance from "./appearance";

const platform = vi.hoisted(() => ({ isWindows: true }));
vi.mock("../../../platform/tauri/platform", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../../platform/tauri/platform")>()),
  get IS_WIN() {
    return platform.isWindows;
  },
}));

function memoryStorage(): Storage {
  const values = new Map<string, string>();
  return {
    get length() {
      return values.size;
    },
    clear: () => values.clear(),
    getItem: (key) => values.get(key) ?? null,
    key: (index) => [...values.keys()][index] ?? null,
    removeItem: (key) => values.delete(key),
    setItem: (key, value) => {
      values.set(key, value);
    },
  };
}

beforeEach(() => {
  vi.stubGlobal("localStorage", memoryStorage());
  platform.isWindows = true;
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe.each([
  [
    "monocode.tabAnimationsEnabled",
    settings.loadTabAnimationsEnabled,
    settings.saveTabAnimationsEnabled,
    false,
    undefined,
  ],
  [
    "monocode.composerRunner",
    settings.loadComposerRunner,
    settings.saveComposerRunner,
    true,
    "monocode:composer-runner-change",
  ],
  [
    "monocode.notesEnabled",
    settings.loadNotesEnabled,
    settings.saveNotesEnabled,
    true,
    "monocode:notes-enabled-change",
  ],
  [
    "monocode.liveAgentsEnabled",
    settings.loadLiveAgentsEnabled,
    settings.saveLiveAgentsEnabled,
    true,
    "monocode:live-agents-enabled-change",
  ],
  [
    "monocode.monosEnabled",
    settings.loadMonosEnabled,
    settings.saveMonosEnabled,
    true,
    "monocode:monos-enabled-change",
  ],
  [
    "monocode.closeToTray",
    settings.loadCloseToTray,
    settings.saveCloseToTray,
    true,
    undefined,
  ],
  [
    "monocode.gridArcadeEnabled",
    settings.loadGridArcadeEnabled,
    settings.saveGridArcadeEnabled,
    true,
    "monocode:grid-arcade-enabled-change",
  ],
  [
    "monocode.claudeHooks",
    settings.loadClaudeHooks,
    settings.saveClaudeHooks,
    true,
    undefined,
  ],
  [
    "monocode.bodyGlass",
    appearance.loadBodyGlass,
    appearance.saveBodyGlass,
    appearance.BODY_GLASS_DEFAULT,
    undefined,
  ],
  [
    "monocode.projectRailOpen",
    appearance.loadProjectRailOpen,
    appearance.saveProjectRailOpen,
    true,
    undefined,
  ],
  [
    "monocode.sessionSidebarOpen",
    appearance.loadSessionSidebarOpen,
    appearance.saveSessionSidebarOpen,
    true,
    undefined,
  ],
  [
    "monocode.transcriptAnchor",
    appearance.loadTranscriptAnchor,
    appearance.saveTranscriptAnchor,
    true,
    "monocode:transcriptanchorchange",
  ],
] as const)("%s", (key, load, save, fallback, eventName) => {
  it("uses its default when unset or unreadable", () => {
    expect(load()).toBe(fallback);
    vi.spyOn(localStorage, "getItem").mockImplementation(() => {
      throw new Error("storage unavailable");
    });
    expect(load()).toBe(fallback);
  });

  it.each([
    ["1", true],
    ["true", true],
    ["0", false],
    ["false", false],
    ["", false],
    ["TRUE", false],
    [" true ", false],
    ["invalid", false],
  ] as const)("reads %j as %s", (stored, expected) => {
    localStorage.setItem(key, stored);
    expect(load()).toBe(expected);
  });

  it.each([false, true])("persists %s before notifying listeners", (value) => {
    const dispatch = vi.spyOn(window, "dispatchEvent");
    const listener = vi.fn((event: Event) => {
      expect((event as CustomEvent<boolean>).detail).toBe(value);
      expect(load()).toBe(value);
    });
    if (eventName) window.addEventListener(eventName, listener);
    try {
      save(value);
      expect(localStorage.getItem(key)).toBe(value ? "1" : "0");
      expect(localStorage.length).toBe(1);
      expect(load()).toBe(value);
      expect(dispatch).toHaveBeenCalledTimes(eventName ? 1 : 0);
      expect(listener).toHaveBeenCalledTimes(eventName ? 1 : 0);
    } finally {
      if (eventName) window.removeEventListener(eventName, listener);
    }
  });

  it("still emits its change event when writing fails", () => {
    const value = !fallback;
    const dispatch = vi.spyOn(window, "dispatchEvent");
    vi.spyOn(localStorage, "setItem").mockImplementation(() => {
      throw new Error("storage quota exceeded");
    });

    expect(() => save(value)).not.toThrow();
    expect(load()).toBe(fallback);
    expect(dispatch).toHaveBeenCalledTimes(eventName ? 1 : 0);
    if (eventName) {
      expect(dispatch).toHaveBeenCalledWith(
        expect.objectContaining({ type: eventName, detail: value }),
      );
    }
  });

  it("tolerates unavailable storage", () => {
    vi.stubGlobal("localStorage", undefined);
    expect(load()).toBe(fallback);
    expect(() => save(!fallback)).not.toThrow();
  });

  it("can persist without a window", () => {
    vi.stubGlobal("window", undefined);
    save(!fallback);
    expect(load()).toBe(!fallback);
  });
});

it("disables close-to-tray outside Windows without consulting storage", () => {
  platform.isWindows = false;
  settings.saveCloseToTray(true);
  const read = vi.spyOn(localStorage, "getItem");

  expect(settings.loadCloseToTray()).toBe(false);
  expect(read).not.toHaveBeenCalled();
});

describe("editor blame settings", () => {
  it("defaults to inline on and the column off", () => {
    expect(settings.loadInlineBlame()).toBe(true);
    expect(settings.loadBlameColumn()).toBe(false);
  });

  it("persists under the monocode prefix", () => {
    settings.saveInlineBlame(false);
    settings.saveBlameColumn(true);
    expect(localStorage.getItem("monocode.editorInlineBlame")).toBe("0");
    expect(localStorage.getItem("monocode.editorBlameColumn")).toBe("1");
    expect(settings.loadInlineBlame()).toBe(false);
    expect(settings.loadBlameColumn()).toBe(true);
  });

  it("notifies subscribers after saving, until they unsubscribe", () => {
    const inline = vi.fn();
    const column = vi.fn();
    const offInline = settings.subscribeInlineBlame(inline);
    const offColumn = settings.subscribeBlameColumn(column);

    settings.saveInlineBlame(false);
    expect(inline).toHaveBeenCalledTimes(1);
    expect(column).not.toHaveBeenCalled();
    settings.saveBlameColumn(true);
    expect(column).toHaveBeenCalledTimes(1);
    expect(inline).toHaveBeenCalledTimes(1);

    window.dispatchEvent(
      new StorageEvent("storage", { key: "monocode.editorInlineBlame" }),
    );
    window.dispatchEvent(new StorageEvent("storage", { key: "other" }));
    expect(inline).toHaveBeenCalledTimes(2);

    offInline();
    offColumn();
    settings.saveInlineBlame(true);
    settings.saveBlameColumn(false);
    expect(inline).toHaveBeenCalledTimes(2);
    expect(column).toHaveBeenCalledTimes(1);
  });

  it("flips the blame column and returns the new value", () => {
    expect(settings.toggleBlameColumn()).toBe(true);
    expect(settings.loadBlameColumn()).toBe(true);
    expect(settings.toggleBlameColumn()).toBe(false);
    expect(settings.loadBlameColumn()).toBe(false);
  });

  it("is findable in settings search", () => {
    expect(settings.searchSettings("inline blame").map((r) => r.settingId)).toContain(
      "inline-blame",
    );
  });
});
