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
import { RequireCapability } from "@/version";
import { Button } from "@/ui/button";
import { DelayedSpinner } from "@/ui/DelayedSpinner";
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

export function FilePreviewDialog({ conversationId, target, onClose }: FilePreviewDialogProps) {
  const name = target ? leafName(target.path) || target.path : "File";
  return (
    <Dialog open={target !== null} onOpenChange={(open) => (open ? undefined : onClose())}>
      <DialogContent
        aria-describedby={undefined}
        data-testid="file-preview-dialog"
        className="h-[min(88dvh,60rem)] max-w-4xl grid-rows-[auto_minmax(0,1fr)] overflow-hidden"
      >
        <DialogHeader>
          <DialogTitle className="flex min-w-0 items-center gap-2 text-title">
            <FileTypeIcon name={name} isDir={false} />
            <span className="min-w-0 truncate" data-testid="file-preview-name">
              {name}
            </span>
          </DialogTitle>
        </DialogHeader>
        <div className="flex min-h-0 flex-col" data-testid="file-preview-body">
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
    <div className="mb-2 flex shrink-0 items-center gap-2">
      <p data-testid="file-preview-path" title={path} className="min-w-0 flex-1 truncate text-small text-muted-text [unicode-bidi:isolate]">
        {path}
      </p>
      <CopyPath path={path} />
    </div>
  );
}

function PreviewContent({ conversationId, target }: { conversationId: string; target: PreviewTarget }) {
  const query = useMentionedFile(conversationId, target.path);
  if (query.isError) {
    const problem = previewProblem(query.error);
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
      return <ImageViewer chunks={data.chunks as BlobPart[]} mime={data.mime} name={name} />;
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
