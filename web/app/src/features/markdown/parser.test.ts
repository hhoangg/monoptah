// Ported from mobile/tests/markdown.test.ts (the reply markdown tests), plus web additions.
import { describe, expect, it } from "vitest";
import { parseInline, parseMarkdown, plainText } from "./parser";

describe("reply markdown", () => {
  it("reads the blocks agents write", () => {
    const blocks = parseMarkdown(
      [
        "## Summary",
        "",
        "Fixed the **bug** in `app.ts`.",
        "",
        "- first",
        "- second",
        "",
        "1. one",
        "2. two",
        "",
        "```ts",
        "const a = 1;",
        "```",
        "",
        "> quoted",
        "",
        "| a | b |",
        "| - | - |",
        "| 1 | 2 |",
      ].join("\n"),
    );
    expect(blocks.map((block) => block.type)).toEqual([
      "heading",
      "paragraph",
      "list",
      "list",
      "code",
      "quote",
      "table",
    ]);
    expect(blocks[4]).toEqual({ type: "code", lang: "ts", text: "const a = 1;" });
    expect(blocks[3]).toMatchObject({ ordered: true, start: 1 });
    expect(blocks[6]).toMatchObject({ rows: { length: 2 } });
  });

  it("keeps an unclosed fence open while the reply streams", () => {
    expect(parseMarkdown("```\nlet x")).toEqual([{ type: "code", lang: "", text: "let x" }]);
  });

  it("reads emphasis without breaking snake_case or stray stars", () => {
    expect(parseInline("a **b** and *c* in snake_case_name")).toEqual([
      { type: "text", text: "a " },
      { type: "strong", children: [{ type: "text", text: "b" }] },
      { type: "text", text: " and " },
      { type: "em", children: [{ type: "text", text: "c" }] },
      { type: "text", text: " in snake_case_name" },
    ]);
    expect(parseInline("2 * 3 * 4")).toEqual([{ type: "text", text: "2 * 3 * 4" }]);
  });

  it("reads links, bare URLs and inline code", () => {
    expect(parseInline("see [docs](https://a.dev) or https://b.dev.")).toEqual([
      { type: "text", text: "see " },
      { type: "link", href: "https://a.dev", children: [{ type: "text", text: "docs" }] },
      { type: "text", text: " or " },
      { type: "link", href: "https://b.dev", children: [{ type: "text", text: "https://b.dev" }] },
      { type: "text", text: "." },
    ]);
    expect(parseInline("`a*b*c`")).toEqual([{ type: "code", text: "a*b*c" }]);
  });
});

describe("web additions", () => {
  it("shows an image as its alt text and never as a node that loads", () => {
    expect(parseInline("a ![diagram](https://evil.test/x.png) b")).toEqual([
      { type: "text", text: "a diagram b" },
    ]);
  });

  it("keeps raw HTML as literal text", () => {
    expect(parseInline("<b>hi</b> <script>alert(1)</script>")).toEqual([
      { type: "text", text: "<b>hi</b> <script>alert(1)</script>" },
    ]);
  });

  it("strips closing hashes from a heading", () => {
    expect(parseMarkdown("## Title ##")[0]).toMatchObject({
      type: "heading",
      level: 2,
      inline: [{ type: "text", text: "Title" }],
    });
  });

  it("plainText drops the markdown", () => {
    expect(plainText("# A\n\n**b** `c`")).toBe("A b c");
  });
});

describe("hostile input stays fast", () => {
  it("reads a long backtick run in linear time (no close found)", () => {
    const started = performance.now();
    const nodes = parseInline("x" + "`".repeat(80_000));
    expect(performance.now() - started).toBeLessThan(500);
    expect(nodes.map((node) => (node.type === "text" ? node.text.length : 0)).reduce((a, b) => a + b, 0)).toBe(80_001);
    const cell = performance.now();
    parseMarkdown("| a | " + "`".repeat(80_000) + "\n| - | - |\n| b | c |");
    expect(performance.now() - cell).toBeLessThan(500);
  });

  it("does not nest a link inside a link label", () => {
    const [link] = parseInline("[https://a.dev](https://b.dev)");
    expect(link).toMatchObject({ type: "link", href: "https://b.dev", children: [{ type: "text", text: "https://a.dev" }] });
  });
});
