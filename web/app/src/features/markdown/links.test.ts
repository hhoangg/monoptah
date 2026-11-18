import { describe, expect, it } from "vitest";
import { inlineFileReference, resolveFileReference } from "./fileLinks";
import { classifyLink } from "./links";

describe("classifyLink", () => {
  it("allows https, http (flagged), mailto and in-app paths", () => {
    expect(classifyLink("https://a.dev/x?y=1&z=2")).toEqual({
      kind: "external",
      href: "https://a.dev/x?y=1&z=2",
      insecure: false,
    });
    expect(classifyLink("http://a.dev")).toMatchObject({ kind: "external", insecure: true });
    expect(classifyLink("mailto:me@a.dev")).toMatchObject({ kind: "mailto" });
    expect(classifyLink("/d/abc/s/1")).toEqual({ kind: "internal", href: "/d/abc/s/1" });
  });

  it.each([
    "javascript:alert(1)",
    "JaVaScRiPt:alert(1)",
    " javascript:alert(1)",
    "java\tscript:alert(1)",
    "java\nscript:alert(1)",
    "javascript&colon;alert(1)",
    "jav&#x61;script:alert(1)",
    "&#106;avascript:alert(1)",
    "javascript​:alert(1)",
    "\u0000javascript:alert(1)",
    "ｊavascript:alert(1)",
    "javascript%3Aalert(1)",
    "data:text/html,<script>alert(1)</script>",
    "DATA:text/html;base64,AAAA",
    "vbscript:msgbox(1)",
    "blob:https://a.dev/uuid",
    "//evil.test/x",
    "\\\\evil.test\\x",
    "/\\evil.test",
    "https:evil.test",
    "https://user:pass@evil.test/",
    "ftp://a.dev/x",
    "tel:+123",
    "#fragment",
    "/etc/passwd",
    "/d/../../x",
    "",
  ])("rejects %j", (href) => {
    expect(classifyLink(href)).toBeNull();
  });

  it("recognises file references", () => {
    expect(classifyLink("src/a.ts:12")).toEqual({ kind: "file", path: "src/a.ts", line: 12 });
    expect(classifyLink("/home/me/app.ts#L7")).toEqual({
      kind: "file",
      path: "/home/me/app.ts",
      line: 7,
    });
    expect(classifyLink("app.ts:3")).toMatchObject({ kind: "file", path: "app.ts", line: 3 });
    expect(classifyLink("file:///home/me/a.rs")).toMatchObject({ kind: "file", path: "/home/me/a.rs" });
  });
});

describe("file references", () => {
  it("reads line and column suffixes", () => {
    expect(resolveFileReference("src/a.ts:12:4")).toEqual({ path: "src/a.ts", line: 12, column: 4 });
    expect(resolveFileReference("README.md")).toEqual({ path: "README.md" });
  });

  it("refuses non-files and traversal", () => {
    expect(resolveFileReference("hello")).toBeNull();
    expect(resolveFileReference("../../secret.txt")).toBeNull();
    expect(resolveFileReference("a b.ts")).toBeNull();
  });

  it("names a file from an inline code span", () => {
    expect(inlineFileReference("src/main.rs:5")).toEqual({ path: "src/main.rs", line: 5 });
    expect(inlineFileReference("npm run build")).toBeNull();
    expect(inlineFileReference("foo.bar()")).toBeNull();
  });
});
