import {
  hasProbedHarnessAvailability,
  isHarnessAvailable,
} from "../../../integrations/harness/core/availabilityState";
import {
  findModel,
  modelEffortSetting,
  modelsFor,
  type AgentModel,
} from "../../sessions/model/models";
import { HARNESSES, type HarnessId } from "../../sessions/model/session";
import type { SessionNode, Workflow } from "./workflow";
import { validateWorkflow, type WorkflowIssue } from "./workflowParse";

const PICK = "Pick one in the step's model list.";

/** The message for a model id the app cannot start. Also what a failed start reports. */
export const modelUnavailable = (value: string): string =>
  `Model "${value}" is not available. ${PICK}`;

/** The message for an effort the app cannot start. Also what a failed start reports. */
export const effortUnavailable = (value: string): string =>
  `Effort "${value}" is not available for this model. ${PICK}`;

/**
 * An installed CLI is only known once the first probe has finished. Until then every harness
 * reads as missing, which would flag every model, so availability is judged after the probe.
 */
function usable(harness: HarnessId): boolean {
  return !hasProbedHarnessAvailability() || isHarnessAvailable(harness);
}

/**
 * The message for what is wrong with the harness, model and effort a session node names, if anything. It
 * applies the rules `sessions.start` applies when the node runs, so what is flagged is what
 * would fail there. A node that sets nothing runs with the Mono's own model and is fine.
 */
export function sessionNodeProblem(node: SessionNode): string | undefined {
  let harness: HarnessId | undefined;
  if (node.harness) {
    if (!HARNESSES.includes(node.harness as HarnessId) || !usable(node.harness as HarnessId))
      return `Provider "${node.harness}" is not available. ${PICK}`;
    harness = node.harness as HarnessId;
  }
  let model: AgentModel | undefined;
  if (node.model) {
    // Without a harness the runner starts the model's own, so the model must exist by itself.
    model = harness
      ? modelsFor(harness).find((entry) => entry.id === node.model)
      : findModel(node.model);
    if (!model || !usable(model.harness))
      return modelUnavailable(node.model);
  }
  // Without a model the effort belongs to the Mono's model, which is not known here.
  if (node.effort && model) {
    const setting = modelEffortSetting(model);
    if (!setting?.options.some((option) => option.value === node.effort))
      return effortUnavailable(node.effort);
  }
  return undefined;
}

/**
 * Session nodes whose harness, model or effort the app cannot start.
 *
 * `validateWorkflow` stays catalog-agnostic; use `workflowIssues` wherever a workflow is
 * gated on being runnable.
 */
export function sessionModelIssues(w: Workflow): WorkflowIssue[] {
  return w.nodes.flatMap((node) => {
    const problem = node.type === "session" ? sessionNodeProblem(node) : undefined;
    return problem ? [{ nodeId: node.id, message: problem }] : [];
  });
}

/** Everything wrong with a workflow's graph and with the models its sessions name. */
export function workflowIssues(w: Workflow): WorkflowIssue[] {
  return [...validateWorkflow(w), ...sessionModelIssues(w)];
}
