import { describe, expect, it } from "vitest";
import {
  isMacTerminalClearShortcut,
  macTerminalShortcutData,
  multiCharacterKeyText,
} from "./terminalKeys";

function key(
  key: string,
  modifiers: Partial<
    Pick<KeyboardEvent, "altKey" | "ctrlKey" | "metaKey" | "shiftKey">
  > = {},
) {
  return {
    key,
    altKey: false,
    ctrlKey: false,
    metaKey: false,
    shiftKey: false,
    ...modifiers,
  };
}

describe("mac terminal editing shortcuts", () => {
  it("sends shell word movement for Option+Arrow", () => {
    expect(macTerminalShortcutData(key("ArrowLeft", { altKey: true }))).toBe(
      "\x1bb",
    );
    expect(macTerminalShortcutData(key("ArrowRight", { altKey: true }))).toBe(
      "\x1bf",
    );
  });

  it("sends line movement and deletion for Command shortcuts", () => {
    expect(macTerminalShortcutData(key("ArrowLeft", { metaKey: true }))).toBe(
      "\x01",
    );
    expect(macTerminalShortcutData(key("ArrowRight", { metaKey: true }))).toBe(
      "\x05",
    );
    expect(macTerminalShortcutData(key("Backspace", { metaKey: true }))).toBe(
      "\x15",
    );
  });

  it("leaves other modifiers and keys to xterm and app shortcuts", () => {
    expect(
      macTerminalShortcutData(
        key("ArrowLeft", { metaKey: true, altKey: true }),
      ),
    ).toBeNull();
    expect(
      macTerminalShortcutData(
        key("ArrowLeft", { altKey: true, shiftKey: true }),
      ),
    ).toBeNull();
    expect(
      macTerminalShortcutData(key("ArrowLeft", { ctrlKey: true })),
    ).toBeNull();
    expect(
      macTerminalShortcutData(key("Backspace", { altKey: true })),
    ).toBeNull();
    expect(
      macTerminalShortcutData(key("Delete", { metaKey: true })),
    ).toBeNull();
  });
});

describe("mac terminal clear shortcut", () => {
  it("matches Command+K only", () => {
    expect(isMacTerminalClearShortcut(key("k", { metaKey: true }))).toBe(true);
    expect(isMacTerminalClearShortcut(key("K", { metaKey: true }))).toBe(true);
    expect(isMacTerminalClearShortcut(key("k", { ctrlKey: true }))).toBe(false);
    expect(
      isMacTerminalClearShortcut(key("k", { metaKey: true, shiftKey: true })),
    ).toBe(false);
    expect(
      isMacTerminalClearShortcut(key("k", { metaKey: true, altKey: true })),
    ).toBe(false);
    expect(isMacTerminalClearShortcut(key("j", { metaKey: true }))).toBe(false);
  });
});

describe("multi-character key text", () => {
  it("treats a whole Vietnamese string in key as text", () => {
    expect(multiCharacterKeyText(key("ếng"))).toBe("ếng");
    expect(multiCharacterKeyText(key("ệt"))).toBe("ệt");
    expect(multiCharacterKeyText(key("ấn"))).toBe("ấn");
    expect(multiCharacterKeyText(key("Ếng", { shiftKey: true }))).toBe("Ếng");
  });

  it("keeps a decomposed single letter together", () => {
    const decomposed = "e\u0302\u0301";
    expect(multiCharacterKeyText(key(decomposed))).toBe(decomposed);
  });

  it("never treats a named key as text", () => {
    const named = [
      "Enter",
      "Tab",
      "Backspace",
      "Delete",
      "Escape",
      "Home",
      "End",
      "PageUp",
      "PageDown",
      "Insert",
      "Clear",
      "ArrowUp",
      "ArrowDown",
      "ArrowLeft",
      "ArrowRight",
      ...Array.from({ length: 24 }, (_, i) => `F${i + 1}`),
      "Shift",
      "Control",
      "Alt",
      "Meta",
      "CapsLock",
      "NumLock",
      "ScrollLock",
      "Dead",
      "Unidentified",
      "Process",
      "ContextMenu",
      "AltGraph",
      "Fn",
      "Copy",
      "Cut",
      "Paste",
      "MediaPlayPause",
      "AudioVolumeUp",
      "Lang1",
    ];
    for (const name of named) {
      expect(multiCharacterKeyText(key(name)), name).toBeNull();
    }
  });

  it("leaves single characters to xterm", () => {
    expect(multiCharacterKeyText(key("a"))).toBeNull();
    expect(multiCharacterKeyText(key("ế"))).toBeNull();
    expect(multiCharacterKeyText(key("A", { shiftKey: true }))).toBeNull();
    expect(multiCharacterKeyText(key(" "))).toBeNull();
    expect(multiCharacterKeyText(key("😀"))).toBeNull();
  });

  it("accepts lowercase ASCII that cannot be a key name", () => {
    expect(multiCharacterKeyText(key("eng"))).toBe("eng");
    expect(multiCharacterKeyText(key("ng"))).toBe("ng");
  });

  it("accepts capitalised ASCII that is not a key name", () => {
    expect(multiCharacterKeyText(key("Tien"))).toBe("Tien");
    expect(multiCharacterKeyText(key("ENG"))).toBe("ENG");
    expect(multiCharacterKeyText(key("Hanoi"))).toBe("Hanoi");
    expect(multiCharacterKeyText(key("Eng"))).toBe("Eng");
  });

  it("still rejects a key name that is only capitalised differently", () => {
    expect(multiCharacterKeyText(key("enter"))).toBe("enter");
    expect(multiCharacterKeyText(key("Enter"))).toBeNull();
  });

  it("rejects strings holding control characters", () => {
    expect(multiCharacterKeyText(key("a\x1b"))).toBeNull();
    expect(multiCharacterKeyText(key("a\n"))).toBeNull();
  });

  it("stays out of shortcut and composition paths", () => {
    expect(multiCharacterKeyText(key("ếng", { ctrlKey: true }))).toBeNull();
    expect(multiCharacterKeyText(key("ếng", { metaKey: true }))).toBeNull();
    expect(multiCharacterKeyText(key("ếng", { altKey: true }))).toBeNull();
    expect(
      multiCharacterKeyText({ ...key("ếng"), isComposing: true }),
    ).toBeNull();
    expect(
      multiCharacterKeyText({ ...key("ếng"), isComposing: false }),
    ).toBe("ếng");
  });
});
