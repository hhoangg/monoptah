import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent,
} from "react";
import { Loader, Plus, X } from "../../../shared/ui/icons";
import { Modal } from "../../../shared/ui/Modal";
import { SecondaryButton } from "../../../shared/ui/SecondaryButton";
import { Select } from "../../settings/ui/primitives";
import {
  DEFAULT_ASK_OPTIONS,
  askContextChips,
  askSubtitle,
  buildAskRequest,
  personNameOf,
} from "../model/askAgent";
import type {
  AskAgentRequest,
  AskContextPart,
  AskScope,
} from "../model/askAgent";
import type { ContributorsModel } from "../model/pipeline";
import type { HarnessId } from "../../sessions/model/session";
import { snapshotFileName, suggestedQuestions } from "../model/snapshot";

export type AskProvider = { id: HarnessId; label: string };

export type AskAgentDialogProps = {
  model: ContributorsModel;
  /** Local project path; only its folder name is sent. */
  project: string;
  scope: AskScope;
  /** Providers the user can pick; with none the footer shows no agent control. */
  providers?: AskProvider[];
  /** Provider selected first; falls back to the first of `providers`. */
  defaultProvider?: HarnessId;
  /** Resolves once the session is started; a rejection is shown in the dialog. */
  onStart: (request: AskAgentRequest) => Promise<void>;
  onClose: () => void;
};

const CHECKBOX =
  "size-3.5 shrink-0 rounded accent-accent focus-visible:outline-2 focus-visible:outline-accent";

function errorText(e: unknown): string {
  const text = e instanceof Error ? e.message : String(e ?? "");
  return text.trim() || "Could not start the session.";
}

export function AskAgentDialog({
  model,
  project,
  scope,
  providers = [],
  defaultProvider,
  onStart,
  onClose,
}: AskAgentDialogProps) {
  const [question, setQuestion] = useState("");
  const [perPr, setPerPr] = useState(DEFAULT_ASK_OPTIONS.perPr);
  const [rawCommits, setRawCommits] = useState(DEFAULT_ASK_OPTIONS.rawCommits);
  const [allowTools, setAllowTools] = useState(DEFAULT_ASK_OPTIONS.allowTools);
  const [omit, setOmit] = useState<AskContextPart[]>([]);
  const [harness, setHarness] = useState(
    providers.find((p) => p.id === defaultProvider)?.id ?? providers[0]?.id,
  );
  const [starting, setStarting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const textarea = useRef<HTMLTextAreaElement>(null);
  // The page may unmount this dialog before the start promise settles.
  const alive = useRef(true);

  useEffect(() => {
    alive.current = true;
    // The modal focuses its close button after mount; typing comes first here.
    const timer = setTimeout(() => textarea.current?.focus(), 0);
    return () => {
      alive.current = false;
      clearTimeout(timer);
    };
  }, []);

  const name = personNameOf(model, scope);
  const chips = useMemo(
    () => askContextChips(model, project, scope),
    [model, project, scope],
  );
  const suggestions = useMemo(
    () => suggestedQuestions(scope, name),
    [scope, name],
  );
  const canStart = question.trim().length > 0 && !starting;

  const toggleChip = (id: AskContextPart) =>
    setOmit((prev) =>
      prev.includes(id) ? prev.filter((p) => p !== id) : [...prev, id],
    );

  const start = async () => {
    if (!canStart) return;
    setStarting(true);
    setError(null);
    try {
      // Built now, not on open, so it matches the data on screen at this moment.
      const request = buildAskRequest({
        model,
        project,
        scope,
        question,
        options: { perPr, rawCommits, allowTools, omit },
        harness,
        now: new Date(),
      });
      await onStart(request);
      if (alive.current) onClose();
    } catch (e) {
      if (alive.current) setError(errorText(e));
    } finally {
      if (alive.current) setStarting(false);
    }
  };

  const onQuestionKey = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      void start();
    }
  };

  const harnessLabel = providers.find((p) => p.id === harness)?.label;

  return (
    <Modal
      // A start in flight cannot be taken back, so closing waits for it.
      onClose={() => {
        if (!starting) onClose();
      }}
      title="Ask Agent"
      description={askSubtitle(scope, name)}
      size="md"
      fitViewport
    >
      <div className="flex flex-col gap-3 px-4 pb-4 pt-3 text-[12px] text-content">
        <section aria-label="Context sent to the agent">
          <h3 className="mb-1.5 text-[11px] uppercase tracking-wide text-content/50">
            Context sent to the agent
          </h3>
          <ul className="flex flex-wrap gap-1.5">
            {chips.map((chip) => {
              const part = chip.id as AskContextPart;
              const removed = chip.removable && omit.includes(part);
              return (
                <li
                  key={chip.id}
                  data-chip={chip.id}
                  data-removed={removed || undefined}
                  title={chip.hint}
                  className={`inline-flex items-center gap-1 rounded-full border border-stroke bg-content/3 py-0.5 pl-2.5 ${
                    chip.removable ? "pr-1" : "pr-2.5"
                  } ${removed ? "text-content/35 line-through" : ""}`}
                >
                  {chip.label}
                  {chip.removable ? (
                    <button
                      type="button"
                      aria-label={`${removed ? "Add back" : "Remove"} ${chip.label}`}
                      onClick={() => toggleChip(part)}
                      className="grid size-4 place-items-center rounded-full text-content/45 hover:bg-content/10 hover:text-content focus-visible:outline-2 focus-visible:outline-accent"
                    >
                      {removed ? (
                        <Plus className="size-3" strokeWidth={1.75} />
                      ) : (
                        <X className="size-3" strokeWidth={1.75} />
                      )}
                    </button>
                  ) : null}
                </li>
              );
            })}
          </ul>
        </section>

        <div className="flex flex-col gap-1.5">
          <label className="flex items-center gap-2">
            <input
              type="checkbox"
              className={CHECKBOX}
              checked={perPr}
              onChange={(e) => setPerPr(e.target.checked)}
            />
            Per-PR breakdown (titles, diff size, iteration, review)
          </label>
          <label className="flex items-center gap-2">
            <input
              type="checkbox"
              className={CHECKBOX}
              checked={rawCommits}
              onChange={(e) => setRawCommits(e.target.checked)}
            />
            Raw commit list (hash, author, date, files)
          </label>
          <label className="flex items-center gap-2">
            <input
              type="checkbox"
              className={CHECKBOX}
              checked={allowTools}
              onChange={(e) => setAllowTools(e.target.checked)}
            />
            <span>
              Allow the agent to run{" "}
              <code className="font-mono text-[11px]">git</code> /{" "}
              <code className="font-mono text-[11px]">gh</code> in the project
              to dig deeper
            </span>
          </label>
        </div>

        <section aria-label="Suggestions">
          <h3 className="mb-1.5 text-[11px] uppercase tracking-wide text-content/50">
            Suggestions
          </h3>
          <div className="flex flex-col items-start gap-1">
            {suggestions.map((text) => (
              <button
                key={text}
                type="button"
                data-suggestion
                onClick={() => {
                  setQuestion(text);
                  textarea.current?.focus();
                }}
                className="rounded-md border border-stroke px-2 py-1 text-left text-content/70 hover:bg-content/5 hover:text-content focus-visible:outline-2 focus-visible:outline-accent"
              >
                {text}
              </button>
            ))}
          </div>
        </section>

        <textarea
          ref={textarea}
          aria-label="Question"
          rows={4}
          value={question}
          disabled={starting}
          placeholder="Ask anything about this data..."
          onChange={(e) => setQuestion(e.target.value)}
          onKeyDown={onQuestionKey}
          className="w-full resize-y rounded-md border border-stroke bg-transparent px-2.5 py-2 text-[13px] text-content outline-none placeholder:text-content/40 focus-visible:border-accent/60 disabled:opacity-60"
        />

        {error ? (
          <p
            role="alert"
            className="break-words rounded-md border border-red-400/30 bg-red-400/5 px-2.5 py-1.5 text-red-400"
          >
            {error}
          </p>
        ) : null}

        <div className="flex flex-wrap items-center gap-2">
          {providers.length > 1 ? (
            <div className="flex items-center gap-1.5">
              <span className="text-content/50">Agent</span>
              <Select
                label="Agent"
                value={harness ?? ""}
                options={providers.map((p) => ({
                  value: p.id,
                  label: p.label,
                }))}
                onChange={(id) => setHarness(id as HarnessId)}
              />
            </div>
          ) : harnessLabel ? (
            <span className="rounded-full border border-stroke px-2.5 py-0.5 text-content/70">
              Agent: <b className="font-medium">{harnessLabel}</b>
            </span>
          ) : null}
          <span className="min-w-0 flex-1 text-[11px] text-content/45">
            Opens a new session with{" "}
            <code className="font-mono">{snapshotFileName(model.period)}</code>{" "}
            attached
          </span>
          <SecondaryButton onClick={onClose} disabled={starting}>
            Cancel
          </SecondaryButton>
          <button
            type="button"
            disabled={!canStart}
            onClick={() => void start()}
            title="Cmd/Ctrl+Enter"
            className="flex items-center gap-1.5 rounded-md bg-accent px-3 py-1 text-[12px] font-medium text-white hover:opacity-90 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent disabled:cursor-default disabled:opacity-40 disabled:hover:opacity-40"
          >
            {starting ? (
              <>
                <Loader className="size-3.5 animate-spin" strokeWidth={1.75} />
                Starting...
              </>
            ) : (
              "Start session"
            )}
          </button>
        </div>
      </div>
    </Modal>
  );
}
