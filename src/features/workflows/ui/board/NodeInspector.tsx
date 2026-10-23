import { useRef, useState, type ReactNode } from "react";
import { IconButton } from "../../../../app/shell/TitleBar";
import { basename } from "../../../../platform/tauri/fs";
import { IS_WIN } from "../../../../platform/tauri/platform";
import { ChevronLeft, Plus, Trash2, X } from "../../../../shared/ui/icons";
import { SearchableSelect } from "../../../../shared/ui/SearchableSelect";
import type { MonoProject } from "../../../monos/model/mono";
import { ScheduleFields } from "../../../monos/ui/NewHabitPage";
import {
  isEffortSettingId,
  modelEffortSetting,
  preferredModelId,
  resolveModel,
} from "../../../sessions/model/models";
import { HARNESSES, type HarnessId } from "../../../sessions/model/session";
import { ModelPicker, ModelSettingRows } from "../../../sessions/ui/ModelPicker";
import {
  AutoTextarea,
  Hint,
  Property,
  Section,
  SwitchRow,
} from "../../../monos/ui/monoPanelParts";
import {
  MAX_TIMEOUT_MINUTES,
  type Checkout,
  type LoopSpec,
  type NodeId,
  type OutputField,
  type OutputFieldType,
  type Workflow,
  type WorkflowInput,
  type WorkflowNode,
} from "../../model/workflow";
import { fieldInReport } from "../../model/workflowReport";
import { inputValueProblems } from "../../model/workflowYaml";
import { FIELD, InputField } from "../WorkflowPage";
import { has, projectInputFor } from "./boardModel";
import { NODE_LABEL, nodeTitle } from "./boardNodes";

type InputMeta = Pick<Workflow, "inputs" | "inputValues">;
type InputValue = Workflow["inputValues"][string];

/** What a session starts with when its node leaves the model unset: the Mono's own choice. */
export type MonoModelDefault = {
  harness: HarnessId;
  model: string;
  modelSettings: Record<string, string>;
};

export type InspectorView = { kind: "node"; id: NodeId } | { kind: "settings" };

const TEXTAREA =
  "block w-full resize-none rounded-md bg-transparent px-2 py-1 text-[12px] leading-5 text-content/75 outline-none placeholder:text-content/35 hover:bg-content/5 focus:bg-content/5 focus:text-content/90";
const NAME_INPUT =
  "h-8 w-full rounded-md bg-transparent px-2 text-[13px] font-medium text-content outline-none placeholder:text-content/35 hover:bg-content/5 focus:bg-content/5";

const OUTPUT_TYPES: { value: OutputFieldType; label: string }[] = [
  { value: "string", label: "Text" },
  { value: "number", label: "Number" },
  { value: "boolean", label: "Yes / no" },
  { value: "list", label: "List" },
  { value: "object", label: "Object" },
];

const INPUT_TYPES: { value: WorkflowInput["type"]; label: string }[] = [
  { value: "project", label: "Project" },
  { value: "string", label: "Text" },
  { value: "number", label: "Number" },
  { value: "boolean", label: "On / off" },
];

const MAX_ROUNDS = Array.from({ length: 10 }, (_, i) => ({
  value: String(i + 1),
  label: `${i + 1}`,
}));

function TextProperty({
  label,
  value,
  placeholder,
  onChange,
}: {
  label: string;
  value: string;
  placeholder?: string;
  onChange: (value: string) => void;
}) {
  return (
    <Property label={label}>
      <input
        aria-label={label}
        value={value}
        placeholder={placeholder}
        spellCheck={false}
        onChange={(event) => onChange(event.target.value)}
        className={FIELD}
      />
    </Property>
  );
}

function NumberProperty({
  label,
  value,
  min,
  max,
  placeholder,
  onChange,
}: {
  label: string;
  value: number | undefined;
  min: number;
  max?: number;
  placeholder?: string;
  onChange: (value: number | undefined) => void;
}) {
  return (
    <Property label={label}>
      <input
        aria-label={label}
        type="number"
        min={min}
        max={max}
        value={value ?? ""}
        placeholder={placeholder}
        onChange={(event) => {
          const text = event.target.value;
          onChange(text === "" ? undefined : Number(text));
        }}
        className={`${FIELD} max-w-24 flex-none`}
      />
    </Property>
  );
}

function SelectProperty({
  label,
  value,
  options,
  onChange,
  placeholder,
}: {
  label: string;
  value: string;
  options: readonly { value: string; label: string }[];
  onChange: (value: string) => void;
  placeholder?: string;
}) {
  return (
    <Property label={label}>
      <SearchableSelect
        label={label}
        value={value}
        options={options}
        placeholder={placeholder}
        variant="pill"
        searchable={options.length > 8}
        onChange={onChange}
      />
    </Property>
  );
}

function LongText({
  label,
  value,
  placeholder,
  mono = false,
  rows = 3,
  onChange,
}: {
  label: string;
  value: string;
  placeholder: string;
  mono?: boolean;
  rows?: number;
  onChange: (value: string) => void;
}) {
  return (
    <Section title={label}>
      <AutoTextarea
        aria-label={label}
        value={value}
        rows={rows}
        placeholder={placeholder}
        spellCheck={mono ? false : undefined}
        onChange={(event) => onChange(event.target.value)}
        className={`${TEXTAREA} ${mono ? "font-mono text-[11px]" : ""}`}
      />
    </Section>
  );
}

const sessionsOf = (w: Workflow, except: NodeId) =>
  w.nodes.filter((n) => n.type === "session" && n.id !== except);

/** The project a `{{inputs.x}}` reference stands for, shown by its name. */
function ProjectPicker({
  workflow,
  node,
  projects,
  onNode,
}: {
  workflow: Workflow;
  node: Extract<WorkflowNode, { type: "session" | "command" }>;
  projects: readonly MonoProject[];
  onNode: (next: WorkflowNode, meta?: InputMeta) => void;
}) {
  const name = /^\{\{inputs\.([A-Za-z_][A-Za-z0-9_]*)\}\}$/.exec(node.project)?.[1];
  const path = name ? workflow.inputValues[name] : undefined;
  const current = typeof path === "string" ? path : "";
  const options = projects.map((p) => ({ value: p.path, label: p.name }));
  if (current && !options.some((o) => o.value === current))
    options.unshift({ value: current, label: basename(current) });
  const choose = (next: string) => {
    const label = options.find((o) => o.value === next)?.label ?? basename(next);
    const made = projectInputFor(workflow, next, label, name);
    let { inputs, inputValues } = made;
    // An input this node left, that nothing else mentions, goes with it.
    if (name && made.ref !== node.project && inputs[name]?.type === "project") {
      // Loop conditions mention inputs too, so the edges are searched along with the nodes.
      const rest = JSON.stringify({
        nodes: workflow.nodes.map((n) => (n.id === node.id ? { ...n, project: "" } : n)),
        edges: workflow.edges,
      });
      if (!rest.includes(`inputs.${name}`)) {
        const { [name]: _gone, ...keptInputs } = inputs;
        const { [name]: _value, ...keptValues } = inputValues;
        inputs = keptInputs;
        inputValues = keptValues;
      }
    }
    onNode({ ...node, project: made.ref }, { inputs, inputValues });
  };
  return (
    <Property label="Project">
      <SearchableSelect
        label="Project"
        value={current}
        options={options}
        placeholder="Choose a project"
        searchPlaceholder="Search projects…"
        variant="pill"
        searchable={options.length > 6}
        onChange={choose}
      />
    </Property>
  );
}

function OutputFields({
  fields,
  onChange,
}: {
  fields: Record<string, OutputField>;
  onChange: (fields: Record<string, OutputField>) => void;
}) {
  const entries = Object.entries(fields);
  const rename = (from: string, to: string) =>
    onChange(Object.fromEntries(entries.map(([k, v]) => [k === from ? to : k, v])));
  const set = (key: string, patch: Partial<OutputField>) =>
    onChange(Object.fromEntries(entries.map(([k, v]) => [k, k === key ? { ...v, ...patch } : v])));
  const add = () => {
    let name = "field";
    for (let n = 2; has(fields, name); n++) name = `field_${n}`;
    onChange({ ...fields, [name]: { type: "string" } });
  };
  return (
    <Section
      title="Output"
      action={
        <IconButton label="Add output field" onClick={add}>
          <Plus className="size-3.5" strokeWidth={1.75} />
        </IconButton>
      }
    >
      {entries.length === 0 ? (
        <p className="px-2 text-[12px] leading-5 text-content/40">
          Later steps can use what this session reports, as {"{{id.field}}"}.
        </p>
      ) : (
        <ul className="flex flex-col gap-1 px-1">
          {entries.map(([key, field]) => (
            <li key={key} className="flex items-center gap-1.5">
              <OutputName
                value={key}
                taken={(name) => name !== key && has(fields, name)}
                onCommit={(name) => rename(key, name)}
              />
              <SearchableSelect
                label={`Type of ${key}`}
                value={field.type}
                options={OUTPUT_TYPES}
                variant="pill"
                searchable={false}
                onChange={(type) => set(key, { type: type as OutputFieldType })}
              />
              <button
                type="button"
                aria-pressed={fieldInReport(field)}
                title="Include this field in the report"
                onClick={() => set(key, { inReport: !fieldInReport(field) })}
                className={`h-7 shrink-0 rounded-md px-2 text-[11px] ${
                  fieldInReport(field)
                    ? "bg-content/15 text-content"
                    : "text-content/45 hover:bg-content/8"
                }`}
              >
                In report
              </button>
              <IconButton
                label={`Remove ${key}`}
                onClick={() => {
                  const { [key]: _gone, ...rest } = fields;
                  onChange(rest);
                }}
              >
                <X className="size-3.5" strokeWidth={1.75} />
              </IconButton>
            </li>
          ))}
        </ul>
      )}
    </Section>
  );
}

/** A field name, kept as typed and applied on blur once it is a usable name. */
function OutputName({
  value,
  taken,
  onCommit,
}: {
  value: string;
  taken: (name: string) => boolean;
  onCommit: (name: string) => void;
}) {
  const [text, setText] = useState(value);
  const [shown, setShown] = useState(value);
  if (shown !== value) {
    setShown(value);
    setText(value);
  }
  const usable = (name: string) => /^[A-Za-z_][A-Za-z0-9_]*$/.test(name) && name !== "__proto__" && !taken(name);
  return (
    <input
      aria-label={`Name of ${value}`}
      value={text}
      spellCheck={false}
      aria-invalid={!usable(text) || undefined}
      onChange={(event) => setText(event.target.value)}
      onBlur={() => (usable(text) && text !== value ? onCommit(text) : setText(value))}
      onKeyDown={(event) => {
        if (event.key === "Enter") event.currentTarget.blur();
      }}
      className={`${FIELD} font-mono text-[11px]`}
    />
  );
}

function Answers({
  answers,
  onChange,
}: {
  answers: string[];
  onChange: (answers: string[]) => void;
}) {
  return (
    <Section
      title="Answers"
      action={
        <IconButton label="Add answer" onClick={() => onChange([...answers, ""])}>
          <Plus className="size-3.5" strokeWidth={1.75} />
        </IconButton>
      }
    >
      <ul className="flex flex-col gap-1 px-1">
        {answers.map((answer, index) => (
          // The list has no ids; an index is the identity of a row being typed in.
          <li key={index} className="flex items-center gap-1.5">
            <input
              aria-label={`Answer ${index + 1}`}
              value={answer}
              spellCheck={false}
              onChange={(event) =>
                onChange(answers.map((a, i) => (i === index ? event.target.value : a)))
              }
              className={FIELD}
            />
            <IconButton
              label={`Remove answer ${index + 1}`}
              onClick={() => onChange(answers.filter((_, i) => i !== index))}
            >
              <X className="size-3.5" strokeWidth={1.75} />
            </IconButton>
          </li>
        ))}
      </ul>
    </Section>
  );
}

/** How many rounds a loop may take and who decides to go round again. */
export function LoopForm({
  loop,
  onChange,
}: {
  loop: LoopSpec;
  onChange: (loop: LoopSpec) => void;
}) {
  return (
    <dl className="flex flex-col gap-0.5">
      <SelectProperty
        label="Max rounds"
        value={String(loop.maxRounds)}
        options={MAX_ROUNDS}
        onChange={(value) => onChange({ ...loop, maxRounds: Number(value) })}
      />
      <SelectProperty
        label="Decided by"
        value={loop.decide === "mono" ? "mono" : "condition"}
        options={[
          { value: "mono", label: "Mono decides" },
          { value: "condition", label: "A condition" },
        ]}
        onChange={(value) =>
          onChange({ ...loop, decide: value === "mono" ? "mono" : loop.decide === "mono" ? "" : loop.decide })
        }
      />
      {loop.decide === "mono" ? null : (
        <TextProperty
          label="Condition"
          value={loop.decide}
          placeholder='{{review.verdict}} != "approve"'
          onChange={(decide) => onChange({ ...loop, decide })}
        />
      )}
    </dl>
  );
}

function checkoutValue(checkout: Checkout | undefined): string {
  return typeof checkout === "object" ? `worktree:${checkout.worktreeOf}` : (checkout ?? "root");
}

const isHarness = (value: string | undefined): value is HarnessId =>
  HARNESSES.includes(value as HarnessId);

/**
 * A session's model, picked the way the composer and the Mono's settings do. A node that sets
 * nothing runs with the Mono's own model, which is what the picker shows until something is picked.
 */
function SessionModel({
  node,
  monoModel,
  onNode,
}: {
  node: Extract<WorkflowNode, { type: "session" }>;
  monoModel: MonoModelDefault | undefined;
  onNode: (next: WorkflowNode) => void;
}) {
  const fallback = monoModel ?? {
    harness: HARNESSES[0],
    model: preferredModelId(HARNESSES[0]),
    modelSettings: {},
  };
  // The same fallbacks the host applies when it starts the session.
  const harness = isHarness(node.harness) ? node.harness : fallback.harness;
  const model =
    node.model ?? (harness === fallback.harness ? fallback.model : preferredModelId(harness));
  const sameAsMono = harness === fallback.harness && model === fallback.model;
  const effortId = modelEffortSetting(resolveModel(harness, model))?.id;
  const values = {
    ...(sameAsMono ? fallback.modelSettings : {}),
    ...(node.effort && effortId ? { [effortId]: node.effort } : {}),
  };
  const { harness: _h, model: _m, effort: _e, ...rest } = node;
  const unset = !node.harness && !node.model && !node.effort;
  return (
    <>
      <Property label="Model">
        <span className="flex min-w-0 flex-1 flex-col items-start">
          <ModelPicker
            harness={harness}
            model={model}
            values={values}
            hideSettings
            side="bottom"
            variant="plain"
            // A new model has its own effort levels, so the old choice does not carry over.
            onChange={(nextHarness, nextModel) =>
              onNode({ ...rest, harness: nextHarness, model: nextModel })
            }
            onSettingsChange={() => {}}
          />
          {unset ? (
            <span className="pb-0.5 text-[11px] text-content/35">Mono default</span>
          ) : (
            <button
              type="button"
              onClick={() => onNode(rest)}
              className="-ml-1.5 rounded px-1.5 py-0.5 text-[11px] text-content/55 hover:bg-content/8 hover:text-content"
            >
              Use Mono default
            </button>
          )}
        </span>
      </Property>
      <ModelSettingRows
        harness={harness}
        model={model}
        values={values}
        side="bottom"
        onSettingsChange={(settings) => {
          const effort = effortId ? settings[effortId] : undefined;
          // An effort belongs to one model, so choosing it fixes the model it was chosen for.
          if (effort) onNode({ ...rest, harness, model, effort });
        }}
        row={({ id, label, control }) =>
          isEffortSettingId(id) ? (
            <Property key={id} label={label}>
              {control}
            </Property>
          ) : null
        }
      />
    </>
  );
}

function NodeFields({
  workflow,
  node,
  projects,
  monoModel,
  onNode,
}: {
  workflow: Workflow;
  node: WorkflowNode;
  projects: readonly MonoProject[];
  monoModel: MonoModelDefault | undefined;
  onNode: (next: WorkflowNode, meta?: InputMeta) => void;
}) {
  const change = (patch: Partial<WorkflowNode>) => onNode({ ...node, ...patch } as WorkflowNode);
  const worktrees = (except: NodeId) =>
    sessionsOf(workflow, except).map((s) => ({
      value: `worktree:${s.id}`,
      label: `Worktree of ${nodeTitle(s)}`,
    }));
  switch (node.type) {
    case "schedule":
      return (
        <dl className="flex flex-col gap-0.5 px-4 py-3">
          <Property label="Runs">
            <ScheduleFields schedule={node.schedule} onChange={(schedule) => change({ schedule })} />
          </Property>
        </dl>
      );
    case "manual":
      return (
        <p className="px-4 py-3 text-[12px] leading-5 text-content/55">
          Starts when you use Run now, or when the Mono runs it. Values it needs are set under
          Workflow settings.
        </p>
      );
    case "session":
      return (
        <>
          <dl className="flex flex-col gap-0.5 px-4 py-3">
            <ProjectPicker workflow={workflow} node={node} projects={projects} onNode={onNode} />
            <SessionModel node={node} monoModel={monoModel} onNode={onNode} />
            <SelectProperty
              label="Checkout"
              value={checkoutValue(node.checkout)}
              options={[
                { value: "root", label: "Project folder" },
                { value: "newWorktree", label: "New worktree" },
                ...worktrees(node.id),
              ]}
              onChange={(value) =>
                change({
                  checkout: value.startsWith("worktree:")
                    ? { worktreeOf: value.slice("worktree:".length) }
                    : (value as "root" | "newWorktree"),
                })
              }
            />
            <SelectProperty
              label="Nest under"
              value={node.nestUnder ?? ""}
              options={[
                { value: "", label: "Nothing" },
                ...sessionsOf(workflow, node.id).map((s) => ({ value: s.id, label: nodeTitle(s) })),
              ]}
              onChange={(value) => {
                const { nestUnder: _old, ...rest } = node;
                onNode(value ? { ...rest, nestUnder: value } : rest);
              }}
            />
            <NumberProperty
              label="Time limit"
              min={1}
              max={MAX_TIMEOUT_MINUTES}
              value={node.timeoutMinutes}
              placeholder="120 min"
              onChange={(timeoutMinutes) => {
                const { timeoutMinutes: _old, ...rest } = node;
                onNode(timeoutMinutes === undefined ? rest : { ...rest, timeoutMinutes });
              }}
            />
          </dl>
          {workflow.edges.some((e) => e.loop && e.from === node.id) ? (
            <div className="px-2">
              <SwitchRow
                label="Reuse the session"
                description="Keep one session across the rounds of its loop"
                on={node.reuseSession !== false}
                onChange={(on) => change({ reuseSession: on })}
              />
            </div>
          ) : null}
          <LongText
            label="Prompt"
            value={node.prompt}
            placeholder="What the session should do. Use {{inputs.x}} and {{node.field}}."
            onChange={(prompt) => change({ prompt })}
          />
          <OutputFields fields={node.output} onChange={(output) => change({ output })} />
        </>
      );
    case "command":
      return (
        <>
          <dl className="flex flex-col gap-0.5 px-4 py-3">
            <ProjectPicker workflow={workflow} node={node} projects={projects} onNode={onNode} />
            <SelectProperty
              label="Runs in"
              value={node.cwd === undefined ? "root" : checkoutValue(node.cwd)}
              options={[{ value: "root", label: "Project folder" }, ...worktrees(node.id)]}
              onChange={(value) => {
                const { cwd: _old, ...rest } = node;
                onNode(
                  value.startsWith("worktree:")
                    ? { ...rest, cwd: { worktreeOf: value.slice("worktree:".length) } }
                    : rest,
                );
              }}
            />
            <NumberProperty
              label="Time limit"
              min={1}
              max={MAX_TIMEOUT_MINUTES}
              value={node.timeoutMinutes}
              placeholder="10 min"
              onChange={(timeoutMinutes) => {
                const { timeoutMinutes: _old, ...rest } = node;
                onNode(timeoutMinutes === undefined ? rest : { ...rest, timeoutMinutes });
              }}
            />
          </dl>
          <LongText
            label="Command"
            mono
            value={node.command}
            placeholder="npm test"
            onChange={(command) => change({ command })}
          />
          <Hint>Fills in from {"{{ }}"} are quoted for the shell. It reports exitCode, stdout and stderr.</Hint>
          {IS_WIN ? (
            <Hint>Command nodes are not supported on Windows yet, so this step will fail when it runs.</Hint>
          ) : null}
        </>
      );
    case "askMono":
      return (
        <>
          <LongText
            label="Question"
            value={node.question}
            placeholder="What the Mono should decide. Use {{node.field}} to include results."
            onChange={(question) => change({ question })}
          />
          <Answers answers={node.answers} onChange={(answers) => change({ answers })} />
        </>
      );
    case "if":
      return (
        <>
          <dl className="flex flex-col gap-0.5 px-4 py-3">
            <SelectProperty
              label="Decided by"
              value={node.decide === "mono" ? "mono" : "condition"}
              options={[
                { value: "condition", label: "A condition" },
                { value: "mono", label: "Mono decides" },
              ]}
              onChange={(value) => {
                const { decide: _d, condition: _c, ...rest } = node;
                onNode(value === "mono" ? { ...rest, decide: "mono" } : { ...rest, condition: "" });
              }}
            />
            {node.decide === "mono" ? null : (
              <TextProperty
                label="Condition"
                value={node.condition ?? ""}
                placeholder='{{review.verdict}} == "approve"'
                onChange={(condition) => change({ condition })}
              />
            )}
          </dl>
          {node.decide === "mono" ? (
            <LongText
              label="Question"
              value={node.question ?? ""}
              placeholder="What the Mono should check before choosing true or false."
              onChange={(question) => {
                const { question: _q, ...rest } = node;
                onNode(question ? { ...rest, question } : rest);
              }}
            />
          ) : (
            <Hint>Operators: ==, !=, contains, empty, not empty.</Hint>
          )}
        </>
      );
    case "forEach":
      return (
        <>
          <dl className="flex flex-col gap-0.5 px-4 py-3">
            <TextProperty
              label="List"
              value={node.in}
              placeholder="{{fetch.tickets}}"
              onChange={(value) => change({ in: value })}
            />
            <TextProperty
              label="Each as"
              value={node.as ?? ""}
              placeholder="item"
              onChange={(value) => {
                const { as: _a, ...rest } = node;
                onNode(value ? { ...rest, as: value } : rest);
              }}
            />
            <NumberProperty
              label="At once"
              min={1}
              max={10}
              value={node.maxParallel}
              placeholder="3"
              onChange={(maxParallel) => {
                const { maxParallel: _m, ...rest } = node;
                onNode(maxParallel === undefined ? rest : { ...rest, maxParallel });
              }}
            />
          </dl>
          <Hint>Drag nodes into the frame on the board to run them once per item.</Hint>
        </>
      );
    case "report":
      return (
        <>
          <LongText
            label="Report"
            value={node.template ?? ""}
            rows={4}
            placeholder="Leave empty for a summary of every branch, or write your own with {{node.field}}."
            onChange={(template) => {
              const { template: _t, ...rest } = node;
              onNode(template ? { ...rest, template } : rest);
            }}
          />
        </>
      );
  }
}

/** The slide-over header: back to the board, what is open, and its actions. */
function InspectorHeader({
  title,
  onBack,
  children,
}: {
  title: string;
  onBack: () => void;
  children?: ReactNode;
}) {
  return (
    <header
      className="flex h-10 shrink-0 items-center gap-1 border-b border-stroke pl-1.5 pr-3"
      data-tauri-drag-region="deep"
    >
      <button
        type="button"
        onClick={onBack}
        data-tauri-drag-region="false"
        className="flex h-6 items-center gap-1 rounded-md pl-1 pr-2 text-[12px] text-content/60 hover:bg-content/5 hover:text-content"
      >
        <ChevronLeft className="size-3.5" strokeWidth={1.75} />
        Back to board
      </button>
      <span className="min-w-0 flex-1 truncate text-right text-[11px] text-content/40">{title}</span>
      {children}
    </header>
  );
}

function AddInput({
  taken,
  onAdd,
}: {
  taken: (name: string) => boolean;
  onAdd: (name: string, type: WorkflowInput["type"]) => void;
}) {
  const [name, setName] = useState("");
  const [type, setType] = useState<WorkflowInput["type"]>("string");
  const problem = !name
    ? undefined
    : !/^[A-Za-z_][A-Za-z0-9_]*$/.test(name) || name === "__proto__"
      ? "Use letters, digits and underscores"
      : taken(name)
        ? "Already declared"
        : undefined;
  const submit = () => {
    if (!name || problem) return;
    onAdd(name, type);
    setName("");
  };
  return (
    <div className="px-2 pt-2">
      <div className="flex items-center gap-1.5 px-1">
        <input
          aria-label="New input name"
          value={name}
          placeholder="New input"
          spellCheck={false}
          onChange={(event) => setName(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter") submit();
          }}
          className={FIELD}
        />
        <SearchableSelect
          label="New input type"
          value={type}
          options={INPUT_TYPES}
          variant="pill"
          searchable={false}
          onChange={(value) => setType(value as WorkflowInput["type"])}
        />
        <IconButton label="Add input" disabled={!name || !!problem} onClick={submit}>
          <Plus className="size-3.5" strokeWidth={1.75} />
        </IconButton>
      </div>
      {problem ? <p className="px-2 pt-1 text-[11px] text-red-500/80">{problem}</p> : null}
    </div>
  );
}

function SettingsFields({
  workflow,
  projects,
  onSettings,
}: {
  workflow: Workflow;
  projects: readonly MonoProject[];
  onSettings: (
    patch: Partial<Pick<Workflow, "description" | "enabled" | "inputs" | "inputValues">>,
  ) => void;
}) {
  const [problem, setProblem] = useState<string>();
  const inputs = Object.entries(workflow.inputs);
  const save = (name: string, value: InputValue | undefined) => {
    if (value === undefined) return setProblem(`Input "${name}" needs a value`);
    const found = inputValueProblems({ [name]: workflow.inputs[name] }, { [name]: value });
    if (found.length) return setProblem(found[0]);
    setProblem(undefined);
    onSettings({ inputValues: { ...workflow.inputValues, [name]: value } });
  };
  const remove = (name: string) => {
    const { [name]: _input, ...keptInputs } = workflow.inputs;
    const { [name]: _value, ...keptValues } = workflow.inputValues;
    onSettings({ inputs: keptInputs, inputValues: keptValues });
  };
  return (
    <>
      <div className="px-2 pt-1">
        <SwitchRow
          label="Enabled"
          description="Runs on its trigger. Every problem has to be fixed first"
          on={workflow.enabled}
          onChange={(enabled) => onSettings({ enabled })}
        />
      </div>
      <LongText
        label="What it does"
        value={workflow.description ?? ""}
        placeholder="A line on what this workflow is for."
        onChange={(description) => onSettings({ description })}
      />
      <Section title="Inputs">
        {inputs.length === 0 ? (
          <p className="px-2 text-[12px] leading-5 text-content/40">
            Values the steps use as {"{{inputs.name}}"}. A project you pick for a step is added here.
          </p>
        ) : (
          <dl className="flex flex-col gap-0.5 px-2">
            {inputs.map(([name, input]) => (
              <Property key={name} label={name}>
                <span className="flex min-w-0 flex-1 items-center gap-1">
                  <InputField
                    name={name}
                    input={input}
                    value={workflow.inputValues[name]}
                    projects={projects}
                    onSave={(value) => save(name, value)}
                  />
                  <IconButton label={`Remove ${name}`} onClick={() => remove(name)}>
                    <X className="size-3.5" strokeWidth={1.75} />
                  </IconButton>
                </span>
              </Property>
            ))}
          </dl>
        )}
        {problem ? <p className="px-2 pt-2 text-[12px] text-red-500/80">{problem}</p> : null}
        <AddInput
          taken={(name) => has(workflow.inputs, name)}
          onAdd={(name, type) => onSettings({ inputs: { ...workflow.inputs, [name]: { type } } })}
        />
      </Section>
    </>
  );
}

/**
 * The page that slides over the board: one node's setup, or the workflow's own settings.
 * It keeps showing what it last showed while it slides away.
 */
export function NodeInspector({
  view,
  workflow,
  projects,
  monoModel,
  issues,
  onNode,
  onSettings,
  onLoop,
  onRemove,
  onBack,
}: {
  view: InspectorView | null;
  workflow: Workflow;
  projects: readonly MonoProject[];
  /** The Mono's own model, which a session without one of its own starts with. */
  monoModel?: MonoModelDefault;
  /** The messages for the open node. */
  issues: string[];
  onNode: (next: WorkflowNode, meta?: InputMeta) => void;
  onSettings: (
    patch: Partial<Pick<Workflow, "description" | "enabled" | "inputs" | "inputValues">>,
  ) => void;
  /** Changes the loop that leaves a node, or takes it away with null. */
  onLoop: (id: NodeId, loop: LoopSpec | null) => void;
  onRemove: (id: NodeId) => void;
  onBack: () => void;
}) {
  const last = useRef<InspectorView | null>(null);
  if (view) last.current = view;
  const shown = view ?? last.current;
  const node =
    shown?.kind === "node" ? workflow.nodes.find((n) => n.id === shown.id) : undefined;
  const loopOut = node ? workflow.edges.find((e) => e.loop && e.from === node.id) : undefined;
  return (
    <div
      data-board-inspector
      data-open={view ? "true" : "false"}
      inert={view ? undefined : true}
      aria-hidden={view ? undefined : true}
      className="board-inspector absolute inset-0 z-10 flex flex-col"
    >
      {shown?.kind === "settings" ? (
        <>
          <InspectorHeader title="Workflow settings" onBack={onBack} />
          <div className="min-h-0 flex-1 overflow-y-auto overscroll-none">
            <SettingsFields workflow={workflow} projects={projects} onSettings={onSettings} />
          </div>
        </>
      ) : node ? (
        <>
          <InspectorHeader title={NODE_LABEL[node.type]} onBack={onBack}>
            <IconButton label="Delete node" onClick={() => onRemove(node.id)}>
              <Trash2 className="size-3.5" strokeWidth={1.75} />
            </IconButton>
          </InspectorHeader>
          <div className="min-h-0 flex-1 overflow-y-auto overscroll-none">
            <div className="px-2 pt-3">
              <input
                aria-label="Title"
                value={node.title ?? ""}
                maxLength={80}
                placeholder={nodeTitle({ ...node, title: undefined })}
                onChange={(event) => {
                  const { title: _t, ...rest } = node;
                  onNode(event.target.value ? { ...rest, title: event.target.value } : rest);
                }}
                className={NAME_INPUT}
              />
              {node.type === "session" || node.type === "command" || node.type === "askMono" ? (
                <p className="px-2 pt-0.5 text-[11px] leading-4 text-content/35">
                  Later steps use its results as {`{{${node.id}.name}}`}.
                </p>
              ) : null}
            </div>
            {issues.length ? (
              <ul className="flex flex-col gap-1 px-4 pt-2">
                {issues.map((message) => (
                  <li
                    key={message}
                    className="flex items-start gap-1.5 text-[11px] leading-4 text-red-500/80"
                  >
                    <span aria-hidden className="mt-1.5 size-1.5 shrink-0 rounded-full bg-red-500/70" />
                    {message}
                  </li>
                ))}
              </ul>
            ) : null}
            <NodeFields
              workflow={workflow}
              node={node}
              projects={projects}
              monoModel={monoModel}
              onNode={onNode}
            />
            {loopOut ? (
              <Section
                title={`Loops back to ${nodeTitle(workflow.nodes.find((n) => n.id === loopOut.to) ?? node)}`}
                action={
                  <button
                    type="button"
                    onClick={() => onLoop(node.id, null)}
                    className="rounded px-1.5 py-0.5 text-[11px] text-content/60 hover:bg-content/8 hover:text-content"
                  >
                    Remove loop
                  </button>
                }
              >
                <div className="px-2">
                  <LoopForm loop={loopOut.loop as LoopSpec} onChange={(loop) => onLoop(node.id, loop)} />
                </div>
              </Section>
            ) : null}
          </div>
        </>
      ) : null}
    </div>
  );
}
