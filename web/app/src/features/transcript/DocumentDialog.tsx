// A document a Mono wrote (F-72), read through `mono.document`. Its text is rendered as Markdown (an AST,
// no HTML sink). A visual document (HTML) is never rendered here: it opens on the desktop.
import { useLinkQuery } from "@/data";
import { Markdown } from "@/features/markdown/Markdown";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/ui/dialog";
import { DelayedSpinner } from "@/ui/DelayedSpinner";
import { Notice } from "@/ui/Notice";
import type { DocumentItem } from "./turnItems";

export function DocumentDialog({ document, onClose }: { document: DocumentItem | null; onClose: () => void }) {
  return (
    <Dialog open={document !== null} onOpenChange={(open) => (open ? undefined : onClose())}>
      <DialogContent aria-describedby={undefined} className="max-h-[85dvh] max-w-2xl grid-rows-[auto_minmax(0,1fr)]" data-testid="document-dialog">
        <DialogHeader>
          <DialogTitle className="pr-10 wrap-anywhere">{document?.title ?? "Document"}</DialogTitle>
        </DialogHeader>
        <div className="min-h-0 overflow-y-auto">{document ? <DocumentBody document={document} /> : null}</div>
      </DialogContent>
    </Dialog>
  );
}

function DocumentBody({ document }: { document: DocumentItem }) {
  const visual = document.kind === "html" || document.kind === "visual";
  const query = useLinkQuery("mono.document", { documentId: document.documentId }, { enabled: !visual });
  if (visual) return <Notice variant="info">This is a visual document. Open it on your desktop to see it.</Notice>;
  if (query.isError) return <Notice variant="error">Could not load this document.</Notice>;
  if (!query.data) return <DelayedSpinner label="Loading the document" />;
  const text = typeof query.data.text === "string" ? query.data.text : typeof query.data.content === "string" ? query.data.content : null;
  if (text === null) return <Notice variant="info">Open this document on your desktop to read it.</Notice>;
  return <Markdown text={text} />;
}
