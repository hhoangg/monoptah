// @vitest-environment happy-dom
import { describe, expect, it, vi } from "vitest";
import {
  acknowledgeQuoteRequest,
  ADD_TO_CHAT_EVENT,
  addToChatTargets,
  appendComposerPrefill,
  appendSelectionQuote,
  composerSeedForAddToChat,
  consumeQuoteRequest,
  isMarkdownBlockquotePosition,
  type QuoteRequest,
} from "./quoteDraft";

describe("appendSelectionQuote", () => {
  it("formats a multiline selection and leaves room for a reply", () => {
    expect(appendSelectionQuote("", " line one\r\nline two ")).toBe(
      "> line one\n> line two\n\n",
    );
  });

  it("preserves intentional blank lines inside a quote", () => {
    expect(appendSelectionQuote("", "one\n\ntwo")).toBe("> one\n>\n> two\n\n");
  });

  it.each([
    ["draft", "draft\n\n> quote\n\n"],
    ["draft\n", "draft\n\n> quote\n\n"],
    ["draft\n\n", "draft\n\n> quote\n\n"],
  ])("separates an existing draft %#", (draft, expected) => {
    expect(appendSelectionQuote(draft, "quote")).toBe(expected);
  });

  it("ignores whitespace-only selections", () => {
    expect(appendSelectionQuote("draft", "  \n ")).toBe("draft");
  });
});

describe("composerSeedForAddToChat", () => {
  it("preserves the requested insertion mode for a new composer", () => {
    expect(composerSeedForAddToChat("selected")).toBe("> selected\n\n");
    expect(composerSeedForAddToChat("Comment", "plain")).toBe("Comment\n\n");
  });
});

describe("isMarkdownBlockquotePosition", () => {
  it("recognizes tokens after nested quote markers", () => {
    const text = "plain /one\n> /two\n  >> @file";
    expect(isMarkdownBlockquotePosition(text, text.indexOf("/one"))).toBe(
      false,
    );
    expect(isMarkdownBlockquotePosition(text, text.indexOf("/two"))).toBe(true);
    expect(isMarkdownBlockquotePosition(text, text.indexOf("@file"))).toBe(
      true,
    );
  });
});

describe("consumeQuoteRequest", () => {
  const request: QuoteRequest = { id: 1, text: "selected" };

  it("consumes a request once", () => {
    expect(consumeQuoteRequest("draft", null, request)).toEqual({
      draft: "draft\n\n> selected\n\n",
      consumedId: 1,
      changed: true,
    });
    expect(consumeQuoteRequest("draft", 1, request)).toEqual({
      draft: "draft",
      consumedId: 1,
      changed: false,
    });
  });

  it("accepts a new id even when the text is identical", () => {
    expect(
      consumeQuoteRequest("", 1, { id: 2, text: "selected" }),
    ).toMatchObject({ consumedId: 2, changed: true });
  });

  it("inserts plain text without quoting", () => {
    expect(
      consumeQuoteRequest("draft", null, {
        id: 3,
        text: "Note: Auth\n\nUse a cookie.",
        mode: "plain",
      }),
    ).toEqual({
      draft: "draft\n\nNote: Auth\n\nUse a cookie.\n\n",
      consumedId: 3,
      changed: true,
    });
  });
});

describe("acknowledgeQuoteRequest", () => {
  it("clears only the request that was acknowledged", () => {
    const current: QuoteRequest = { id: 2, text: "newer" };
    expect(acknowledgeQuoteRequest(current, 2)).toBeUndefined();
    expect(acknowledgeQuoteRequest(current, 1)).toBe(current);
  });
});

describe("prefill mode", () => {
  it("keeps the text exactly, with no blank line after it", () => {
    expect(appendComposerPrefill("", "Help me build a workflow that ")).toBe(
      "Help me build a workflow that ",
    );
    expect(composerSeedForAddToChat("Start ", "prefill")).toBe("Start ");
  });

  it("goes after an existing draft on its own paragraph", () => {
    expect(appendComposerPrefill("draft", "Start ")).toBe("draft\n\nStart ");
    expect(appendComposerPrefill("draft\n\n", "Start ")).toBe(
      "draft\n\nStart ",
    );
    expect(appendComposerPrefill("draft", "  \n")).toBe("draft");
  });

  it("is not added again when the draft already ends with it", () => {
    expect(appendComposerPrefill("Start ", "Start ")).toBe("Start ");
    expect(appendComposerPrefill("draft\n\nStart ", "Start ")).toBe("draft\n\nStart ");
    expect(appendComposerPrefill("Start more", "Start ")).toBe("Start more\n\nStart ");
  });

  it("is consumed once like any other request", () => {
    const request: QuoteRequest = { id: 4, text: "Start ", mode: "prefill" };
    expect(consumeQuoteRequest("", null, request)).toEqual({
      draft: "Start ",
      consumedId: 4,
      changed: true,
    });
    expect(consumeQuoteRequest("Start ", 4, request).changed).toBe(false);
  });
});

describe("requestAddToChat", () => {
  it("sends a prefill untrimmed and aimed at one session", async () => {
    const { requestAddToChat } = await import("./quoteDraft");
    const seen = vi.fn();
    const listener = (event: Event) => seen((event as CustomEvent).detail);
    window.addEventListener(ADD_TO_CHAT_EVENT, listener);
    requestAddToChat("Start ", "prefill", "mono-chat");
    requestAddToChat("  quoted  ");
    window.removeEventListener(ADD_TO_CHAT_EVENT, listener);
    expect(seen).toHaveBeenNthCalledWith(1, {
      text: "Start ",
      mode: "prefill",
      sessionId: "mono-chat",
    });
    expect(seen).toHaveBeenNthCalledWith(2, { text: "quoted", mode: "quote" });
  });

  it("is taken by the named session only", () => {
    const aimed = { text: "x", mode: "prefill" as const, sessionId: "a" };
    expect(addToChatTargets(aimed, "a")).toBe(true);
    expect(addToChatTargets(aimed, "b")).toBe(false);
    expect(addToChatTargets({ text: "x", mode: "quote" }, "b")).toBe(true);
  });
});
