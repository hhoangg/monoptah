import { useState, type CSSProperties } from "react";
import { IconButton } from "../../../app/shell/TitleBar";
import { basename } from "../../../platform/tauri/fs";
import {
  Pause,
  Pencil,
  Play,
  Share,
  Square,
  Trash2,
  Zap,
} from "../../../shared/ui/icons";
import { SearchableSelect } from "../../../shared/ui/SearchableSelect";
import type { MonoProject } from "../../monos/model/mono";
import { duration, RunningFor, when } from "../../monos/ui/HabitPage";
import { PageHeader, Property, Section } from "../../monos/ui/monoPanelParts";
import {
  triggerOf,
  type Workflow,
  type WorkflowInput,
  type WorkflowRun,
} from "../model/workflow";
import { updateWorkflows } from "../model/workflowStore";
import { inputValueProblems } from "../model/workflowYaml";
import {
  NeedsReviewBadge,
  useLiveRunId,
  useWorkflowRuns,
  WorkflowNotice,
  workflowTriggerLabel,
  type ReviewIntent,
  type WorkflowActions,
} from "./MonoWorkflows";
import { exportWorkflowToFile } from "./workflowExportFile";
import { RUN_STATUS } from "./WorkflowRunPage";

type InputValue = Workflow["inputValues"][string];

export const FIELD =
  "h-7 min-w-0 flex-1 rounded-md bg-content/10 px-2 text-[12px] text-content outline-none placeholder:text-content/35 hover:bg-content/[0.14] focus-visible:bg-content/[0.14]";

/** One declared input, edited in place: a project picker, a switch, or a text box. */
export function InputField({
  name,
  input,
  value,
  projects,
  onSave,
}: {
  name: string;
  input: WorkflowInput;
  value: InputValue | undefined;
  projects: readonly MonoProject[];
  /** The text or choice to keep; a number arrives as typed, to be checked. */
  onSave: (value: InputValue | undefined) => void;
}) {
  if (input.type === "project") {
    const current = typeof value === "string" ? value : "";
    const options = projects.map(({ path, name }) => ({
      value: path,
      label: name,
    }));
    // A project the Mono no longer has stays choosable, so the value is not hidden.
    if (current && !options.some((option) => option.value === current))
      options.unshift({ value: current, label: basename(current) });
    return (
      <SearchableSelect
        label={name}
        value={current}
        options={options}
        placeholder="Choose a project"
        searchPlaceholder="Search projects…"
        variant="pill"
        searchable={options.length > 6}
        onChange={onSave}
      />
    );
  }
  if (input.type === "boolean")
    return (
      <SearchableSelect
        label={name}
        value={value === undefined ? "" : value ? "on" : "off"}
        options={[
          { value: "on", label: "On" },
          { value: "off", label: "Off" },
        ]}
        placeholder="Choose"
        variant="pill"
        searchable={false}
        onChange={(next) => onSave(next === "on")}
      />
    );
  const text = value === undefined ? "" : String(value);
  return (
    <input
      // A value saved elsewhere replaces what was typed.
      key={text}
      aria-label={name}
      defaultValue={text}
      inputMode={input.type === "number" ? "decimal" : undefined}
      placeholder={input.description ?? name}
      spellCheck={false}
      onBlur={(event) => {
        const typed = event.target.value;
        if (typed === text) return;
        if (input.type === "string") return onSave(typed);
        onSave(typed.trim() === "" ? undefined : Number(typed));
      }}
      onKeyDown={(event) => {
        if (event.key === "Enter") event.currentTarget.blur();
      }}
      className={FIELD}
    />
  );
}

/** One workflow up close: its trigger, inputs and how its recent runs went. */
export function WorkflowPage({
  monoId,
  workflow,
  color,
  projects,
  actions,
  onBack,
  onEditBoard,
  onOpenRun,
  onRemove,
  onReview,
}: {
  monoId: string;
  workflow: Workflow;
  /** The Mono's color, for the run in progress. */
  color: string;
  /** The Mono's projects, for inputs that name one. */
  projects: readonly MonoProject[];
  actions: WorkflowActions;
  onBack: () => void;
  onEditBoard: () => void;
  onOpenRun: (runId: string) => void;
  /** Called once the workflow is removed, to close its page. */
  onRemove: () => void;
  /** Opens the review a workflow that needs one must pass before it can run or be enabled. */
  onReview: (intent: ReviewIntent) => void;
}) {
  const [notice, setNotice] = useState<string>();
  const liveRunId = useLiveRunId(workflow.id);
  const running = liveRunId !== undefined;
  const starting = !running && !!workflow.runRequested;
  const runs = useWorkflowRuns(monoId, workflow.id);
  const trigger = triggerOf(workflow);
  const inputs = Object.entries(workflow.inputs);

  const saveInput = (name: string, value: InputValue | undefined) => {
    const problems =
      value === undefined
        ? [`Input "${name}" needs a value`]
        : inputValueProblems(
            { [name]: workflow.inputs[name] },
            { [name]: value },
          );
    if (value === undefined || problems.length)
      return setNotice(problems[0]);
    setNotice(undefined);
    void updateWorkflows(monoId, (list) => ({
      workflows: list.map((w) =>
        w.id === workflow.id
          ? { ...w, inputValues: { ...w.inputValues, [name]: value } }
          : w,
      ),
      result: undefined,
    })).catch((error) => {
      console.warn("Could not save the input", error);
      setNotice(error instanceof Error && error.message ? error.message : "Could not save the input.");
    });
  };

  return (
    <div
      className="flex min-h-0 flex-1 flex-col"
      style={{ "--mono-color": color } as CSSProperties}
      data-workflow-page={workflow.id}
    >
      <PageHeader title={workflow.name} onBack={onBack}>
        <IconButton label="Edit board" onClick={onEditBoard}>
          <Pencil className="size-3.5" strokeWidth={1.75} />
        </IconButton>
        <IconButton
          label={running ? "Already running" : starting ? "Starting" : "Run now"}
          disabled={running || starting}
          onClick={() =>
            workflow.needsReview
              ? onReview("run")
              : void actions.runNow(workflow.id).then(setNotice)
          }
        >
          <Zap className="size-3.5" strokeWidth={1.75} />
        </IconButton>
        {running ? (
          <IconButton label="Stop" onClick={() => void actions.stop(workflow.id)}>
            <Square className="size-3.5" strokeWidth={1.75} />
          </IconButton>
        ) : null}
        <IconButton
          label={workflow.enabled ? "Pause" : "Resume"}
          onClick={() =>
            workflow.needsReview && !workflow.enabled
              ? onReview("enable")
              : void actions.toggle(workflow.id).then(setNotice)
          }
        >
          {workflow.enabled ? (
            <Pause className="size-3.5" strokeWidth={1.75} />
          ) : (
            <Play className="size-3.5" strokeWidth={1.75} />
          )}
        </IconButton>
        <IconButton
          label="Export"
          onClick={() => void exportWorkflowToFile(workflow).then(setNotice)}
        >
          <Share className="size-3.5" strokeWidth={1.75} />
        </IconButton>
        <IconButton
          label="Remove"
          onClick={() =>
            void actions.remove(workflow.id).then((refused) => {
              if (refused) setNotice(refused);
              else onRemove();
            })
          }
        >
          <Trash2 className="size-3.5" strokeWidth={1.75} />
        </IconButton>
      </PageHeader>

      <div className="min-h-0 flex-1 overflow-y-auto overscroll-none">
        <WorkflowNotice>{notice}</WorkflowNotice>
        <dl className="flex flex-col gap-0.5 px-4 py-3">
          {workflow.needsReview ? (
            <Property label="Status">
              <NeedsReviewBadge />
            </Property>
          ) : null}
          <Property label="Runs">
            {workflow.enabled ? workflowTriggerLabel(workflow) : "Paused"}
          </Property>
          {starting ? (
            <Property label="Next">Starting…</Property>
          ) : workflow.enabled &&
            trigger?.type === "schedule" &&
            workflow.nextRunAt !== undefined ? (
            <Property label="Next">{when(workflow.nextRunAt)}</Property>
          ) : null}
        </dl>

        {workflow.description ? (
          <Section title="What it does">
            <p className="whitespace-pre-wrap px-2 text-[12px] leading-5 text-content/75">
              {workflow.description}
            </p>
          </Section>
        ) : null}

        {inputs.length ? (
          <Section title="Inputs">
            <dl className="flex flex-col gap-0.5 px-2">
              {inputs.map(([name, input]) => (
                <Property key={name} label={name}>
                  <InputField
                    name={name}
                    input={input}
                    value={workflow.inputValues[name]}
                    projects={projects}
                    onSave={(value) => saveInput(name, value)}
                  />
                </Property>
              ))}
            </dl>
          </Section>
        ) : null}

        <Section title="Recent runs">
          {runs && runs.length === 0 ? (
            <p className="px-2 py-3 text-[12px] text-content/40">
              It hasn't run yet.
            </p>
          ) : (
            <ul className="flex flex-col gap-px">
              {(runs ?? []).map((run) => (
                <RunRow key={run.id} run={run} onOpen={() => onOpenRun(run.id)} />
              ))}
            </ul>
          )}
        </Section>
      </div>
    </div>
  );
}

function RunRow({ run, onOpen }: { run: WorkflowRun; onOpen: () => void }) {
  const status = RUN_STATUS[run.status];
  return (
    <li>
      <button
        type="button"
        data-run={run.id}
        onClick={onOpen}
        className="flex w-full items-center gap-2.5 rounded-md px-2 py-2 text-left text-[12px] hover:bg-content/5"
      >
        <span className={`size-2 shrink-0 rounded-full ${status.dot}`} />
        <span className="min-w-0 flex-1 truncate text-content/85">
          {status.label}
        </span>
        <span className="shrink-0 tabular-nums text-content/40">
          {when(run.startedAt)}
          {" · "}
          {run.endedAt ? (
            duration(run.endedAt - run.startedAt)
          ) : run.status === "running" ? (
            <RunningFor since={run.startedAt} />
          ) : (
            "-"
          )}
        </span>
      </button>
    </li>
  );
}
