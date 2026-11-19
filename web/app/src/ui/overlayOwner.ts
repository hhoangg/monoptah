import { createContext, useContext } from "react";

/**
 * Names the component (a composer) that owns the overlays opened below it. React context crosses portals, so the shared
 * menus, sheets and dialogs read it and mark their portaled DOM with `data-overlay-owner`; the owner can then tell its own
 * overlays from any unrelated one by DOM containment.
 */
export const OverlayOwnerContext = createContext<string | undefined>(undefined);

/** The `data-overlay-owner` attribute for a portaled overlay (nothing when it has no owner). */
export function useOverlayOwnerAttr(): { "data-overlay-owner"?: string } {
  const owner = useContext(OverlayOwnerContext);
  return owner === undefined ? {} : { "data-overlay-owner": owner };
}
