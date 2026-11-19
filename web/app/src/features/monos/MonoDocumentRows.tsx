// The documents a Mono wrote, newest first, as rows of a card (F-76 and the documents list page). With `limit`,
// only that many, and a "See all" row under them that opens the list page.
import { Link } from "@tanstack/react-router";
import { Icon } from "@/ui/Icon";
import { ago, type MonoDocumentRow } from "./profile";

export function MonoDocumentRows({ desktopId, monoId, documents, limit }: { desktopId: string; monoId: string; documents: MonoDocumentRow[]; limit?: number }) {
  if (documents.length === 0) return <p className="px-2 py-3 text-small text-muted-text">No documents yet.</p>;
  const newest = [...documents].sort((a, b) => b.updatedAt - a.updatedAt);
  const shown = limit === undefined ? newest : newest.slice(0, limit);
  const more = newest.length - shown.length;
  return (
    <ul>
      {shown.map((document) => (
        <li key={document.id}>
          <Link
            to="/d/$desktopId/monos/$monoId/profile/file/$name"
            params={{ desktopId, monoId, name: document.id }}
            aria-label={`Open ${document.title}`}
            className="flex min-h-12 items-center gap-3 rounded-row px-2 hover:bg-selection"
          >
            <Icon name={["html", "visual"].includes(document.kind) ? "metrics" : "note"} size={18} />
            <span className="min-w-0 flex-1 truncate text-row">{document.title}</span>
            <span className="text-meta text-muted-text">{ago(document.updatedAt)}</span>
          </Link>
        </li>
      ))}
      {more > 0 ? (
        <li>
          <Link
            to="/d/$desktopId/monos/$monoId/profile/documents"
            params={{ desktopId, monoId }}
            aria-label={`See all ${newest.length} documents`}
            className="flex min-h-12 items-center gap-3 rounded-row px-2 hover:bg-selection"
          >
            <Icon name="list" size={18} />
            <span className="min-w-0 flex-1 truncate text-row text-[color:var(--accent-color)]">See all</span>
            <span className="text-meta text-muted-text">{newest.length}</span>
            <Icon name="chevronRight" size={16} className="text-muted-text" />
          </Link>
        </li>
      ) : null}
    </ul>
  );
}
