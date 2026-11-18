import { describe, expect, it } from "vitest";
import { osLabel, versionLabel } from "./format";

describe("osLabel", () => {
  it("maps the OS ids a desktop reports to product names", () => {
    expect(osLabel("macos")).toBe("macOS");
    expect(osLabel("darwin")).toBe("macOS");
    expect(osLabel("windows")).toBe("Windows");
    expect(osLabel("linux")).toBe("Linux");
  });

  it("keeps an unknown id as plain text", () => {
    expect(osLabel("freebsd")).toBe("freebsd");
  });

  it("feeds versionLabel", () => {
    expect(versionLabel("macos", "0.16.0")).toBe("macOS, v0.16.0");
  });
});
