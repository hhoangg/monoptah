import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { Markdown } from "./Markdown";
import { parseMarkdown } from "./parser";

// The renderer only ever creates elements from a fixed set. Raw HTML in the input must stay text.
const ALLOWED_TAGS = new Set([
  "DIV", "P", "H1", "H2", "H3", "H4", "H5", "H6", "STRONG", "EM", "DEL", "A", "CODE", "PRE", "SPAN",
  "UL", "OL", "LI", "BLOCKQUOTE", "HR", "TABLE", "TBODY", "TR", "TH", "TD", "BUTTON", "SVG", "PATH",
  "G", "CIRCLE", "LINE", "RECT", "POLYLINE", "POLYGON", "ELLIPSE",
]);

const HTML_PAYLOADS = [
  "<script>alert(1)</script>",
  "<img src=x onerror=alert(1)>",
  "<svg onload=alert(1)><circle/></svg>",
  "<svg><script>alert(1)</script></svg>",
  "<iframe src=javascript:alert(1)></iframe>",
  "<a href=\"javascript:alert(1)\">x</a>",
  "<details open ontoggle=alert(1)>",
  "<style>*{background:url(https://evil.test/x)}</style>",
  "<math><mi xlink:href=javascript:alert(1)>x</mi></math>",
  "<object data=data:text/html,<script>alert(1)</script>>",
  "&lt;script&gt;alert(1)&lt;/script&gt;",
  "&#60;img src=x onerror=alert(1)&#62;",
  "<base href=//evil.test/>",
  "<form action=https://evil.test><input name=x>",
  "<meta http-equiv=refresh content=0;url=https://evil.test>",
];

const LINK_PAYLOADS = [
  "[x](javascript:alert(1))",
  "[x](JAVASCRIPT:alert(1))",
  "[x](java\tscript:alert(1))",
  "[x](jav&#x61;script:alert(1))",
  "[x](&#106;avascript:alert(1))",
  "[x](javascript&colon;alert(1))",
  "[x](javascript​:alert(1))",
  "[x]( javascript:alert(1))",
  "[x](data:text/html;base64,PHNjcmlwdD5hbGVydCgxKTwvc2NyaXB0Pg==)",
  "[x](vbscript:msgbox(1))",
  "[x](blob:https://a.dev/uuid)",
  "[x](//evil.test)",
  "[x](\\\\evil.test\\a)",
  "[x](https://user:pw@evil.test)",
  "[x]( javascript:alert(1))",
  "![x](https://evil.test/track.png)",
  "![x](javascript:alert(1))",
  "![](data:image/svg+xml,<svg onload=alert(1)>)",
  "[![x](https://evil.test/a.png)](javascript:alert(1))",
  "<javascript:alert(1)>",
  "[x][ref]\n\n[ref]: javascript:alert(1)",
];

function check(container: HTMLElement) {
  for (const element of container.querySelectorAll("*")) {
    expect(ALLOWED_TAGS.has(element.tagName.toUpperCase()), element.tagName).toBe(true);
    for (const attribute of element.getAttributeNames()) {
      expect(attribute.startsWith("on"), `${element.tagName} ${attribute}`).toBe(false);
    }
  }
  expect(container.querySelector("script, img, iframe, object, embed, style, form, input, base, meta, math")).toBeNull();
  for (const anchor of container.querySelectorAll("a")) {
    const href = anchor.getAttribute("href") ?? "";
    expect(href).toMatch(/^(https?:\/\/|mailto:|\/d\/)/);
    expect(href.toLowerCase()).not.toContain("javascript");
    if (/^https?:/.test(href)) {
      expect(anchor.getAttribute("target")).toBe("_blank");
      expect(anchor.getAttribute("rel")).toContain("noopener");
      expect(anchor.getAttribute("rel")).toContain("noreferrer");
    }
  }
}

describe("XSS corpus", () => {
  it.each(HTML_PAYLOADS)("keeps %j as text", (payload) => {
    const { container } = render(<Markdown text={`before ${payload} after`} />);
    check(container);
    expect(container.textContent).toContain(payload.slice(0, 8));
  });

  it.each(HTML_PAYLOADS)("keeps %j as text in a block position", (payload) => {
    const { container } = render(<Markdown text={`${payload}\n\n- ${payload}\n\n> ${payload}`} />);
    check(container);
  });

  it.each(LINK_PAYLOADS)("never makes %j a dangerous element", (payload) => {
    const { container } = render(<Markdown text={`start ${payload} end`} />);
    check(container);
  });

  it("renders a blocked link as plain text, keeping its label", () => {
    const { container } = render(<Markdown text="[click me](javascript:alert(1))" />);
    expect(container.querySelector("a")).toBeNull();
    expect(container.textContent).toBe("click me");
  });

  it("renders an image as its alt text", () => {
    const { container } = render(<Markdown text="![a chart](https://evil.test/x.png)" />);
    expect(container.querySelector("img")).toBeNull();
    expect(container.textContent).toBe("a chart");
  });

  it("gives allowed external links rel and target", () => {
    const { container } = render(<Markdown text="[ok](https://a.dev) and http://b.dev" />);
    const anchors = [...container.querySelectorAll("a")];
    expect(anchors).toHaveLength(2);
    for (const anchor of anchors) {
      expect(anchor.getAttribute("target")).toBe("_blank");
      expect(anchor.getAttribute("rel")).toBe("noopener noreferrer nofollow");
    }
    expect(anchors[1]?.dataset.insecure).toBe("true");
  });

  it("keeps HTML inside fenced code literal", () => {
    const { container } = render(<Markdown text={"```html\n<script>alert(1)</script>\n```"} />);
    check(container);
    expect(container.textContent).toContain("<script>alert(1)</script>");
  });
});

describe("hostile sizes", () => {
  const bound = (name: string, input: string, ms = 1500) => {
    const started = performance.now();
    parseMarkdown(input);
    const elapsed = performance.now() - started;
    expect(elapsed, name).toBeLessThan(ms);
  };

  it("parses huge and pathological inputs in bounded time", () => {
    bound("plain", "word ".repeat(200_000));
    bound("backticks", "`".repeat(200_000));
    bound("open brackets", "[".repeat(200_000));
    bound("open links", "[a](".repeat(50_000));
    bound("stars", "*a ".repeat(100_000));
    bound("underscores", "_a ".repeat(100_000));
    bound("quotes", ">".repeat(100_000));
    bound("nested quotes", "> ".repeat(50_000) + "x");
    bound("nested lists", "- ".repeat(50_000) + "x");
    bound("hashes", "#".repeat(100_000) + "x");
    bound("heading spaces", "# " + " ".repeat(100_000) + "x");
    bound("table spaces", "a|b\n" + " ".repeat(100_000) + "x");
    bound("many lines", "line\n".repeat(100_000));
    bound("many fences", "```\na\n```\n".repeat(30_000));
    bound("bare urls", "https://a.dev/x ".repeat(50_000));
    bound("urls glued", "https://".repeat(50_000));
    bound("rules", "- ".repeat(50_000) + "\n");
  });

  it("renders a deeply nested input without overflowing the stack", () => {
    const { container } = render(<Markdown text={"> ".repeat(5_000) + "deep"} />);
    expect(container.textContent).toContain("deep");
  });

  it("renders a very large reply in bounded time", () => {
    const text = Array.from({ length: 2000 }, (_, i) => `Paragraph ${i} with **bold**, \`code\` and [a link](https://a.dev/${i}).`).join("\n\n");
    const started = performance.now();
    const { container } = render(<Markdown text={text} />);
    expect(performance.now() - started).toBeLessThan(5000);
    expect(container.querySelectorAll("p")).toHaveLength(2000);
  });
});
