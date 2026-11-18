import { describe, expect, it } from "vitest";
import { parsePairingInput } from "./pairingLink";

const ORIGIN = "https://app.example";
const FP = "a".repeat(64);

describe("parsePairingInput", () => {
  it("accepts a same-origin pair link and keeps #fp", () => {
    expect(parsePairingInput(`${ORIGIN}/pair/ABCD2345#fp=${FP}`, ORIGIN)).toEqual({ code: "ABCD2345", fp: FP });
  });

  it("accepts a same-origin pair link without #fp", () => {
    expect(parsePairingInput(`${ORIGIN}/pair/ABCD2345`, ORIGIN)).toEqual({ code: "ABCD2345" });
  });

  it("accepts a bare code, in any case, with spaces or a hyphen", () => {
    expect(parsePairingInput("abcd2345", ORIGIN)).toEqual({ code: "ABCD2345" });
    expect(parsePairingInput("  ABCD-2345 ", ORIGIN)).toEqual({ code: "ABCD2345" });
  });

  it("rejects a link of another origin, even with a valid code", () => {
    expect(parsePairingInput(`https://evil.example/pair/ABCD2345#fp=${FP}`, ORIGIN)).toBeNull();
    expect(parsePairingInput(`http://app.example/pair/ABCD2345`, ORIGIN)).toBeNull();
    expect(parsePairingInput(`https://app.example.evil.example/pair/ABCD2345`, ORIGIN)).toBeNull();
  });

  it("rejects other paths, malformed codes, and empty input", () => {
    for (const input of ["", "   ", `${ORIGIN}/login`, `${ORIGIN}/pair/ABCD2345/extra`, `${ORIGIN}/pair/ABC`, `${ORIGIN}/pair/ABCD0O1I`, "ABCD234", "ABCD23456", "javascript:alert(1)", "not a url"]) {
      expect(parsePairingInput(input, ORIGIN), input).toBeNull();
    }
  });
});
