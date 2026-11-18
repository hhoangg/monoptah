// The git route /d/:desktopId/p/:projectId/git: branch, ahead/behind, upstream and PR (F-67).
import { Link } from "@tanstack/react-router";
import { useCatalog } from "@/data";
import { useLinkState } from "@/link";
import { sessionCan } from "@/ui/hooks";
import { Notice } from "@/ui/Notice";
import { GitStatusView } from "./GitStatusView";

export function GitScreen({ desktopId, projectId }: { desktopId: string; projectId: string }) {
  const capabilities = useLinkState((state) => state.capabilities);
  const project = useCatalog().data?.projects.find((candidate) => candidate.id === projectId);
  const allowed = sessionCan("git", { target: null, desktopCapabilities: capabilities });
  return (
    <div className="mx-auto flex min-h-0 w-full max-w-3xl flex-col" data-testid="git-screen">
      <header className="px-4 pt-3 pb-2">
        <h1 className="text-headline font-semibold">Git</h1>
        <p className="truncate text-small text-muted-text">
          <Link to="/d/$desktopId/p/$projectId" params={{ desktopId, projectId }} className="hover:underline">
            {project?.name ?? "Project"}
          </Link>
          {" / "}
          <Link to="/d/$desktopId/p/$projectId/changes/$" params={{ desktopId, projectId, _splat: "" }} className="hover:underline">
            Changes
          </Link>
        </p>
      </header>
      {allowed ? <GitStatusView projectId={projectId} /> : <Notice className="m-4">Git is not available for this project.</Notice>}
    </div>
  );
}
