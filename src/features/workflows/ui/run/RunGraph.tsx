import "@xyflow/react/dist/base.css";
import "../board/workflowBoard.css";
import {
  Background,
  BackgroundVariant,
  MarkerType,
  ReactFlow,
  ReactFlowProvider,
  type Node,
  type NodeChange,
  type NodeProps,
} from "@xyflow/react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { NodeId, WorkflowRun } from "../../model/workflow";
import { GROUP_PAD, NODE_W, type FlowEdge, type FlowNode } from "../board/boardModel";
import { nodeTypes as boardNodeTypes, RUN_DOT } from "../board/boardNodes";
import { edgeTypes } from "../board/LoopEdge";
import { ZoomControls } from "../board/WorkflowBoard";
import {
  defaultRunView,
  groupViews,
  LANE_HEADER,
  nodeRunKey,
  runGraph,
  type GroupView,
  type RunEdgeState,
  type RunNodeStatus,
  type RunView,
} from "./runGraphModel";

const VIEW_KEY = "monocode.workflowRunView.";

const isView = (value: unknown): value is RunView => value === "lanes" || value === "single";

/** Whether the run has a For each, the only thing the two views draw differently. */
export const hasForEach = (run: WorkflowRun): boolean => run.definition.nodes.some((n) => n.type === "forEach");

/** The view of a run: the one chosen last for this workflow, else the one that fits its size. */
export function useRunView(
  workflowId: string,
  run: WorkflowRun | undefined,
): { view: RunView; toggle: () => void } {
  const [chosen, setChosen] = useState<RunView | undefined>(() => {
    try {
      const stored = localStorage.getItem(VIEW_KEY + workflowId);
      return isView(stored) ? stored : undefined;
    } catch {
      return undefined;
    }
  });
  const view = chosen ?? (run ? defaultRunView(run) : "lanes");
  const toggle = () => {
    const next: RunView = view === "lanes" ? "single" : "lanes";
    setChosen(next);
    try {
      localStorage.setItem(VIEW_KEY + workflowId, next);
    } catch {
      // The choice lasts until the page closes; nothing else breaks.
    }
  };
  return { view, toggle };
}

/** The node to show details for when none was picked: the one running, else the last to finish. */
export function defaultSelection(run: WorkflowRun): string | undefined {
  const nodes = run.nodes ?? [];
  const isGroup = (id: NodeId) => run.definition.nodes.find((n) => n.id === id)?.type === "forEach";
  const live = (n: (typeof nodes)[number]) => n.status === "running" || n.status === "waitingApproval";
  const running = nodes.find((n) => live(n) && !isGroup(n.nodeId)) ?? nodes.find(live);
  if (running) return nodeRunKey(running.nodeId, running.branch);
  let last: (typeof nodes)[number] | undefined;
  for (const n of nodes) {
    if (n.endedAt !== undefined && (!last || n.endedAt >= (last.endedAt ?? 0))) last = n;
  }
  const found = last ?? nodes[nodes.length - 1];
  return found ? nodeRunKey(found.nodeId, found.branch) : undefined;
}

// --- the strips above the lanes ----------------------------------------------------------------

type BarData =
  | { kind: "lane"; title: string; status: RunNodeStatus }
  | {
      kind: "tabs";
      items: GroupView["items"];
      selected: number;
      onPick: (index: number) => void;
    }
  | {
      kind: "rounds";
      rounds: number[];
      selected: number;
      onPick: (round: number) => void;
    };
type BarNode = Node<BarData, "runBar">;
type RunFlowNode = FlowNode | BarNode;

function Dot({ status }: { status: RunNodeStatus }) {
  return <span aria-hidden className={`size-1.5 shrink-0 rounded-full ${RUN_DOT[status]}`} />;
}

function Tabs({
  label,
  items,
  selected,
  onPick,
  attribute,
}: {
  label: string;
  items: { key: number; title: string; status?: RunNodeStatus }[];
  selected: number;
  onPick: (key: number) => void;
  attribute: string;
}) {
  const strip = useRef<HTMLDivElement>(null);
  const current = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    // Scroll the strip itself: scrollIntoView would also move the canvas around it.
    const box = strip.current;
    const tab = current.current;
    if (!box || !tab) return;
    if (tab.offsetLeft < box.scrollLeft) box.scrollLeft = Math.max(0, tab.offsetLeft - TAB_MARGIN);
    else if (tab.offsetLeft + tab.offsetWidth > box.scrollLeft + box.clientWidth)
      box.scrollLeft = tab.offsetLeft + tab.offsetWidth - box.clientWidth + TAB_MARGIN;
  }, [selected]);
  return (
    <div
      ref={strip}
      role="tablist"
      aria-label={label}
      {...{ [attribute]: "" }}
      // The strip scrolls sideways under the finger or wheel instead of panning the canvas.
      className="run-bar run-tabs nodrag nopan nowheel"
      style={{ overflowX: "auto", position: "relative" }}
    >
      {items.map((item) => (
        <button
          key={item.key}
          ref={item.key === selected ? current : undefined}
          type="button"
          role="tab"
          aria-selected={item.key === selected}
          data-run-tab=""
          title={item.title}
          className="run-tab"
          onClick={() => onPick(item.key)}
        >
          {item.status ? <Dot status={item.status} /> : null}
          <span>{item.title}</span>
        </button>
      ))}
    </div>
  );
}

function RunBar({ data }: NodeProps<BarNode>) {
  if (data.kind === "lane") {
    return (
      <div data-run-lane-title="" className="run-lane-title h-full">
        <Dot status={data.status} />
        <span title={data.title}>{data.title}</span>
      </div>
    );
  }
  if (data.kind === "tabs") {
    return (
      <Tabs
        label="Items"
        attribute="data-run-tabs"
        items={data.items.map((i) => ({ key: i.index, title: i.title, status: i.status }))}
        selected={data.selected}
        onPick={data.onPick}
      />
    );
  }
  return (
    <div className="flex h-full justify-end">
      <Tabs
        label="Rounds"
        attribute="data-run-rounds"
        items={data.rounds.map((r) => ({ key: r, title: `Round ${r}` }))}
        selected={data.selected}
        onPick={data.onPick}
      />
    </div>
  );
}

const nodeTypes = { ...boardNodeTypes, runBar: RunBar };

// --- the graph ---------------------------------------------------------------------------------

const ARROW = { type: MarkerType.ArrowClosed, width: 12, height: 12 } as const;
const EDGE_COLOR: Record<RunEdgeState, string> = {
  // The same colours as the lines, so an arrowhead is as strong as the line into it.
  taken: "color-mix(in srgb, var(--wf-done) 75%, transparent)",
  active: "var(--mono-color)",
  idle: "color-mix(in srgb, var(--color-content) 20%, transparent)",
};
const TAB_MARGIN = 4;
const BAR_H = LANE_HEADER - 4;
const ROUNDS_H = 22;

type Pick = Record<NodeId, { round?: number; item?: number }>;

/** The strips that sit with each For each: lane titles or item tabs, and the round selector. */
function barsOf(
  nodes: FlowNode[],
  groups: GroupView[],
  view: RunView,
  pick: (group: NodeId, change: { round?: number; item?: number }) => void,
): BarNode[] {
  const bars: BarNode[] = [];
  for (const group of nodes) {
    if (group.type !== "group") continue;
    const gv = groups.find((g) => g.groupId === group.id);
    if (!gv) continue;
    const width = group.width ?? NODE_W;
    const kids = nodes.filter((n) => n.parentId === group.id);
    if (gv.rounds.length > 1) {
      // Right of the column the edge into the group runs down, so that edge stays visible.
      const w = Math.min(width - (GROUP_PAD + NODE_W / 2 + 12), gv.rounds.length * 64 + 8);
      bars.push({
        id: `bar-rounds:${group.id}`,
        type: "runBar",
        position: { x: group.position.x + width - w, y: group.position.y - ROUNDS_H - 4 },
        width: w,
        height: ROUNDS_H,
        zIndex: 10,
        draggable: false,
        selectable: false,
        focusable: false,
        data: {
          kind: "rounds",
          rounds: gv.rounds,
          selected: gv.round,
          onPick: (round) => pick(group.id, { round, item: undefined }),
        },
      });
    }
    if (kids.length === 0 || gv.items.length === 0) continue;
    const top = Math.min(...kids.map((k) => k.position.y));
    const y = group.position.y + top - LANE_HEADER;
    if (view === "single") {
      bars.push({
        id: `bar-tabs:${group.id}`,
        type: "runBar",
        position: { x: group.position.x + GROUP_PAD, y },
        width: width - GROUP_PAD * 2,
        height: BAR_H,
        zIndex: 10,
        draggable: false,
        selectable: false,
        focusable: false,
        data: {
          kind: "tabs",
          items: gv.items,
          selected: gv.item,
          onPick: (item) => pick(group.id, { item }),
        },
      });
      continue;
    }
    // One title above each lane, as wide as the lane's cards.
    const lanes = new Map<string, { x: number; right: number }>();
    for (const kid of kids) {
      const branch = kid.id.slice(kid.id.indexOf("@") + 1);
      const lane = lanes.get(branch) ?? { x: Infinity, right: -Infinity };
      lane.x = Math.min(lane.x, kid.position.x);
      lane.right = Math.max(lane.right, kid.position.x + (kid.width ?? NODE_W));
      lanes.set(branch, lane);
    }
    [...lanes.values()]
      .sort((a, b) => a.x - b.x)
      .forEach((lane, k) => {
        const item = gv.items[k];
        if (!item) return;
        bars.push({
          id: `bar-lane:${group.id}:${k}`,
          type: "runBar",
          position: { x: group.position.x + lane.x, y },
          width: lane.right - lane.x,
          height: BAR_H,
          zIndex: 10,
          draggable: false,
          selectable: false,
          focusable: false,
          data: { kind: "lane", title: item.title, status: item.status },
        });
      });
  }
  return bars;
}

/**
 * A run as a read-only graph of its definition snapshot, drawn with the board's own cards.
 * The two views are the same graph: every item of a For each side by side, or one at a time.
 */
export function RunGraph({
  run,
  view,
  selectedKey,
  onSelect,
}: {
  run: WorkflowRun;
  view: RunView;
  selectedKey?: string;
  onSelect: (key: string) => void;
}) {
  const [pick, setPick] = useState<Pick>({});
  // React Flow measures every card itself. The nodes are rebuilt from the run on every refresh, so
  // what it measured is kept here and handed back, or each card would blink while it is measured again.
  const measured = useRef(new Map<string, { width: number; height: number }>());
  // One clock for the whole drawing, so "Running 3m" and the groups agree.
  const now = useMemo(() => Date.now(), [run]);
  const groups = useMemo(() => groupViews(run, pick, now), [run, pick, now]);
  const graph = useMemo(() => runGraph(run, view, groups, now), [run, view, groups, now]);
  const choose = useCallback(
    (group: NodeId, change: { round?: number; item?: number }) =>
      setPick((p) => ({ ...p, [group]: { ...p[group], ...change } })),
    [],
  );
  const nodes = useMemo<RunFlowNode[]>(
    () => [
      ...graph.nodes.map((n) => ({
        ...n,
        selected: n.data.run?.nodeRunKey === selectedKey,
        // In a single lane the cards stack up behind it, for the items that are not shown.
        className:
          view === "single" && n.parentId && (groups.find((g) => g.groupId === n.parentId)?.items.length ?? 0) > 1
            ? "run-stacked"
            : undefined,
        draggable: false,
        connectable: false,
        measured: measured.current.get(n.id),
      })),
      ...barsOf(graph.nodes, groups, view, choose).map((n) => ({ ...n, measured: measured.current.get(n.id) })),
    ],
    [graph.nodes, groups, view, choose, selectedKey],
  );
  const byId = useMemo(() => new Map(graph.nodes.map((n) => [n.id, n.data.run?.nodeRunKey])), [graph.nodes]);
  const onNodesChange = useCallback(
    (changes: NodeChange<RunFlowNode>[]) => {
      for (const change of changes) {
        if (change.type === "dimensions" && change.dimensions) measured.current.set(change.id, change.dimensions);
        // Picking with the keyboard (Enter or Space on a focused card) arrives as a selection.
        else if (change.type === "select" && change.selected) {
          const key = byId.get(change.id);
          if (key) onSelect(key);
        }
      }
    },
    [byId, onSelect],
  );
  const edges = useMemo<FlowEdge[]>(
    () =>
      graph.edges.map((e) => ({
        ...e,
        className: `run-${e.data.run}`,
        selectable: false,
        focusable: false,
        markerEnd: { ...ARROW, color: e.type === "loop" ? "var(--mono-color)" : EDGE_COLOR[e.data.run] },
      })),
    [graph.edges],
  );
  return (
    <div className="workflow-board relative min-h-[320px] flex-1" data-run-graph="">
      {/* A new set of lanes is a new picture: start from the whole of it again. */}
      <ReactFlowProvider key={`${view}:${graph.nodes.length}`}>
        <ReactFlow<RunFlowNode, FlowEdge>
          nodes={nodes}
          edges={edges}
          nodeTypes={nodeTypes}
          edgeTypes={edgeTypes}
          onNodesChange={onNodesChange}
          onNodeClick={(_, node) => {
            if (node.type !== "runBar" && node.data && "run" in node.data && node.data.run) {
              onSelect(node.data.run.nodeRunKey);
            }
          }}
          nodesDraggable={false}
          nodesConnectable={false}
          elementsSelectable
          deleteKeyCode={null}
          fitView
          fitViewOptions={{ padding: 0.12, maxZoom: 1 }}
          minZoom={0.3}
          maxZoom={1.5}
          panOnScroll
          zoomOnScroll={false}
          proOptions={{ hideAttribution: true }}
        >
          <Background variant={BackgroundVariant.Dots} gap={18} size={1} />
        </ReactFlow>
        <ZoomControls />
      </ReactFlowProvider>
    </div>
  );
}
