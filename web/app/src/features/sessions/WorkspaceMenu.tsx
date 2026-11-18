// F-19: the workspace menu (ported from mobile/src/features/workspace/WorkspaceMenu.tsx): where work
// happens in a project. It names the working copy and offers the project's checkout, each existing
// worktree (`project.worktrees`), a link to view the changes and, where a screen starts sessions, a new
// worktree. On the project screen a pick scopes the sessions list (F-22). "New worktree" is disabled for a
// read-only device.
import { Link } from "@tanstack/react-router";
import { useLinkQuery } from "@/data";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuSeparator, DropdownMenuTrigger } from "@/ui/dropdown-menu";
import { Icon } from "@/ui/Icon";
import type { Worktree } from "./sessionListFilter";

const trimSlashes = (path: string) => path.replace(/\/+$/, "");

export interface WorkspaceMenuProps {
  desktopId: string;
  projectId: string;
  /** The project's own checkout path. */
  projectPath: string;
  /** The project checkout's branch, to name it. */
  checkoutBranch?: string;
  /** The worktree picked, or null for the project's checkout. */
  value: Worktree | null;
  onChange: (value: Worktree | null) => void;
  /** Offers "New worktree" (the new-session page); the project screen has none. */
  onNewWorktree?: () => void;
  readOnly: boolean;
}

export function WorkspaceMenu({ desktopId, projectId, projectPath, checkoutBranch, value, onChange, onNewWorktree, readOnly }: WorkspaceMenuProps) {
  const worktrees = useLinkQuery("project.worktrees", { projectId });
  // The checkout is listed by git as a worktree too; it has its own row.
  const others = (worktrees.data?.worktrees ?? []).filter((worktree) => trimSlashes(worktree.path) !== trimSlashes(projectPath));
  const label = value ? value.name : checkoutBranch || "Project checkout";
  const params = { desktopId, projectId };
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          aria-label={`Workspace: ${label}`}
          data-scoped={!!value}
          className="inline-flex min-h-11 max-w-full items-center gap-1.5 rounded-pill bg-fill px-3.5 text-label text-content hover:bg-selection data-[scoped=true]:text-brand pointer-fine:min-h-9"
        >
          <Icon name="branch" size={16} className="shrink-0" />
          <span className="truncate">{label}</span>
          <Icon name="chevronDown" size={12} className="shrink-0 text-muted-text" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start">
        <DropdownMenuLabel>Workspace</DropdownMenuLabel>
        <DropdownMenuItem role="menuitemradio" aria-checked={!value} onSelect={() => onChange(null)}>
          <Icon name="folder" size={18} />
          <span className="flex min-w-0 flex-1 flex-col">
            <span>Project checkout</span>
            {checkoutBranch ? <span className="truncate text-meta text-muted-text">{checkoutBranch}</span> : null}
          </span>
          {!value ? <Icon name="check" size={16} /> : null}
        </DropdownMenuItem>
        {others.map((worktree) => {
          const picked = !!value && trimSlashes(value.path) === trimSlashes(worktree.path);
          return (
            <DropdownMenuItem key={worktree.path} role="menuitemradio" aria-checked={picked} onSelect={() => onChange({ path: worktree.path, name: worktree.name })}>
              <Icon name="branch" size={18} />
              <span className="min-w-0 flex-1 truncate">{worktree.name}</span>
              {picked ? <Icon name="check" size={16} /> : null}
            </DropdownMenuItem>
          );
        })}
        <DropdownMenuSeparator />
        <DropdownMenuItem asChild>
          <Link to="/d/$desktopId/p/$projectId/changes/$" params={{ ...params, _splat: "" }}>
            <Icon name="list" size={18} />
            View changes
          </Link>
        </DropdownMenuItem>
        {onNewWorktree ? (
          <DropdownMenuItem disabled={readOnly} title={readOnly ? "This device is read-only" : undefined} onSelect={onNewWorktree}>
            <Icon name="add" size={18} />
            New worktree
          </DropdownMenuItem>
        ) : null}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
