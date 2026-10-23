import { useState } from "react";
import { ChevronDown, ChevronRight } from "../../../../shared/ui/icons";
import { duration, RunningFor, when } from "../../../monos/ui/HabitPage";
import { Property } from "../../../monos/ui/monoPanelParts";
import { archivedItemsKey, parseBranch, type NodeRun, type RoundRecord, type WorkflowRun } from "../../model/workflow";
import { NODE_LABEL, nodeTitle, RUN_DOT } from "../board/boardNodes";
import { isAsking } from "../../model/workflowEngine";
import { itemTitle, nodeRunsByKey, statusOf, type RunNodeStatus } from "./runGraphModel";

const SHORT = 200;
const TAIL = 600;

const STATUS_LABEL: Record<RunNodeStatus, string> = {
  done: "Done",
  running: "Running",
  waiting: "Waiting for approval",
  failed: "Failed",
  skipped: "Skipped",
  pending: "Not started",
  stopped: "Stopped",
};

const COMMAND_FIELDS = ["exitCode", "stdout", "stderr"];

function StatusLabel({ status, asking }: { status: RunNodeStatus; asking?: boolean }) {
  return (
    <>
      <span aria-hidden className={`mr-2 size-2 shrink-0 rounded-full ${RUN_DOT[status]}`} />
      {asking ? "Asking the Mono" : STATUS_LABEL[status]}
    </>
  );
}

/** A value as the details show it: text as it is, anything else as JSON. */
function shown(value: unknown): string {
  if (typeof value === "string") return value;
  try {
    return JSON.stringify(value) ?? String(value);
  } catch {
    return String(value);
  }
}

function OutputValue({ name, value }: { name: string; value: unknown }) {
  const [all, setAll] = useState(false);
  const text = shown(value);
  const long = text.length > SHORT;
  return (
    <Property label={name}>
      <span className="min-w-0 whitespace-pre-wrap break-words py-1.5 leading-4">
        {long && !all ? `${text.slice(0, SHORT)}...` : text}
        {long ? (
          <button
            type="button"
            onClick={() => setAll(!all)}
            className="ml-1.5 text-[11px] text-[var(--mono-color)] hover:underline"
          >
            {all ? "Show less" : "Show more"}
          </button>
        ) : null}
      </span>
    </Property>
  );
}

function Tail({ label, text }: { label: string; text: string }) {
  const tail = text.length > TAIL ? `...${text.slice(-TAIL)}` : text;
  return (
    <div className="px-2 pb-1.5">
      <p className="pb-1 text-[11px] leading-4 text-content/45">{label}</p>
      <pre className="max-h-24 overflow-auto whitespace-pre-wrap break-words rounded-md bg-content/5 px-2 py-1.5 font-mono text-[11px] leading-4 text-content/75">
        {tail}
      </pre>
    </div>
  );
}

function SessionButton({ id, onOpen }: { id: string; onOpen: (sessionId: string) => void }) {
  return (
    <button
      type="button"
      onClick={() => onOpen(id)}
      className="inline-flex items-center gap-0.5 rounded-md px-1.5 py-0.5 text-[12px] text-[var(--mono-color)] hover:bg-content/5"
    >
      Open session
      <ChevronRight className="size-3" strokeWidth={1.75} aria-hidden />
    </button>
  );
}

function span(r: { startedAt?: number; endedAt?: number }): string | undefined {
  return r.startedAt !== undefined && r.endedAt !== undefined ? duration(r.endedAt - r.startedAt) : undefined;
}

function Round({ record, onOpenSession }: { record: RoundRecord; onOpenSession: (sessionId: string) => void }) {
  const status = statusOf({ nodeId: "", branch: "", ...record });
  const decision = record.decision
    ? `${record.decision.value}${record.decision.reason ? `: ${record.decision.reason}` : ""}`
    : undefined;
  return (
    <li data-round-record={record.round} className="flex flex-col gap-0.5 rounded-md px-2 py-1.5 text-[12px] leading-5">
      <div className="flex items-center text-content/85">
        <StatusLabel status={status} />
        <span className="ml-1 text-content/45">
          Round {record.round}
          {span(record) ? ` · ${span(record)}` : ""}
          {record.startedAt !== undefined ? ` · ${when(record.startedAt)}` : ""}
        </span>
        {record.sessionId ? (
          <span className="ml-auto">
            <SessionButton id={record.sessionId} onOpen={onOpenSession} />
          </span>
        ) : null}
      </div>
      {decision ? <p className="whitespace-pre-wrap break-words text-content/70">Decided {decision}</p> : null}
      {record.error ? (
        <p className="whitespace-pre-wrap break-words text-red-500/80">{record.error}</p>
      ) : null}
    </li>
  );
}

/** The title of the node, with the item it ran for when it sits in a For each lane. */
function heading(run: WorkflowRun, node: NodeRun): string {
  const def = run.definition.nodes.find((n) => n.id === node.nodeId);
  const title = def ? nodeTitle(def) : node.nodeId;
  const parts = parseBranch(node.branch);
  if (!parts) return title;
  const list = run.items?.[parts.round === undefined ? parts.groupId : archivedItemsKey(parts.groupId, parts.round)];
  return `${title} · ${itemTitle(list, parts.index)}`;
}

/**
 * What a run did with one node: its state, the rounds a loop already finished, why it was
 * decided the way it was, its output and, for a command, how it exited.
 */
export function RunNodeDetails({
  run,
  nodeRunKey,
  onOpenSession,
}: {
  run: WorkflowRun;
  nodeRunKey: string | undefined;
  onOpenSession: (sessionId: string) => void;
}) {
  const [open, setOpen] = useState(true);
  const node = nodeRunKey ? nodeRunsByKey(run).get(nodeRunKey) : undefined;
  const def = node ? run.definition.nodes.find((n) => n.id === node.nodeId) : undefined;
  const status = statusOf(node);
  const output = node?.output ?? {};
  const isCommand = def?.type === "command";
  const fields = Object.entries(output).filter(([name]) => !(isCommand && COMMAND_FIELDS.includes(name)));
  return (
    <section data-run-details className="shrink-0 border-t border-stroke">
      <button
        type="button"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
        className="flex w-full items-center gap-1.5 px-4 py-2 text-left"
      >
        {open ? (
          <ChevronDown className="size-3 text-content/40" strokeWidth={1.75} aria-hidden />
        ) : (
          <ChevronRight className="size-3 text-content/40" strokeWidth={1.75} aria-hidden />
        )}
        <span className="text-[11px] font-medium uppercase tracking-wide text-content/40">Details</span>
        <span className="min-w-0 flex-1 truncate text-[12px] text-content/85">
          {node ? heading(run, node) : ""}
        </span>
      </button>
      {open ? (
        <div className="max-h-[30vh] overflow-y-auto overscroll-none px-2 pb-3">
          {node ? (
            <>
              <dl className="px-2">
                <Property label="Type">{def ? NODE_LABEL[def.type] : "Node"}</Property>
                <Property label="Status">
                  <StatusLabel status={status} asking={isAsking(node)} />
                  {node.round > 1 ? <span className="ml-1 text-content/45">round {node.round}</span> : null}
                </Property>
                {node.startedAt !== undefined ? <Property label="Started">{when(node.startedAt)}</Property> : null}
                {node.startedAt !== undefined ? (
                  <Property label="Took">
                    {node.endedAt !== undefined ? (
                      duration(node.endedAt - node.startedAt)
                    ) : node.status === "running" ? (
                      <RunningFor since={node.startedAt} />
                    ) : (
                      "-"
                    )}
                  </Property>
                ) : null}
                {node.decision ? (
                  <Property label="Decision">
                    <span className="min-w-0 whitespace-pre-wrap break-words py-1.5 leading-4">
                      {node.decision.value}
                      {node.decision.reason ? `: ${node.decision.reason}` : ""}
                    </span>
                  </Property>
                ) : null}
                {node.sessionId ? (
                  <Property label="Session">
                    <SessionButton id={node.sessionId} onOpen={onOpenSession} />
                  </Property>
                ) : null}
              </dl>
              {node.error ? (
                <p className="whitespace-pre-wrap break-words px-4 pb-2 pt-1 text-[12px] leading-5 text-red-500/80">
                  {node.error}
                </p>
              ) : null}
              {node.history?.length ? (
                <div className="pt-1">
                  <h4 className="px-2 pb-1 text-[11px] font-medium uppercase tracking-wide text-content/40">Rounds</h4>
                  <ul className="flex flex-col">
                    {node.history.map((record) => (
                      <Round key={record.round} record={record} onOpenSession={onOpenSession} />
                    ))}
                  </ul>
                </div>
              ) : null}
              {isCommand && (output.exitCode !== undefined || output.stdout || output.stderr) ? (
                <div className="pt-1">
                  <dl className="px-2">
                    {output.exitCode !== undefined ? (
                      <Property label="Exit code">
                        <span className="tabular-nums">{shown(output.exitCode)}</span>
                      </Property>
                    ) : null}
                  </dl>
                  {typeof output.stdout === "string" && output.stdout ? (
                    <Tail label="Output" text={output.stdout} />
                  ) : null}
                  {typeof output.stderr === "string" && output.stderr ? (
                    <Tail label="Errors" text={output.stderr} />
                  ) : null}
                </div>
              ) : null}
              {fields.length ? (
                <div className="pt-1">
                  <h4 className="px-2 pb-1 text-[11px] font-medium uppercase tracking-wide text-content/40">Output</h4>
                  <dl className="px-2">
                    {fields.map(([name, value]) => (
                      <OutputValue key={name} name={name} value={value} />
                    ))}
                  </dl>
                </div>
              ) : null}
            </>
          ) : (
            <p className="px-4 py-2 text-[12px] text-content/45">Pick a node to see what it did.</p>
          )}
        </div>
      ) : null}
    </section>
  );
}
