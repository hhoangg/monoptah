import { save } from "@tauri-apps/plugin-dialog";
import { writeTextFile } from "../../../platform/tauri/fs";
import type { Workflow } from "../model/workflow";
import { exportPathProblem, exportWorkflow } from "../model/workflowYaml";

/** A file name for the workflow: lowercase words joined by hyphens. */
export function workflowFileName(name: string): string {
  const slug = name
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 60);
  return `${slug || "workflow"}.monoptah-workflow.yaml`;
}

/**
 * Saves the shareable definition where the user picks. Resolves to a message when the
 * workflow cannot be shared or the save failed, and to undefined when it was saved or the
 * user cancelled.
 */
export async function exportWorkflowToFile(
  workflow: Workflow,
): Promise<string | undefined> {
  const problem = exportPathProblem(workflow);
  if (problem) return `Can't export this workflow: ${problem}.`;
  try {
    const path = await save({
      title: "Export workflow",
      defaultPath: workflowFileName(workflow.name),
      filters: [{ name: "Monoptah workflow", extensions: ["yaml", "yml"] }],
    });
    if (!path) return undefined;
    await writeTextFile(path, exportWorkflow(workflow));
    return undefined;
  } catch (error) {
    console.warn("Could not export the workflow", error);
    return "Could not save the workflow file.";
  }
}
