// The changes route /d/:desktopId/p/:projectId/changes/*: an empty splat is the list, a path the diff of that file.
import { Link, useNavigate } from "@tanstack/react-router";
import { useCatalog } from "@/data";
import { useLinkState } from "@/link";
import { sessionCan } from "@/ui/hooks";
import { Notice } from "@/ui/Notice";
import { ChangesBrowser } from "./ChangesBrowser";

export function ChangesScreen({ desktopId, projectId, path }: { desktopId: string; projectId: string; path: string }) {
  const navigate = useNavigate();
  const capabilities = useLinkState((state) => state.capabilities);
  const project = useCatalog().data?.projects.find((candidate) => candidate.id === projectId);
  const allowed = sessionCan("changes", { target: null, desktopCapabilities: capabilities });
  const canGit = sessionCan("git", { target: null, desktopCapabilities: capabilities });
  return (
    <div className="mx-auto flex min-h-0 w-full max-w-3xl flex-col" data-testid="changes-screen">
      <header className="flex items-start gap-3 px-4 pt-3 pb-2">
        <div className="min-w-0 flex-1">
          <h1 className="text-headline font-semibold">Changes</h1>
          <p className="truncate text-small text-muted-text">
            <Link to="/d/$desktopId/p/$projectId" params={{ desktopId, projectId }} className="hover:underline">
              {project?.name ?? "Project"}
            </Link>
            {canGit ? (
              <>
                {" / "}
                <Link to="/d/$desktopId/p/$projectId/git" params={{ desktopId, projectId }} className="hover:underline">
                  Git status
                </Link>
              </>
            ) : null}
          </p>
        </div>
      </header>
      {allowed ? (
        <div className="flex h-[calc(100dvh-2.75rem-env(safe-area-inset-top)-5rem)] min-h-96 flex-col">
          <ChangesBrowser
            projectId={projectId}
            path={path}
            onNavigate={(next) => void navigate({ to: "/d/$desktopId/p/$projectId/changes/$", params: { desktopId, projectId, _splat: next } })}
          />
        </div>
      ) : (
        <Notice className="m-4">Changes are not available for this project.</Notice>
      )}
    </div>
  );
}
