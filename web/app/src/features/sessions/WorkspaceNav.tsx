// F-12: the workspace list, laid out as a messaging app's chat list (ported from
// mobile/src/features/workspace/WorkspaceDrawer.tsx): Monos with their latest line and when, then
// Projects, narrowed by the filter (All, Unread, Working now) and the search. It is the content of the
// vaul drawer on the mobile layout and of the persistent sidebar from 640 px up (design-system 11). The desktop nav entries
// (`desktop-nav-items` slot, Settings, Desktops) sit at the foot, except beside the activity rail, which carries them.
import { Link, useNavigate, useRouterState } from "@tanstack/react-router";
import { useEffect, useState } from "react";
import { useCatalog } from "@/data";
import { requestCreateMono } from "@/features/monos/createRequest";
import { useLinkState } from "@/link";
import { useCapability } from "@/version";
import { safeHexColor } from "@/lib/color";
import { cn } from "@/lib/utils";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuSeparator, DropdownMenuTrigger } from "@/ui/dropdown-menu";
import { Icon } from "@/ui/Icon";
import { Input } from "@/ui/input";
import { Slot } from "@/ui/slots";
import { useDrawerFilter, type DrawerFilter, type DrawerShow } from "./drawerFilter";
import { markMonoSeen, useCatalogUnreadMonos } from "./monoSeen";
import { projectColor } from "./projectColors";
import { ProjectMascot } from "./ProjectMascot";
import { chatListTime } from "./relativeTime";
import { canCreateMono, filterWorkspace, monoUnread, type CatalogMono, type CatalogProject } from "./workspaceFilter";

const FILTERS: ReadonlyArray<{ value: DrawerFilter; label: string }> = [
  { value: "all", label: "All" },
  { value: "unread", label: "Unread" },
  { value: "working", label: "Working now" },
];
const SHOWS: ReadonlyArray<{ value: DrawerShow; label: string }> = [
  { value: "both", label: "Monos and projects" },
  { value: "monos", label: "Monos" },
  { value: "projects", label: "Projects" },
];

/** The start of a message as one plain line: markdown marks dropped, a reaction shown as its emoji. */
export function oneLine(text: string): string {
  // The pattern backtracks on a long run of spaces, so only a short message (a reaction is) is matched.
  const head = text.trim();
  const reaction = head.length <= 64 ? /^<reaction>\s*(.{1,16}?)\s*<\/reaction>$/su.exec(head) : null;
  if (reaction) return `Reacted ${reaction[1]}`;
  return text
    .slice(0, 600)
    .replace(/```[\s\S]*?```/g, " ")
    .replace(/[*_`#>~]/g, "")
    .replace(/\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/\s+/g, " ")
    .trim();
}

/** A project's or Mono's colour: the desktop's when it is a plain `#rrggbb`, else the shared name hash (F-16). */
export function accentFor(name: string, color: string | undefined): string {
  return safeHexColor(color, projectColor(name));
}

export function ProjectRow({ desktopId, project, active, onNavigate }: { desktopId: string; project: CatalogProject; active: boolean; onNavigate?: () => void }) {
  return (
    <Link
      to="/d/$desktopId/p/$projectId"
      params={{ desktopId, projectId: project.id }}
      onClick={onNavigate}
      aria-current={active ? "page" : undefined}
      className={cn("flex min-h-13 items-center gap-3.5 rounded-row px-2.5 py-2 hover:bg-content-a04", active && "bg-selection hover:bg-selection")}
    >
      <span className="flex w-10 shrink-0 justify-center">
        <ProjectMascot project={project.path} name={project.mascot} color={accentFor(project.name, project.color)} size={24} active={!!project.busy} />
      </span>
      <span className="min-w-0 flex-1 truncate text-row font-semibold" title={project.name}>{project.name}</span>
      {project.busy ? <span aria-label="Working" className="size-2 shrink-0 rounded-pill bg-brand" /> : null}
    </Link>
  );
}

export function MonoRow({
  desktopId,
  mono,
  active,
  unread: unreadGiven,
  onNavigate,
}: {
  desktopId: string;
  mono: CatalogMono;
  active: boolean;
  /** A reply not seen on this device (F-17); the desktop's own count when omitted. */
  unread?: boolean;
  onNavigate?: () => void;
}) {
  const unread = unreadGiven ?? monoUnread(mono);
  const detail = mono.status === "working" ? "Working…" : mono.status === "needs-you" ? "Needs you" : oneLine(mono.lastMessage ?? "") || "No messages yet";
  return (
    <Link
      to="/d/$desktopId/monos/$monoId"
      params={{ desktopId, monoId: mono.id }}
      onClick={onNavigate}
      aria-current={active ? "page" : undefined}
      className={cn("flex min-h-16 items-center gap-3.5 rounded-[16px] px-2.5 py-2.5 hover:bg-content-a04", active && "bg-selection hover:bg-selection")}
    >
      <span
        aria-hidden="true"
        className="flex size-10 shrink-0 items-center justify-center rounded-pill text-body font-semibold text-white"
        style={{ backgroundColor: accentFor(mono.name, mono.color) }}
      >
        {(mono.name.trim().charAt(0) || "M").toUpperCase()}
      </span>
      <span className="flex min-w-0 flex-1 flex-col gap-0.5">
        <span className="flex items-center gap-2">
          <span className="min-w-0 flex-1 truncate text-row font-semibold" title={mono.name}>{mono.name}</span>
          {mono.lastActiveAt ? <span className={cn("shrink-0 text-small tabular text-muted-text", unread && "text-brand-text")}>{chatListTime(mono.lastActiveAt)}</span> : null}
        </span>
        <span className="flex items-center gap-2">
          <span className={cn("min-w-0 flex-1 truncate text-label text-muted-text", mono.status === "needs-you" && "text-caution")}>{detail}</span>
          {unread ? <span aria-label="Unread" className="size-2.5 shrink-0 rounded-pill bg-brand" /> : null}
        </span>
      </span>
    </Link>
  );
}

function FilterMenu({ filter, show, onChange }: { filter: DrawerFilter; show: DrawerShow; onChange: (patch: { filter?: DrawerFilter; show?: DrawerShow }) => void }) {
  const on = filter !== "all" || show !== "both";
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          aria-label={on ? "Filter, on" : "Filter"}
          data-on={on}
          className="inline-flex size-11 items-center justify-center rounded-pill bg-fill text-content hover:bg-selection data-[on=true]:text-brand pointer-fine:size-9"
        >
          <Icon name="filter" size={20} />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        <DropdownMenuLabel>Show</DropdownMenuLabel>
        {SHOWS.map((option) => (
          <DropdownMenuItem key={option.value} onSelect={() => onChange({ show: option.value })} aria-checked={show === option.value} role="menuitemradio">
            <span className="flex-1">{option.label}</span>
            {show === option.value ? <Icon name="check" size={16} /> : null}
          </DropdownMenuItem>
        ))}
        <DropdownMenuSeparator />
        <DropdownMenuLabel>Which</DropdownMenuLabel>
        {FILTERS.map((option) => (
          <DropdownMenuItem key={option.value} onSelect={() => onChange({ filter: option.value })} aria-checked={filter === option.value} role="menuitemradio">
            <span className="flex-1">{option.label}</span>
            {filter === option.value ? <Icon name="check" size={16} /> : null}
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/** The active project or Mono id from the current path. */
function useActivePlace(desktopId: string): { projectId?: string; monoId?: string } {
  const pathname = useRouterState({ select: (state) => state.location.pathname });
  const prefix = `/d/${encodeURIComponent(desktopId)}/`;
  if (!pathname.startsWith(prefix)) return {};
  const [kind, id] = pathname.slice(prefix.length).split("/");
  const decoded = id ? decodeURIComponent(id) : undefined;
  if (kind === "p") return { projectId: decoded };
  if (kind === "monos") return { monoId: decoded };
  return {};
}

export interface WorkspaceNavProps {
  desktopId: string;
  /** Called when a link is followed, so the drawer can close. */
  onNavigate?: () => void;
  /** Called by the New Mono button (visible only when `canCreateMono`); by default it opens the dialog on the Mono list. */
  onNewMono?: () => void;
  /** The activity rail carries Cloud, Desktop settings and All desktops (tablet and desktop): leave them out of the foot. */
  railed?: boolean;
}

export function WorkspaceNav({ desktopId, onNavigate, onNewMono, railed = false }: WorkspaceNavProps) {
  const catalog = useCatalog();
  const monoTools = useCapability("monoTools");
  const readOnly = useLinkState((state) => state.permission === "read");
  const desktopName = useLinkState((state) => state.desktopName);
  const navigate = useNavigate();
  const newMono = () => {
    if (onNewMono) {
      onNewMono();
      return;
    }
    // The Mono list is not mounted yet: it takes the request once it is (createRequest.ts).
    requestCreateMono();
    onNavigate?.();
    void navigate({ to: "/d/$desktopId/monos", params: { desktopId } });
  };
  const [{ filter, show }, update] = useDrawerFilter();
  const [query, setQuery] = useState("");
  const active = useActivePlace(desktopId);
  const data = catalog.data;
  const unreadIds = useCatalogUnreadMonos(desktopId);
  const { monos, projects } = data ? filterWorkspace(data, query, { filter, show }, unreadIds) : { monos: [], projects: [] };

  // A Mono open on screen has its latest reply seen (F-17).
  const openMono = data?.monos.find((mono) => mono.id === active.monoId);
  useEffect(() => {
    if (openMono) markMonoSeen(desktopId, openMono);
  }, [desktopId, openMono]);
  const narrowed = query.trim() !== "" || filter !== "all" || show !== "both";

  return (
    <nav aria-label="Workspace" data-testid="workspace-nav" className="flex h-full min-h-0 flex-col bg-drawer">
      <div className="flex items-center gap-2.5 px-4 pt-3 pb-2">
        <span className="min-w-0 flex-1 truncate text-title font-semibold" title={desktopName ?? undefined}>{desktopName ?? "Monoptah"}</span>
        <FilterMenu filter={filter} show={show} onChange={update} />
        {canCreateMono(monoTools) && !readOnly ? (
          <button type="button" aria-label="New Mono" onClick={newMono} className="inline-flex size-11 items-center justify-center rounded-pill bg-fill hover:bg-selection pointer-fine:size-9">
            <Icon name="add" size={20} />
          </button>
        ) : null}
      </div>
      <div className="px-4 pb-2">
        <Input type="search" aria-label="Filter projects" placeholder="Filter projects" autoComplete="off" value={query} onChange={(event) => setQuery(event.target.value)} />
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto px-1.5 pb-3">
        {monos.length > 0 ? (
          <section aria-label="Monos" className="flex flex-col gap-0.5">
            <h2 className="px-2.5 pb-1 text-meta font-medium text-muted-text">Monos</h2>
            {monos.map((mono) => (
              <MonoRow key={mono.id} desktopId={desktopId} mono={mono} active={mono.id === active.monoId} unread={unreadIds.has(mono.id)} onNavigate={onNavigate} />
            ))}
          </section>
        ) : null}
        {projects.length > 0 ? (
          <section aria-label="Projects" className={cn("flex flex-col gap-0.5", monos.length > 0 && "mt-4")}>
            <h2 className="px-2.5 pb-1 text-meta font-medium text-muted-text">Projects</h2>
            {projects.map((project) => (
              <ProjectRow key={project.id} desktopId={desktopId} project={project} active={project.id === active.projectId} onNavigate={onNavigate} />
            ))}
          </section>
        ) : null}
        {data && monos.length === 0 && projects.length === 0 ? (
          <p className="px-4 pt-3 text-label text-muted-text">{narrowed ? "Nothing matches." : "No Monos or projects on this desktop yet."}</p>
        ) : null}
        {!data && catalog.isPending ? <p className="px-4 pt-3 text-label text-muted-text">Loading…</p> : null}
        {!data && catalog.isError ? <div className="flex items-center gap-2 px-4 pt-3 text-label text-muted-text">
            <span className="min-w-0 flex-1">Could not load the workspace.</span>
            <button type="button" onClick={() => void catalog.refetch()} className="inline-flex min-h-11 items-center rounded-pill px-3 text-brand-text hover:bg-content-a04 pointer-fine:min-h-9">
              Retry
            </button>
          </div> : null}
      </div>
      {railed ? null : (
        <div className="flex flex-col gap-0.5 border-t border-stroke p-1.5">
          <Slot name="desktop-nav-items" desktopId={desktopId} />
          <Link
            to="/d/$desktopId/settings"
            params={{ desktopId }}
            onClick={onNavigate}
            className="flex min-h-11 items-center gap-3 rounded-row px-2.5 text-row hover:bg-content-a04 [&.active]:bg-selection"
          >
            <Icon name="settings" size={20} />
            Desktop settings
          </Link>
          <Link to="/desktops" onClick={onNavigate} className="flex min-h-11 items-center gap-3 rounded-row px-2.5 text-row hover:bg-content-a04">
            <Icon name="computer" size={20} />
            All desktops
          </Link>
        </div>
      )}
    </nav>
  );
}
