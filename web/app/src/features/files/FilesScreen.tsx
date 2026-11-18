// The files route /d/:desktopId/p/:projectId/files/* (F-59 to F-62): `path` is the splat, "" is the tree root.
import { Link, useNavigate } from "@tanstack/react-router";
import { useCatalog } from "@/data";
import { useLinkState } from "@/link";
import { DelayedSpinner } from "@/ui/DelayedSpinner";
import { sessionCan } from "@/ui/hooks";
import { Notice } from "@/ui/Notice";
import { useMediaQuery } from "@/ui/useMediaQuery";
import { FilesBrowser } from "./FilesBrowser";
import { useEntryKind } from "./useEntryKind";

const SPLIT_QUERY = "(min-width: 1024px)";

export function FilesScreen({ desktopId, projectId, path }: { desktopId: string; projectId: string; path: string }) {
  const navigate = useNavigate();
  const split = useMediaQuery(SPLIT_QUERY);
  const capabilities = useLinkState((state) => state.capabilities);
  const catalog = useCatalog();
  const project = catalog.data?.projects.find((candidate) => candidate.id === projectId);
  const allowed = sessionCan("files", { target: null, desktopCapabilities: capabilities });
  const kind = useEntryKind(projectId, path, allowed);

  return (
    <div className="flex min-h-0 w-full flex-col" data-testid="files-screen">
      <header className="flex items-start gap-3 px-4 pt-3 pb-2">
        <div className="min-w-0 flex-1">
          <h1 className="text-headline font-semibold">Files</h1>
          <p className="truncate text-small text-muted-text">
            <Link to="/d/$desktopId/p/$projectId" params={{ desktopId, projectId }} className="hover:underline">
              {project?.name ?? "Project"}
            </Link>
            {path ? ` / ${path}` : ""}
          </p>
        </div>
      </header>
      <div className="h-[calc(100dvh-2.75rem-env(safe-area-inset-top)-5rem)] min-h-96">
        {!allowed ? (
          <Notice className="m-4">Files are not available for this project.</Notice>
        ) : kind === "loading" ? (
          <DelayedSpinner label="Loading" className="m-6" />
        ) : (
          <FilesBrowser
            desktopId={desktopId}
            projectId={projectId}
            path={path}
            isFile={kind === "file"}
            layout={split ? "split" : "stack"}
            onNavigate={(next) => void navigate({ to: "/d/$desktopId/p/$projectId/files/$", params: { desktopId, projectId, _splat: next } })}
          />
        )}
      </div>
    </div>
  );
}
