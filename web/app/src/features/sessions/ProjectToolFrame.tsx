// The frame of the project tool screens (Files, Changes, Git): the workspace header with the drawer button, a 44 px back
// control to the project and the read-only chip, the project name under the title, and a Files | Changes | Git tab row
// that marks the current page. Full-height screens (Files, Changes) fill the app shell's main pane and scroll inside.
import { Link } from "@tanstack/react-router";
import type { ReactNode } from "react";
import { useCatalog } from "@/data";
import { useLinkState } from "@/link";
import { cn } from "@/lib/utils";
import { sessionCan } from "@/ui/hooks";
import { PathLabel } from "@/ui/PathLabel";
import { WorkspaceFrame } from "./WorkspaceFrame";

export type ProjectTool = "files" | "changes" | "git";

const TAB_CLASS = "inline-flex min-h-11 items-center rounded-pill px-3.5 text-label font-medium text-muted-text hover:bg-selection aria-[current=page]:bg-selection aria-[current=page]:text-content pointer-fine:min-h-9";

export interface ProjectToolFrameProps {
  desktopId: string;
  projectId: string;
  current: ProjectTool;
  title: string;
  /** Extra text after the project name, for example the open path. */
  detail?: string;
  /** True for screens that fill the pane height and scroll inside (Files, Changes). */
  fill?: boolean;
  testId: string;
  children: ReactNode;
}

export function ProjectToolFrame({ desktopId, projectId, current, title, detail, fill = false, testId, children }: ProjectToolFrameProps) {
  const project = useCatalog().data?.projects.find((candidate) => candidate.id === projectId);
  const capabilities = useLinkState((state) => state.capabilities);
  const context = { target: null, desktopCapabilities: capabilities };
  const tabs: Array<{ tool: ProjectTool; label: string }> = [];
  if (sessionCan("files", context) || current === "files") tabs.push({ tool: "files", label: "Files" });
  if (sessionCan("changes", context) || current === "changes") tabs.push({ tool: "changes", label: "Changes" });
  if (sessionCan("git", context) || current === "git") tabs.push({ tool: "git", label: "Git" });
  const name = project?.name ?? "Project";

  return (
    <WorkspaceFrame
      desktopId={desktopId}
      title={title}
      subtitle={
        detail ? (
          <span className="flex min-w-0 items-baseline gap-1">
            <span className="max-w-[40%] shrink-0 truncate">{name} /</span>
            <PathLabel path={detail} />
          </span>
        ) : (
          name
        )
      }
      back={{ to: "/d/$desktopId/p/$projectId", params: { desktopId, projectId }, label: `Back to ${name}` }}
      fill={fill}
      testId={testId}
    >
      <nav aria-label="Project pages" className="flex shrink-0 items-center gap-1 pb-2">
        {tabs.map((tab) => (
          <ToolTab key={tab.tool} desktopId={desktopId} projectId={projectId} tool={tab.tool} label={tab.label} active={tab.tool === current} />
        ))}
      </nav>
      <div className={cn(fill ? "flex min-h-0 flex-1 flex-col" : undefined)}>{children}</div>
    </WorkspaceFrame>
  );
}

function ToolTab({ desktopId, projectId, tool, label, active }: { desktopId: string; projectId: string; tool: ProjectTool; label: string; active: boolean }) {
  const aria = active ? ("page" as const) : undefined;
  if (tool === "files") return <Link to="/d/$desktopId/p/$projectId/files/$" params={{ desktopId, projectId, _splat: "" }} aria-current={aria} className={TAB_CLASS}>{label}</Link>;
  if (tool === "changes") return <Link to="/d/$desktopId/p/$projectId/changes/$" params={{ desktopId, projectId, _splat: "" }} aria-current={aria} className={TAB_CLASS}>{label}</Link>;
  return <Link to="/d/$desktopId/p/$projectId/git" params={{ desktopId, projectId }} aria-current={aria} className={TAB_CLASS}>{label}</Link>;
}
