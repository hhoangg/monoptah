// F-15: the command-style picker. Search (fuzzy, `@/lib/fuzzy`) over the projects, Monos and 20 most recent
// sessions of the current desktop; Up/Down move, Enter opens, Esc closes. A dialog from 640px, a sheet below
// (ResponsiveOverlay). Opened by Mod+K (shortcuts.ts) or `openCommandPalette()` from any button.
import { useNavigate } from "@tanstack/react-router";
import { useEffect, useId, useMemo, useRef, useState } from "react";
import { useCatalog } from "@/data";
import { requestCreateMono } from "@/features/monos/createRequest";
import { useReadOnly } from "@/features/sessions/WorkspaceFrame";
import { fuzzyMatch } from "@/lib/fuzzy";
import { cn } from "@/lib/utils";
import { DelayedSpinner } from "@/ui/DelayedSpinner";
import { Icon, type IconName } from "@/ui/Icon";
import { Input } from "@/ui/input";
import { MatchText } from "@/ui/MatchText";
import { ResponsiveOverlay } from "@/ui/ResponsiveOverlay";
import { setViewportDebug, viewportDebugCommand } from "@/ui/viewportDebugFlag";
import { closeCommandPalette, openCommandPalette, useCommandPalette } from "./palette";
import { PANEL_LABELS, useProjectTools } from "./projectTools";
import type { PanelKind } from "./panels";

export { openCommandPalette };

const RECENT_SESSIONS = 20;

type ItemKind = "action" | "go" | "project" | "mono" | "session";

export interface PaletteItem {
  id: string;
  kind: ItemKind;
  label: string;
  detail?: string;
  positions: number[];
}

const GROUPS: ReadonlyArray<{ kind: ItemKind; label: string; icon: IconName }> = [
  { kind: "action", label: "This project", icon: "folder" },
  { kind: "go", label: "Go to", icon: "bot" },
  { kind: "project", label: "Projects", icon: "folder" },
  { kind: "mono", label: "Monos", icon: "bot" },
  { kind: "session", label: "Recent sessions", icon: "chat" },
];

interface Source {
  /** Actions on the current project (Files, Changes, Git, Background); absent outside a project screen. */
  actions?: ReadonlyArray<{ id: string; label: string; icon?: IconName }>;
  /** Places and actions of the desktop itself (the Mono list, a new Mono). */
  places?: ReadonlyArray<{ id: string; label: string }>;
  projects: ReadonlyArray<{ id: string; name: string; path: string }>;
  monos: ReadonlyArray<{ id: string; name: string }>;
  sessions: ReadonlyArray<{ id: string; projectId: string; title: string; updatedAt: number }>;
}

/** The items for `query`: grouped (projects, Monos, sessions), best match first, sessions capped at 20. */
export function paletteItems(source: Source, query: string): PaletteItem[] {
  const rank = (label: string) => fuzzyMatch(query, label);
  const items: PaletteItem[] = [];
  const add = (list: Array<{ item: PaletteItem; score: number }>, limit: number) => {
    list.sort((a, b) => b.score - a.score);
    for (const entry of list.slice(0, limit)) items.push(entry.item);
  };
  for (const action of source.actions ?? []) {
    const hit = rank(action.label);
    if (hit) items.push({ id: action.id, kind: "action", label: action.label, positions: hit.positions });
  }
  for (const place of source.places ?? []) {
    const hit = rank(place.label);
    if (hit) items.push({ id: place.id, kind: "go", label: place.label, positions: hit.positions });
  }
  const projectNames = new Map(source.projects.map((project) => [project.id, project.name]));

  const projects: Array<{ item: PaletteItem; score: number }> = [];
  for (const project of source.projects) {
    const hit = rank(project.name);
    if (hit) projects.push({ score: hit.score, item: { id: project.id, kind: "project", label: project.name, detail: project.path, positions: hit.positions } });
  }
  add(projects, Infinity);

  const monos: Array<{ item: PaletteItem; score: number }> = [];
  for (const mono of source.monos) {
    const hit = rank(mono.name);
    if (hit) monos.push({ score: hit.score, item: { id: mono.id, kind: "mono", label: mono.name, positions: hit.positions } });
  }
  add(monos, Infinity);

  const sessions: Array<{ item: PaletteItem; score: number }> = [];
  const byRecency = [...source.sessions].sort((a, b) => b.updatedAt - a.updatedAt);
  // With no query the 20 most recent; with one, every session competes and the best 20 stay (recency breaks ties).
  const pool = query.trim() ? byRecency : byRecency.slice(0, RECENT_SESSIONS);
  pool.forEach((session, index) => {
    const hit = rank(session.title);
    if (!hit) return;
    const project = projectNames.get(session.projectId);
    sessions.push({
      score: hit.score - index * 1e-6,
      item: { id: session.id, kind: "session", label: session.title, ...(project ? { detail: project } : {}), positions: hit.positions },
    });
  });
  add(sessions, RECENT_SESSIONS);
  return items;
}

export function CommandPalette({ desktopId }: { desktopId: string }) {
  const open = useCommandPalette((state) => state.open);
  return (
    <ResponsiveOverlay open={open} onOpenChange={(next) => (next ? openCommandPalette() : closeCommandPalette())} title="Search" fullHeight>
      <PaletteBody desktopId={desktopId} />
    </ResponsiveOverlay>
  );
}

function PaletteBody({ desktopId }: { desktopId: string }) {
  const catalog = useCatalog();
  const navigate = useNavigate();
  const listId = useId();
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const listRef = useRef<HTMLDivElement>(null);

  const readOnly = useReadOnly();
  const tools = useProjectTools();
  const toolPanels = tools.panels.join("\0");
  const hasScope = tools.scope !== null;

  const data = catalog.data;
  const items = useMemo(() => {
    if (!data) return [];
    const actions = hasScope
      ? [
          ...(toolPanels ? toolPanels.split("\0") : []).map((kind) => ({ id: kind, label: PANEL_LABELS[kind as PanelKind] })),
          { id: "background", label: "Background" },
        ]
      : [];
    // F-13, F-75: the Mono list, and a new Mono where this device may make one.
    const places = [{ id: "monos", label: "Monos" }, ...(data.monoTools === true && !readOnly ? [{ id: "new-mono", label: "New Mono" }] : [])];
    return paletteItems({ actions, places, projects: data.projects, monos: data.monos, sessions: data.sessions }, query);
  }, [data, query, toolPanels, hasScope, readOnly]);
  const current = Math.min(active, Math.max(0, items.length - 1));
  const optionId = (index: number) => `${listId}-${index}`;

  useEffect(() => {
    listRef.current?.querySelector(`[id="${listId}-${current}"]`)?.scrollIntoView?.({ block: "nearest" });
  }, [current, listId]);

  const go = (item: PaletteItem) => {
    closeCommandPalette();
    // The panels open as the sheet below 640px and as the docked or overlay panel above it.
    if (item.kind === "action") {
      if (item.id === "background") tools.openBackground();
      else tools.open(item.id as PanelKind);
    } else if (item.kind === "go") {
      if (item.id === "new-mono") requestCreateMono();
      void navigate({ to: "/d/$desktopId/monos", params: { desktopId } });
    } else if (item.kind === "project") void navigate({ to: "/d/$desktopId/p/$projectId", params: { desktopId, projectId: item.id } });
    else if (item.kind === "mono") void navigate({ to: "/d/$desktopId/monos/$monoId", params: { desktopId, monoId: item.id } });
    else void navigate({ to: "/d/$desktopId/s/$sessionId", params: { desktopId, sessionId: item.id } });
  };

  const onKeyDown = (event: React.KeyboardEvent) => {
    if (event.nativeEvent.isComposing) return;
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      if (items.length === 0) return;
      const step = event.key === "ArrowDown" ? 1 : -1;
      setActive((current + step + items.length) % items.length);
    } else if (event.key === "Enter") {
      // TEMPORARY: Enter on exactly "debug viewport" / "debug off" toggles the viewport debug overlay (the installed
      // app has no URL bar). Only on Enter, so typing a search that starts with these words never reloads.
      const debug = viewportDebugCommand(query);
      if (debug !== null) {
        event.preventDefault();
        setViewportDebug(debug);
        window.location.reload();
        return;
      }
      const item = items[current];
      if (item) {
        event.preventDefault();
        go(item);
      }
    }
  };

  return (
    <div className="flex flex-col gap-3" data-testid="command-palette" data-command-palette="">
      <Input
        type="search"
        role="combobox"
        aria-label="Search projects, Monos and sessions"
        aria-expanded="true"
        aria-controls={listId}
        aria-activedescendant={items.length > 0 ? optionId(current) : undefined}
        aria-autocomplete="list"
        placeholder="Search projects, Monos and sessions"
        autoComplete="off"
        autoFocus
        value={query}
        onChange={(event) => {
          setQuery(event.target.value);
          setActive(0);
        }}
        onKeyDown={onKeyDown}
      />
      <div ref={listRef} id={listId} role="listbox" aria-label="Results" className="flex max-h-[min(24rem,60dvh)] flex-col gap-3 overflow-y-auto">
        {GROUPS.map((group) => {
          const groupItems = items.filter((item) => item.kind === group.kind);
          if (groupItems.length === 0) return null;
          return (
            <div key={group.kind} role="group" aria-label={group.label} className="flex flex-col gap-0.5">
              <div aria-hidden="true" className="px-2.5 pb-1 text-meta font-medium text-muted-text">
                {group.label}
              </div>
              {groupItems.map((item) => {
                const mine = items.indexOf(item);
                const selected = mine === current;
                return (
                  <div
                    key={`${item.kind}-${item.id}`}
                    id={optionId(mine)}
                    role="option"
                    aria-selected={selected}
                    onClick={() => go(item)}
                    onPointerMove={() => setActive(mine)}
                    className={cn("flex min-h-11 cursor-pointer items-center gap-3 rounded-row px-2.5 py-2", selected && "bg-selection")}
                  >
                    <Icon name={group.icon} size={20} className="shrink-0 text-muted-text" />
                    <span className="min-w-0 flex-1 truncate text-row">
                      <MatchText text={item.label} positions={item.positions} active={query.trim() !== ""} />
                    </span>
                    {item.detail ? <span className="max-w-[45%] truncate text-label text-muted-text">{item.detail}</span> : null}
                  </div>
                );
              })}
            </div>
          );
        })}
        {data && items.length === 0 ? <p className="px-2.5 py-3 text-label text-muted-text">Nothing matches.</p> : null}
        {!data && catalog.isPending ? <div className="px-2.5 py-3"><DelayedSpinner label="Loading" /></div> : null}
        {!data && catalog.isError ? <p className="px-2.5 py-3 text-label text-muted-text">Could not load the workspace.</p> : null}
      </div>
    </div>
  );
}
