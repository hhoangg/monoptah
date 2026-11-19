import { describe, expect, it } from "vitest";
import { safeClipboardText } from "./clipboardText";

describe("safeClipboardText", () => {
  it("keeps ordinary text, newlines, tabs and non-Latin letters", () => {
    expect(safeClipboardText("a\tb\nc é 日本 👍")).toBe("a\tb\nc é 日本 👍");
  });

  it("drops C0 controls (an ESC that ends a bracketed paste) and DEL", () => {
    expect(safeClipboardText("ls\u001b[201~; rm x\r\u0000\u0007\u007f")).toBe("ls[201~; rm x");
  });

  it("drops bidi override and isolate characters", () => {
    expect(safeClipboardText("a‮b⁦c⁩d‪e‏f")).toBe("abcdef");
  });
});
