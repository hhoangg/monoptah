import { useState, type ReactNode } from "react";
import { ChevronDown } from "../../../shared/ui/icons";
import { habitScheduleLabel } from "../../monos/model/monoHabits";
import { Hint, Property, Section } from "../../monos/ui/monoPanelParts";
import { isTrigger, type WorkflowEdge, type WorkflowNode } from "../model/workflow";
import type { ImportPreview } from "../model/workflowYaml";
import { NODE_LABEL, nodeTitle } from "./board/boardNodes";

export const ERROR = "text-red-500/80";

/**
 * The text with the characters that change how it reads or what it looks like, without
 * showing, made visible as <U+XXXX>: every format character (bidirectional controls,
 * zero-width characters, tag characters, soft hyphens), the line and paragraph separators,
 * a carriage return that is not half of a line break, and the other control characters.
 * What is reviewed is then what runs. The `u` flag matches whole code points, so a character
 * outside the BMP is one marker with its full number.
 */
const HIDDEN = /[\p{Cf}\u2028\u2029]|\r(?!\n)|(?![\n\t\r])\p{Cc}/gu;
export function visible(text: string): string {
  return text.replace(
    HIDDEN,
    (char) =>
      `<U+${(char.codePointAt(0) ?? 0).toString(16).toUpperCase().padStart(4, "0")}>`,
  );
}

/** How the file names a step: its title as written, else its id read as words. */
function stepLabel(node: WorkflowNode): string {
  return visible(node.title?.trim() ? node.title : nodeTitle(node));
}

function stepName(preview: ImportPreview, nodeId: string): string {
  const node = preview.workflow.nodes.find((entry) => entry.id === nodeId);
  return node ? `${stepLabel(node)} (${visible(node.id)})` : visible(nodeId);
}

/** Where a session step runs and with what, as its file says: nothing set means the Mono's own. */
function sessionDetail(
  preview: ImportPreview,
  node: Extract<WorkflowNode, { type: "session" }>,
): string {
  const { checkout } = node;
  let where = "Project folder";
  if (checkout === "newWorktree") where = "New worktree";
  else if (typeof checkout === "object") {
    const source = preview.workflow.nodes.find((entry) => entry.id === checkout.worktreeOf);
    where = `Worktree of ${source ? stepLabel(source) : visible(checkout.worktreeOf)}`;
  }
  return [
    ...(node.harness ? [`Harness: ${visible(node.harness)}`] : []),
    `Model: ${node.model ? visible(node.model) : node.harness ? "its default" : "Mono default"}`,
    ...(node.effort ? [`Effort: ${visible(node.effort)}`] : []),
    `Checkout: ${where}`,
  ].join(" · ");
}

function triggerLabel(preview: ImportPreview): string {
  const trigger = preview.workflow.nodes.find(isTrigger);
  if (!trigger) return "No trigger";
  return trigger.type === "schedule"
    ? habitScheduleLabel(trigger.schedule)
    : "Manual";
}

/** Text exactly as the file has it, in the monospace the app uses for code. */
function Verbatim({ children }: { children: string }) {
  return (
    <pre className="whitespace-pre-wrap break-words rounded-md bg-content/5 px-2.5 py-2 font-mono text-[11px] leading-4 text-content/75">
      {visible(children)}
    </pre>
  );
}

function Labelled({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="px-2 pb-3 last:pb-0">
      <p className="px-0.5 pb-1 text-[11px] leading-4 text-content/45">{label}</p>
      {children}
    </div>
  );
}

/** A loop back edge: where it goes and the exact text that decides whether it repeats. */
function LoopRow({
  preview,
  edge,
}: {
  preview: ImportPreview;
  edge: WorkflowEdge;
}) {
  const { loop } = edge;
  if (!loop) return null;
  return (
    <Labelled
      label={`After ${stepName(preview, edge.from)} it may loop back to ${stepName(preview, edge.to)}, up to ${loop.maxRounds} ${loop.maxRounds === 1 ? "round" : "rounds"}`}
    >
      {loop.decide === "mono" ? (
        <p className="px-0.5 text-[12px] leading-5 text-content/75">
          The Mono decides each time whether it repeats.
        </p>
      ) : (
        <Verbatim>{loop.decide}</Verbatim>
      )}
    </Labelled>
  );
}

/**
 * What a user reads before a workflow may run: its name and trigger, what it says it does,
 * its problems, its steps and loops, every shell command verbatim and every prompt. The import
 * page and the review screen of a stored workflow both render this, so they cannot drift.
 * Everything that reaches an agent or the shell is shown as the workflow has it.
 */
export function WorkflowReviewSections({
  preview,
  problemsHint,
}: {
  preview: ImportPreview;
  /** What the problems mean for the action at hand. */
  problemsHint?: ReactNode;
}) {
  const [promptsOpen, setPromptsOpen] = useState(true);
  return (
    <>
      <dl className="flex flex-col gap-0.5 px-4 py-3">
        <Property label="Name">{visible(preview.workflow.name)}</Property>
        <Property label="Runs">{triggerLabel(preview)}</Property>
      </dl>

      {preview.workflow.description ? (
        <Section title="What it does">
          <p className="whitespace-pre-wrap px-2 text-[12px] leading-5 text-content/75">
            {visible(preview.workflow.description)}
          </p>
        </Section>
      ) : null}

      {preview.issues.length ? (
        <Section title="Problems">
          <ul className="flex flex-col gap-1 px-2">
            {preview.issues.map((issue, index) => (
              <li
                key={index}
                className={`flex items-start gap-1.5 text-[11px] leading-4 ${ERROR}`}
              >
                <span
                  aria-hidden
                  className="mt-1.5 size-1.5 shrink-0 rounded-full bg-red-500/70"
                />
                {visible(issue.message)}
              </li>
            ))}
          </ul>
          {problemsHint ? <Hint>{problemsHint}</Hint> : null}
        </Section>
      ) : null}

      <Section title="Steps">
        <ol className="flex flex-col gap-1 px-2 pb-1 text-[12px] leading-5">
          {preview.workflow.nodes.map((node, index) => (
            <li key={index} className="flex min-w-0 gap-2">
              <span className="w-24 shrink-0 text-content/45">
                {NODE_LABEL[node.type]}
              </span>
              <span className="min-w-0 flex-1 whitespace-pre-wrap break-words text-content/85">
                {stepLabel(node)}
                {node.type === "session" ? (
                  <span className="block text-[11px] leading-4 text-content/45">
                    {sessionDetail(preview, node)}
                  </span>
                ) : null}
              </span>
            </li>
          ))}
        </ol>
        {preview.workflow.edges.map((edge, index) =>
          edge.loop ? (
            <LoopRow
              key={index}
              preview={preview}
              edge={edge}
            />
          ) : null,
        )}
      </Section>

      <Section title="Commands this workflow runs">
        {preview.commands.length ? (
          preview.commands.map(({ nodeId, command }, index) => (
            <Labelled key={index} label={stepName(preview, nodeId)}>
              <Verbatim>{command}</Verbatim>
            </Labelled>
          ))
        ) : (
          <p className="px-2 text-[12px] text-content/40">
            It runs no shell commands.
          </p>
        )}
      </Section>

      <Section
        title="Prompts"
        action={
          <button
            type="button"
            data-prompts-toggle
            aria-expanded={promptsOpen}
            aria-label={promptsOpen ? "Hide prompts" : "Show prompts"}
            onClick={() => setPromptsOpen((open) => !open)}
            className="grid size-4.5 place-items-center rounded text-content/45 hover:bg-content/8 hover:text-content"
          >
            <ChevronDown
              className={`size-3 transition-transform ${promptsOpen ? "" : "-rotate-90"}`}
              strokeWidth={1.75}
            />
          </button>
        }
      >
        {!promptsOpen ? null : preview.prompts.length ? (
          preview.prompts.map(({ nodeId, text: prompt }, index) => (
            <Labelled key={index} label={stepName(preview, nodeId)}>
              <Verbatim>{prompt}</Verbatim>
            </Labelled>
          ))
        ) : (
          <p className="px-2 text-[12px] text-content/40">
            It sends no prompts.
          </p>
        )}
      </Section>
    </>
  );
}
