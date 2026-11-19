// The tab strip of one pane (design-system.md section 11): a tablist of the pane's sessions and Mono chats, a close
// button per tab, drag to reorder or to another pane, and the pane's split and close buttons. Tabs scroll sideways
// when they do not fit (edge fades show it) and the active one scrolls into view. An "All tabs" menu opens or closes any tab.
//
// The tablist holds only tabs (aria-required-children). The per-tab close button is a mouse and touch affordance that
// is hidden from assistive technology: a tab closes from the keyboard with Delete (aria-keyshortcuts on the tab).
import { useCallback, useEffect, useRef, useState, type DragEvent, type KeyboardEvent, type MouseEvent } from "react";
import { useCatalog } from "@/data";
import { cn } from "@/lib/utils";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from "@/ui/dropdown-menu";
import { Icon } from "@/ui/Icon";
import { IconTip } from "@/ui/tooltip";
import { isMac } from "../shortcuts";
import { parseTab, type Pane, type TabKey } from "./paneTree";
import { TAB_DRAG_TYPE, endTabDrag, getTabDrag, startTabDrag } from "./tabDrag";
import type { TabActions } from "./useTabActions";

const MOD = () => (isMac() ? "Meta" : "Control");

const iconButton =
  "inline-flex size-11 shrink-0 items-center justify-center rounded-pill text-muted-text hover:bg-selection hover:text-content focus-visible:outline-2 focus-visible:outline-brand pointer-fine:size-8";

/** The title and icon of a tab, from the catalog; a tab the catalog does not list yet (or any more) keeps a plain name. */
function useTabTitle() {
  const catalog = useCatalog().data;
  return (key: TabKey): { title: string; known: boolean; mono: boolean } => {
    const tab = parseTab(key);
    if (tab?.kind === "mono") {
      const mono = catalog?.monos.find((candidate) => candidate.id === tab.id);
      return { title: mono?.name ?? "Mono", known: !!mono, mono: true };
    }
    const session = catalog?.sessions.find((candidate) => candidate.id === tab?.id);
    return { title: session?.title || "Session", known: !!session, mono: false };
  };
}

export interface TabStripProps {
  desktopId: string;
  pane: Pane;
  /** 1 based, for the accessible names. */
  number: number;
  paneCount: number;
  focused: boolean;
  /** The tab shown as selected: the pane's active tab, unless the pane shows a page that is not a tab. */
  selected: TabKey | null;
  hiddenPanes: number;
  /** Whether a split of the focused pane in each direction would fit. */
  canSplit: { row: boolean; column: boolean };
  actions: TabActions;
}

export function TabStrip({ desktopId, pane, number, paneCount, focused, selected, hiddenPanes, canSplit, actions }: TabStripProps) {
  const titleOf = useTabTitle();
  const listRef = useRef<HTMLDivElement>(null);
  const [dropAt, setDropAt] = useState<number | null>(null);
  // Which sides have tabs scrolled out of sight (the strip overflows), for the edge fades and the "All tabs" menu.
  const [overflow, setOverflow] = useState({ left: false, right: false });
  const selectedRef = useRef<TabKey | null>(selected);
  const measureOverflow = useCallback(() => {
    const list = listRef.current;
    if (!list) return;
    const left = list.scrollLeft > 1;
    const right = list.scrollLeft + list.clientWidth < list.scrollWidth - 1;
    setOverflow((previous) => (previous.left === left && previous.right === right ? previous : { left, right }));
  }, []);
  /** Scrolls the selected tab into view (the strip may have been resized, or moved in the DOM, which resets its scroll). */
  const revealSelected = useCallback(() => {
    const key = selectedRef.current;
    if (!key) return;
    for (const element of listRef.current?.querySelectorAll<HTMLElement>('[role="tab"]') ?? []) {
      if (element.dataset.key === key) element.scrollIntoView?.({ block: "nearest", inline: "nearest" });
    }
  }, []);
  useEffect(() => {
    measureOverflow();
    const list = listRef.current;
    // The strip itself and every tab: a resize, a split that moved the strip, or a title that changes a tab's width.
    const observer =
      typeof ResizeObserver === "function" && list
        ? new ResizeObserver(() => {
            revealSelected();
            measureOverflow();
          })
        : null;
    if (list) {
      observer?.observe(list);
      for (const child of list.children) observer?.observe(child);
    }
    window.addEventListener("resize", measureOverflow);
    return () => {
      observer?.disconnect();
      window.removeEventListener("resize", measureOverflow);
    };
  }, [measureOverflow, revealSelected, pane.tabs.length]);

  // Keep the selected tab in view when the strip overflows.
  useEffect(() => {
    selectedRef.current = selected;
    revealSelected();
    measureOverflow();
  }, [selected, pane.tabs.length, revealSelected, measureOverflow]);

  const tabButtons = () => [...(listRef.current?.querySelectorAll<HTMLButtonElement>('[role="tab"]') ?? [])];

  const onTabKeyDown = (event: KeyboardEvent<HTMLButtonElement>, key: TabKey, index: number) => {
    const buttons = tabButtons();
    if (event.altKey && (event.key === "ArrowLeft" || event.key === "ArrowRight")) {
      // Alt+Arrow moves the tab itself, the keyboard way to reorder.
      event.preventDefault();
      const to = index + (event.key === "ArrowRight" ? 1 : -1);
      if (to >= 0 && to < pane.tabs.length) {
        actions.drop(key, { paneId: pane.id, index: to });
        requestAnimationFrame(() => tabButtons()[to]?.focus());
      }
      return;
    }
    let target: number | null = null;
    if (event.key === "ArrowRight") target = (index + 1) % buttons.length;
    else if (event.key === "ArrowLeft") target = (index - 1 + buttons.length) % buttons.length;
    else if (event.key === "Home") target = 0;
    else if (event.key === "End") target = buttons.length - 1;
    else if (event.key === "Delete" || event.key === "Backspace") {
      // Backspace too: the Mac Delete key reports it.
      event.preventDefault();
      actions.close(pane.id, key);
      // The tab that takes its place (the right neighbour, else the left one) keeps the keyboard focus.
      requestAnimationFrame(() => {
        const rest = tabButtons();
        rest[Math.min(index, rest.length - 1)]?.focus();
      });
      return;
    }
    if (target === null) return;
    event.preventDefault();
    buttons[target]?.focus();
  };

  const insertionIndex = (event: DragEvent<HTMLElement>): number => {
    const wrapper = (event.target as Element).closest<HTMLElement>("[data-tab-index]");
    if (!wrapper) return pane.tabs.length;
    const index = Number(wrapper.dataset.tabIndex);
    const rect = wrapper.getBoundingClientRect();
    return event.clientX < rect.left + rect.width / 2 ? index : index + 1;
  };

  const onDragOver = (event: DragEvent<HTMLElement>) => {
    const drag = getTabDrag();
    if (!drag || drag.desktopId !== desktopId) return;
    event.preventDefault();
    event.dataTransfer.dropEffect = "move";
    setDropAt(insertionIndex(event));
  };

  const onDrop = (event: DragEvent<HTMLElement>) => {
    const drag = getTabDrag();
    setDropAt(null);
    if (!drag || drag.desktopId !== desktopId) return;
    event.preventDefault();
    const index = insertionIndex(event);
    endTabDrag();
    // Dropping inside the same pane counts the gap left by the dragged tab itself.
    const from = drag.paneId === pane.id ? pane.tabs.indexOf(drag.key) : -1;
    actions.drop(drag.key, { paneId: pane.id, index: from >= 0 && from < index ? index - 1 : index });
  };

  const onAuxClick = (event: MouseEvent, key: TabKey) => {
    if (event.button !== 1) return;
    event.preventDefault();
    actions.close(pane.id, key);
  };

  const hasSelected = selected !== null && pane.tabs.includes(selected);
  const paneName = paneCount > 1 ? `Pane ${number}` : "Open chats";

  return (
    <div data-testid="tab-strip" className="flex shrink-0 items-stretch border-b border-stroke bg-drawer">
      <div className="relative flex min-w-0 flex-1">
      <div
        ref={listRef}
        role="tablist"
        aria-label={paneName}
        aria-orientation="horizontal"
        onDragOver={onDragOver}
        onDragLeave={() => setDropAt(null)}
        onDrop={onDrop}
        onScroll={measureOverflow}
        onWheel={(event) => {
          // A mouse wheel scrolls the strip sideways.
          if (event.deltaY !== 0 && event.deltaX === 0 && listRef.current) listRef.current.scrollLeft += event.deltaY;
        }}
        className="flex min-w-0 flex-1 overflow-x-auto overscroll-x-contain [scrollbar-width:none] [&::-webkit-scrollbar]:hidden"
      >
        {pane.tabs.map((key, index) => {
          const info = titleOf(key);
          const isSelected = key === selected;
          const tabbable = isSelected || (index === 0 && !hasSelected);
          return (
            <div
              key={key}
              role="presentation"
              data-tab-index={index}
              className={cn(
                "group relative flex h-11 min-w-[7.5rem] max-w-[14rem] shrink-0 items-stretch border-r border-stroke pointer-fine:h-9",
                isSelected ? "bg-selection text-content" : "text-muted-text hover:bg-fill hover:text-content",
                dropAt === index && "before:absolute before:inset-y-1 before:left-0 before:z-10 before:w-0.5 before:bg-brand",
                dropAt === index + 1 && index === pane.tabs.length - 1 && "after:absolute after:inset-y-1 after:right-0 after:z-10 after:w-0.5 after:bg-brand",
              )}
            >
              <button
                type="button"
                role="tab"
                id={`tab-${desktopId}-${pane.id}-${key}`}
                data-key={key}
                aria-selected={isSelected}
                aria-keyshortcuts="Delete Backspace"
                tabIndex={tabbable ? 0 : -1}
                draggable
                onDragStart={(event) => {
                  event.dataTransfer.effectAllowed = "move";
                  event.dataTransfer.setData(TAB_DRAG_TYPE, key);
                  startTabDrag({ desktopId, paneId: pane.id, key });
                }}
                onDragEnd={() => {
                  setDropAt(null);
                  endTabDrag();
                }}
                onClick={() => actions.activate(pane.id, key)}
                onMouseDown={(event) => {
                  // The browser's middle-button autoscroll would swallow the close click.
                  if (event.button === 1) event.preventDefault();
                }}
                onAuxClick={(event) => onAuxClick(event, key)}
                onKeyDown={(event) => onTabKeyDown(event, key, index)}
                className={cn(
                  "flex min-w-0 flex-1 items-center gap-2 pr-1 pl-3 text-left text-label focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-brand",
                  !info.known && "italic",
                )}
              >
                <Icon name={info.mono ? "bot" : "chat"} size={16} className="shrink-0" />
                <span className="min-w-0 flex-1 truncate">{info.title}</span>
              </button>
              <button
                type="button"
                tabIndex={-1}
                aria-hidden="true"
                data-testid="tab-close"
                data-tab-key={key}
                title={`Close ${info.title}`}
                onClick={() => actions.close(pane.id, key)}
                className="inline-flex w-9 shrink-0 items-center justify-center text-muted-text hover:text-content pointer-coarse:w-11 pointer-fine:w-7 pointer-fine:opacity-0 pointer-fine:group-focus-within:opacity-100 pointer-fine:group-hover:opacity-100 focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-brand"
              >
                <Icon name="close" size={14} />
              </button>
              {isSelected ? <span aria-hidden="true" className={cn("absolute inset-x-0 bottom-0 h-0.5", focused ? "bg-brand" : "bg-stroke")} /> : null}
            </div>
          );
        })}
      </div>
      {overflow.left ? <span aria-hidden="true" data-testid="tabs-fade-left" className="pointer-events-none absolute inset-y-0 left-0 w-6 bg-gradient-to-r from-drawer to-transparent" /> : null}
      {overflow.right ? <span aria-hidden="true" data-testid="tabs-fade-right" className="pointer-events-none absolute inset-y-0 right-0 w-6 bg-gradient-to-l from-drawer to-transparent" /> : null}
      </div>
      <div className="flex shrink-0 items-center gap-0.5 px-1">
        {pane.tabs.length > 0 ? (
          <DropdownMenu>
            <IconTip label="All tabs">
              <DropdownMenuTrigger asChild>
                <button type="button" aria-label="All tabs" className={iconButton}>
                  <Icon name="chevronDown" size={18} />
                </button>
              </DropdownMenuTrigger>
            </IconTip>
            <DropdownMenuContent align="end" className="max-h-80 max-w-72 overflow-y-auto">
              {pane.tabs.map((key) => {
                const info = titleOf(key);
                return (
                  <DropdownMenuItem key={key} aria-current={key === selected ? "true" : undefined} onSelect={() => actions.activate(pane.id, key)}>
                    <Icon name={info.mono ? "bot" : "chat"} size={16} className="shrink-0" />
                    <span className={cn("min-w-0 flex-1 truncate", key === selected && "font-semibold", !info.known && "italic")}>{info.title}</span>
                    {key === selected ? <Icon name="check" size={16} className="shrink-0 text-brand-text" /> : null}
                  </DropdownMenuItem>
                );
              })}
              {/* Closing by touch and with a screen reader: the per-tab close buttons are hidden from assistive tech. */}
              <DropdownMenuSeparator />
              {pane.tabs.map((key) => (
                <DropdownMenuItem key={`close-${key}`} onSelect={() => actions.close(pane.id, key)}>
                  <Icon name="close" size={16} className="shrink-0" />
                  <span className="min-w-0 flex-1 truncate">{`Close ${titleOf(key).title}`}</span>
                </DropdownMenuItem>
              ))}
            </DropdownMenuContent>
          </DropdownMenu>
        ) : null}
        {hiddenPanes > 0 ? (
          <span data-testid="hidden-panes" title="Other panes are hidden: the window is too narrow" className="px-2 text-meta text-muted-text">
            +{hiddenPanes} hidden
          </span>
        ) : null}
        <IconTip label={`Split right (${isMac() ? "⌘D" : "Ctrl+D"})`}>
          <button type="button" aria-label="Split right" aria-keyshortcuts={`${MOD()}+D`} disabled={!canSplit.row} onClick={() => actions.split("row")} className={cn(iconButton, "disabled:opacity-40")}>
            <Icon name="splitRight" size={18} />
          </button>
        </IconTip>
        <IconTip label={`Split down (${isMac() ? "⇧⌘D" : "Ctrl+Shift+D"})`}>
          <button type="button" aria-label="Split down" aria-keyshortcuts={`${MOD()}+Shift+D`} disabled={!canSplit.column} onClick={() => actions.split("column")} className={cn(iconButton, "disabled:opacity-40")}>
            <Icon name="splitDown" size={18} />
          </button>
        </IconTip>
        {paneCount > 1 ? (
          <IconTip label="Close pane">
            <button type="button" aria-label="Close pane" onClick={() => actions.closePane(pane.id)} className={iconButton}>
              <Icon name="close" size={18} />
            </button>
          </IconTip>
        ) : null}
      </div>
    </div>
  );
}
