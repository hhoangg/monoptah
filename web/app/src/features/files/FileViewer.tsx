// F-60: the file viewer. Virtualized lines (@tanstack/react-virtual), wrap toggle (monoptah.files.wrap),
// markdown preview through the safe renderer, a truncation notice and client-side shiki highlighting
// through the markdown highlight worker. The desktop's `highlighted` palette and `project.fileHighlight`
// are not used. A hidden file (`path_denied`) shows "This file is hidden by Monoptah".
import { useVirtualizer } from "@tanstack/react-virtual";
import { useEffect, useMemo, useRef, useState } from "react";
import { useLinkQuery } from "@/data";
import { Markdown } from "@/features/markdown/Markdown";
import { MAX_HIGHLIGHT_CHARS, type CodeToken } from "@/features/markdown/highlight/types";
import { requestCodeHighlight } from "@/features/markdown/highlight/plugin";
import { fileKind, languageId } from "@/lib/fileKind";
import { cn } from "@/lib/utils";
import { Button } from "@/ui/button";
import { DelayedSpinner } from "@/ui/DelayedSpinner";
import { Icon } from "@/ui/Icon";
import { Notice } from "@/ui/Notice";
import { PathLabel } from "@/ui/PathLabel";
import { SegmentedControl } from "@/ui/SegmentedControl";
import { FileTypeIcon } from "./FileTypeIcon";
import { FileProblem } from "./FileProblem";
import { HIDDEN_FILE_MESSAGE, nameOf, problemOf } from "./filePaths";
import { useFilePrefs } from "./filePrefs";
import { CodeLine } from "./highlightSpans";

const LINE_HEIGHT = 20;
const OVERSCAN = 24;

/** Lines of `text`; a final newline does not add an empty line. */
export function splitLines(text: string): string[] {
  const lines = text.split("\n");
  if (lines.length > 1 && lines[lines.length - 1] === "") lines.pop();
  return lines.map((line) => (line.endsWith("\r") ? line.slice(0, -1) : line));
}

/** Tokens per line for `text` in `language`, or undefined until ready (and for unsupported or huge files). */
function useTokens(text: string, language: string | null): CodeToken[][] | undefined {
  const [done, setDone] = useState<{ key: string; tokens: CodeToken[][] } | undefined>();
  const key = `${language ?? ""}\0${text}`;
  useEffect(() => {
    if (!language || text.length > MAX_HIGHLIGHT_CHARS) return;
    return requestCodeHighlight({ code: text, language }, (result) => setDone({ key, tokens: result.tokens }));
  }, [key, language, text]);
  return done?.key === key ? done.tokens : undefined;
}

function CodeLines({ text, language, wrap }: { text: string; language: string | null; wrap: boolean }) {
  const lines = useMemo(() => splitLines(text), [text]);
  const tokens = useTokens(text, language);
  const parent = useRef<HTMLDivElement>(null);
  const gutter = String(lines.length).length;
  const longest = useMemo(() => lines.reduce((max, line) => Math.max(max, line.length), 0), [lines]);
  const virtualizer = useVirtualizer({
    count: lines.length,
    getScrollElement: () => parent.current,
    estimateSize: () => LINE_HEIGHT,
    overscan: OVERSCAN,
    // Wrapped lines have no fixed height: measure them. Re-measure when wrapping flips.
    measureElement: wrap ? undefined : () => LINE_HEIGHT,
  });
  useEffect(() => {
    virtualizer.measure();
  }, [virtualizer, wrap]);

  return (
    <div
      ref={parent}
      role="region"
      aria-label="File contents"
      data-testid="file-lines"
      data-wrap={wrap}
      tabIndex={0}
      className="min-h-0 flex-1 overflow-auto font-mono text-small"
    >
      <div
        className="relative"
        style={{ height: virtualizer.getTotalSize(), width: wrap ? "100%" : `calc(${longest + gutter + 3}ch + 1rem)`, minWidth: "100%" }}
      >
        {virtualizer.getVirtualItems().map((item) => (
          <div
            key={item.key}
            ref={wrap ? virtualizer.measureElement : undefined}
            data-index={item.index}
            data-testid="file-line"
            className={cn("absolute top-0 left-0 flex w-full gap-3 px-2", wrap ? "whitespace-pre-wrap break-words" : "whitespace-pre")}
            style={{ transform: `translateY(${item.start}px)`, ...(wrap ? { minHeight: LINE_HEIGHT } : { height: LINE_HEIGHT }), lineHeight: `${LINE_HEIGHT}px` }}
          >
            <span aria-hidden="true" className="shrink-0 text-right text-faint select-none" style={{ width: `${gutter}ch` }}>
              {item.index + 1}
            </span>
            <span className="min-w-0 flex-1">
              <CodeLine text={lines[item.index] ?? ""} tokens={tokens?.[item.index]} />
            </span>
          </div>
        ))}
      </div>
    </div>
  );
}

export interface FileViewerProps {
  projectId: string;
  path: string;
  /** Opens another project file (markdown links). */
  onOpenPath?: (path: string) => void;
}

export function FileViewer({ projectId, path, onOpenPath }: FileViewerProps) {
  const file = useLinkQuery("project.file", { projectId, path });
  const wrap = useFilePrefs((state) => state.wrap);
  const setWrap = useFilePrefs((state) => state.setWrap);
  const [mode, setMode] = useState<"code" | "preview">("code");
  const name = nameOf(path);
  const isMarkdown = fileKind(name) === "markdown";
  const language = languageId(name);

  let body;
  if (file.isError) {
    body =
      problemOf(file.error) === "denied" ? (
        <FileProblem title={HIDDEN_FILE_MESSAGE}>Monoptah keeps this kind of file (for example secrets) off your devices.</FileProblem>
      ) : (
        <FileProblem
          title="This file could not be opened"
          actions={
            <Button type="button" variant="outline" onClick={() => void file.refetch()}>
              Try again
            </Button>
          }
        >
          It may have been moved or deleted on your desktop.
        </FileProblem>
      );
  } else if (!file.data) {
    body = <DelayedSpinner label="Loading file" className="m-6" />;
  } else if (isMarkdown && mode === "preview") {
    body = (
      <div data-testid="markdown-preview" className="min-h-0 flex-1 overflow-auto px-4 py-3">
        <Markdown text={file.data.text} {...(onOpenPath ? { onOpenFile: (target: string) => onOpenPath(target) } : {})} />
      </div>
    );
  } else {
    body = <CodeLines text={file.data.text} language={language} wrap={wrap} />;
  }

  return (
    <section aria-label={`File ${name}`} data-testid="file-viewer" className="flex h-full min-h-0 flex-col">
      <header className="flex min-h-11 shrink-0 items-center gap-2 border-b border-stroke px-3">
        <FileTypeIcon name={name} isDir={false} />
        <PathLabel path={path} className="flex-1 text-label" />
        {isMarkdown && file.data ? (
          <SegmentedControl
            label="Markdown view"
            options={[
              { value: "code", label: "Code" },
              { value: "preview", label: "Preview" },
            ]}
            value={mode}
            onChange={setMode}
          />
        ) : null}
        <button
          type="button"
          aria-label="Wrap lines"
          aria-pressed={wrap}
          onClick={() => setWrap(!wrap)}
          className={cn(
            "inline-flex size-11 shrink-0 items-center justify-center rounded-pill hover:bg-selection pointer-fine:size-9",
            wrap ? "bg-selection text-content" : "text-muted-text",
          )}
        >
          <Icon name="wrap" size={18} />
        </button>
      </header>
      {file.data?.truncated ? (
        <Notice className="m-3 shrink-0" title="Large file">
          Only the first part of this file is shown.
        </Notice>
      ) : null}
      {body}
    </section>
  );
}
