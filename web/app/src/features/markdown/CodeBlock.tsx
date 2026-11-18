// Copied from src/features/sessions/ui/HighlightedCodeBlock.tsx at ca73ac2b
// Adapted: no streamdown container, tokens from the local engine rendered as React spans
// (never an HTML string), colours from two CSS variables, copy button, no transcript context.
import {
  memo,
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
} from "react";
import { Icon } from "@/ui/Icon";
import "@/styles/markdown.css";
import { requestCodeHighlight } from "./highlight/plugin";
import type { CodeToken, HighlightResult } from "./highlight/types";
import { safeClipboardText } from "@/lib/clipboardText";

type Line = CodeToken[];

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

// Complete lines keep their token arrays while a fence grows, so memoizing here avoids
// recreating thousands of token elements on every streamed update.
const HighlightedLine = memo(function HighlightedLine({ tokens }: { tokens: Line }) {
  const empty = tokens.length === 0 || (tokens.length === 1 && tokens[0]?.content === "");
  return (
    <span className="md-code-line">
      {empty
        ? "\n"
        : tokens.map((token, index) => (
            <span key={index} className={tokenClass(token)} style={tokenStyle(token)}>
              {token.content}
            </span>
          ))}
    </span>
  );
});

const PlainLine = memo(function PlainLine({ text }: { text: string }) {
  return <span className="md-code-line">{text || "\n"}</span>;
});

const HighlightedBody = memo(function HighlightedBody({ result }: { result: HighlightResult }) {
  // Complete lines keep their token array between results (see mergeStreaming), so the
  // memoized line skips its render by identity.
  return result.tokens.map((tokens, index) => <HighlightedLine key={index} tokens={tokens} />);
});

/** Tokens for `source`, reusing the previous result's complete lines while the fence grows. */
function mergeStreaming(
  previous: { language: string; source: string; result: HighlightResult } | undefined,
  language: string,
  source: string,
  result: HighlightResult,
): HighlightResult {
  if (previous?.language !== language || !source.startsWith(previous.source)) return result;
  const complete = previous.source.slice(0, previous.source.lastIndexOf("\n") + 1).split("\n").length - 1;
  return { tokens: [...previous.result.tokens.slice(0, complete), ...result.tokens.slice(complete)] };
}

export interface CodeBlockProps {
  code: string;
  /** The fence tag, shown in the header; unknown tags render as plain text. */
  language?: string;
  /** True while the reply is still streaming this fence. */
  isIncomplete?: boolean;
  lineNumbers?: boolean;
  startLine?: number;
}

export function CodeBlock({
  code,
  language = "",
  isIncomplete = false,
  lineNumbers = false,
  startLine = 1,
}: CodeBlockProps) {
  const source = code.replace(/\n+$/, "");
  const [highlighted, setHighlighted] = useState<{
    language: string;
    source: string;
    result: HighlightResult;
  }>();

  useEffect(() => {
    if (highlighted?.language === language && highlighted.source === source) return;
    return requestCodeHighlight(
      { code: source, language },
      (result) => {
        setHighlighted((previous) => ({
          language,
          source,
          result: mergeStreaming(previous, language, source, result),
        }));
      },
      isIncomplete ? 0 : 1,
    );
  }, [source, language, isIncomplete, highlighted]);

  // New code is visible at once, independently of tokenizer latency: the highlighted
  // complete lines, then the rest as plain text until its tokens arrive.
  const result = useMemo<HighlightResult | undefined>(() => {
    if (highlighted?.language !== language) return undefined;
    if (highlighted.source === source) return highlighted.result;
    if (!source.startsWith(highlighted.source)) return undefined;
    const boundary = highlighted.source.lastIndexOf("\n") + 1;
    const complete = highlighted.source.slice(0, boundary).split("\n").length - 1;
    return {
      tokens: [
        ...highlighted.result.tokens.slice(0, complete),
        ...source
          .slice(boundary)
          .split("\n")
          .map((content): Line => [{ content }]),
      ],
    };
  }, [highlighted, language, source]);

  const label = language.trim().slice(0, 32);
  return (
    <div className="md-code" data-incomplete={isIncomplete || undefined} data-language={label || undefined}>
      <div className="md-code-header">
        <span className="md-code-lang">{label || "text"}</span>
        <CopyButton text={source} />
      </div>
      <div className="md-code-body">
        <pre>
          <code
            className={lineNumbers ? "md-code-numbered" : undefined}
            style={
              lineNumbers ? ({ counterReset: `md-line ${startLine - 1}` } as CSSProperties) : undefined
            }
          >
            {result ? (
              <HighlightedBody result={result} />
            ) : (
              source.split("\n").map((line, index) => <PlainLine key={index} text={line} />)
            )}
          </code>
        </pre>
      </div>
    </div>
  );
}

function CopyButton({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  useEffect(() => () => clearTimeout(timer.current), []);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(safeClipboardText(text));
    } catch {
      return;
    }
    setCopied(true);
    clearTimeout(timer.current);
    timer.current = setTimeout(() => setCopied(false), 1500);
  };
  return (
    <button
      type="button"
      className="md-code-copy"
      onClick={() => void copy()}
      aria-label={copied ? "Copied" : "Copy code"}
      title={copied ? "Copied" : "Copy code"}
    >
      <Icon name={copied ? "check" : "copy"} size={14} />
    </button>
  );
}
