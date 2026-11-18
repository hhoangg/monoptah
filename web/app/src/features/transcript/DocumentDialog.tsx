// A document a Mono wrote (F-72), read through `mono.document`. Its text is rendered as Markdown (an AST,
// no HTML sink). A visual document is drawn by the SVG renderer from its `tree`; HTML is never rendered here.
import { useLinkQuery } from "@/data";
import { Markdown } from "@/features/markdown/Markdown";
import { readDocument } from "@/features/monos/profile";
import { VisualTree, parseVisualTree } from "@/features/visuals/VisualTree";
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
  const query = useLinkQuery("mono.document", { documentId: document.documentId });
  if (query.isError) return <Notice variant="error">Could not load this document.</Notice>;
  const loaded = query.data === undefined ? null : readDocument(query.data);
  if (!loaded) return <DelayedSpinner label="Loading the document" />;
  // The desktop sends `body` (Markdown) and, for a visual, `tree` (its component tree), as the Mono page reads them.
  const tree = parseVisualTree(loaded.tree);
  if (tree) return <VisualTree node={tree} />;
  if (loaded.kind === "html" || loaded.kind === "visual" || document.kind === "html" || document.kind === "visual") {
    return <Notice variant="info">{loaded.treeError ?? "This is a visual document. Open it on your desktop to see it."}</Notice>;
  }
  if (loaded.body === "") return <Notice variant="info">Open this document on your desktop to read it.</Notice>;
  return <Markdown text={loaded.body} />;
}
