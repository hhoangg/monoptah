import { describe, expect, it } from "vitest";
import { PROJECT_COLORS, projectColor } from "./projectColors";

// Expected values were computed by running the mobile function
// (mobile/src/features/workspace/projectColors.ts) on these names.
const MOBILE_VALUES: ReadonlyArray<[string, string]> = [
  ["monoptah", "hsl(175 55% 48%)"],
  ["Example Alpha", "hsl(211 92% 62%)"],
  ["Example Beta", "hsl(175 55% 48%)"],
  ["", "hsl(211 92% 62%)"],
  ["a", "hsl(12 80% 58%)"],
  ["web", "hsl(330 70% 62%)"],
  ["mobile", "hsl(45 90% 55%)"],
  ["héllo wörld 🚀", "hsl(45 90% 55%)"],
  ["/Users/dev/projects/very-long-project-name-0123456789", "hsl(330 70% 62%)"],
];

describe("projectColor (F-16)", () => {
  it.each(MOBILE_VALUES)("gives the mobile colour for %j", (name, expected) => {
    expect(projectColor(name)).toBe(expected);
  });

  it("never returns the neutral first entry", () => {
    for (let i = 0; i < 200; i++) expect(projectColor(`project-${i}`)).not.toBe(PROJECT_COLORS[0]);
  });
});
