// shiki with the JavaScript regex engine, consumed as tokens (never codeToHtml).
import { createHighlighterCore } from "shiki/core";
import { createJavaScriptRegexEngine } from "shiki/engine/javascript";

export interface Tok { content: string; color: string | undefined }

export async function tokenize(code: string): Promise<Tok[][]> {
  const hl = await createHighlighterCore({
    themes: [import("shiki/themes/github-dark.mjs")],
    langs: [import("shiki/langs/typescript.mjs")],
    engine: createJavaScriptRegexEngine(),
  });
  const { tokens } = hl.codeToTokens(code, { lang: "typescript", theme: "github-dark" });
  return tokens.map((line) => line.map((t) => ({ content: t.content, color: t.color })));
}
