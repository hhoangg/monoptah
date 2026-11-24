import { describe, expect, it } from "vitest";
import { stripEmoTokens } from "./emoji";

describe("stripEmoTokens", () => {
  it("leaves text without a token alone", () => {
    expect(stripEmoTokens("Plain text: nothing here")).toBe("Plain text: nothing here");
  });

  it("removes an inline token with one neighbouring space", () => {
    expect(stripEmoTokens("Good morning :emo[happy] all done")).toBe("Good morning all done");
    expect(stripEmoTokens("Done :emo[happy]")).toBe("Done");
    expect(stripEmoTokens("Done.:emo[happy] Next")).toBe("Done. Next");
  });

  it("removes a line that held only a token, with its line break", () => {
    expect(stripEmoTokens("Hello\n:emo[happy]\nWorld")).toBe("Hello\nWorld");
    expect(stripEmoTokens("Hello\n- :emo[happy]\nWorld")).toBe("Hello\nWorld");
  });

  it("trims the name and accepts any characters up to the closing bracket", () => {
    expect(stripEmoTokens("a :emo[ very happy ] b")).toBe("a b");
    expect(stripEmoTokens("a :emo[] b")).toBe("a b");
  });

  it("leaves a token that is not valid: a bracket or a line break before the close, or too long a name", () => {
    expect(stripEmoTokens("a :emo[wa[ve] b")).toBe("a :emo[wa[ve] b");
    expect(stripEmoTokens("a :emo[wa\nve] b")).toBe("a :emo[wa\nve] b");
    const long = `a :emo[${"x".repeat(33)}] b`;
    expect(stripEmoTokens(long)).toBe(long);
    expect(stripEmoTokens(`a :emo[${"x".repeat(32)}] b`)).toBe("a b");
  });

  it("counts the name in characters, not bytes", () => {
    expect(stripEmoTokens(`a :emo[${"\u{1F600}".repeat(32)}] b`)).toBe("a b");
    expect(stripEmoTokens(`a :emo[${"\u{1F600}".repeat(33)}] b`)).toBe(`a :emo[${"\u{1F600}".repeat(33)}] b`);
  });

  it("keeps a token inside an inline code span", () => {
    expect(stripEmoTokens("write `:emo[happy]` to ask, :emo[sad] now")).toBe("write `:emo[happy]` to ask, now");
    expect(stripEmoTokens("write ``a :emo[x] b`` ok")).toBe("write ``a :emo[x] b`` ok");
  });

  it("keeps a token inside a fenced block, and strips the ones around it", () => {
    const text = "before :emo[a]\n```\n:emo[b]\n```\n~~~\n:emo[c]\n~~~\nafter :emo[d]";
    expect(stripEmoTokens(text)).toBe("before\n```\n:emo[b]\n```\n~~~\n:emo[c]\n~~~\nafter");
  });

  it("treats an unclosed fence as code up to the end", () => {
    expect(stripEmoTokens("x :emo[a]\n```\n:emo[b]")).toBe("x\n```\n:emo[b]");
  });

  it("hides an unfinished token at the end of a streaming reply", () => {
    expect(stripEmoTokens("Hello :emo[hap")).toBe("Hello ");
    expect(stripEmoTokens("Hello :emo[")).toBe("Hello ");
    expect(stripEmoTokens("Hello :em")).toBe("Hello ");
    expect(stripEmoTokens("Hello :")).toBe("Hello ");
    expect(stripEmoTokens("Hello :emo[happy]")).toBe("Hello");
  });

  it("keeps a trailing partial token of a finished text with trimTail false", () => {
    expect(stripEmoTokens("Note:", { trimTail: false })).toBe("Note:");
    expect(stripEmoTokens("a :emo[x] b:", { trimTail: false })).toBe("a b:");
  });

  it("keeps a colon that does not start a token", () => {
    expect(stripEmoTokens("Note: ready")).toBe("Note: ready");
    expect(stripEmoTokens("Time 10:30")).toBe("Time 10:30");
  });

  it("does not hide an unfinished token inside code", () => {
    expect(stripEmoTokens("use `:emo[ha")).toBe("use `:emo[ha");
    expect(stripEmoTokens("```\n:emo[ha")).toBe("```\n:emo[ha");
  });
});
