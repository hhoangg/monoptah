// The drawer's list logic, ported from mobile/src/features/workspace/WorkspaceDrawer.tsx: Monos heard
// from last first, narrowed by the search and the filter; a project has no replies to read, so only
// "All" and "Working now" keep it.
import type { ResultOf } from "@monoptah/protocol";
import type { DrawerFilterState } from "./drawerFilter";

type WorkspaceList = ResultOf<"workspace.list">;
export type CatalogMono = WorkspaceList["monos"][number];
export type CatalogProject = WorkspaceList["projects"][number];

/** This slice of the Mono create flow (U-09, U-12) is not built yet; "New Mono" stays hidden until it is. */
export const MONO_CREATE_UI_SHIPPED = false;

/** "New Mono" shows only when the desktop advertises `monoTools` AND the create UI exists. */
export function canCreateMono(monoTools: boolean): boolean {
  return monoTools && MONO_CREATE_UI_SHIPPED;
}

export const monoUnread = (mono: CatalogMono): boolean => (mono.unread ?? 0) > 0;
export const monoWorking = (mono: CatalogMono): boolean => !!mono.status && mono.status !== "idle";

export function filterWorkspace(
  catalog: Pick<WorkspaceList, "monos" | "projects">,
  query: string,
  { filter, show }: DrawerFilterState,
  /** The Monos with a reply not seen on this device (F-17); the desktop's own count when omitted. */
  unreadIds?: ReadonlySet<string>,
): { monos: CatalogMono[]; projects: CatalogProject[] } {
  const needle = query.trim().toLowerCase();
  const monos =
    show === "projects"
      ? []
      : [...catalog.monos]
          .sort((a, b) => (b.lastActiveAt ?? 0) - (a.lastActiveAt ?? 0))
          .filter(
            (mono) =>
              (!needle || mono.name.toLowerCase().includes(needle)) &&
              (filter === "all" || (filter === "unread" ? (unreadIds ? unreadIds.has(mono.id) : monoUnread(mono)) : monoWorking(mono))),
          );
  const projects =
    show === "monos"
      ? []
      : catalog.projects.filter((project) => (!needle || project.name.toLowerCase().includes(needle)) && (filter === "all" || (filter === "working" && !!project.busy)));
  return { monos, projects };
}
