// Stand-in for the in-house markdown renderer of security.md 8 item 1: an AST rendered as React
// elements, no HTML string anywhere, any HTML in the input stays literal text, links follow the URL
// allowlist. The real renderer is a port of mobile/src/features/sessions/markdownParser.ts (WS-W);
// this stub only proves that this shape needs no HTML sink under the CSP and Trusted Types policy.
import type { ReactNode } from "react";

type Node =
  | { t: "h"; text: string }
  | { t: "p"; parts: Array<string | { href: string; text: string }> };

const AST: Node[] = [
  { t: "h", text: "Markdown stand-in" },
  { t: "p", parts: ["Literal HTML stays text: <img src=x onerror=alert(1)> and <b>bold</b>. ", { href: "https://example.com/", text: "safe link" }, " ", { href: "javascript:alert(1)", text: "unsafe link" }] },
];

const ALLOWED = /^(https?:|mailto:|\/d\/)/i;

function render(n: Node, i: number): ReactNode {
  if (n.t === "h") return <h3 key={i}>{n.text}</h3>;
  return (
    <p key={i}>
      {n.parts.map((p, j) =>
        typeof p === "string" ? p : ALLOWED.test(p.href)
          ? <a key={j} href={p.href} rel="noopener noreferrer nofollow" target="_blank">{p.text}</a>
          : p.text)}
    </p>
  );
}

export function Markdown() {
  return <section data-testid="markdown">{AST.map(render)}</section>;
}
