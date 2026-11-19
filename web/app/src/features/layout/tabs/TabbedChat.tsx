// The chat area from 640px: session tabs and split panes (design-system.md section 11). Rendered by ChatPane.
//
// The URL stays the source of truth of the focused pane: a session or Mono URL opens (or focuses) its tab, and
// choosing a tab, focusing a pane or closing a tab navigates. A URL that is not a tab (New session, Files, Changes,
// Git, a Mono profile) shows its page, the route `children`, in the focused pane in place of that pane's active tab.
// Only the active tab of each pane is mounted, so a long list of tabs holds no live transcripts.
//
// A pane keeps its React state (the composer draft, open menus) when the layout around it changes: a split, a closed
// neighbour or a narrow window that hides the others. Each pane view renders through a portal into a host element of
// its own that lives as long as the pane is on screen, and the layout tree only decides which slot the host sits in.
// Moving a host in the DOM resets the browser's scroll offsets and focus, so PaneHosts.moveInto restores both. A pane
// that is hidden while the window is too narrow is unmounted and comes back with its tabs but fresh DOM state.
import { useRouterState } from "@tanstack/react-router";
import { Fragment, useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { MonoScreen } from "@/features/monos/MonoScreen";
import "@/features/sessions/register";
import { SessionScreen } from "@/features/sessions/SessionScreen";
import { PaneHosts } from "./paneHosts";
import { PaneContext } from "./PaneContext";
import { PaneDivider } from "./PaneDivider";
import { PaneDropZones } from "./PaneDropZones";
import { canSplit, fits, openTab, paneOrder, parseTab, type Pane, type PaneNode, type TabKey, type Workspace } from "./paneTree";
import { TabStrip } from "./TabStrip";
import { tabOfPath } from "./tabRoutes";
import { useTabShortcuts } from "./tabShortcuts";
import { getWorkspace, tabsKey, updateWorkspace, useWorkspace } from "./tabsStore";
import { useTabActions, type Size, type TabActions } from "./useTabActions";

function TabScreen({ desktopId, tabKey }: { desktopId: string; tabKey: TabKey }) {
  const tab = parseTab(tabKey);
  if (!tab) return null;
  return tab.kind === "session" ? <SessionScreen key={tab.id} desktopId={desktopId} sessionId={tab.id} /> : <MonoScreen key={tab.id} desktopId={desktopId} monoId={tab.id} />;
}

/** The size of `ref`, kept current; null while it cannot be measured (the first render, tests without layout). */
function useElementSize(): [React.RefObject<HTMLDivElement | null>, Size | null] {
  const ref = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState<Size | null>(null);
  useLayoutEffect(() => {
    const element = ref.current;
    if (!element) return;
    const measure = () => {
      const rect = element.getBoundingClientRect();
      const next = rect.width > 0 && rect.height > 0 ? { width: Math.round(rect.width), height: Math.round(rect.height) } : null;
      setSize((previous) => (previous?.width === next?.width && previous?.height === next?.height ? previous : next));
    };
    measure();
    const observer = typeof ResizeObserver === "function" ? new ResizeObserver(measure) : null;
    observer?.observe(element);
    window.addEventListener("resize", measure);
    return () => {
      observer?.disconnect();
      window.removeEventListener("resize", measure);
    };
  }, []);
  return [ref, size];
}

interface TreeContext {
  desktopId: string;
  ws: Workspace;
  urlKey: TabKey | null;
  actions: TabActions;
  hiddenPanes: number;
  canSplit: { row: boolean; column: boolean };
  children?: ReactNode;
}

function PaneView({ pane, context }: { pane: Pane; context: TreeContext }) {
  const { desktopId, ws, urlKey, actions, hiddenPanes, canSplit: splittable, children } = context;
  const focused = ws.focused === pane.id;
  const order = paneOrder(ws.root);
  const multi = order.length > 1;
  // The focused pane shows the page of the URL when that page is not a tab.
  const showPage = focused && urlKey === null;
  const selected = focused && urlKey === null ? null : pane.active;
  const showStrip = pane.tabs.length > 0 || multi;
  const claimFocus = () => {
    // Read at call time: a pointer event can arrive before the render that follows a focus change.
    if (getWorkspace(desktopId).focused !== pane.id) actions.focus(pane.id);
  };

  return (
    <PaneContext.Provider value={{ focused, multi }}>
      <div
        data-testid={`pane-${pane.id}`}
        data-focused={focused}
        role={multi ? "group" : undefined}
        aria-label={multi ? `Pane ${order.indexOf(pane.id) + 1}` : undefined}
        onPointerDownCapture={multi ? claimFocus : undefined}
        onFocusCapture={multi ? claimFocus : undefined}
        className="relative flex min-h-0 min-w-0 flex-1 flex-col"
      >
        {showStrip ? <TabStrip desktopId={desktopId} pane={pane} number={order.indexOf(pane.id) + 1} paneCount={order.length} focused={focused} selected={selected} hiddenPanes={focused ? hiddenPanes : 0} canSplit={splittable} actions={actions} /> : null}
        <div className="relative flex min-h-0 min-w-0 flex-1 flex-col">
          <div data-testid="pane-body" className="@container relative flex min-h-0 min-w-0 flex-1 flex-col overflow-y-auto">
            {showPage ? children : pane.active ? <TabScreen desktopId={desktopId} tabKey={pane.active} /> : multi ? <EmptyPane /> : null}
          </div>
          {/* Over the visible pane body (not inside its scroller), below the strip so tabs can still be dropped on it. */}
          <PaneDropZones desktopId={desktopId} paneId={pane.id} actions={actions} />
        </div>
      </div>
    </PaneContext.Provider>
  );
}

function EmptyPane() {
  return (
    <div data-testid="empty-pane" className="flex flex-1 items-center justify-center p-6 text-center text-label text-muted-text">
      Open a chat from the list, or drag a tab here.
    </div>
  );
}

/** The place of one pane in the layout: it adopts the pane's host element, which carries the pane view. */
function PaneSlot({ host, hosts }: { host: HTMLElement; hosts: PaneHosts }) {
  const ref = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    if (ref.current) hosts.moveInto(host, ref.current);
  });
  // A slot that goes away takes its DOM out first: note the scroll offsets and the focus before that happens.
  useLayoutEffect(() => () => hosts.snapshot(host), [hosts, host]);
  return <div ref={ref} className="flex min-h-0 min-w-0 flex-1" />;
}

function Tree({ node, path, context, hosts }: { node: PaneNode; path: number[]; context: TreeContext; hosts: PaneHosts }) {
  if (node.type === "leaf") return <PaneSlot host={hosts.of(node.id)} hosts={hosts} />;
  const row = node.dir === "row";
  return (
    <div data-testid="pane-split" data-dir={node.dir} className={`flex min-h-0 min-w-0 flex-1 ${row ? "flex-row" : "flex-col"}`}>
      {node.children.map((child, index) => (
        <Fragment key={child.type === "leaf" ? child.id : `split-${index}`}>
          {index > 0 ? (
            <PaneDivider
              dir={node.dir}
              sizes={node.sizes}
              index={index - 1}
              label={row ? "Resize panes" : "Resize panes vertically"}
              onResize={(sizes, commit) => context.actions.resize(path, sizes, commit)}
            />
          ) : null}
          <div style={{ flex: `${node.sizes[index] ?? 1} 1 0%` }} className="flex min-h-0 min-w-0">
            <Tree node={child} path={[...path, index]} context={context} hosts={hosts} />
          </div>
        </Fragment>
      ))}
    </div>
  );
}

export function TabbedChat({ desktopId, children }: { desktopId: string; children?: ReactNode }) {
  const ws = useWorkspace(desktopId);
  const urlKey = useRouterState({ select: (state) => tabOfPath(state.location.pathname) });
  const [ref, size] = useElementSize();
  // The element a pane view is rendered into: one per pane, so moving the pane in the layout never remounts it.
  const [hosts] = useState(() => new PaneHosts());
  const actions = useTabActions(desktopId, size);
  useTabShortcuts(actions);

  // A session or Mono URL opens its tab (or focuses the pane that has it) before the first paint.
  useLayoutEffect(() => {
    if (urlKey) updateWorkspace(desktopId, (current) => openTab(current, urlKey));
  }, [desktopId, urlKey]);

  // Another browser tab replaced the workspace: this page's URL tab must still be in it and shown in its pane. openTab
  // returns the same workspace when that already holds, so nothing is written then (no ping-pong between browser tabs).
  useLayoutEffect(() => {
    const onStorage = (event: StorageEvent) => {
      if (event.key !== null && event.key !== tabsKey(desktopId)) return;
      if (urlKey) updateWorkspace(desktopId, (current) => openTab(current, urlKey));
    };
    window.addEventListener("storage", onStorage);
    return () => window.removeEventListener("storage", onStorage);
  }, [desktopId, urlKey]);

  // Too narrow for the panes: only the focused pane shows (the others keep their tabs).
  const all = fits(ws.root, size);
  const root: PaneNode = all ? ws.root : { type: "leaf", id: ws.focused };
  const hiddenPanes = all ? 0 : paneOrder(ws.root).length - 1;

  const visible = paneOrder(root);
  // The host of a pane that left the workspace, or is only hidden (too narrow), is dropped and made again on return.
  const visibleIds = visible.join(",");
  useEffect(() => hosts.retain(visibleIds.split(",")), [hosts, visibleIds]);
  const context: TreeContext = { desktopId, ws, urlKey, actions, hiddenPanes, canSplit: { row: canSplit(ws, "row", size), column: canSplit(ws, "column", size) }, children };

  return (
    <div ref={ref} data-testid="tabbed-chat" data-panes={visible.length} className="relative flex min-h-0 min-w-0 flex-1 overflow-hidden">
      <Tree node={root} path={[]} context={context} hosts={hosts} />
      {visible.map((id) => {
        const pane = ws.panes[id];
        return pane ? createPortal(<PaneView pane={pane} context={context} />, hosts.of(id), id) : null;
      })}
    </div>
  );
}
