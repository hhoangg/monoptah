import { Handle, Position, type NodeProps } from "@xyflow/react";
import {
  Clock,
  CircleHelp,
  GitBranch,
  MessageMultiple,
  Play,
  RefreshCw,
  Send,
  Terminal,
  type IconComponent,
} from "../../../../shared/ui/icons";
import { habitScheduleLabel } from "../../../monos/model/monoHabits";
import type { NodeType, WorkflowNode } from "../../model/workflow";
import {
  GROUP_HEADER,
  GROUP_PAD,
  LOOP_IN,
  LOOP_OUT,
  NODE_W,
  type FlowNode,
} from "./boardModel";
import type { RunNodeStatus } from "../run/runGraphModel";

export const NODE_ICON: Record<NodeType, IconComponent> = {
  schedule: Clock,
  manual: Play,
  session: MessageMultiple,
  command: Terminal,
  askMono: CircleHelp,
  if: GitBranch,
  forEach: RefreshCw,
  report: Send,
};

export const NODE_LABEL: Record<NodeType, string> = {
  schedule: "Schedule",
  manual: "Manual",
  session: "Session",
  command: "Command",
  askMono: "Ask Mono",
  if: "If",
  forEach: "For each",
  report: "Report to Mono",
};

/** What the node is called on the board: its title, else its id read as words. */
export function nodeTitle(node: WorkflowNode): string {
  const title = node.title?.trim();
  if (title) return title;
  const words = node.id.replace(/_+/g, " ").trim();
  return words.charAt(0).toUpperCase() + words.slice(1);
}

const firstLine = (text: string) => text.split("\n").find((line) => line.trim())?.trim() ?? "";

/** The second line of a node's card: the part of its setup that says what it does. */
export function nodeSummary(node: WorkflowNode): string {
  switch (node.type) {
    case "schedule":
      return habitScheduleLabel(node.schedule);
    case "manual":
      return "Starts when you run it";
    case "session":
      return firstLine(node.prompt) || "No prompt yet";
    case "command":
      return firstLine(node.command) || "No command yet";
    case "askMono":
      return firstLine(node.question) || "No question yet";
    case "if":
      return node.decide === "mono" ? "Mono decides" : firstLine(node.condition ?? "") || "No condition yet";
    case "forEach":
      return node.in || "No list yet";
    case "report":
      return node.template ? "Custom report" : "Summary of every branch";
  }
}

/** The dot that says how a node stands in a run: the app's usual status colours. */
export const RUN_DOT: Record<RunNodeStatus, string> = {
  done: "bg-emerald-400/80",
  running: "animate-pulse bg-[var(--mono-color)]",
  waiting: "bg-amber-400/80",
  failed: "bg-red-500/70",
  stopped: "bg-content/40",
  skipped: "border border-content/30",
  pending: "border border-content/30",
};

/** "Done · 12s · round 2" without its round: the badge beside the title says that. */
const withoutRound = (label: string) => label.replace(/ · round \d+$/, "");

const LOOPS_OUT: readonly NodeType[] = ["session", "command", "askMono"];

/** A trigger, action, If or Report node: an icon tile and two lines, like a habit row. */
export function BoardNode({ id, data, selected }: NodeProps<FlowNode>) {
  const node = data.node;
  const Icon = NODE_ICON[node.type];
  const issues = data.issues ?? [];
  const run = data.run;
  // In a run graph the handles only anchor the lines; nothing can be drawn from them.
  const connectable = !run;
  const handle = run ? "board-handle board-handle-run" : "board-handle";
  return (
    <div
      data-board-node={id}
      data-type={node.type}
      data-run-status={run?.status}
      data-selected={selected || undefined}
      className="board-node flex flex-col rounded-lg border border-content/10 bg-content/5 px-2 py-2"
    >
      {node.type === "schedule" || node.type === "manual" ? null : (
        <Handle type="target" position={Position.Top} className={handle} isConnectable={connectable} />
      )}
      {node.type === "schedule" || node.type === "manual" || node.type === "report" ? null : (
        <Handle
          id={LOOP_IN}
          type="target"
          position={Position.Right}
          className={`board-handle board-handle-loop${run ? " board-handle-run" : ""}`}
          style={{ top: "30%" }}
          isConnectable={connectable}
          data-used={data.loopIn || undefined}
        />
      )}
      <span className="flex items-center gap-2.5">
        <span
          aria-hidden
          className="grid size-7 shrink-0 place-items-center rounded-md bg-content/6 text-content/60"
        >
          <Icon className="size-3.5" strokeWidth={1.75} />
        </span>
        <span className="flex min-w-0 flex-1 flex-col">
          <span className="flex min-w-0 items-center gap-1.5">
            <span className="truncate text-[12px] leading-5 text-content/85">{nodeTitle(node)}</span>
            {run && run.round > 1 ? (
              <span
                data-run-round
                className="shrink-0 rounded-full bg-[color-mix(in_srgb,var(--mono-color)_14%,transparent)] px-1.5 text-[10px] leading-4 text-[color-mix(in_srgb,var(--mono-color)_80%,var(--color-content))]"
              >
                round {run.round}
              </span>
            ) : null}
          </span>
          {run ? (
            <span data-run-label title={run.label} className="truncate text-[11px] leading-4 text-content/40">
              {withoutRound(run.label)}
            </span>
          ) : issues.length ? (
            <span
              data-node-issue
              title={issues.join("\n")}
              className="truncate text-[11px] leading-4 text-red-500/80"
            >
              {issues[0]}
              {issues.length > 1 ? ` (+${issues.length - 1})` : ""}
            </span>
          ) : (
            <span className="truncate text-[11px] leading-4 text-content/40">{nodeSummary(node)}</span>
          )}
        </span>
        {run ? (
          <span aria-hidden data-run-dot className={`size-2 shrink-0 rounded-full ${RUN_DOT[run.status]}`} />
        ) : issues.length ? (
          <span
            aria-hidden
            className="size-2 shrink-0 rounded-full bg-red-500/70"
          />
        ) : null}
      </span>
      {node.type === "if" ? (
        <>
          <Handle id="true" type="source" position={Position.Bottom} className={handle} style={{ left: "28%" }} isConnectable={connectable} />
          <Handle id="false" type="source" position={Position.Bottom} className={handle} style={{ left: "72%" }} isConnectable={connectable} />
          <span aria-hidden className="board-handle-label" style={{ left: "28%" }}>
            true
          </span>
          <span aria-hidden className="board-handle-label" style={{ left: "72%" }}>
            false
          </span>
        </>
      ) : node.type === "report" ? null : (
        <Handle type="source" position={Position.Bottom} className={handle} isConnectable={connectable} />
      )}
      {LOOPS_OUT.includes(node.type) ? (
        <Handle
          id={LOOP_OUT}
          type="source"
          position={Position.Right}
          className={`board-handle board-handle-loop${run ? " board-handle-run" : ""}`}
          style={{ top: "70%" }}
          isConnectable={connectable}
          data-used={data.loopOut || undefined}
        />
      ) : null}
    </div>
  );
}

/** A For each: a frame around the nodes that run once per item. */
export function GroupNode({ id, data, selected }: NodeProps<FlowNode>) {
  const node = data.node;
  const issues = data.issues ?? [];
  const run = data.run;
  // The handle sits inside the 1px border, so one less puts it over the column of the nodes.
  const middle = GROUP_PAD + NODE_W / 2 - 1;
  const handle = run ? "board-handle board-handle-run" : "board-handle";
  return (
    <div
      data-board-node={id}
      data-type="forEach"
      data-run-status={run?.status}
      data-selected={selected || undefined}
      className="board-group h-full w-full rounded-xl border border-content/10 bg-content/[0.03]"
    >
      <Handle type="target" position={Position.Top} className={handle} style={{ left: middle }} isConnectable={!run} />
      <div
        className="flex items-center gap-1.5 px-3 text-[11px] font-medium uppercase tracking-wide text-content/40"
        style={{ height: GROUP_HEADER }}
      >
        <RefreshCw className="size-3 shrink-0" strokeWidth={1.75} aria-hidden />
        <span className="shrink-0">For each</span>
        <span className="min-w-0 truncate normal-case tracking-normal text-content/55">
          {node.title?.trim() || (node.type === "forEach" ? node.in : "")}
        </span>
        {run ? (
          <>
            <span
              data-run-label
              title={run.label}
              className="ml-auto min-w-0 shrink-[2] truncate normal-case tracking-normal text-content/40"
            >
              {run.label}
            </span>
            <span aria-hidden data-run-dot className={`size-2 shrink-0 rounded-full ${RUN_DOT[run.status]}`} />
          </>
        ) : issues.length ? (
          <span
            data-node-issue
            title={issues.join("\n")}
            className="ml-auto size-2 shrink-0 rounded-full bg-red-500/70"
          />
        ) : null}
      </div>
      {data.empty ? (
        <p className="px-3 pt-1 text-[11px] leading-4 text-content/35">Drag nodes in to run them once per item.</p>
      ) : null}
      <Handle type="source" position={Position.Bottom} className={handle} style={{ left: middle }} isConnectable={!run} />
    </div>
  );
}

export const nodeTypes = { board: BoardNode, group: GroupNode };
