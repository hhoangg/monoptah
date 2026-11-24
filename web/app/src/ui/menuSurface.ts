/**
 * The one surface of every popup menu (dropdowns, the composer's `/` menu, later pickers): design-system.md section 8.
 * Tailwind only sees literal strings, so they are written out in full here.
 *
 * Geometry, the same as `SettingsCard`: the menu has the same inset on all four sides and an item highlight has the
 * radius of the menu minus that inset (`rounded-row`), so the two curves are concentric. The 1px border counts toward the
 * inset (padding is the inset minus 1px), so the distance from the outer edge to the highlight is exactly
 * `--settings-card-inset` and the item radius is exactly card radius - inset. No shadow: the design system removes them
 * and the border is the edge.
 *
 * `data-menu-surface` / `data-menu-item` let tailwind.css give the items the squircle corner of the surface.
 */
export const MENU_SURFACE = "rounded-card border border-stroke bg-popover p-[calc(var(--settings-card-inset)-1px)] text-popover-foreground";
export const MENU_SURFACE_ATTR = { "data-menu-surface": "" } as const;

/** An item of a menu: the radius that is concentric with `MENU_SURFACE`. */
export const MENU_ITEM = "rounded-row";
export const MENU_ITEM_ATTR = { "data-menu-item": "" } as const;
/** The highlight of the active item, for items that set a class (the `/` menu) and for Radix items (`data-highlighted`). */
export const MENU_ITEM_ACTIVE = "bg-selection";
export const MENU_ITEM_HIGHLIGHTED = "data-[highlighted]:bg-selection";
