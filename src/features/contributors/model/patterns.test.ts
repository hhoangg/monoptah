import { describe, expect, it } from "vitest";
import {
  globToRegExp,
  isIgnoredPath,
  matchesAnyIdentity,
  matchGlob,
  matchPattern,
} from "./patterns";

describe("matchPattern", () => {
  it("matches the whole string only", () => {
    expect(matchPattern("abc", "abc")).toBe(true);
    expect(matchPattern("abc", "abcd")).toBe(false);
    expect(matchPattern("abc", "xabc")).toBe(false);
  });

  it("is case-insensitive on both sides", () => {
    expect(matchPattern("Foo@Bar.COM", "foo@bar.com")).toBe(true);
    expect(matchPattern("*@ANTHROPIC.com", "Noreply@Anthropic.Com")).toBe(true);
  });

  it("supports * as any run of characters including empty", () => {
    expect(matchPattern("*claude*", "Claude Code")).toBe(true);
    expect(matchPattern("*claude*", "claude")).toBe(true);
    expect(matchPattern("*@ci-runner*", "build@ci-runner-12.corp")).toBe(true);
    expect(matchPattern("a*c", "ac")).toBe(true);
    expect(matchPattern("a*c", "abbbc")).toBe(true);
    expect(matchPattern("a*c", "abbbd")).toBe(false);
  });

  it("treats every other character literally", () => {
    expect(matchPattern("*[bot]*", "dependabot[bot]")).toBe(true);
    expect(matchPattern("*[bot]*", "dependabotb")).toBe(false);
    expect(matchPattern("a.c", "abc")).toBe(false);
    expect(matchPattern("a.c", "a.c")).toBe(true);
    expect(matchPattern("a+b", "a+b")).toBe(true);
    expect(matchPattern("a?b", "axb")).toBe(false);
    expect(matchPattern("(x)", "(x)")).toBe(true);
  });

  it("never matches an empty pattern", () => {
    expect(matchPattern("", "")).toBe(false);
    expect(matchPattern("", "abc")).toBe(false);
    expect(matchPattern("   ", "abc")).toBe(false);
  });

  it("trims the pattern", () => {
    expect(matchPattern("  abc ", "abc")).toBe(true);
  });

  it("handles unicode", () => {
    expect(matchPattern("*Đặng*", "Nguyễn Đặng Hoàng")).toBe(true);
    expect(matchPattern("hoàng", "HOÀNG")).toBe(true);
  });
});

describe("matchesAnyIdentity", () => {
  const bot = {
    name: "dependabot[bot]",
    email: "49699333+dependabot[bot]@users.noreply.github.com",
  };

  it("matches on email", () => {
    expect(
      matchesAnyIdentity(["noreply@github.com"], {
        name: "GitHub",
        email: "noreply@github.com",
      }),
    ).toBe(true);
  });

  it("matches on name", () => {
    expect(
      matchesAnyIdentity(["*claude*"], { name: "Claude", email: "x@y.z" }),
    ).toBe(true);
  });

  it("matches on login when present", () => {
    expect(
      matchesAnyIdentity(["copilot-swe-agent[bot]"], {
        name: "",
        email: "",
        login: "copilot-swe-agent[bot]",
      }),
    ).toBe(true);
  });

  it("matches the gh:<login> key form", () => {
    expect(
      matchesAnyIdentity(["gh:alice"], { name: "", email: "", login: "Alice" }),
    ).toBe(true);
  });

  it("matches when any pattern matches", () => {
    expect(matchesAnyIdentity(["nope", "*[bot]*"], bot)).toBe(true);
    expect(matchesAnyIdentity(["nope", "other"], bot)).toBe(false);
  });

  it("returns false for no patterns", () => {
    expect(matchesAnyIdentity([], bot)).toBe(false);
  });

  it("does not let a wildcard match an empty email or name", () => {
    expect(matchesAnyIdentity(["*"], { name: "", email: "" })).toBe(false);
  });

  it("ignores blank patterns", () => {
    expect(matchesAnyIdentity(["", "  "], bot)).toBe(false);
  });
});

describe("matchGlob", () => {
  it("matches a slash-less glob against the basename at any depth", () => {
    expect(matchGlob("*.lock", "yarn.lock")).toBe(true);
    expect(matchGlob("*.lock", "a/b/yarn.lock")).toBe(true);
    expect(matchGlob("*.lock", "yarn.lock.bak")).toBe(false);
    expect(matchGlob("package-lock.json", "web/package-lock.json")).toBe(true);
    expect(matchGlob("package-lock.json", "web/my-package-lock.json")).toBe(
      false,
    );
    expect(matchGlob("Cargo.lock", "src-tauri/Cargo.lock")).toBe(true);
  });

  it("does not match a slash-less glob against directory names", () => {
    expect(matchGlob("*.lock", "a.lock/readme.md")).toBe(false);
  });

  it("anchors a glob that contains a slash at the root", () => {
    expect(matchGlob("dist/**", "dist/a.js")).toBe(true);
    expect(matchGlob("dist/**", "dist/a/b/c.js")).toBe(true);
    expect(matchGlob("dist/**", "packages/dist/a.js")).toBe(false);
    expect(matchGlob("dist/**", "distant/a.js")).toBe(false);
  });

  it("does not match the directory name itself for dir/**", () => {
    expect(matchGlob("dist/**", "dist")).toBe(false);
  });

  it("treats * as any chars except slash", () => {
    expect(matchGlob("src/*.ts", "src/a.ts")).toBe(true);
    expect(matchGlob("src/*.ts", "src/a/b.ts")).toBe(false);
    expect(matchGlob("src/a*", "src/abc")).toBe(true);
  });

  it("treats ** as any chars including slash", () => {
    expect(matchGlob("src/**/x.ts", "src/x.ts")).toBe(true);
    expect(matchGlob("src/**/x.ts", "src/a/x.ts")).toBe(true);
    expect(matchGlob("src/**/x.ts", "src/a/b/x.ts")).toBe(true);
    expect(matchGlob("src/**/x.ts", "src/a/b/y.ts")).toBe(false);
    expect(matchGlob("src/**/x.ts", "other/a/x.ts")).toBe(false);
  });

  it("lets **/x match x at any depth including the root", () => {
    expect(matchGlob("**/x", "x")).toBe(true);
    expect(matchGlob("**/x", "a/x")).toBe(true);
    expect(matchGlob("**/x", "a/b/x")).toBe(true);
    expect(matchGlob("**/x", "a/xx")).toBe(false);
    expect(matchGlob("**/*.snap", "a.snap")).toBe(true);
    expect(matchGlob("**/*.snap", "src/__s__/a.snap")).toBe(true);
    expect(matchGlob("**/*.min.js", "public/app.min.js")).toBe(true);
    expect(matchGlob("**/*.min.js", "app.min.js")).toBe(true);
    expect(matchGlob("**/*.min.js", "app.min.jsx")).toBe(false);
  });

  it("supports **/dir/** at any depth", () => {
    expect(matchGlob("**/generated/**", "generated/a.ts")).toBe(true);
    expect(matchGlob("**/generated/**", "src/generated/a/b.ts")).toBe(true);
    expect(matchGlob("**/generated/**", "src/notgenerated/a.ts")).toBe(false);
  });

  it("supports * inside a slash-less glob with several wildcards", () => {
    expect(matchGlob("*.generated.*", "api.generated.ts")).toBe(true);
    expect(matchGlob("*.generated.*", "src/api.generated.ts")).toBe(true);
    expect(matchGlob("*.generated.*", "api.generated")).toBe(false);
  });

  it("supports ? as a single non-slash character", () => {
    expect(matchGlob("a?c.ts", "abc.ts")).toBe(true);
    expect(matchGlob("a?c.ts", "ac.ts")).toBe(false);
    expect(matchGlob("a?c.ts", "abbc.ts")).toBe(false);
    expect(matchGlob("src/a?c", "src/a/c")).toBe(false);
  });

  it("escapes regex metacharacters", () => {
    expect(matchGlob("a.b", "a.b")).toBe(true);
    expect(matchGlob("a.b", "axb")).toBe(false);
    expect(matchGlob("(a)+.ts", "(a)+.ts")).toBe(true);
    expect(matchGlob("a[1].ts", "a[1].ts")).toBe(true);
    expect(matchGlob("a[1].ts", "a1.ts")).toBe(false);
    expect(matchGlob("a$b^c|d", "a$b^c|d")).toBe(true);
  });

  it("strips a leading slash and anchors", () => {
    expect(matchGlob("/build/**", "build/a.js")).toBe(true);
    expect(matchGlob("/build/**", "x/build/a.js")).toBe(false);
    expect(matchGlob("/README.md", "README.md")).toBe(true);
    expect(matchGlob("/README.md", "docs/README.md")).toBe(false);
  });

  it("treats a trailing slash as a whole directory", () => {
    expect(matchGlob("vendor/", "vendor/a/b.go")).toBe(true);
    expect(matchGlob("vendor/", "other/vendor/a.go")).toBe(false);
  });

  it("matches everything for a lone **", () => {
    expect(matchGlob("**", "a")).toBe(true);
    expect(matchGlob("**", "a/b/c")).toBe(true);
  });

  it("normalizes backslashes and a leading ./ in the path", () => {
    expect(matchGlob("dist/**", "dist\\a\\b.js")).toBe(true);
    expect(matchGlob("dist/**", "./dist/a.js")).toBe(true);
  });

  it("is case-sensitive like git", () => {
    expect(matchGlob("*.lock", "YARN.LOCK")).toBe(false);
  });

  it("never matches a blank glob", () => {
    expect(matchGlob("", "a")).toBe(false);
    expect(matchGlob("  ", "a")).toBe(false);
  });

  it("handles unicode paths", () => {
    expect(matchGlob("docs/**", "docs/tài-liệu/hướng-dẫn.md")).toBe(true);
    expect(matchGlob("*.md", "ghi-chú.md")).toBe(true);
  });
});

describe("globToRegExp", () => {
  it("returns an anchored RegExp", () => {
    const re = globToRegExp("*.lock");
    expect(re).toBeInstanceOf(RegExp);
    expect(re.test("a.lock")).toBe(true);
    expect(re.test("a.lock.x")).toBe(false);
  });

  it("returns the same compiled RegExp for the same glob", () => {
    expect(globToRegExp("*.lock")).toBe(globToRegExp("*.lock"));
    expect(globToRegExp("*.lock")).not.toBe(globToRegExp("*.snap"));
  });

  it("is stateless across repeated test calls", () => {
    const re = globToRegExp("*.lock");
    expect(re.test("a.lock")).toBe(true);
    expect(re.test("a.lock")).toBe(true);
  });

  it("collapses runs of three or more stars to **", () => {
    expect(globToRegExp("a/***/b").test("a/x/y/b")).toBe(true);
  });
});

describe("isIgnoredPath", () => {
  const globs = ["*.lock", "dist/**", "**/*.snap"];

  it("is true when any glob matches", () => {
    expect(isIgnoredPath(globs, "Cargo.lock")).toBe(true);
    expect(isIgnoredPath(globs, "dist/a.js")).toBe(true);
    expect(isIgnoredPath(globs, "src/__snapshots__/a.snap")).toBe(true);
  });

  it("is false when none match", () => {
    expect(isIgnoredPath(globs, "src/a.ts")).toBe(false);
  });

  it("is false for no globs", () => {
    expect(isIgnoredPath([], "src/a.ts")).toBe(false);
  });

  it("skips blank globs", () => {
    expect(isIgnoredPath(["", " "], "src/a.ts")).toBe(false);
  });
});
