import { open } from "@tauri-apps/plugin-dialog";
import { readTextFile } from "../../../platform/tauri/fs";

/**
 * Lets the user pick a workflow file and reads it. Resolves to the file's text, to a message
 * when it could not be read, and to undefined when the user cancelled.
 */
export async function pickWorkflowFile(): Promise<
  { text: string } | { error: string } | undefined
> {
  try {
    const path = await open({
      title: "Import workflow",
      multiple: false,
      directory: false,
      filters: [{ name: "Monoptah workflow", extensions: ["yaml", "yml", "json"] }],
    });
    if (typeof path !== "string" || !path) return undefined;
    return { text: await readTextFile(path) };
  } catch (error) {
    console.warn("Could not read the workflow file", error);
    // The command rejects with plain text, such as "File is too large to edit (maximum 8 MB)."
    const reason =
      typeof error === "string" ? error : error instanceof Error ? error.message : "";
    return { error: `Could not read the file${reason ? `: ${reason}` : "."}` };
  }
}
