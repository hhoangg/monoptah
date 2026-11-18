// F-68: the unified diff of one file. A gutter with old and new numbers, hunk headers, a wrap toggle, context
// folding with `diffFold.ts` (fold blocks expand by 20 lines or all) and shiki token spans from the markdown
// highlight worker. The desktop's `runs` and `palette` are not used. Every text is a React child.
import { memo, useEffect, useMemo, useState } from "react";
import { requestCodeHighlight } from "@/features/markdown/highlight/plugin";
import { MAX_HIGHLIGHT_CHARS, type CodeToken } from "@/features/markdown/highlight/types";
import { CodeLine } from "@/features/files/highlightSpans";
import { useFilePrefs } from "@/features/files/filePrefs";
import { languageId } from "@/lib/fileKind";
import { cn } from "@/lib/utils";
import { Icon } from "@/ui/Icon";
import { IconTip } from "@/ui/tooltip";
import { PathLabel } from "@/ui/PathLabel";
import { UNIFIED_FOLD_STEP, expandFold, foldUnifiedLines, revealedFold, type FoldReveal } from "./diffFold";
import { STATUS_LABEL, formatCount, splitPath, type ChangedFile, type DiffLine } from "./gitModel";

/** Token runs per non-hunk line (index = position among the non-hunk lines), or undefined while pending. */
function useDiffTokens(lines: readonly DiffLine[], language: string | null): ReadonlyMap<DiffLine, CodeToken[]> | undefined {
  const code = useMemo(() => lines.filter((line) => line.kind !== "hunk"), [lines]);
  const text = useMemo(() => code.map((line) => line.text).join("\n"), [code]);
  const [done, setDone] = useState<{ key: string; map: Map<DiffLine, CodeToken[]> } | undefined>();
  const key = `${language ?? ""}\0${text}`;
  useEffect(() => {
    if (!language || text.length === 0 || text.length > MAX_HIGHLIGHT_CHARS) return;
    return requestCodeHighlight({ code: text, language }, (result) => {
      const map = new Map<DiffLine, CodeToken[]>();
      code.forEach((line, index) => {
        const tokens = result.tokens[index];
        if (tokens) map.set(line, tokens);
      });
      setDone({ key, map });
    });
  }, [key, language, text, code]);
  return done?.key === key ? done.map : undefined;
}

const SIGN: Record<DiffLine["kind"], string> = { add: "+", del: "-", context: " ", hunk: "" };

const Row = memo(function Row({ line, tokens, wrap }: { line: DiffLine; tokens: CodeToken[] | undefined; wrap: boolean }) {
  if (line.kind === "hunk") {
    return (
      <div role="listitem" data-kind="hunk" className="bg-fill px-3 py-0.5 text-muted-text">
        {line.text}
      </div>
    );
  }
  return (
    <div
      role="listitem"
      data-kind={line.kind}
      className={cn("flex", line.kind === "add" && "bg-additions/10", line.kind === "del" && "bg-deletions/10")}
    >
      <span aria-hidden="true" className="hidden w-10 shrink-0 pr-2 text-right text-faint select-none @md:block">
        {line.oldNumber ?? ""}
      </span>
      <span aria-hidden="true" className="w-8 shrink-0 pr-1 text-right text-faint select-none @md:w-10 @md:pr-2">
        {line.newNumber ?? ""}
      </span>
      <span aria-hidden="true" className={cn("w-4 shrink-0 select-none", line.kind === "add" && "text-additions", line.kind === "del" && "text-deletions")}>
        {SIGN[line.kind]}
      </span>
      <span className={cn("min-w-0 flex-1 pr-3", wrap ? "break-words whitespace-pre-wrap" : "whitespace-pre")}>
        <CodeLine text={line.text} tokens={tokens} />
      </span>
    </div>
  );
});

export function DiffView({ file }: { file: ChangedFile }) {
  // Shared with the file viewer: remembered across files, and on by default on a phone-width window.
  const wrap = useFilePrefs((state) => state.wrap);
  const setWrap = useFilePrefs((state) => state.setWrap);
  const [reveals, setReveals] = useState<ReadonlyMap<string, FoldReveal>>(new Map());
  const blocks = useMemo(() => foldUnifiedLines(file.lines), [file.lines]);
  const tokens = useDiffTokens(file.lines, languageId(splitPath(file.relative).name));

  const expand = (id: string, total: number, direction: "down" | "all") =>
    setReveals((current) => new Map(current).set(id, expandFold(current.get(id), total, direction)));

  return (
    <section aria-label={`Diff of ${file.relative}`} data-testid="diff-view" className="@container flex min-h-0 flex-1 flex-col">
      <header className="flex min-h-11 shrink-0 items-center gap-2 border-b border-stroke px-3">
        <PathLabel path={file.relative} className="flex-1 text-label" />
        <span className="text-small text-muted-text">{STATUS_LABEL[file.status]}</span>
        {file.additions > 0 ? <span className="text-small text-additions tabular-nums">+{formatCount(file.additions)}</span> : null}
        {file.deletions > 0 ? <span className="text-small text-deletions tabular-nums">-{formatCount(file.deletions)}</span> : null}
        <IconTip label="Wrap lines">
          <button
            type="button"
            aria-label="Wrap lines"
            aria-pressed={wrap}
            onClick={() => setWrap(!wrap)}
            className={cn("inline-flex size-11 shrink-0 items-center justify-center rounded-pill hover:bg-selection pointer-fine:size-9", wrap ? "bg-selection text-content" : "text-muted-text")}
          >
            <Icon name="wrap" size={18} />
          </button>
        </IconTip>
      </header>
      <div role="list" aria-label="Changed lines" tabIndex={0} className="min-h-0 flex-1 overflow-auto font-mono text-small leading-5">
        <div className={wrap ? "" : "w-max min-w-full"}>
          {file.lines.length === 0 ? <p className="p-4 text-muted-text">There is no diff to show for this file.</p> : null}
          {blocks.map((block, index) => {
            if (block.kind === "hunk") return block.lines.map((line, i) => <Row key={`${index}-${i}`} line={line} tokens={tokens?.get(line)} wrap={wrap} />);
            const total = block.lines.length;
            const { head, tail, hidden } = revealedFold(total, reveals.get(block.id));
            return (
              <div key={`${index}-${block.id}`} data-testid="diff-fold">
                {block.lines.slice(0, head).map((line, i) => (
                  <Row key={`h${i}`} line={line} tokens={tokens?.get(line)} wrap={wrap} />
                ))}
                {hidden > 0 ? (
                  <div role="listitem" className="flex flex-wrap items-center gap-2 bg-fill px-3 py-1 text-muted-text">
                    <Icon name="chevronUpDown" size={16} />
                    <span>{hidden} unchanged {hidden === 1 ? "line" : "lines"}</span>
                    <button type="button" onClick={() => expand(block.id, total, "down")} className="min-h-11 rounded-pill px-3 hover:bg-selection pointer-fine:min-h-9">
                      Show {Math.min(UNIFIED_FOLD_STEP, hidden)} more
                    </button>
                    <button type="button" onClick={() => expand(block.id, total, "all")} className="min-h-11 rounded-pill px-3 hover:bg-selection pointer-fine:min-h-9">
                      Show all {hidden}
                    </button>
                  </div>
                ) : null}
                {block.lines.slice(total - tail).map((line, i) => (
                  <Row key={`t${i}`} line={line} tokens={tokens?.get(line)} wrap={wrap} />
                ))}
              </div>
            );
          })}
        </div>
      </div>
    </section>
  );
}
