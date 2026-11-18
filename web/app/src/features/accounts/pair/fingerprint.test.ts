import { describe, expect, it } from "vitest";
import { canonicalFingerprint, checkFingerprint, fingerprintFromHash, fingerprintLines } from "./fingerprint";
import { safeLabel } from "./untrusted";

const FP = "0123456789abcdef".repeat(4);

describe("checkFingerprint", () => {
  it("matches the full fingerprint, ignoring case and separators", () => {
    expect(checkFingerprint(`#fp=${FP}`, FP)).toBe("match");
    expect(checkFingerprint(`fp=${FP.toUpperCase()}`, FP)).toBe("match");
    expect(checkFingerprint(`#fp=${FP.match(/.{4}/g)?.join(" ")}`, FP)).toBe("match");
  });

  it("is a mismatch for a different value, a prefix or garbage", () => {
    expect(checkFingerprint(`#fp=${"f".repeat(64)}`, FP)).toBe("mismatch");
    expect(checkFingerprint(`#fp=${FP.slice(0, 32)}`, FP)).toBe("mismatch");
    expect(checkFingerprint("#fp=not-hex", FP)).toBe("mismatch");
    expect(checkFingerprint(`#fp=${FP.slice(0, 63)}g`, FP)).toBe("mismatch");
  });

  it("is missing without an fp value", () => {
    expect(checkFingerprint("", FP)).toBe("missing");
    expect(checkFingerprint("#", FP)).toBe("missing");
    expect(checkFingerprint("#fp=", FP)).toBe("missing");
    expect(checkFingerprint("#other=1", FP)).toBe("missing");
    expect(fingerprintFromHash("#fp=abc")).toBe("abc");
  });
});

describe("formatting", () => {
  it("shows the first 32 hex chars as 8 groups of 4 in two lines", () => {
    expect(fingerprintLines(FP)).toEqual(["0123 4567 89ab cdef", "0123 4567 89ab cdef"]);
    expect(canonicalFingerprint(" AB:cd-12 ")).toBe("abcd12");
  });
});

describe("safeLabel", () => {
  it("drops hidden characters, collapses space and truncates", () => {
    expect(safeLabel("a\u202Eb\u200Bc\n\td")).toBe("abc d");
    expect(safeLabel("x".repeat(100), 10)).toBe(`${"x".repeat(9)}…`);
    expect(safeLabel("   \u202E ", 10, "Unnamed")).toBe("Unnamed");
  });
});
