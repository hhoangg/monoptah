// The preview of a file an assistant message mentions (F-45, F-60): a dialog over the chat. It reads the file
// through `chat.mentionedFile` (only when the desktop advertises `mentionedFiles`) and shows it by kind: text with
// the file viewer's highlighted lines, markdown rendered, html in a sandboxed iframe, an image in a zoom viewer, or
// a note that the file cannot be previewed.
import { useState } from "react";
import { CodeLines } from "@/features/files/FileViewer";
import { FileProblem } from "@/features/files/FileProblem";
import { FileTypeIcon } from "@/features/files/FileTypeIcon";
import { useFilePrefs } from "@/features/files/filePrefs";
import { languageId, leafName } from "@/lib/fileKind";
import { safeClipboardText } from "@/lib/clipboardText";
import { useLinkDesktopId } from "@/link";
import { RequireCapability, useCapability } from "@/version";
import { Button } from "@/ui/button";
import { DelayedSpinner } from "@/ui/DelayedSpinner";
import { cn } from "@/lib/utils";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/ui/dialog";
import { Icon } from "@/ui/Icon";
import { HtmlPreview } from "./HtmlPreview";
import { ImageViewer } from "./ImageViewer";
import { MarkdownPreview } from "./MarkdownPreview";
import { formatSize, previewProblem } from "./mentionedFile";
import { useMentionRequester, useMentionedFile, type PreviewData } from "./useMentionedFile";

export interface PreviewTarget {
  /** The mention as written (line suffix already cut). */
  path: string;
  line?: number;
}

export interface FilePreviewDialogProps {
  conversationId: string;
  target: PreviewTarget | null;
  onClose: () => void;
}

/** The dialog over the viewport edge to edge, with the device safe areas as padding (CSS only: iOS Safari has no Fullscreen API for elements). */
const FULLSCREEN_CLASS =
  "top-0 left-0 h-dvh max-h-none w-full max-w-none translate-x-0 translate-y-0 rounded-none border-0 pt-[max(1rem,env(safe-area-inset-top))] pr-[max(1rem,env(safe-area-inset-right))] pb-[max(1rem,env(safe-area-inset-bottom))] pl-[max(1rem,env(safe-area-inset-left))]";

export function FilePreviewDialog({ conversationId, target, onClose }: FilePreviewDialogProps) {
  const name = target ? leafName(target.path) || target.path : "File";
  // Fullscreen belongs to one opened target: a closed dialog, or another target, is never fullscreen.
  const [fullscreenFor, setFullscreenFor] = useState<PreviewTarget | null>(null);
  const fullscreen = target !== null && fullscreenFor === target;
  return (
    <Dialog
      open={target !== null}
      onOpenChange={(open) => {
        if (open) return;
        setFullscreenFor(null);
        onClose();
      }}
    >
      <DialogContent
        aria-describedby={undefined}
        data-testid="file-preview-dialog"
        data-fullscreen={fullscreen}
        // Fullscreen: the close button sits inside the safe areas (notch, home indicator side).
        closeClassName={fullscreen ? "top-[max(0.75rem,env(safe-area-inset-top))] right-[max(0.75rem,env(safe-area-inset-right))]" : undefined}
        // Escape leaves fullscreen first; the next one closes the dialog.
        onEscapeKeyDown={(event) => {
          if (!fullscreen) return;
          event.preventDefault();
          setFullscreenFor(null);
        }}
        className={cn(
          "h-[min(88dvh,60rem)] max-w-4xl grid-rows-[auto_minmax(0,1fr)] overflow-hidden",
          fullscreen && FULLSCREEN_CLASS,
        )}
      >
        <DialogHeader className="min-w-0">
          <div className="flex min-w-0 items-center gap-2">
            <DialogTitle className="flex min-w-0 flex-1 items-center gap-2 text-title">
              <FileTypeIcon name={name} isDir={false} />
              <span className="min-w-0 truncate" data-testid="file-preview-name">
                {name}
              </span>
            </DialogTitle>
            <button
              type="button"
              data-testid="file-preview-fullscreen"
              aria-label={fullscreen ? "Exit full screen" : "Full screen"}
              aria-pressed={fullscreen}
              onClick={() => setFullscreenFor(fullscreen ? null : target)}
              className="inline-flex size-11 shrink-0 items-center justify-center rounded-pill text-muted-text hover:bg-selection pointer-fine:size-9"
            >
              <Icon name={fullscreen ? "exitFullscreen" : "fullscreen"} size={20} />
            </button>
          </div>
        </DialogHeader>
        <div className="flex min-h-0 min-w-0 flex-col" data-testid="file-preview-body">
          {target ? (
            <RequireCapability name="mentionedFiles" fallback="notice">
              <PreviewContent conversationId={conversationId} target={target} />
            </RequireCapability>
          ) : null}
        </div>
      </DialogContent>
    </Dialog>
  );
}

function CopyPath({ path }: { path: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <Button
      type="button"
      variant="outline"
      size="sm"
      data-testid="file-preview-copy-path"
      aria-label="Copy path"
      onClick={() => {
        void navigator.clipboard.writeText(safeClipboardText(path)).then(
          () => setCopied(true),
          () => undefined,
        );
      }}
    >
      <Icon name={copied ? "check" : "copy"} size={16} />
      {copied ? "Copied" : "Copy path"}
    </Button>
  );
}

function PathLine({ path }: { path: string }) {
  return (
    <div className="mb-2 flex min-w-0 shrink-0 items-center gap-2">
      <p data-testid="file-preview-path" title={path} className="min-w-0 flex-1 truncate text-small text-muted-text [unicode-bidi:isolate]">
        {path}
      </p>
      <CopyPath path={path} />
    </div>
  );
}

/** A jump to a line in an html or markdown file wants the lines of its source, not the rendered page. */
function wantsSource(target: PreviewTarget): boolean {
  return target.line !== undefined && /\.(html?|md|markdown|mdx)$/i.test(target.path);
}

function PreviewContent({ conversationId, target }: { conversationId: string; target: PreviewTarget }) {
  const query = useMentionedFile(conversationId, target.path, wantsSource(target));
  const desktopCurrent = useCapability("mentionedFiles2");
  if (query.isError) {
    const problem = previewProblem(query.error, { desktopCurrent });
    return (
      <div data-testid="file-preview-error" data-problem={problem.kind} className="min-h-0 flex-1 overflow-auto">
        <FileProblem
          title={problem.title}
          actions={
            problem.retry ? (
              <Button type="button" variant="outline" onClick={() => void query.refetch()}>
                Try again
              </Button>
            ) : undefined
          }
        >
          {problem.detail}
        </FileProblem>
      </div>
    );
  }
  if (!query.data) return <DelayedSpinner label="Loading file" className="m-6 self-center" />;
  return (
    <>
      {query.data.kind === "binary" ? null : <PathLine path={query.data.meta.path} />}
      <PreviewKind key={`${target.path}:${query.dataUpdatedAt}`} conversationId={conversationId} target={target} data={query.data} />
    </>
  );
}

function PreviewKind({ conversationId, target, data }: { conversationId: string; target: PreviewTarget; data: PreviewData }) {
  const request = useMentionRequester(conversationId);
  const desktopId = useLinkDesktopId();
  const wrap = useFilePrefs((state) => state.wrap);
  const name = data.meta.name;
  // Stable across renders of this file: the markdown images are cached under it.
  const cacheKey = useStableKey(["link", desktopId, "mentionedFile", conversationId, target.path]);
  switch (data.kind) {
    case "text":
      return <CodeLines text={data.text} language={languageId(name)} wrap={wrap} {...(target.line !== undefined ? { line: target.line } : {})} testId="preview-lines" />;
    case "markdown":
      return <MarkdownPreview text={data.text} path={target.path} request={request} cacheKey={cacheKey} />;
    case "html":
      return <HtmlPreview document={data.document} name={name} limited={data.limited} />;
    case "image":
      return <ImageViewer chunks={data.chunks} mime={data.mime} name={name} />;
    case "binary":
      return (
        <div data-testid="preview-binary" className="min-h-0 flex-1 overflow-auto">
          <FileProblem title="Cannot preview this file" actions={<CopyPath path={data.meta.path} />}>
            {`${formatSize(data.meta.size)} - ${data.meta.path}`}
          </FileProblem>
        </div>
      );
  }
}

function useStableKey(key: readonly unknown[]): readonly unknown[] {
  const [stable] = useState(key);
  return stable;
}
