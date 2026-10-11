// @vitest-environment happy-dom
import { describe, expect, it } from "vitest";
import { cidReferences } from "./mailCid";
import { contentIdFor, serializeCompose, type TipTapNode } from "./mailCompose";

const t = (text: string, marks?: TipTapNode["marks"]): TipTapNode => ({
  type: "text",
  text,
  ...(marks ? { marks } : {}),
});
const p = (...content: TipTapNode[]): TipTapNode => ({
  type: "paragraph",
  content,
});
const li = (...content: TipTapNode[]): TipTapNode => ({
  type: "listItem",
  content,
});
const doc = (...content: TipTapNode[]): TipTapNode => ({
  type: "doc",
  content,
});
const link = (href: string) => [{ type: "link", attrs: { href } }];
const run = (d: TipTapNode, quotedHtml?: string) =>
  serializeCompose({ bodyJson: d, draftId: "d1", quotedHtml });

/** Every element tag in the output, with its style attribute. */
function elements(html: string) {
  return [
    ...new DOMParser()
      .parseFromString(html, "text/html")
      .body.querySelectorAll("*"),
  ];
}

const everything = doc(
  { type: "heading", attrs: { level: 1 }, content: [t("H1")] },
  { type: "heading", attrs: { level: 2 }, content: [t("H2")] },
  { type: "heading", attrs: { level: 3 }, content: [t("H3")] },
  p(
    t("a", [{ type: "bold" }]),
    t("b", [{ type: "italic" }]),
    t("c", [{ type: "underline" }]),
    t("d", [{ type: "strike" }]),
    t("e", [{ type: "code" }]),
    { type: "hardBreak" },
    t("link", link("https://x.com")),
  ),
  { type: "bulletList", content: [li(p(t("one")))] },
  { type: "orderedList", content: [li(p(t("two")))] },
  { type: "blockquote", content: [p(t("quoted"))] },
  { type: "codeBlock", content: [t("let x = 1;")] },
  { type: "horizontalRule" },
  {
    type: "image",
    attrs: { src: "blob:1", alt: "pic", "data-mono-att": "7" },
  },
);

describe("mailCompose html", () => {
  it("emits an inline style on every element of every supported node", () => {
    const { html } = run(everything);
    const els = elements(html);
    const tags = new Set(els.map((e) => e.tagName.toLowerCase()));
    for (const tag of [
      "h1",
      "h2",
      "h3",
      "p",
      "strong",
      "em",
      "u",
      "s",
      "code",
      "a",
      "br",
      "ul",
      "ol",
      "li",
      "blockquote",
      "pre",
      "hr",
      "img",
    ]) {
      expect(tags.has(tag), `missing <${tag}>`).toBe(true);
    }
    for (const el of els) {
      if (el.tagName === "BR") continue;
      expect(
        el.getAttribute("style"),
        `<${el.tagName}> has no style`,
      ).toBeTruthy();
    }
  });

  it("escapes user text so typed markup survives and & does not corrupt", () => {
    const { html, text } = run(doc(p(t('<b>x</b> & "y"'))));
    expect(html).toContain("&lt;b&gt;x&lt;/b&gt; &amp; &quot;y&quot;");
    expect(html).not.toContain("<b>");
    expect(text).toBe('<b>x</b> & "y"');
  });

  it("drops unsafe link schemes but keeps the text", () => {
    const { html } = run(doc(p(t("click", link("javascript:alert(1)")))));
    expect(html).not.toContain("javascript:");
    expect(html).toContain("click");
  });

  it("does not render a trailing empty paragraph", () => {
    const a = run(doc(p(t("hi")))).html;
    const b = run(doc(p(t("hi")), p())).html;
    expect(b).toBe(a);
  });

  it("renders an unknown node visibly instead of dropping it", () => {
    const withText = run(
      doc({ type: "callout", content: [p(t("kept words"))] }),
    );
    expect(withText.html).toContain("kept words");
    expect(withText.text).toContain("kept words");
    const leaf = run(doc({ type: "mystery" }));
    expect(leaf.html).toContain("[mystery]");
    expect(leaf.text).toBe("[mystery]");
  });

  it("appends the quoted original unchanged, after the body", () => {
    const quoted =
      '<blockquote class="q"><p onclick="x">  raw & odd </p></blockquote>';
    const { html } = run(doc(p(t("reply"))), quoted);
    expect(html).toContain(quoted);
    expect(html.indexOf("reply")).toBeLessThan(html.indexOf(quoted));
  });

  it("appends the quoted text unchanged after the body, in the text part only", () => {
    const quotedText = "A wrote:\n> raw & <odd>\n>\n>  spaced";
    const out = serializeCompose({
      bodyJson: doc(p(t("reply"))),
      draftId: "d1",
      quotedHtml: "<p>html copy</p>",
      quotedText,
    });
    expect(out.text).toBe(`reply\n\n${quotedText}`);
    expect(out.html).not.toContain("A wrote:");
    expect(out.html).toContain("<p>html copy</p>");
  });

  it("sends the quoted text alone when the reply body is empty", () => {
    const out = serializeCompose({
      bodyJson: doc({ type: "paragraph" }),
      draftId: "d1",
      quotedText: "A wrote:\n> hi",
    });
    expect(out.text).toBe("A wrote:\n> hi");
  });
});

describe("mailCompose text", () => {
  it("keeps list markers, quote prefixes and appends link URLs", () => {
    const { text } = run(
      doc(
        { type: "bulletList", content: [li(p(t("a"))), li(p(t("b")))] },
        {
          type: "orderedList",
          attrs: { start: 1 },
          content: [li(p(t("c"))), li(p(t("d")))],
        },
        { type: "blockquote", content: [p(t("q1")), p(t("q2"))] },
        p(t("site", link("https://x.com"))),
      ),
    );
    expect(text).toBe(
      "- a\n- b\n\n1. c\n2. d\n\n> q1\n>\n> q2\n\nsite (https://x.com)",
    );
  });

  it("does not repeat a URL that is already the link text", () => {
    const { text } = run(doc(p(t("https://x.com", link("https://x.com")))));
    expect(text).toBe("https://x.com");
    const mail = run(doc(p(t("a@b.co", link("mailto:a@b.co")))));
    expect(mail.text).toBe("a@b.co");
  });

  it("appends a URL once for a link split across marks", () => {
    const { text } = run(
      doc(
        p(
          t("bold", [...link("https://x.com"), { type: "bold" }]),
          t(" plain", link("https://x.com")),
        ),
      ),
    );
    expect(text).toBe("bold plain (https://x.com)");
  });

  it("indents nested lists by the parent marker width, two levels", () => {
    const ol = (...c: TipTapNode[]): TipTapNode => ({
      type: "orderedList",
      content: c,
    });
    const ul = (...c: TipTapNode[]): TipTapNode => ({
      type: "bulletList",
      content: c,
    });
    const { text } = run(
      doc(
        ul(
          li(
            p(t("a")),
            ol(li(p(t("a1"))), li(p(t("a2")), ul(li(p(t("deep")))))),
          ),
          li(p(t("b"))),
        ),
      ),
    );
    expect(text).toBe(
      ["- a", "  1. a1", "  2. a2", "     - deep", "- b"].join("\n"),
    );
    const flipped = run(doc(ol(li(p(t("x")), ul(li(p(t("y"))))))));
    expect(flipped.text).toBe("1. x\n   - y");
  });
});

const img = (id: string): TipTapNode => ({
  type: "image",
  attrs: { src: "blob:abc", alt: "pic", "data-mono-att": id },
});

describe("mailCompose hardening", () => {
  it("escapes attribute and code-block sites individually", () => {
    const out = run(
      doc(
        p(t("x", link('https://e.com/" onmouseover="x'))),
        {
          type: "image",
          attrs: { alt: '" onerror="x', "data-mono-att": "1" },
        },
        { type: "image", attrs: { alt: '<i onerror="x">' } },
        { type: "codeBlock", content: [t("</pre><script>")] },
        { type: "<x onload=1>" },
      ),
    );
    expect(out.html).toContain("&quot; onmouseover=&quot;x");
    expect(out.html).toContain('alt="&quot; onerror=&quot;x"');
    expect(out.html).toContain("[image: &lt;i onerror=&quot;x&quot;&gt;]");
    expect(out.html).toContain("&lt;/pre&gt;&lt;script&gt;");
    expect(out.html).toContain("[&lt;x onload=1&gt;]");
    expect(out.html).not.toContain("<script");
    expect(elements(out.html).some((e) => e.hasAttribute("onerror"))).toBe(
      false,
    );
    expect(elements(out.html).some((e) => e.hasAttribute("onmouseover"))).toBe(
      false,
    );
  });

  it("strips NUL, C0 controls and lone surrogates but keeps tab and newline", () => {
    const { html, text } = run(doc(p(t("a\u0000b\u0007c\ud800d\te"))));
    expect(html).toContain("abcd\te");
    expect(text).toBe("abcd\te");
    expect(run(doc(p(t("ok 😀")))).text).toBe("ok 😀");
  });

  it("does not double the gap inside a list item", () => {
    const { html } = run(
      doc({ type: "bulletList", content: [li(p(t("one")))] }),
    );
    expect(html).not.toMatch(/<li[^>]*><p/);
  });

  it("renders list item content, including multiple blocks and nested lists", () => {
    const { html } = run(
      doc({
        type: "bulletList",
        content: [
          li(p(t("lone"))),
          li(p(t("first")), p(t("second"))),
          li(p(t("parent")), {
            type: "orderedList",
            content: [li(p(t("child")))],
          }),
        ],
      }),
    );
    expect(html).toMatch(/<li[^>]*>lone<\/li>/);
    expect(html).toMatch(
      /<li[^>]*><p[^>]*>first<\/p><p[^>]*>second<\/p><\/li>/,
    );
    expect(html).toMatch(
      /<li[^>]*><p[^>]*>parent<\/p><ol[^>]*><li[^>]*>child<\/li><\/ol><\/li>/,
    );
  });

  it("ignores a monoAtt attribute: only data-mono-att names an attachment", () => {
    const out = run(doc({ type: "image", attrs: { alt: "a", monoAtt: "5" } }));
    expect(out.inlineImages).toEqual([]);
  });
});

describe("mailCompose images", () => {
  it("keeps cids distinct across ids differing only by case and across drafts", () => {
    expect(contentIdFor("d", "A")).not.toBe(contentIdFor("d", "a"));
    expect(contentIdFor("d1", "1")).not.toBe(contentIdFor("d2", "1"));
    expect(contentIdFor("d", "A")).toBe(contentIdFor("d", "A").toLowerCase());
  });

  it("keeps cids injective where hex escapes could run together", () => {
    // Variable-width hex made " 0" and U+0200 both come out as "_200".
    expect(contentIdFor("d", " 0")).not.toBe(contentIdFor("d", "\u0200"));
    expect(contentIdFor("d", "att 7")).not.toBe(contentIdFor("d", "att\u0207"));
  });

  it("treats an empty attachment id as no attachment", () => {
    const out = run(
      doc({ type: "image", attrs: { alt: "a", "data-mono-att": "" } }),
    );
    expect(out.inlineImages).toEqual([]);
    expect(out.html).not.toContain("cid:");
  });

  it("cannot match a cid already inside the quoted original", () => {
    const quoted = '<img src="cid:mono-1@monoptah.local">';
    const out = run(doc(img("1")), quoted);
    expect(cidReferences(out.html)).toHaveLength(2);
    expect(out.html.split(out.inlineImages[0].contentId)).toHaveLength(2);
  });

  it("turns an inline image into a cid reference plus a matching attachment", () => {
    const out = run(doc(img("att 7")));
    expect(out.inlineImages).toEqual([
      {
        attachmentId: "att 7",
        contentId: contentIdFor("d1", "att 7"),
        alt: "pic",
      },
    ]);
    expect(out.html).toContain(`src="cid:${out.inlineImages[0].contentId}"`);
    expect(out.html).not.toContain("blob:");
  });

  it("produces cid values the reader resolves back to the same content id", () => {
    const out = run(doc(img("Att/9")));
    const found = cidReferences(out.html);
    expect(found).toEqual([out.inlineImages[0].contentId]);
  });

  it("uses one attachment entry when the same image appears twice", () => {
    const out = run(doc(img("1"), img("1"), img("2")));
    expect(out.inlineImages.map((i) => i.attachmentId)).toEqual(["1", "2"]);
  });

  it("shows an image with no pending attachment as text, not a broken src", () => {
    const out = run(
      doc({ type: "image", attrs: { src: "blob:x", alt: "lost" } }),
    );
    expect(out.inlineImages).toEqual([]);
    expect(out.html).toContain("[image: lost]");
    expect(out.html).not.toContain("blob:");
  });
});

describe("mailCompose bcc", () => {
  it("never puts Bcc in the body, even when the draft row carries it", () => {
    const draft = {
      bodyJson: doc(p(t("hello"))),
      draftId: "d1",
      quotedHtml: null,
      to: ["to@example.com"],
      cc: ["cc@example.com"],
      bcc: ["secret@example.com"],
    };
    const out = serializeCompose(draft);
    for (const s of [out.html, out.text]) {
      expect(s.toLowerCase()).not.toContain("bcc");
      expect(s).not.toContain("secret@example.com");
    }
  });
});

describe("mailCompose draft reopen", () => {
  it("reproduces the same html from body_json after a storage round trip", () => {
    const first = run(everything, "<p>quoted</p>");
    // body_json is stored as text in SQLite, so go through a real string.
    const stored = JSON.stringify(everything);
    const reopened = run(JSON.parse(stored) as TipTapNode, "<p>quoted</p>");
    expect(reopened.html).toBe(first.html);
    expect(reopened.text).toBe(first.text);
    expect(reopened.inlineImages).toEqual(first.inlineImages);
  });

  it("is sensitive to the document, so equality above means something", () => {
    const a = run(doc(p(t("one")))).html;
    const b = run(doc(p(t("two")))).html;
    expect(a).not.toBe(b);
  });
});
