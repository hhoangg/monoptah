import { describe, expect, it } from "vitest";
import { fuzzyMatch, rankBy, scorePath } from "./fuzzy";

describe("fuzzyMatch", () => {
  it("matches characters in order and reports positions", () => {
    expect(fuzzyMatch("abc", "a_b_c")?.positions).toEqual([0, 2, 4]);
  });

  it("returns null when characters are missing or out of order", () => {
    expect(fuzzyMatch("abc", "acb")).toBeNull();
    expect(fuzzyMatch("z", "abc")).toBeNull();
  });

  it("matches everything for an empty query", () => {
    expect(fuzzyMatch("  ", "anything")).toEqual({ score: 0, positions: [] });
  });

  it("is case-insensitive", () => {
    expect(fuzzyMatch("READ", "readme.md")?.positions).toEqual([0, 1, 2, 3]);
  });

  it("requires every whitespace token and merges sorted positions", () => {
    const hit = fuzzyMatch("md read", "readme.md");
    expect(hit?.positions).toEqual([0, 1, 2, 3, 4, 8]);
    expect(fuzzyMatch("md zzz", "readme.md")).toBeNull();
  });

  it("ranks consecutive runs and word starts higher", () => {
    const tight = fuzzyMatch("foo", "foo.ts")!;
    const loose = fuzzyMatch("foo", "fxoxo.ts")!;
    expect(tight.score).toBeGreaterThan(loose.score);
    const boundary = fuzzyMatch("b", "a-b")!;
    const middle = fuzzyMatch("b", "aab")!;
    expect(boundary.score).toBeGreaterThan(middle.score);
  });

  it("rewards camelCase boundaries", () => {
    const camel = fuzzyMatch("fb", "fooBar")!;
    const flat = fuzzyMatch("fb", "foobar")!;
    expect(camel.score).toBeGreaterThan(flat.score);
  });
});

describe("scorePath", () => {
  it("prefers file name hits and indexes the full relative path", () => {
    const name = scorePath("app", "src/app.ts", "app.ts")!;
    expect(name.positions).toEqual([4, 5, 6]);
    const dirOnly = scorePath("app", "app/index.ts", "index.ts")!;
    expect(name.score).toBeGreaterThan(dirOnly.score);
    expect(dirOnly.positions).toEqual([0, 1, 2]);
  });

  it("returns an empty hit for an empty query", () => {
    expect(scorePath("", "a/b", "b")).toEqual({ score: 0, positions: [] });
  });
});

describe("rankBy", () => {
  it("filters and sorts best first, keeping input order for ties", () => {
    const items = ["a_b", "zzz", "ab", "ab"];
    const ranked = rankBy("ab", items, (s) => s);
    expect(ranked.map((r) => r.item)).toEqual(["a_b", "ab", "ab"]);
    expect(ranked.map((r) => r.hit.score)).toEqual([...ranked.map((r) => r.hit.score)].sort((a, b) => b - a));
  });
});
