import { describe, expect, it } from "vitest";
import { clickTarget, safeNotificationUrl } from "./safeUrl";

describe("safeNotificationUrl", () => {
  it.each(["/d/dk1/s/s9", "/d/dk1/monos/m1", "/d/dk%201/s/a%2Fb", "/d/dk1/s/s9?x=1#top", "/d/dk1/monos/m1", "/settings", "/settings?x=1"])("accepts %s", (url) => {
    expect(safeNotificationUrl(url)).toBe(url);
  });

  it.each([
    ["absolute https", "https://evil.example/d/x"],
    ["absolute http", "http://evil.example/d/x"],
    ["protocol relative", "//evil.example/d/x"],
    ["backslash after slash", "/\\evil.example"],
    ["backslash in path", "/d/x\\..\\evil"],
    ["javascript scheme", "javascript:alert(1)"],
    ["data scheme", "data:text/html,x"],
    ["root is not a session or Mono URL", "/"],
    ["other app path", "/login"],
    ["settings lookalike", "/settingsx"],
    ["settings dot segments", "/settings/../login"],
    ["prefix lookalike", "/dx/s/1"],
    ["dot segments leave /d/", "/d/../settings"],
    ["dot segments to protocol relative", "/d/../../evil.example"],
    ["tab", "/d/a\tb"],
    ["newline", "/d/a\nb"],
    ["empty", ""],
    ["not a string", 42],
    ["too long", `/d/${"a".repeat(2100)}`],
  ])("rejects %s", (_name, url) => {
    expect(safeNotificationUrl(url)).toBeNull();
  });

  it("opens the root when there is no usable URL", () => {
    expect(clickTarget("//evil")).toBe("/");
    expect(clickTarget(undefined)).toBe("/");
    expect(clickTarget("/d/dk1/s/s9")).toBe("/d/dk1/s/s9");
  });
});
