import "@xyflow/react/dist/base.css";
import "./workflowBoard.css";
import {
  applyEdgeChanges,
  applyNodeChanges,
  Background,
  BackgroundVariant,
  ConnectionLineType,
  MarkerType,
  ReactFlow,
  ReactFlowProvider,
  useReactFlow,
  type Connection,
  type EdgeChange,
  type NodeChange,
} from "@xyflow/react";
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
  type CSSProperties,
  type ReactNode,
} from "react";
import { IconButton } from "../../../../app/shell/TitleBar";
import { isImeComposition } from "../../../../shared/lib/keyboard";
import {
  Maximize2,
  MoreHorizontal,
  Minus,
  Pause,
  Play,
  Plus,
  Settings,
  Share,
  Zap,
} from "../../../../shared/ui/icons";
import { Modal } from "../../../../shared/ui/Modal";
import { Popover } from "../../../../shared/ui/Popover";
import type { MonoProject } from "../../../monos/model/mono";
import { subscribeMonoFiles } from "../../../monos/model/monoFiles";
import { anchorSchedule, nextHabitRunAt } from "../../../monos/model/monoHabits";
import { PageHeader } from "../../../monos/ui/monoPanelParts";
import { getModelSnapshot, subscribeModels } from "../../../sessions/model/models";
import {
  triggerOf,
  WORKFLOWS_MAX,
  type LoopSpec,
  type NodeId,
  type NodeType,
  type Workflow,
  type WorkflowNode,
} from "../../model/workflow";
import type { WorkflowIssue } from "../../model/workflowParse";
import { workflowIssues } from "../../model/workflowModels";
import { loadWorkflows, updateWorkflows } from "../../model/workflowStore";
import { inputValueProblems } from "../../model/workflowYaml";
import {
  useLiveRunId,
  workflowActions,
  WorkflowNotice,
  type ReviewIntent,
} from "../MonoWorkflows";
import { exportWorkflowToFile } from "../workflowExportFile";
import {
  blankWorkflow,
  connectionKind,
  definitionKey,
  dropCrossingEdges,
  deletion,
  dropNodes,
  EMPTY_GROUP_H,
  fitGroups,
  fromFlow,
  GAP_Y,
  GROUP_PAD,
  groupable,
  has,
  issuesByNode,
  isGroup,
  makeEdge,
  NODE_H,
  NODE_W,
  autoPlace,
  newNode,
  pushBelow,
  settleDrop,
  toFlow,
  type FlowEdge,
  type FlowNode,
} from "./boardModel";
import { nodeTypes, nodeTitle } from "./boardNodes";
import { edgeTypes } from "./LoopEdge";
import { LoopForm, NodeInspector, type InspectorView, type MonoModelDefault } from "./NodeInspector";
import { NodePicker } from "./NodePicker";

type Board = { nodes: FlowNode[]; edges: FlowEdge[] };
type Meta = Pick<Workflow, "name" | "description" | "enabled" | "inputs" | "inputValues">;

// currentColor lets the stylesheet colour the arrowheads (see workflowBoard.css).
const ARROW = { type: MarkerType.ArrowClosed, width: 12, height: 12, color: "currentColor" } as const;
const LOOP_ARROW = { ...ARROW, color: "var(--mono-color)" } as const;
const DEFAULT_LOOP: LoopSpec = { maxRounds: 3, decide: "mono" };
const REVIEW_FIRST = "Review this workflow before enabling it.";

/** The stored definition is not the one this board last synced: someone else changed it. */
class ChangedElsewhere extends Error {
  constructor(readonly stored: Workflow) {
    super("This workflow changed elsewhere");
  }
}

function metaOf(w: Workflow): Meta {
  return {
    name: w.name,
    description: w.description,
    enabled: w.enabled,
    inputs: w.inputs,
    inputValues: w.inputValues,
  };
}

function boardOf(w: Workflow): Board {
  return toFlow(w);
}

/** Everything wrong with a workflow that would keep it from running. */
function problemsOf(w: Workflow): WorkflowIssue[] {
  const unset = new Set(
    w.nodes.flatMap((n) => ((n.type === "session" || n.type === "command") && !n.project ? [n.id] : [])),
  );
  return [
    ...workflowIssues(w).map((issue) =>
      issue.nodeId !== undefined &&
      unset.has(issue.nodeId) &&
      issue.message.startsWith("project must be exactly")
        ? { ...issue, message: "Choose a project" }
        : issue,
    ),
    ...inputValueProblems(w.inputs, w.inputValues).map((message) => ({ message })),
  ];
}

/** The stored copy of a workflow with the board's definition on it, managed fields kept. */
function merged(stored: Workflow | undefined, next: Workflow, now: number): Workflow {
  const {
    name,
    description,
    inputs,
    nodes,
    edges,
    enabled,
    id,
    createdAt,
    inputValues: values,
  } = next;
  const inputValues = Object.fromEntries(
    Object.entries(values).filter(([key]) => has(inputs, key)),
  );
  const { nextRunAt: _old, description: _desc, ...rest } = stored ?? { id, createdAt };
  const base = {
    ...rest,
    id,
    createdAt: stored?.createdAt ?? createdAt,
    name,
    ...(description?.trim() ? { description } : {}),
    inputs,
    inputValues,
    nodes: nodes.map((node) =>
      node.type === "schedule" && !node.schedule.intervalFrom
        ? { ...node, schedule: anchorSchedule(node.schedule, now) }
        : node,
    ),
    edges,
    enabled,
  } as Workflow;
  const trigger = triggerOf(base);
  if (!enabled || trigger?.type !== "schedule") return base;
  // A schedule that is unchanged keeps its place; a new, edited or resumed one starts afresh.
  const before = stored && triggerOf(stored);
  const same =
    stored?.enabled &&
    before?.type === "schedule" &&
    JSON.stringify(before.schedule) === JSON.stringify(trigger.schedule) &&
    stored.nextRunAt !== undefined;
  return {
    ...base,
    nextRunAt: same ? stored.nextRunAt : nextHabitRunAt(trigger.schedule, now),
  };
}

const ifHandle = (handle: string | null | undefined) =>
  handle === "true" || handle === "false" ? handle : undefined;

function ZoomControls() {
  const flow = useReactFlow();
  return (
    <div className="board-zoom absolute bottom-2 left-2 z-[5] flex flex-col rounded-lg border border-content/10 p-0.5">
      <IconButton label="Zoom in" onClick={() => void flow.zoomIn({ duration: 150 })}>
        <Plus className="size-3.5" strokeWidth={1.75} />
      </IconButton>
      <IconButton label="Zoom out" onClick={() => void flow.zoomOut({ duration: 150 })}>
        <Minus className="size-3.5" strokeWidth={1.75} />
      </IconButton>
      <IconButton
        label="Fit to screen"
        onClick={() => void flow.fitView({ padding: 0.12, maxZoom: 1, duration: 200 })}
      >
        <Maximize2 className="size-3.5" strokeWidth={1.75} />
      </IconButton>
    </div>
  );
}

function MenuItem({
  icon,
  label,
  hint,
  disabled,
  onClick,
}: {
  icon: ReactNode;
  label: string;
  hint?: string;
  disabled?: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      role="menuitem"
      disabled={disabled}
      title={hint}
      onMouseDown={(event) => event.preventDefault()}
      onClick={onClick}
      className="flex h-7 w-full items-center gap-2 rounded-lg px-2 text-left text-[12px] leading-none text-content enabled:hover:bg-content/5 disabled:opacity-40"
    >
      <span className="grid size-4 shrink-0 place-items-center text-content/60">{icon}</span>
      <span className="min-w-0 flex-1 truncate">{label}</span>
    </button>
  );
}

type Props = {
  monoId: string;
  /** The stored workflow to edit; without one the board starts a new workflow. */
  workflow?: Workflow;
  /** The Mono's color. */
  color: string;
  /** The Mono's projects, for the steps and inputs that name one. */
  projects: readonly MonoProject[];
  /** The Mono's own model, which a session step without one of its own starts with. */
  monoModel?: MonoModelDefault;
  onBack: () => void;
  /** Opens the review a stored workflow that needs one must pass before it can run or be enabled. */
  onReview?: (intent: ReviewIntent) => void;
};

/**
 * The board: a workflow drawn top to bottom, edited in place. Edits stay in this component
 * until Save; an edit made elsewhere (the Mono's CLI) shows up live while there are none.
 */
export function WorkflowBoard(props: Props) {
  return (
    <ReactFlowProvider>
      <BoardEditor {...props} />
    </ReactFlowProvider>
  );
}

function BoardEditor({ monoId, workflow, color, projects, monoModel, onBack, onReview = () => {} }: Props) {
  const flow = useReactFlow();
  const [base, setBase] = useState<Workflow>(() => workflow ?? blankWorkflow());
  const [board, setBoard] = useState<Board>(() => boardOf(base));
  const [meta, setMeta] = useState<Meta>(() => metaOf(base));
  const [isNew, setIsNew] = useState(!workflow);
  const [baseline, setBaseline] = useState(() => definitionKey(base));
  const [view, setView] = useState<InspectorView | null>(null);
  const [picker, setPicker] = useState(false);
  const [menu, setMenu] = useState(false);
  const [loopAsk, setLoopAsk] = useState<{
    from: NodeId;
    to: NodeId;
    x: number;
    y: number;
    loop: LoopSpec;
  } | null>(null);
  const [leaving, setLeaving] = useState(false);
  const [notice, setNotice] = useState<string>();
  const [saving, setSaving] = useState(false);
  const [showIssues, setShowIssues] = useState(false);
  // An edit made elsewhere; `refused` when it stopped a Save, which then waits for a choice.
  const [elsewhere, setElsewhere] = useState<{ workflow: Workflow; refused: boolean } | null>(null);
  const addAnchor = useRef<HTMLSpanElement>(null);
  const moreAnchor = useRef<HTMLSpanElement>(null);
  const pointer = useRef({ x: 0, y: 0 });
  const liveRunId = useLiveRunId(base.id);

  const current: Workflow = useMemo(
    () => ({ ...fromFlow(base, board.nodes, board.edges), ...meta }),
    [base, board, meta],
  );
  // A model list that arrives later changes which models are available.
  const catalog = useSyncExternalStore(subscribeModels, getModelSnapshot);
  const issues = useMemo(() => problemsOf(current), [current, catalog]);
  const byNode = useMemo(() => issuesByNode(issues), [issues]);
  const changed = definitionKey(current) !== baseline;
  // Enabling goes through the review, never through Save.
  const unreviewed = !!base.needsReview && current.enabled;
  const blocked = current.enabled && (issues.length > 0 || unreviewed);

  // The latest of these, for handlers that outlive a render.
  const live = useRef({ current, changed, baseline, seen: baseline });
  live.current = { ...live.current, current, changed, baseline };

  const load = useCallback((w: Workflow) => {
    setBase(w);
    setBoard(boardOf(w));
    setMeta(metaOf(w));
    const key = definitionKey(w);
    setBaseline(key);
    live.current.seen = key;
    setElsewhere(null);
  }, []);

  // An edit made elsewhere: follow it while the board is clean, otherwise ask.
  useEffect(
    () =>
      subscribeMonoFiles(() => {
        void loadWorkflows(monoId)
          .then((list) => {
            const stored = list.find((w) => w.id === live.current.current.id);
            if (!stored) return;
            // A review (or a flag) changes no part of the definition, so the key does not see it.
            setBase((b) => {
              if (!!b.needsReview === !!stored.needsReview) return b;
              const { needsReview: _flag, ...rest } = b;
              return stored.needsReview ? { ...rest, needsReview: true } : rest;
            });
            const key = definitionKey(stored);
            if (key === live.current.seen) return;
            live.current.seen = key;
            if (key === live.current.baseline) return;
            if (live.current.changed) setElsewhere({ workflow: stored, refused: false });
            else load(stored);
          })
          .catch(() => {});
      }),
    [monoId, load],
  );

  useEffect(() => {
    if (view?.kind === "node" && !current.nodes.some((n) => n.id === view.id)) setView(null);
  }, [view, current.nodes]);

  // --- drawing ---------------------------------------------------------------------------------

  const loopEnds = useMemo(() => {
    const from = new Set<NodeId>();
    const to = new Set<NodeId>();
    for (const e of current.edges) {
      if (!e.loop) continue;
      from.add(e.from);
      to.add(e.to);
    }
    return { from, to };
  }, [current.edges]);

  const drawnNodes = useMemo(
    () =>
      board.nodes.map((n) => ({
        ...n,
        // A child may be dragged out of its group; where it lands decides its parent.
        extent: undefined,
        data: {
          ...n.data,
          issues: byNode.get(n.id),
          loopIn: loopEnds.to.has(n.id),
          loopOut: loopEnds.from.has(n.id),
          empty: n.type === "group" && !board.nodes.some((c) => c.parentId === n.id),
        },
      })),
    [board.nodes, byNode, loopEnds],
  );
  const drawnEdges = useMemo(
    () =>
      board.edges.map((e) => ({ ...e, markerEnd: e.type === "loop" ? LOOP_ARROW : ARROW })),
    [board.edges],
  );

  const onNodesChange = useCallback((changes: NodeChange<FlowNode>[]) => {
    const removed = changes.flatMap((c) => (c.type === "remove" ? [c.id] : []));
    const rest = changes.filter((c) => c.type !== "remove");
    setBoard((b) => {
      const applied = { ...b, nodes: applyNodeChanges(rest, b.nodes) };
      return removed.length ? dropNodes(applied.nodes, applied.edges, removed) : applied;
    });
  }, []);
  const onEdgesChange = useCallback((changes: EdgeChange<FlowEdge>[]) => {
    setBoard((b) => ({ ...b, edges: applyEdgeChanges(changes, b.edges) }));
  }, []);

  const onNodeDragStop = useCallback((_: unknown, __: unknown, dragged: FlowNode[]) => {
    setBoard((b) => {
      let nodes = b.nodes;
      for (const n of dragged) nodes = settleDrop(nodes, n.id);
      // A node that changed frame cannot stay wired across it.
      const moved = dragged
        .map((n) => n.id)
        .filter(
          (id) => b.nodes.find((n) => n.id === id)?.parentId !== nodes.find((n) => n.id === id)?.parentId,
        );
      return { nodes, edges: moved.length ? dropCrossingEdges(nodes, b.edges, moved) : b.edges };
    });
  }, []);

  const connect = useCallback(
    (c: Connection) => {
      const w = live.current.current;
      const kind = connectionKind(w, c.source, c.target, ifHandle(c.sourceHandle));
      if (kind === "invalid") return;
      if (kind === "loop") {
        setLoopAsk({
          from: c.source,
          to: c.target,
          loop: DEFAULT_LOOP,
          x: pointer.current.x,
          y: pointer.current.y,
        });
        return;
      }
      const handle = c.sourceHandle === "true" || c.sourceHandle === "false" ? c.sourceHandle : undefined;
      setBoard((b) => ({
        ...b,
        edges: [
          // An If node's true or false leads to one place.
          ...b.edges.filter(
            (e) => !(handle && !e.data?.loop && e.source === c.source && e.sourceHandle === handle),
          ),
          makeEdge(c.source, c.target, { handle }),
        ],
      }));
    },
    [],
  );

  // Deleting a For each takes only its frame: its nodes stay, now outside it, with their wires.
  const beforeDelete = useCallback(
    async ({ nodes, edges }: { nodes: FlowNode[]; edges: FlowEdge[] }) => deletion(nodes, edges),
    [],
  );

  const isValidConnection = useCallback(
    (c: { source: string; target: string; sourceHandle?: string | null }) =>
      connectionKind(live.current.current, c.source, c.target, ifHandle(c.sourceHandle)) !==
      "invalid",
    [],
  );

  // --- editing --------------------------------------------------------------------------------

  const updateNode = useCallback((next: WorkflowNode, inputMeta?: Pick<Workflow, "inputs" | "inputValues">) => {
    setBoard((b) => ({
      ...b,
      nodes: b.nodes.map((n) => (n.id === next.id ? { ...n, data: { ...n.data, node: next } } : n)),
    }));
    if (inputMeta) setMeta((m) => ({ ...m, ...inputMeta }));
  }, []);

  const setLoop = useCallback((id: NodeId, loop: LoopSpec | null) => {
    setBoard((b) => {
      const existing = b.edges.find((e) => e.data?.loop && e.source === id);
      if (!existing) return b;
      return {
        ...b,
        edges: loop
          ? b.edges.map((e) => (e === existing ? makeEdge(e.source, e.target, { loop }) : e))
          : b.edges.filter((e) => e !== existing),
      };
    });
  }, []);

  const remove = useCallback((ids: NodeId[]) => {
    setBoard((b) => dropNodes(b.nodes, b.edges, ids));
  }, []);

  const addNode = (type: NodeType) => {
    setPicker(false);
    const w = current;
    const node = newNode(type, w);
    const selected = board.nodes.find((n) => n.selected);
    const parent = groupable(node)
      ? selected?.type === "group"
        ? selected.id
        : selected?.parentId
      : undefined;
    const step = NODE_H + GAP_Y;
    const toNode = (position: { x: number; y: number }, parentId?: NodeId): FlowNode => ({
      id: node.id,
      type: isGroup(node) ? "group" : "board",
      position,
      width: isGroup(node) ? NODE_W + GROUP_PAD * 2 : NODE_W,
      ...(isGroup(node) ? { height: EMPTY_GROUP_H } : {}),
      selected: true,
      data: { node },
      ...(parentId ? { parentId, extent: "parent" as const } : {}),
    });
    setBoard((b) => {
      const deselected = b.nodes.map((n) => (n.selected ? { ...n, selected: false } : n));
      const report = deselected.find((n) => n.data.node.type === "report" && !n.parentId);
      const flowing = type === "session" || type === "command" || type === "askMono" || type === "forEach";
      let nodes: FlowNode[];
      let edges = b.edges;
      if (isTriggerType(type)) {
        const top = Math.min(0, ...deselected.filter((n) => !n.parentId).map((n) => n.position.y));
        nodes = [...deselected, toNode({ x: 0, y: top - step })];
      } else if (!parent && report && flowing) {
        // Before the Report node, which stays last: it moves down and the edges into it follow.
        const grow = type === "forEach" ? EMPTY_GROUP_H + GAP_Y : step;
        // At the Report's place, or below whatever hangs lowest above it, with a gap.
        const bottom = Math.max(
          ...deselected
            .filter((n) => !n.parentId && n.id !== report.id)
            .map((n) => n.position.y + (n.type === "group" ? (n.height ?? 0) : NODE_H)),
        );
        const y = Math.max(report.position.y, bottom + GAP_Y);
        const at = { x: type === "forEach" ? report.position.x - GROUP_PAD : report.position.x, y };
        nodes = deselected.map((n) =>
          n.id === report.id ? { ...n, position: { x: n.position.x, y: y + grow } } : n,
        );
        nodes = [...nodes, toNode(at)];
        const into = b.edges.filter((e) => !e.data?.loop && e.target === report.id);
        edges = [
          ...b.edges.filter((e) => !into.includes(e)),
          ...into.map((e) => makeEdge(e.source, node.id, { handle: e.sourceHandle === "true" || e.sourceHandle === "false" ? e.sourceHandle : undefined })),
          ...(into.length ? [makeEdge(node.id, report.id)] : []),
        ];
      } else {
        const place = autoPlace(w, parent);
        const at = isGroup(node) && !parent ? { x: place.x - GROUP_PAD, y: place.y } : place;
        nodes = [...deselected, toNode(at, parent)];
        // Wired after the node that hangs lowest in its scope, when that one leads nowhere yet.
        const scope = w.nodes.filter((n) => (n.parent ?? undefined) === parent);
        const lowest = scope.reduce<WorkflowNode | undefined>(
          (low, n) => (!low || (n.ui?.y ?? 0) >= (low.ui?.y ?? 0) ? n : low),
          undefined,
        );
        if (
          lowest &&
          lowest.type !== "report" &&
          lowest.type !== "if" &&
          !w.edges.some((e) => !e.loop && e.from === lowest.id) &&
          connectionKind({ ...w, nodes: [...w.nodes, { ...node, ...(parent ? { parent } : {}) }] }, lowest.id, node.id) === "edge"
        )
          edges = [...edges, makeEdge(lowest.id, node.id)];
      }
      const placed = sortParentsFirst(nodes);
      return { nodes: pushBelow(placed, fitGroups(placed)), edges };
    });
    setView({ kind: "node", id: node.id });
    requestAnimationFrame(() => void flow.fitView({ padding: 0.12, maxZoom: 1, duration: 200 }));
  };

  const confirmLoop = () => {
    if (!loopAsk) return;
    const { from, to, loop } = loopAsk;
    setLoopAsk(null);
    setBoard((b) => ({ ...b, edges: [...b.edges, makeEdge(from, to, { loop })] }));
  };

  // --- saving and leaving ------------------------------------------------------------------------

  const save = async (paused = false, overwrite = false): Promise<boolean> => {
    if (saving) return false;
    const next: Workflow = { ...current, enabled: paused ? false : current.enabled };
    setSaving(true);
    setNotice(undefined);
    try {
      const saved = await updateWorkflows(monoId, (list) => {
        const stored = list.find((w) => w.id === next.id);
        // Not over a definition this board has not seen; Overwrite is the explicit way past.
        if (stored && !overwrite && definitionKey(stored) !== live.current.seen)
          throw new ChangedElsewhere(stored);
        if (!stored && list.length >= WORKFLOWS_MAX)
          throw new Error(`At most ${WORKFLOWS_MAX} workflows.`);
        // The stored copy decides, not what this board loaded: it may have been flagged since.
        if (stored?.needsReview && next.enabled) throw new Error(REVIEW_FIRST);
        const result = merged(stored, next, Date.now());
        return {
          workflows: stored ? list.map((w) => (w.id === next.id ? result : w)) : [...list, result],
          result,
        };
      });
      setBase(saved);
      // The save may have anchored an interval schedule; the board takes that, so it is clean.
      setBoard((b) => ({
        ...b,
        nodes: b.nodes.map((n) => {
          const stored = saved.nodes.find((s) => s.id === n.id);
          // Only a schedule the save anchored; an edit made while saving is kept.
          return n.data.node.type === "schedule" &&
            !n.data.node.schedule.intervalFrom &&
            stored?.type === "schedule"
            ? { ...n, data: { ...n.data, node: { ...n.data.node, schedule: stored.schedule } } }
            : n;
        }),
      }));
      setMeta((m) => ({ ...m, enabled: saved.enabled }));
      setIsNew(false);
      const key = definitionKey(saved);
      setBaseline(key);
      live.current.seen = key;
      setElsewhere(null);
      return true;
    } catch (error) {
      if (error instanceof ChangedElsewhere) {
        setElsewhere({ workflow: error.stored, refused: true });
        return false;
      }
      console.warn("Could not save the workflow", error);
      setNotice(error instanceof Error && error.message ? error.message : "Could not save the workflow.");
      return false;
    } finally {
      setSaving(false);
    }
  };

  const canSave = (changed || isNew) && !blocked && !saving;

  const requestBack = () => {
    if (changed) setLeaving(true);
    else onBack();
  };

  const runNow = async () => {
    setMenu(false);
    if (base.needsReview) return onReview("run");
    setNotice(await workflowActions(monoId).runNow(current.id));
  };

  const exportIt = async () => {
    setMenu(false);
    setNotice(await exportWorkflowToFile(current));
  };

  const problemCount = issues.length;
  return (
    <div
      className="workflow-board flex min-h-0 flex-1 flex-col"
      style={{ "--mono-color": color } as CSSProperties}
      data-workflow-board={current.id}
      onKeyDown={(event) => {
        if ((event.metaKey || event.ctrlKey) && event.key === "s" && !isImeComposition(event.nativeEvent)) {
          event.preventDefault();
          if (canSave) void save();
        }
      }}
      onPointerDownCapture={(event) => (pointer.current = { x: event.clientX, y: event.clientY })}
      onPointerUpCapture={(event) => (pointer.current = { x: event.clientX, y: event.clientY })}
    >
      <PageHeader
        onBack={requestBack}
        title={
          <input
            aria-label="Workflow name"
            value={meta.name}
            maxLength={80}
            placeholder="Name"
            data-tauri-drag-region="false"
            onChange={(event) => setMeta((m) => ({ ...m, name: event.target.value }))}
            className="h-7 w-full min-w-0 rounded-md bg-transparent px-1.5 text-[13px] font-medium text-content outline-none placeholder:text-content/35 hover:bg-content/5 focus:bg-content/5"
          />
        }
      >
        <span ref={addAnchor} className="flex">
          <IconButton label="Add node" onClick={() => setPicker((open) => !open)}>
            <Plus className="size-3.5" strokeWidth={1.75} />
          </IconButton>
        </span>
        <span ref={moreAnchor} className="flex">
          <IconButton label="More" onClick={() => setMenu((open) => !open)}>
            <MoreHorizontal className="size-3.5" strokeWidth={1.75} />
          </IconButton>
        </span>
        <button
          type="button"
          aria-label="Save"
          disabled={!canSave}
          onClick={() => void save()}
          data-tauri-drag-region="false"
          className="h-6 rounded-md bg-content/10 px-2 text-[12px] text-content enabled:hover:bg-content/[0.14] disabled:opacity-40"
        >
          Save
        </button>
      </PageHeader>

      <WorkflowNotice>{notice}</WorkflowNotice>

      {elsewhere ? (
        <div
          data-board-changed
          role="status"
          className="flex flex-wrap items-center gap-x-2 gap-y-1 border-b border-stroke px-4 py-1.5 text-[11px] leading-4 text-content/60"
        >
          <span className="min-w-0 flex-1">
            {elsewhere.refused
              ? "This workflow changed elsewhere, so your changes were not saved"
              : "This workflow changed elsewhere"}
          </span>
          <button
            type="button"
            onClick={() => load(elsewhere.workflow)}
            className="rounded px-1.5 py-0.5 text-content/70 hover:bg-content/8 hover:text-content"
          >
            Reload
          </button>
          {elsewhere.refused ? (
            <button
              type="button"
              disabled={saving || blocked}
              onClick={() => void save(false, true)}
              className="rounded px-1.5 py-0.5 text-content/70 hover:bg-content/8 hover:text-content disabled:opacity-40"
            >
              Overwrite
            </button>
          ) : (
            <button
              type="button"
              onClick={() => setElsewhere(null)}
              className="rounded px-1.5 py-0.5 text-content/70 hover:bg-content/8 hover:text-content"
            >
              Keep mine
            </button>
          )}
        </div>
      ) : null}

      {unreviewed ? (
        <div
          data-board-review
          className="flex items-center gap-2 border-b border-stroke px-4 py-1.5 text-[11px] leading-4 text-content/60"
        >
          <span className="min-w-0 flex-1">{REVIEW_FIRST}</span>
          <button
            type="button"
            onClick={() => onReview("enable")}
            className="shrink-0 rounded px-1.5 py-0.5 text-content/70 hover:bg-content/8 hover:text-content"
          >
            Review
          </button>
          {problemCount ? null : (
            <button
              type="button"
              disabled={saving}
              onClick={() => void save(true)}
              className="shrink-0 rounded px-1.5 py-0.5 text-content/70 hover:bg-content/8 hover:text-content disabled:opacity-40"
            >
              Save paused
            </button>
          )}
        </div>
      ) : null}

      {problemCount ? (
        <div
          data-board-issues
          className="border-b border-stroke px-4 py-1.5 text-[11px] leading-4 text-content/60"
        >
          <div className="flex items-center gap-2">
            <button
              type="button"
              aria-expanded={showIssues}
              onClick={() => setShowIssues((open) => !open)}
              className="flex min-w-0 flex-1 items-center gap-1.5 text-left"
            >
              <span aria-hidden className="size-1.5 shrink-0 rounded-full bg-red-500/70" />
              <span className="shrink-0 text-content/80">
                {problemCount} {problemCount === 1 ? "problem" : "problems"}
              </span>
              <span className="min-w-0 truncate text-content/45">{issues[0].message}</span>
            </button>
            {current.enabled ? (
              <button
                type="button"
                disabled={saving}
                onClick={() => void save(true)}
                className="shrink-0 rounded px-1.5 py-0.5 text-content/70 hover:bg-content/8 hover:text-content disabled:opacity-40"
              >
                Save paused
              </button>
            ) : null}
          </div>
          {showIssues ? (
            <ul className="flex flex-col gap-0.5 pb-0.5 pt-1.5 pl-3">
              {issues.map((issue, index) => (
                <li key={`${index}:${issue.message}`}>{issue.message}</li>
              ))}
            </ul>
          ) : null}
        </div>
      ) : null}

      <div className="relative min-h-0 flex-1 overflow-hidden">
        <div
          className="board-layer absolute inset-0"
          data-covered={view ? "true" : undefined}
          inert={view ? true : undefined}
        >
          <ReactFlow<FlowNode, FlowEdge>
            nodes={drawnNodes}
            edges={drawnEdges}
            nodeTypes={nodeTypes}
            edgeTypes={edgeTypes}
            onNodesChange={onNodesChange}
            onEdgesChange={onEdgesChange}
            onConnect={connect}
            isValidConnection={isValidConnection}
            onNodeDragStop={onNodeDragStop}
            onNodeClick={(_, node) => setView({ kind: "node", id: node.id })}
            connectionLineType={ConnectionLineType.SmoothStep}
            fitView
            fitViewOptions={{ padding: 0.12, maxZoom: 1 }}
            minZoom={0.3}
            maxZoom={1.5}
            panOnScroll
            zoomOnScroll={false}
            nodeDragThreshold={2}
            // Not while the inspector is open: Backspace on one of its buttons must not delete a node.
            deleteKeyCode={view ? null : ["Backspace", "Delete"]}
            onBeforeDelete={beforeDelete}
            proOptions={{ hideAttribution: true }}
          >
            <Background variant={BackgroundVariant.Dots} gap={18} size={1} />
          </ReactFlow>
          <ZoomControls />
        </div>
        <NodeInspector
          view={view}
          workflow={current}
          projects={projects}
          monoModel={monoModel}
          issues={view?.kind === "node" ? (byNode.get(view.id) ?? []) : []}
          onNode={updateNode}
          onSettings={(patch) => setMeta((m) => ({ ...m, ...patch }))}
          onLoop={setLoop}
          onRemove={(id) => {
            remove([id]);
            setView(null);
          }}
          onBack={() => setView(null)}
        />
      </div>

      {picker ? (
        <NodePicker
          anchor={addAnchor}
          has={{
            trigger: current.nodes.some((n) => n.type === "schedule" || n.type === "manual"),
            report: current.nodes.some((n) => n.type === "report"),
          }}
          onPick={addNode}
          onClose={() => setPicker(false)}
        />
      ) : null}

      {menu ? (
        <Popover
          anchor={moreAnchor}
          align="end"
          width={216}
          onDismiss={() => setMenu(false)}
          role="menu"
          aria-label="Workflow menu"
          className="p-1"
        >
          <MenuItem
            icon={<Settings className="size-3.5" strokeWidth={1.75} />}
            label="Workflow settings"
            onClick={() => {
              setMenu(false);
              setView({ kind: "settings" });
            }}
          />
          <MenuItem
            icon={<Share className="size-3.5" strokeWidth={1.75} />}
            label="Export"
            onClick={() => void exportIt()}
          />
          <MenuItem
            icon={<Zap className="size-3.5" strokeWidth={1.75} />}
            label="Run now"
            disabled={isNew || changed || liveRunId !== undefined}
            hint={isNew || changed ? "Save first" : liveRunId ? "Already running" : undefined}
            onClick={() => void runNow()}
          />
          <MenuItem
            icon={
              current.enabled ? (
                <Pause className="size-3.5" strokeWidth={1.75} />
              ) : (
                <Play className="size-3.5" strokeWidth={1.75} />
              )
            }
            label={current.enabled ? "Pause" : "Resume"}
            onClick={() => {
              setMenu(false);
              if (base.needsReview && !current.enabled) return onReview("enable");
              setMeta((m) => ({ ...m, enabled: !m.enabled }));
            }}
          />
        </Popover>
      ) : null}

      {loopAsk ? (
        <Popover
          anchor={{ x: loopAsk.x, y: loopAsk.y }}
          width={296}
          onDismiss={() => setLoopAsk(null)}
          role="dialog"
          aria-label="New loop"
          className="p-2"
        >
          <p className="px-2 pb-1 pt-1 text-[12px] leading-5 text-content/70">
            Go back to{" "}
            <span className="text-content">
              {nodeTitle(current.nodes.find((n) => n.id === loopAsk.to) ?? current.nodes[0])}
            </span>{" "}
            until it is right.
          </p>
          <LoopForm loop={loopAsk.loop} onChange={(loop) => setLoopAsk({ ...loopAsk, loop })} />
          <div className="flex justify-end gap-2 px-1 pb-1 pt-2">
            <button
              type="button"
              onClick={() => setLoopAsk(null)}
              className="rounded-md px-3 py-1 text-[12px] text-content/70 hover:bg-content/8"
            >
              Cancel
            </button>
            <button
              type="button"
              onClick={confirmLoop}
              className="rounded-md bg-content/10 px-3 py-1 text-[12px] text-content hover:bg-content/[0.14]"
            >
              Add loop
            </button>
          </div>
        </Popover>
      ) : null}

      {leaving ? (
        <Modal title="Save your changes?" size="sm" onClose={() => setLeaving(false)}>
          <div className="flex flex-col gap-3 px-4 pb-4 pt-3">
            <p className="text-[13px] leading-5 text-content/70">
              {blocked
                ? `This workflow has changes you have not saved, and ${
                    issues.length
                      ? "problems that keep it from being saved while enabled"
                      : "it needs a review before it can be enabled"
                  }.`
                : "This workflow has changes you have not saved."}
            </p>
            <div className="flex justify-end gap-2 pt-1">
              <button
                type="button"
                onClick={() => setLeaving(false)}
                className="rounded-md px-3 py-1.5 text-[12px] text-content/70 hover:bg-content/8"
              >
                Keep editing
              </button>
              <button
                type="button"
                onClick={onBack}
                className="rounded-md px-3 py-1.5 text-[12px] text-content/70 hover:bg-content/8"
              >
                Discard
              </button>
              <button
                type="button"
                disabled={saving}
                onClick={() =>
                  void save(blocked).then((saved) => {
                    if (saved) onBack();
                    else setLeaving(false);
                  })
                }
                className="rounded-md bg-content/10 px-3 py-1.5 text-[12px] font-medium text-content enabled:hover:bg-content/[0.14] disabled:opacity-50"
              >
                {blocked ? "Save paused" : "Save"}
              </button>
            </div>
          </div>
        </Modal>
      ) : null}
    </div>
  );
}

function isTriggerType(type: NodeType): boolean {
  return type === "schedule" || type === "manual";
}

function sortParentsFirst(nodes: FlowNode[]): FlowNode[] {
  return [...nodes.filter((n) => n.type === "group"), ...nodes.filter((n) => n.type !== "group")];
}
