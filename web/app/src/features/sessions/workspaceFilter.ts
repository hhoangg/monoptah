// The drawer's list logic, ported from mobile/src/features/workspace/WorkspaceDrawer.tsx: Monos heard
// from last first. The list is never narrowed.
import type { ResultOf } from "@monoptah/protocol";

type WorkspaceList = ResultOf<"workspace.list">;
export type CatalogMono = WorkspaceList["monos"][number];
export type CatalogProject = WorkspaceList["projects"][number];

/** The Mono create flow (U-09, U-12) is built (`CreateMonoDialog`, opened from the Mono list). */
export const MONO_CREATE_UI_SHIPPED = true;

/** "New Mono" shows only when the desktop advertises `monoTools` AND the create UI exists. */
export function canCreateMono(monoTools: boolean): boolean {
  return monoTools && MONO_CREATE_UI_SHIPPED;
}

export const monoUnread = (mono: CatalogMono): boolean => (mono.unread ?? 0) > 0;

/** Monos heard from last first. */
export function sortMonos(monos: readonly CatalogMono[]): CatalogMono[] {
  return [...monos].sort((a, b) => (b.lastActiveAt ?? 0) - (a.lastActiveAt ?? 0));
}
