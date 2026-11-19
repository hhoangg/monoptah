// Token spans of a highlighted line. The token helpers are adapted from features/markdown/CodeBlock.tsx
// (not exported there). Text is always a React child and colours are hex-checked CSS variables, so no
// string is ever parsed as HTML.
import { memo, type CSSProperties } from "react";
import "@/styles/markdown.css";
import type { CodeToken } from "@/features/markdown/highlight/types";

const HEX = /^#[0-9a-f]{3,8}$/i;

function tokenStyle(token: CodeToken): CSSProperties | undefined {
  const style: Record<string, string> = {};
  if (token.light && HEX.test(token.light)) style["--md-tok-light"] = token.light;
  if (token.dark && HEX.test(token.dark)) style["--md-tok-dark"] = token.dark;
  return Object.keys(style).length ? (style as CSSProperties) : undefined;
}

function tokenClass(token: CodeToken): string {
  let name = "md-token";
  const flags = token.fontStyle ?? 0;
  if (flags & 1) name += " md-token-italic";
  if (flags & 2) name += " md-token-bold";
  if (flags & 4) name += " md-token-underline";
  return name;
}

export const CodeLine = memo(function CodeLine({ text, tokens }: { text: string; tokens?: readonly CodeToken[] | undefined }) {
  if (!tokens || tokens.length === 0) return <>{text || "\n"}</>;
  return (
    <>
      {tokens.map((token, index) => (
        <span key={index} className={tokenClass(token)} style={tokenStyle(token)}>
          {token.content}
        </span>
      ))}
    </>
  );
});
