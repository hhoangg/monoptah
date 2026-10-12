import { describe, expect, it } from "vitest";
import { parseFolderId } from "./useMailFolders";

describe("parseFolderId", () => {
  it("reads a plain positive integer", () => {
    expect(parseFolderId("12")).toBe(12);
    expect(parseFolderId("1")).toBe(1);
  });

  it.each([
    null,
    "",
    "0",
    "-3",
    "1.5",
    "1e3",
    "12abc",
    " 4",
    "NaN",
    "undefined",
  ])("is no folder for %j, never NaN", (pref) => {
    expect(parseFolderId(pref)).toBeNull();
  });

  it("refuses an id too large to be exact", () => {
    expect(parseFolderId("99999999999999999999")).toBeNull();
  });
});
