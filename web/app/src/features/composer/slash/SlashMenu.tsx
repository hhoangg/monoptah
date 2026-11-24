// The `/` menu of a composer (the web twin of the native "Skills & Commands" popup): a listbox anchored above the
// composer. On a wide screen with a pointer it is a popover; on a phone it is a bottom sheet docked over the
// keyboard line, with rows tall enough for a thumb. The text field keeps focus the whole time (the rows never take
// it: a press on one is `preventDefault`ed), so the keyboard stays up and typing keeps filtering. The field is the
// combobox: the composer owns the keys (up and down, Enter or Tab, Escape) and names the active row with
// `aria-activedescendant`.
import { useEffect, useRef } from "react";
import type { SlashCommand } from "@monoptah/protocol";
import { cn } from "@/lib/utils";
import { useAnchoredPlacement, revealInList } from "@/ui/anchoredPlacement";
import { Icon, type IconName } from "@/ui/Icon";
import { MENU_ITEM, MENU_ITEM_ACTIVE, MENU_ITEM_ATTR, MENU_SURFACE, MENU_SURFACE_ATTR } from "@/ui/menuSurface";

export const SLASH_MENU_LABEL = "Skills and commands";

/** Tallest popover (18rem). */
const POPOVER_MAX_PX = 288;
/** The phone sheet takes at most this share of the visual viewport (not `dvh`: that ignores the on-screen keyboard). */
const SHEET_SHARE = 0.55;
const popoverCap = () => POPOVER_MAX_PX;
const sheetCap = (viewportHeight: number) => viewportHeight * SHEET_SHARE;

/** The design system's icon for a command id (the desktop names the icon after the command). */
const ICONS: Record<string, IconName> = {
  draft: "note",
  orchestrator: "layers",
  operator: "robot",
  plan: "list",
  btw: "chat",
  compact: "exitFullscreen",
  "add-to-folder": "folder",
  skill: "sparkle",
};

export function slashIcon(icon: string): IconName {
  return ICONS[icon] ?? "sparkle";
}

export function slashOptionId(menuId: string, row: SlashCommand): string {
  return `${menuId}-${row.kind}-${row.id}`;
}

export interface SlashMenuProps {
  /** Id of the listbox; the field points `aria-controls` at it. */
  id: string;
  /** The composer's overlay owner id: a press on the menu counts as a press inside the composer. */
  owner: string;
  rows: readonly SlashCommand[];
  loading: boolean;
  failed?: boolean;
  active: number;
  /** A phone layout: a bottom sheet instead of a popover. */
  sheet: boolean;
  onActive: (index: number) => void;
  onChoose: (row: SlashCommand) => void;
  onClose: () => void;
}

export function SlashMenu({ id, owner, rows, loading, failed = false, active, sheet, onActive, onChoose, onClose }: SlashMenuProps) {
  const root = useRef<HTMLDivElement>(null);
  const list = useRef<HTMLUListElement>(null);
  // Fits the visible area (visual viewport and clipping ancestors) above the composer, or below when there is no room.
  const placement = useAnchoredPlacement(root, true, { cap: sheet ? sheetCap : popoverCap });
  const side = placement?.side ?? "above";
  // Only the list scrolls to keep the active row in view: `scrollIntoView` would also move the page.
  // Keyed by the ids, not the `rows` array: the composer rebuilds it on every render, and a re-render must not snap a
  // list the user is scrolling back to the active row.
  const rowsKey = rows.map((row) => `${row.kind}-${row.id}`).join("|");
  useEffect(() => {
    const current = list.current?.querySelector<HTMLElement>('[aria-selected="true"]');
    if (list.current && current) revealInList(list.current, current);
  }, [active, rowsKey, placement?.maxHeight]);

  return (
    <div
      ref={root}
      {...MENU_SURFACE_ATTR}
      data-testid="slash-menu"
      data-overlay-owner={owner}
      data-layout={sheet ? "sheet" : "popover"}
      data-side={side}
      style={placement ? { maxHeight: placement.maxHeight } : undefined}
      className={cn(
        "absolute z-30 flex flex-col overflow-hidden",
        MENU_SURFACE,
        side === "above" ? "bottom-full mb-2" : "top-full mt-2",
        // `max-h-72` only holds until the first measure: the measured height replaces it.
        "max-h-72",
        sheet ? "inset-x-0" : "left-0 w-[min(26rem,100%)]",
      )}
      // Keep the caret in the field: a press on the menu must not blur it.
      onMouseDown={(event) => event.preventDefault()}
    >
      {sheet ? (
        <div className="relative flex h-11 shrink-0 items-center px-3">
          <span aria-hidden="true" className="mx-auto block h-1 w-9 rounded-pill bg-stroke" />
          <button type="button" aria-label="Close" onClick={onClose} className="absolute right-0 top-0 inline-flex size-11 items-center justify-center rounded-pill text-muted-text hover:bg-selection">
            <Icon name="close" size={16} />
          </button>
        </div>
      ) : null}
      <ul ref={list} id={id} role="listbox" aria-label={SLASH_MENU_LABEL} className="m-0 min-h-0 flex-1 list-none overflow-y-auto p-0">
        {rows.length === 0 ? (
          <li role="presentation" className="px-3 py-3 text-meta text-muted-text">
            {failed ? "Could not load the commands. Try typing / again." : loading ? "Loading..." : "No matching command or skill"}
          </li>
        ) : null}
        {rows.map((row, index) => (
          <li
            key={`${row.kind}-${row.id}`}
            id={slashOptionId(id, row)}
            role="option"
            aria-selected={index === active}
            aria-disabled={!row.available || undefined}
            data-command={row.id}
            data-kind={row.kind}
            title={row.available ? undefined : row.reason}
            onPointerMove={() => index !== active && onActive(index)}
            onClick={() => onChoose(row)}
            {...MENU_ITEM_ATTR}
            className={cn(
              `flex cursor-pointer items-center gap-3 ${MENU_ITEM} px-3 text-row`,
              sheet ? "min-h-12" : "min-h-10 py-1.5",
              index === active && MENU_ITEM_ACTIVE,
              !row.available && "cursor-not-allowed opacity-50",
            )}
          >
            <Icon name={slashIcon(row.icon)} size={18} className="shrink-0 text-muted-text" />
            <span className="min-w-0 flex-1">
              <span className="flex items-center gap-2">
                <span className="truncate font-medium text-content">{row.kind === "skill" ? `/${row.label}` : row.label}</span>
                {row.active ? <span className="rounded-pill bg-fill px-1.5 text-meta text-brand">On</span> : null}
              </span>
              <span className="block truncate text-meta text-muted-text">{row.available ? row.detail : (row.reason ?? row.detail)}</span>
            </span>
          </li>
        ))}
      </ul>
    </div>
  );
}
