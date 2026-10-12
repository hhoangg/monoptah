import { describe, expect, it } from "vitest";
import { buildTextQuote } from "./mailQuote";

describe("buildTextQuote", () => {
  it("prefixes every line and keeps blank lines as a bare marker", () => {
    expect(buildTextQuote("one\n\ntwo", "A wrote:")).toBe(
      "A wrote:\n> one\n>\n> two",
    );
  });

  it("nests an already quoted line one level deeper", () => {
    expect(buildTextQuote("> earlier", "A wrote:")).toBe(
      "A wrote:\n> > earlier",
    );
  });

  it("normalises CRLF and drops trailing blank lines", () => {
    expect(buildTextQuote("a\r\nb\r\n\r\n", "A wrote:")).toBe(
      "A wrote:\n> a\n> b",
    );
  });

  it("quotes nothing for a message without a text body", () => {
    expect(buildTextQuote(null, "A wrote:")).toBe("A wrote:\n>");
  });
});
