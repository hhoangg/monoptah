// Port of mobile/src/features/sessions/Markdown.tsx to the DOM. The reply is parsed to an AST
// (parser.ts) and rendered as React elements: there is no HTML sink anywhere, raw HTML in the
// text is shown literally, links go through the allowlist (links.ts) and images are not loaded.
import { Fragment, memo, useMemo, type CSSProperties, type ReactNode } from "react";
import { FileTypeIcon } from "@/features/files/FileTypeIcon";
import { useMediaQuery } from "@/ui/useMediaQuery";
import "@/styles/markdown.css";
import { CodeBlock } from "./CodeBlock";
import { ExternalLink, inlineText } from "./ExternalLink";
import { inlineFileReference } from "./fileLinks";
import { classifyLink } from "./links";
import {
  numberWords,
  parseMarkdown,
  splitWords,
  type Inline,
  type MarkdownBlock,
} from "./parser";
import { closeOpenMarkers, useStreamedText, useWordFade, type WordClock } from "./wordReveal";

/** First of the private-use characters that stand for a picture in the text handed to the parser. */
const MARK_BASE = 0xe100;
/** How many pictures one text can hold (the protocol's most picks of a text item). */
const MARK_COUNT = 64;
const MARK = /[\ue100-\ue13f]/;
const MARKS = /[\ue100-\ue13f]/g;
const MARK_SPLIT = /([\ue100-\ue13f])/;

/** Pictures drawn inside the text: `at` are UTF-16 offsets of `text`, `render(k)` draws the k-th one. */
export interface MarkdownEmoji {
  at: readonly number[];
  /** Which of the pictures are large (a line of their own). */
  large?: readonly boolean[];
  render: (index: number) => ReactNode;
}

/**
 * `text` with one private-use character at each offset. The parser treats it as a letter, the renderer swaps it for
 * the picture, and every place that is not prose (code, link targets, alt text) drops it. Text that already holds
 * such a character is left alone, so a real one can never be mistaken for a picture.
 */
export function markEmoji(text: string, at: readonly number[], large: readonly boolean[] = []): string {
  if (at.length === 0 || at.length > MARK_COUNT || MARK.test(text)) return text;
  let out = "";
  let from = 0;
  for (let index = 0; index < at.length; ) {
    const to = Math.min(Math.max(at[index] ?? 0, from), text.length);
    let end = index;
    while (end < at.length && Math.min(Math.max(at[end] ?? 0, from), text.length) === to) end++;
    out += text.slice(from, to);
    from = to;
    let alone = false;
    for (let k = index; k < end; k++) {
      out += String.fromCharCode(MARK_BASE + k);
      alone ||= large[k] === true;
    }
    // A large picture is a line of its own (the desktop removed that line, so its offset is the start of the next kept
    // one): end the line, or the marker would keep the next line from starting a heading, list item, quote or fence.
    if (alone && (to === 0 || text[to - 1] === "\n")) out += "\n";
    index = end;
  }
  return out + text.slice(from);
}

const withoutMarks = (text: string): string => (MARK.test(text) ? text.replace(MARKS, "") : text);

export interface MarkdownProps {
  text: string;
  /** As the desktop's `zen-fold-prose`: muted ink, headings as bold lines. */
  muted?: boolean;
  /** The reply is still streaming: hold back a half-written word and fade new words in. */
  streaming?: boolean;
  /** Fade in the words there on mount too: a reply that arrives whole. */
  animate?: boolean;
  /** Called when a file link or a file-naming code span is pressed. Without it files show unlinked. */
  onOpenFile?: (path: string, line?: number) => void;
  /**
   * File preview only: draws an image whose source is a relative path (`![a](img/x.png)`). Without it (chat) every
   * image stays its alt text, and a remote image is never loaded either way.
   */
  renderImage?: (src: string, alt: string) => ReactNode;
  /** Called for in-app `/d/...` links; without it they navigate as plain links. */
  onNavigate?: (href: string) => void;
  /** Mono pictures to draw at their offsets of `text` (a Mono's reply only). */
  emoji?: MarkdownEmoji;
  className?: string;
}

interface Handlers {
  renderEmoji?: (index: number) => ReactNode;
  onOpenFile?: (path: string, line?: number) => void;
  renderImage?: (src: string, alt: string) => ReactNode;
  onNavigate?: (href: string) => void;
}

/** Words still fading in, and the clock they fade by. */
type Fade = { clock: WordClock; now: number; ms: number; from: number };

export const Markdown = memo(function Markdown({
  text,
  muted = false,
  streaming = false,
  animate = false,
  onOpenFile,
  renderImage,
  onNavigate,
  emoji,
  className,
}: MarkdownProps) {
  const reduceMotion = useMediaQuery("(prefers-reduced-motion: reduce)");
  const emojiAt = emoji?.at;
  const emojiLarge = emoji?.large;
  const marked = useMemo(() => (emojiAt ? markEmoji(text, emojiAt, emojiLarge) : text), [text, emojiAt, emojiLarge]);
  const renderEmoji = emoji?.render;
  const shown = useStreamedText(marked, streaming && !reduceMotion);
  const { blocks, count, ranges } = useMemo(() => {
    const parsed = parseMarkdown(streaming ? closeOpenMarkers(shown) : shown, { localImages: renderImage !== undefined });
    return { blocks: parsed, ...numberWords(parsed) };
  }, [shown, streaming, renderImage]);
  const { clock, now, fadeMs, live } = useWordFade(count, (animate || streaming) && !reduceMotion);
  const from = clock.firstFading(now, fadeMs);
  const fade: Fade | undefined = reduceMotion || !live ? undefined : { clock, now, ms: fadeMs, from };
  const handlers = useMemo<Handlers>(() => ({ onOpenFile, renderImage, onNavigate, renderEmoji }), [onOpenFile, renderImage, onNavigate, renderEmoji]);
  const last = blocks.length - 1;
  return (
    <div
      className={["agent-markdown", muted ? "agent-markdown-muted" : "", className ?? ""]
        .filter(Boolean)
        .join(" ")}
      style={{ "--md-fade-ms": `${fadeMs}ms` } as CSSProperties}
    >
      {blocks.map((block, index) => (
        // Blocks with nothing fading keep their props, so they skip the re-render.
        <BlockView
          key={index}
          block={block}
          fade={fade && (ranges[index]?.[1] ?? 0) > from ? fade : undefined}
          handlers={handlers}
          streamingTail={streaming && index === last}
        />
      ))}
    </div>
  );
});

function Blocks({
  blocks,
  fade,
  handlers,
}: {
  blocks: MarkdownBlock[];
  fade?: Fade;
  handlers: Handlers;
}) {
  return (
    <>
      {blocks.map((block, index) => (
        <BlockView key={index} block={block} fade={fade} handlers={handlers} />
      ))}
    </>
  );
}

/** Each parsed block's contents as text, to tell a re-parsed block from a changed one. */
const signatures = new WeakMap<MarkdownBlock, string>();
function signature(block: MarkdownBlock) {
  let value = signatures.get(block);
  if (value === undefined) {
    value = JSON.stringify(block);
    signatures.set(block, value);
  }
  return value;
}

type BlockProps = {
  block: MarkdownBlock;
  fade?: Fade;
  handlers: Handlers;
  streamingTail?: boolean;
};

/** A streaming reply is parsed anew with every burst; the blocks before the one being
 *  written come out the same and skip the render. */
function sameBlock(previous: BlockProps, next: BlockProps) {
  return (
    previous.fade === next.fade &&
    previous.handlers === next.handlers &&
    previous.streamingTail === next.streamingTail &&
    (previous.block === next.block || signature(previous.block) === signature(next.block))
  );
}

const BlockView = memo(function BlockView({ block, fade, handlers, streamingTail }: BlockProps) {
  switch (block.type) {
    case "paragraph":
      return (
        <p className="md-p">
          <Inlines nodes={block.inline} fade={fade} handlers={handlers} />
        </p>
      );
    case "heading": {
      const Tag = `h${Math.min(6, Math.max(1, block.level))}` as "h1";
      return (
        <Tag className={`md-heading md-h${block.level <= 2 ? "large" : "small"}`}>
          <Inlines nodes={block.inline} fade={fade} handlers={handlers} />
        </Tag>
      );
    }
    case "code":
      return <CodeBlock code={withoutMarks(block.text)} language={block.lang} isIncomplete={streamingTail} />;
    case "list": {
      const Tag = block.ordered ? "ol" : "ul";
      return (
        <Tag
          className="md-list"
          start={block.ordered && block.start !== 1 ? block.start : undefined}
        >
          {block.items.map((item, index) => (
            <li key={index}>
              <Blocks blocks={item} fade={fade} handlers={handlers} />
            </li>
          ))}
        </Tag>
      );
    }
    case "quote":
      return (
        <blockquote className="md-quote">
          <Blocks blocks={block.children} fade={fade} handlers={handlers} />
        </blockquote>
      );
    case "table":
      return (
        <div className="md-table-wrap">
          <table className="md-table">
            <tbody>
              {block.rows.map((row, rowIndex) => (
                <tr key={rowIndex}>
                  {row.map((cell, cellIndex) =>
                    rowIndex === 0 ? (
                      <th key={cellIndex}>
                        <Inlines nodes={cell} fade={fade} handlers={handlers} />
                      </th>
                    ) : (
                      <td key={cellIndex}>
                        <Inlines nodes={cell} fade={fade} handlers={handlers} />
                      </td>
                    ),
                  )}
                </tr>
              ))}
            </tbody>
          </table>
          {block.truncated ? <p className="text-meta text-muted-text">This table is cut off: only the first rows and columns are shown.</p> : null}
        </div>
      );
    case "rule":
      return <hr className="md-rule" />;
  }
}, sameBlock);

function Inlines({
  nodes,
  fade,
  handlers,
}: {
  nodes: Inline[];
  fade?: Fade;
  handlers: Handlers;
}) {
  return (
    <>
      {nodes.map((node, index) => (
        <Fragment key={index}>{renderInline(node, fade, handlers)}</Fragment>
      ))}
    </>
  );
}

/** A run of text with each picture mark swapped for its picture; marks are dropped when nothing draws them. */
function withEmoji(text: string, handlers: Handlers): ReactNode {
  if (!MARK.test(text)) return text;
  return text.split(MARK_SPLIT).map((part, index) =>
    part.length === 1 && MARK.test(part) ? (
      <Fragment key={index}>{handlers.renderEmoji?.(part.charCodeAt(0) - MARK_BASE)}</Fragment>
    ) : (
      part
    ),
  );
}

/**
 * A run of text with its fading words in their own spans. Words already in stay one plain
 * string; a word keeps its span, by its number, until it has settled.
 */
function FadingText({ text, word, fade, handlers }: { text: string; word: number; fade: Fade; handlers: Handlers }) {
  const out: ReactNode[] = [];
  let plain = "";
  let index = word;
  for (const part of splitWords(text)) {
    if (/^\s/.test(part) || index < fade.from) {
      if (MARK.test(part)) {
        if (plain) out.push(plain);
        plain = "";
        out.push(<Fragment key={`m${index}`}>{withEmoji(part, handlers)}</Fragment>);
      } else {
        plain += part;
      }
      if (!/^\s/.test(part)) index++;
      continue;
    }
    if (plain) out.push(plain);
    plain = "";
    out.push(
      <span key={index} className="md-word">
        {withEmoji(part, handlers)}
      </span>,
    );
    index++;
  }
  if (plain) out.push(plain);
  return <>{out}</>;
}

function FileRef({
  path,
  line,
  children,
  handlers,
}: {
  path: string;
  line?: number;
  children: ReactNode;
  handlers: Handlers;
}) {
  const open = handlers.onOpenFile;
  if (!open) return <>{children}</>;
  return (
    <button type="button" className="md-file-ref" data-testid="file-mention" data-path={path} onClick={() => open(path, line)}>
      {children}
    </button>
  );
}

function renderInline(node: Inline, fade: Fade | undefined, handlers: Handlers): ReactNode {
  switch (node.type) {
    case "text":
      return fade && node.word !== undefined ? (
        <FadingText text={node.text} word={node.word} fade={fade} handlers={handlers} />
      ) : (
        withEmoji(node.text, handlers)
      );
    case "code": {
      const code = withoutMarks(node.text);
      const file = inlineFileReference(code);
      if (!file) return <code className="md-inline-code">{code}</code>;
      return (
        <FileRef path={file.path} line={file.line} handlers={handlers}>
          <FileTypeIcon name={file.path} isDir={false} size={14} className="md-file-icon" />
          <code className="md-inline-code md-file-name">{code}</code>
        </FileRef>
      );
    }
    case "image":
      return handlers.renderImage?.(withoutMarks(node.src), withoutMarks(node.alt)) ?? withoutMarks(node.alt);
    case "strong":
      return (
        <strong>
          <Inlines nodes={node.children} fade={fade} handlers={handlers} />
        </strong>
      );
    case "em":
      return (
        <em>
          <Inlines nodes={node.children} fade={fade} handlers={handlers} />
        </em>
      );
    case "strike":
      return (
        <del>
          <Inlines nodes={node.children} fade={fade} handlers={handlers} />
        </del>
      );
    case "link": {
      // Native draws no picture inside a link: the marks of its text are dropped.
      const children = <Inlines nodes={node.children} fade={fade} handlers={handlers.renderEmoji ? { ...handlers, renderEmoji: undefined } : handlers} />;
      const target = classifyLink(withoutMarks(node.href));
      if (!target) return <span className="md-link-blocked">{children}</span>;
      switch (target.kind) {
        case "file":
          return (
            <FileRef path={target.path} line={target.line} handlers={handlers}>
              {children}
            </FileRef>
          );
        case "internal":
          return (
            <a
              className="md-link"
              href={target.href}
              onClick={(event) => {
                if (!handlers.onNavigate) return;
                event.preventDefault();
                handlers.onNavigate(target.href);
              }}
            >
              {children}
            </a>
          );
        case "mailto":
          return (
            <a className="md-link" href={target.href} rel="noopener noreferrer nofollow">
              {children}
            </a>
          );
        case "external":
          return (
            <ExternalLink href={target.href} insecure={target.insecure} label={withoutMarks(inlineText(node.children))}>
              {children}
            </ExternalLink>
          );
      }
    }
  }
}
