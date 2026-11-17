import { beforeEach, describe, expect, it } from "vitest";
import { loginRedirectTarget, postSignInTarget, restoreFingerprintFragment, safeRedirectTarget, saveFingerprintFragment } from "./redirect";

const FP = "ab".repeat(32);

describe("safeRedirectTarget", () => {
  it.each([
    ["/", "/"],
    ["/desktops", "/desktops"],
    ["/pair/c0de?x=1", "/pair/c0de?x=1"],
    ["/d/dk1/p/p1/files/a%20b.rs", "/d/dk1/p/p1/files/a%20b.rs"],
  ])("accepts %s", (input, expected) => {
    expect(safeRedirectTarget(input)).toBe(expected);
  });

  it.each([
    "//evil.example",
    "///evil.example",
    "/\\evil.example",
    "\\\\evil.example",
    "https://evil.example/",
    "http://evil.example",
    "javascript:alert(1)",
    "evil.example",
    "",
    "/\tfoo",
    "/\n//evil.example",
    "/%2F%2Fevil.example/../..",
    42,
    null,
    undefined,
  ])("refuses %j", (input) => {
    const result = safeRedirectTarget(input);
    // The percent-encoded case stays a same-origin path, never a protocol-relative URL.
    if (input === "/%2F%2Fevil.example/../..") expect(result?.startsWith("//")).toBe(false);
    else expect(result).toBeNull();
  });

  it("builds the redirect parameter without the hash", () => {
    expect(loginRedirectTarget("/pair/c0de", "?a=1")).toBe("/pair/c0de?a=1");
  });
});

describe("fingerprint fragment", () => {
  beforeEach(() => sessionStorage.clear());

  it("saves #fp and restores it once for the same path", () => {
    saveFingerprintFragment("/pair/c0de", `#fp=${FP}`);
    expect(restoreFingerprintFragment("/other")).toBeNull();
    expect(restoreFingerprintFragment("/pair/c0de")).toBe(`#fp=${FP}`);
    expect(restoreFingerprintFragment("/pair/c0de")).toBeNull();
  });

  it("ignores a hash that is not a fingerprint", () => {
    saveFingerprintFragment("/pair/c0de", "#invite=X");
    saveFingerprintFragment("/pair/c0de", "#fp=<script>");
    expect(sessionStorage.length).toBe(0);
  });

  it("postSignInTarget restores #fp on the redirect target and falls back to /", () => {
    saveFingerprintFragment("/pair/c0de", `fp=${FP}`);
    expect(postSignInTarget("/pair/c0de")).toBe(`/pair/c0de#fp=${FP}`);
    expect(postSignInTarget("//evil.example")).toBe("/");
    expect(postSignInTarget(undefined)).toBe("/");
  });
});
