// `/d/:desktopId/monos/:monoId/profile/file/:name`: one file of a Mono's profile.
//  - F-79: `?mind=soul` or `?mind=memory` is the Mono's mind, shown as a read-only viewer (`mono.file`). Saving
//    (`mono.saveFile`) is denied for web devices in v1 (security.md section 7): there is no Save button, no
//    editor and the method is never called.
//  - F-76: a name without `mind` is a document id (`mono.document`): Markdown through the safe renderer, or a visual
//    tree drawn by the SVG renderer (F-73).
import { Link } from "@tanstack/react-router";
import { useCatalog, useLinkQuery } from "@/data";
import { Markdown } from "@/features/markdown/Markdown";
import { VisualTree, parseVisualTree } from "@/features/visuals/VisualTree";
import { DelayedSpinner } from "@/ui/DelayedSpinner";
import { Notice } from "@/ui/Notice";
import { BACK_LINK_CLASS, BackIcon, MonoPage } from "./MonoPage";
import { readDocument, useMonoProfile } from "./profile";

const MIND_FILES = { soul: "Soul", memory: "Memory" } as const;
type MindFile = keyof typeof MIND_FILES;

export function MonoFileScreen({ desktopId, monoId, name, mind }: { desktopId: string; monoId: string; name: string; mind?: MindFile | undefined }) {
  // Only the `mind` search param opens the viewer: a document id of `soul`, `memory` or `Memory.md` is a document.
  return mind ? <MindViewer desktopId={desktopId} monoId={monoId} file={mind} /> : <DocumentPage desktopId={desktopId} monoId={monoId} documentId={name} />;
}

function Back({ desktopId, monoId }: { desktopId: string; monoId: string }) {
  return (
    <Link to="/d/$desktopId/monos/$monoId/profile" params={{ desktopId, monoId }} aria-label="Back" className={BACK_LINK_CLASS}>
      <BackIcon />
    </Link>
  );
}

function MindViewer({ desktopId, monoId, file }: { desktopId: string; monoId: string; file: MindFile }) {
  const catalog = useCatalog().data;
  const unavailable = catalog !== undefined && catalog.monoFiles === false;
  const query = useLinkQuery("mono.file", { monoId, file }, { enabled: !unavailable });
  const title = MIND_FILES[file];
  return (
    <MonoPage testId="mono-file" title={title} back={<Back desktopId={desktopId} monoId={monoId} />}>
      {unavailable ? (
        <Notice>This desktop does not share the Mono's {title.toLowerCase()}. Update the Monoptah app on it.</Notice>
      ) : query.isError ? (
        <Notice variant="error">{query.error.message || `Could not load the ${title.toLowerCase()}.`}</Notice>
      ) : !query.data ? (
        <DelayedSpinner label={`Loading the ${title.toLowerCase()}`} className="m-6" />
      ) : (
        <>
          <textarea
            readOnly
            aria-label={title}
            value={query.data.text}
            rows={18}
            className="min-h-64 w-full resize-y rounded-card bg-surface p-3 font-mono text-small outline-none focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[var(--accent-color)]"
          />
          <p className="px-1 text-meta text-muted-text">Viewing only. Edit the {title.toLowerCase()} on your desktop.</p>
        </>
      )}
    </MonoPage>
  );
}

function DocumentPage({ desktopId, monoId, documentId }: { desktopId: string; monoId: string; documentId: string }) {
  const query = useLinkQuery("mono.document", { documentId });
  const { profile } = useMonoProfile(monoId);
  const document = query.data === undefined ? null : readDocument(query.data);
  const listed = profile?.documents.find((candidate) => candidate.id === documentId);
  const title = document?.title || listed?.title || "Document";
  const tree = document ? parseVisualTree(document.tree) : null;
  const visual = document?.kind === "html" || document?.kind === "visual";
  return (
    <MonoPage testId="mono-document" title={title} back={<Back desktopId={desktopId} monoId={monoId} />}>
      {query.isError ? (
        <Notice variant="error">{/not[_ ]selected/i.test(query.error.message) ? "Not on this device's projects" : query.error.message || "Could not load this document."}</Notice>
      ) : !document ? (
        <DelayedSpinner label="Loading the document" className="m-6" />
      ) : tree ? (
        <VisualTree node={tree} />
      ) : visual ? (
        <Notice>{document.treeError ?? "Open this visual on your desktop to see it."}</Notice>
      ) : (
        <Markdown text={document.body} />
      )}
    </MonoPage>
  );
}
