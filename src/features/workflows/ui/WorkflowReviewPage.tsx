import { useState, type CSSProperties } from "react";
import { PageHeader } from "../../monos/ui/monoPanelParts";
import { definitionFingerprint, type Workflow } from "../model/workflow";
import { reviewOf } from "../model/workflowYaml";
import {
  WorkflowNotice,
  type ReviewIntent,
  type WorkflowActions,
} from "./MonoWorkflows";
import { WorkflowReviewSections } from "./WorkflowReviewSections";

const CHANGED =
  "This workflow changed while you were reviewing it. Review it again.";
const PRIMARY =
  "h-6 rounded-md bg-content/10 px-2 text-[12px] text-content enabled:hover:bg-content/[0.14] disabled:opacity-40";
const SECONDARY =
  "h-6 rounded-md px-2 text-[12px] text-content/70 hover:bg-content/8 hover:text-content";

/**
 * The review a workflow needs before it may run or be enabled: what it would run, as stored
 * right now. Approving only counts for the definition shown; if it changed meanwhile, nothing
 * is approved and the new version is shown to be read again.
 */
export function WorkflowReviewPage({
  workflow,
  intent,
  color,
  actions,
  onBack,
}: {
  /** The stored workflow, kept current by the caller. */
  workflow: Workflow;
  intent: ReviewIntent;
  /** The Mono's color. */
  color: string;
  actions: WorkflowActions;
  onBack: () => void;
}) {
  // What is on screen is what approving applies to.
  const [shown, setShown] = useState(workflow);
  const [seen, setSeen] = useState(() => definitionFingerprint(workflow));
  const [notice, setNotice] = useState<string>();
  const [busy, setBusy] = useState(false);
  const incoming = definitionFingerprint(workflow);
  if (incoming !== seen) {
    setSeen(incoming);
    if (incoming !== definitionFingerprint(shown)) {
      setShown(workflow);
      setNotice(CHANGED);
    }
  }

  const approve = async () => {
    if (busy) return;
    setBusy(true);
    setNotice(undefined);
    const result = await actions.approve(
      shown.id,
      intent,
      definitionFingerprint(shown),
    );
    setBusy(false);
    if (result.kind === "done") return onBack();
    if (result.kind === "changed") {
      setShown(result.workflow);
      setNotice(CHANGED);
    } else setNotice(result.notice);
  };

  return (
    <div
      className="flex min-h-0 flex-1 flex-col"
      style={{ "--mono-color": color } as CSSProperties}
      data-workflow-review={workflow.id}
    >
      <PageHeader title="Review workflow" onBack={onBack}>
        <button
          type="button"
          data-tauri-drag-region="false"
          onClick={onBack}
          className={SECONDARY}
        >
          Cancel
        </button>
        <button
          type="button"
          data-tauri-drag-region="false"
          disabled={busy}
          onClick={() => void approve()}
          className={PRIMARY}
        >
          {intent === "run" ? "Approve and run" : "Approve and enable"}
        </button>
      </PageHeader>

      <div className="min-h-0 flex-1 overflow-y-auto overscroll-none">
        <WorkflowNotice>{notice}</WorkflowNotice>
        <WorkflowReviewSections
          preview={reviewOf(shown)}
          problemsHint="It cannot be enabled or run until these are fixed."
        />
      </div>
    </div>
  );
}
