// The drawer's list logic, ported from mobile/src/features/workspace/WorkspaceDrawer.tsx: Monos heard
// from last first. The list is never narrowed.
import type { ResultOf } from "@monoptah/protocol";
import { useCatalog } from "@/data";
import { useCapability } from "@/version";

type WorkspaceList = ResultOf<"workspace.list">;
export type CatalogMono = WorkspaceList["monos"][number];
export type CatalogProject = WorkspaceList["projects"][number];

/** The Mono create flow (U-09, U-12) is built (`CreateMonoDialog`, opened from the Mono list). */
export const MONO_CREATE_UI_SHIPPED = true;

/**
 * "New Mono" shows only when the desktop offers Mono tools AND the create UI exists. The real desktop sends
 * `monoTools` in the `workspace.list` catalog; an older desktop advertised it as a capability, so either counts.
 */
export function canCreateMono(catalogMonoTools: boolean | undefined, advertised = false): boolean {
  return (catalogMonoTools === true || advertised) && MONO_CREATE_UI_SHIPPED;
}

/** The one rule for "this desktop offers Mono tools": every Mono screen, the nav and the palette go through it. */
export function useMonoTools(): boolean {
  const catalog = useCatalog();
  const advertised = useCapability("monoTools");
  return catalog.data?.monoTools === true || advertised;
}

export const monoUnread = (mono: CatalogMono): boolean => (mono.unread ?? 0) > 0;

/** Monos heard from last first. */
export function sortMonos(monos: readonly CatalogMono[]): CatalogMono[] {
  return [...monos].sort((a, b) => (b.lastActiveAt ?? 0) - (a.lastActiveAt ?? 0));
}
