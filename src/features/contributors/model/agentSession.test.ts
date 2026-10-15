import { describe, expect, it } from "vitest";
import {
  composeSessionPrompt,
  contributorsSessionTitle,
  failureMessage,
  resolveHarness,
  utf8ToBase64,
} from "./agentSession";

const decode = (base64: string) =>
  new TextDecoder().decode(
    Uint8Array.from(atob(base64), (char) => char.charCodeAt(0)),
  );

describe("utf8ToBase64", () => {
  it("encodes ASCII like btoa", () => {
    expect(utf8ToBase64('{"a":1}')).toBe(btoa('{"a":1}'));
    expect(utf8ToBase64("")).toBe("");
  });

  it("round-trips non-ASCII text and emoji", () => {
    const text = 'Nguyễn Văn A <a@b.c> "naïve" 日本語 🚀';
    expect(decode(utf8ToBase64(text))).toBe(text);
  });

  it("handles input larger than one chunk", () => {
    const text = "é😀x".repeat(60_000);
    expect(decode(utf8ToBase64(text))).toBe(text);
  });
});

describe("contributorsSessionTitle", () => {
  it("names the period from the file name", () => {
    expect(contributorsSessionTitle("contributors-2026-10.json")).toBe(
      "Contributors 2026-10",
    );
  });

  it("falls back to a plain title for other names", () => {
    expect(contributorsSessionTitle("notes.json")).toBe("Contributors");
    expect(contributorsSessionTitle("contributors-.json")).toBe("Contributors");
  });
});

describe("composeSessionPrompt", () => {
  it("keeps the prompt when the snapshot is attached", () => {
    expect(
      composeSessionPrompt({
        prompt: "Question",
        path: "/tmp/x.json",
        attached: true,
      }),
    ).toBe("Question");
  });

  it("points at the written file when it could not be attached", () => {
    const text = composeSessionPrompt({
      prompt: "Question",
      path: "/tmp/monocode-attachments/1-contributors-2026-10.json",
      attached: false,
    });
    expect(text.startsWith("Question\n")).toBe(true);
    expect(text).toContain(
      "/tmp/monocode-attachments/1-contributors-2026-10.json",
    );
    expect(text).toContain("could not be attached");
  });
});

describe("resolveHarness", () => {
  const known = ["claude", "codex"] as const;

  it("returns undefined when no provider was chosen", () => {
    expect(resolveHarness(undefined, known)).toBeUndefined();
    expect(resolveHarness("", known)).toBeUndefined();
  });

  it("accepts a known provider and rejects an unknown one", () => {
    expect(resolveHarness("codex", known)).toBe("codex");
    expect(() => resolveHarness("nope", known)).toThrow(
      'The provider "nope" is not available.',
    );
  });
});

describe("failureMessage", () => {
  it("prefixes the step and keeps the original text", () => {
    expect(
      failureMessage(
        "write the snapshot",
        "File is too large to attach (maximum 20 MB).",
      ),
    ).toBe(
      "Could not write the snapshot: File is too large to attach (maximum 20 MB).",
    );
    expect(failureMessage("attach the snapshot", new Error("boom"))).toBe(
      "Could not attach the snapshot: boom",
    );
  });

  it("copes with values that carry no text", () => {
    expect(failureMessage("write the snapshot", undefined)).toBe(
      "Could not write the snapshot.",
    );
    expect(failureMessage("write the snapshot", "")).toBe(
      "Could not write the snapshot.",
    );
  });
});
