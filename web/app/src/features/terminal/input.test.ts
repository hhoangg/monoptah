import { describe, expect, it } from "vitest";
import { applyModifiers, arrowSequence, chunkEnd, controlCharacter, fromBase64, toBase64, utf8 } from "./input";

describe("base64", () => {
  it("round-trips bytes, including a big buffer and non-ASCII text", () => {
    const text = "héllo 世界 \u{1F600}";
    expect(new TextDecoder().decode(fromBase64(toBase64(utf8(text)))!)).toBe(text);
    const big = Uint8Array.from({ length: 100_000 }, (_, i) => i % 256);
    expect(fromBase64(toBase64(big))).toEqual(big);
  });
  it("answers null for text that is not base64 and empty bytes for the empty string", () => {
    expect(fromBase64("not base64!")).toBeNull();
    expect(fromBase64("")).toEqual(new Uint8Array(0));
  });
});

describe("chunkEnd", () => {
  it("takes the whole input when it fits", () => {
    expect(chunkEnd(utf8("abc"), 10)).toBe(3);
  });
  it("cuts at the limit for ASCII", () => {
    expect(chunkEnd(utf8("a".repeat(20)), 8)).toBe(8);
  });
  it("never cuts a multi-byte character in half", () => {
    const bytes = utf8("ab€€€"); // 2 + 3 * 3 bytes
    expect(chunkEnd(bytes, 4)).toBe(2);
    expect(chunkEnd(bytes, 5)).toBe(5);
    expect(chunkEnd(bytes, 7)).toBe(5);
  });
  it("still makes progress when the limit is below one character", () => {
    expect(chunkEnd(utf8("€€"), 2)).toBe(2);
  });
});

describe("control characters", () => {
  it("maps letters to C0 codes in either case", () => {
    expect(controlCharacter("c")).toBe("\x03");
    expect(controlCharacter("C")).toBe("\x03");
    expect(controlCharacter("d")).toBe("\x04");
    expect(controlCharacter("z")).toBe("\x1a");
    expect(controlCharacter("[")).toBe("\x1b");
    expect(controlCharacter("?")).toBe("\x7f");
  });
  it("has none for digits or several characters", () => {
    expect(controlCharacter("1")).toBeNull();
    expect(controlCharacter("ab")).toBeNull();
    expect(controlCharacter("")).toBeNull();
  });
});

describe("applyModifiers", () => {
  it("sticky Ctrl turns the next letter into its control character", () => {
    expect(applyModifiers("c", { ctrl: true, alt: false })).toBe("\x03");
    expect(applyModifiers("l", { ctrl: true, alt: false })).toBe("\x0c");
  });
  it("Alt prefixes ESC, with Ctrl too", () => {
    expect(applyModifiers("b", { ctrl: false, alt: true })).toBe("\x1bb");
    expect(applyModifiers("b", { ctrl: true, alt: true })).toBe("\x1b\x02");
  });
  it("leaves a paste, an escape sequence and a key without a control form alone", () => {
    expect(applyModifiers("ls -la", { ctrl: true, alt: false })).toBe("ls -la");
    expect(applyModifiers("\x1b[A", { ctrl: true, alt: false })).toBe("\x1b[A");
    expect(applyModifiers("1", { ctrl: true, alt: false })).toBe("1");
  });
  it("changes nothing with no modifier on", () => {
    expect(applyModifiers("c", { ctrl: false, alt: false })).toBe("c");
  });
});

describe("arrow sequences", () => {
  it("are CSI sequences, and SS3 in application cursor mode", () => {
    expect(arrowSequence("up", false)).toBe("\x1b[A");
    expect(arrowSequence("down", false)).toBe("\x1b[B");
    expect(arrowSequence("right", false)).toBe("\x1b[C");
    expect(arrowSequence("left", false)).toBe("\x1b[D");
    expect(arrowSequence("up", true)).toBe("\x1bOA");
  });
});
