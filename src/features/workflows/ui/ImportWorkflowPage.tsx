import {
  useDeferredValue,
  useMemo,
  useState,
  type CSSProperties,
} from "react";
import type { MonoProject } from "../../monos/model/mono";
import {
  AutoTextarea,
  Hint,
  PageHeader,
  Property,
  Section,
} from "../../monos/ui/monoPanelParts";
import { WORKFLOWS_MAX, type WorkflowInput } from "../model/workflow";
import { updateWorkflows } from "../model/workflowStore";
import {
  MAX_FILE_BYTES,
  anchorNode,
  finishImport,
  inputValueProblems,
  previewImport,
  type ImportPreview,
} from "../model/workflowYaml";
import { pickWorkflowFile } from "./workflowImportFile";
import { FIELD, InputField } from "./WorkflowPage";
import { ERROR, visible, WorkflowReviewSections } from "./WorkflowReviewSections";

/**
 * What the user typed or chose for each input, before it is read as a value, with the type
 * it was entered for: a file that declares the input differently does not inherit it.
 */
type Draft = Record<string, { type: WorkflowInput["type"]; value: string | boolean }>;

const BUTTON =
  "h-6 rounded-md bg-content/10 px-2 text-[12px] text-content enabled:hover:bg-content/[0.14] disabled:opacity-40";
const TEXT_AREA =
  "block w-full resize-none rounded-md bg-transparent px-2 py-1 font-mono text-[11px] leading-4 text-content/75 outline-none placeholder:font-sans placeholder:text-[12px] placeholder:text-content/35 hover:bg-content/5 focus:bg-content/5 focus:text-content/90";

/** What the user typed or chose, as the values the workflow takes. */
function valuesOf(
  inputs: Record<string, WorkflowInput>,
  draft: Draft,
): Record<string, string | number | boolean> {
  const values: Record<string, string | number | boolean> = {};
  for (const [name, input] of Object.entries(inputs)) {
    const entry = draft[name];
    if (entry === undefined || entry.type !== input.type) continue;
    const { value } = entry;
    if (input.type !== "number") values[name] = value;
    else if (typeof value === "string" && value.trim() !== "")
      values[name] = Number(value);
  }
  return values;
}

/** One input to fill: text and numbers update as they are typed, so the button follows. */
function ImportInput({
  name,
  input,
  value,
  raw,
  projects,
  onChange,
}: {
  name: string;
  input: WorkflowInput;
  value: string | number | boolean | undefined;
  raw: string | boolean | undefined;
  projects: readonly MonoProject[];
  onChange: (value: string | boolean | undefined) => void;
}) {
  // What the file names these with is shown as it is, hidden characters made visible.
  const shown = visible(name);
  const described: WorkflowInput =
    input.description === undefined ? input : { ...input, description: visible(input.description) };
  if (input.type === "project" || input.type === "boolean")
    return (
      <InputField
        name={shown}
        input={described}
        value={value}
        projects={projects}
        onSave={(next) => onChange(next as string | boolean | undefined)}
      />
    );
  return (
    <input
      aria-label={shown}
      value={typeof raw === "string" ? raw : ""}
      inputMode={input.type === "number" ? "decimal" : undefined}
      placeholder={described.description ?? shown}
      spellCheck={false}
      onChange={(event) => onChange(event.target.value)}
      className={FIELD}
    />
  );
}

/**
 * Brings in a workflow file: the user reads what it would run and fills its inputs, and it is
 * saved paused. Everything that reaches an agent or the shell is shown as the file has it.
 */
export function ImportWorkflowPage({
  monoId,
  color,
  projects,
  workflowCount,
  onBack,
  onImported,
}: {
  monoId: string;
  /** The Mono's color. */
  color: string;
  /** The Mono's projects, for inputs that name one. */
  projects: readonly MonoProject[];
  /** How many workflows the Mono has now, when known. */
  workflowCount: number | undefined;
  onBack: () => void;
  /** Called with the new workflow's id once it is saved. */
  onImported: (workflowId: string) => void;
}) {
  const [text, setText] = useState("");
  const [fileError, setFileError] = useState<string>();
  const [draft, setDraft] = useState<Draft>({});
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string>();
  // Typing in the box does not wait for a large file to be read back.
  const deferred = useDeferredValue(text);
  const read = useMemo(
    () => (deferred.trim() ? previewImport(deferred) : undefined),
    [deferred],
  );
  const preview = read?.ok ? read.preview : undefined;

  const change = (next: string) => {
    setText(next);
    setFileError(undefined);
    setError(undefined);
  };
  const choose = async () => {
    const picked = await pickWorkflowFile();
    if (!picked) return;
    if ("error" in picked) return setFileError(picked.error);
    if (new TextEncoder().encode(picked.text).length > MAX_FILE_BYTES) {
      // The model's own refusal; the text itself stays out of the page.
      const refused = previewImport(picked.text);
      change("");
      return setFileError(refused.ok ? "Workflow file is too large" : refused.error);
    }
    change(picked.text);
  };

  const inputs = preview?.workflow.inputs ?? {};
  const names = Object.keys(inputs);
  const values = valuesOf(inputs, draft);
  const problems = preview ? inputValueProblems(inputs, values) : [];
  const full = workflowCount !== undefined && workflowCount >= WORKFLOWS_MAX;
  const limit = `A Mono can have at most ${WORKFLOWS_MAX} workflows`;
  const noProject =
    projects.length === 0 &&
    Object.values(inputs).some((input) => input.type === "project");
  // The preview must be of the text in the box, not of what it said a moment ago.
  const ready =
    !!preview && deferred === text && problems.length === 0 && !full && !saving;

  const save = (reviewed: ImportPreview) => {
    if (!ready) return;
    setSaving(true);
    setError(undefined);
    const now = Date.now();
    void (async () => {
      try {
        const finished = finishImport(reviewed, values, now);
        const created = {
          ...finished,
          nodes: finished.nodes.map((node) => anchorNode(node, now)),
          // It is read again, and approved, before it can run or be enabled.
          needsReview: true,
        };
        await updateWorkflows(monoId, (list) => {
          if (list.length >= WORKFLOWS_MAX) throw new Error(limit);
          return { workflows: [...list, created], result: undefined };
        });
        onImported(created.id);
      } catch (reason) {
        console.warn("Could not import the workflow", reason);
        setSaving(false);
        setError(
          reason instanceof Error && reason.message === limit
            ? `${limit}.`
            : "Could not save the workflow.",
        );
      }
    })();
  };

  return (
    <div
      className="flex min-h-0 flex-1 flex-col"
      style={{ "--mono-color": color } as CSSProperties}
      data-import-workflow
    >
      <PageHeader title="Import workflow" onBack={onBack}>
        <button
          type="button"
          data-import-paused
          data-tauri-drag-region="false"
          disabled={!ready}
          onClick={() => preview && save(preview)}
          className={BUTTON}
        >
          Import paused
        </button>
      </PageHeader>

      <div className="min-h-0 flex-1 overflow-y-auto overscroll-none">
        <Section
          title="Workflow file"
          action={
            <button
              type="button"
              data-choose-file
              onClick={() => void choose()}
              className="rounded px-1.5 py-0.5 text-[11px] text-content/60 hover:bg-content/8 hover:text-content"
            >
              Choose file
            </button>
          }
        >
          <AutoTextarea
            aria-label="Workflow file text"
            value={text}
            rows={3}
            maxHeight={160}
            spellCheck={false}
            placeholder="Or paste the text of a workflow file here."
            onChange={(event) => change(event.target.value)}
            className={TEXT_AREA}
          />
          {fileError ? (
            <p role="alert" className={`px-2 pt-2 text-[12px] ${ERROR}`}>
              {fileError}
            </p>
          ) : null}
          {read && !read.ok ? (
            <p role="alert" className={`px-2 pt-2 text-[12px] leading-5 ${ERROR}`}>
              {read.error}
            </p>
          ) : null}
          {!text.trim() && !fileError ? (
            <Hint>
              Nothing is saved until you import, and an imported workflow starts
              paused.
            </Hint>
          ) : null}
        </Section>

        {preview ? (
          <>
            <WorkflowReviewSections
              preview={preview}
              problemsHint="You can still import it. It stays paused until these are fixed."
            />

            {names.length ? (
              <Section title="Inputs">
                <dl className="flex flex-col gap-0.5 px-2">
                  {names.map((name) => (
                    <Property key={name} label={visible(name)}>
                      <span className="flex min-w-0 flex-1 flex-col gap-1 py-0.5">
                        <ImportInput
                          name={name}
                          input={inputs[name]}
                          value={values[name]}
                          raw={
                            draft[name]?.type === inputs[name].type
                              ? draft[name].value
                              : undefined
                          }
                          projects={projects}
                          onChange={(next) =>
                            setDraft((now) => {
                              const { [name]: _gone, ...rest } = now;
                              return next === undefined
                                ? rest
                                : {
                                    ...rest,
                                    [name]: { type: inputs[name].type, value: next },
                                  };
                            })
                          }
                        />
                        {inputs[name].description &&
                        (inputs[name].type === "project" ||
                          inputs[name].type === "boolean") ? (
                          <span className="text-[11px] leading-4 text-content/35">
                            {inputs[name].description}
                          </span>
                        ) : null}
                      </span>
                    </Property>
                  ))}
                </dl>
              </Section>
            ) : null}

            <div className="border-t border-stroke px-2 pb-4" data-import-status>
              {error ? (
                <p role="alert" className={`px-2 pt-3 text-[12px] ${ERROR}`}>
                  {error}
                </p>
              ) : null}
              {full ? (
                <Hint>{limit}. Remove one to import this.</Hint>
              ) : noProject ? (
                <Hint>Add a project to this Mono first, to choose it here.</Hint>
              ) : problems.length ? (
                <Hint>Fill every input to import. {problems[0]}.</Hint>
              ) : null}
            </div>
          </>
        ) : null}
      </div>
    </div>
  );
}
