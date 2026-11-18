// Data hooks of the Changes and Git screens: the 10 s polled queries and the action runner.
import { useCallback } from "react";
import { makeCommandId, useLinkQuery } from "@/data";
import { useLinkConnection, useLinkState } from "@/link";
import { useCapability } from "@/version/capabilities";
import { POLL_MS } from "./gitModel";

export function useChangesQuery(projectId: string) {
  return useLinkQuery("project.changes", { projectId }, { refetchInterval: POLL_MS });
}

export function useGitQuery(projectId: string) {
  return useLinkQuery("project.git", { projectId }, { refetchInterval: POLL_MS });
}

export function useReadOnlyDevice(): boolean {
  return useLinkState((state) => state.permission === "read");
}

/** Remote push in Git, as the desktop advertises it, and only for devices that may write. */
export function useCanPush(): boolean {
  const advertised = useCapability("gitPush");
  const readOnly = useReadOnlyDevice();
  return advertised && !readOnly;
}

export type GitActionInput =
  | { action: "stage" | "unstage"; paths: string[] }
  | { action: "stageAll" | "unstageAll" }
  | { action: "generate" }
  | { action: "commit"; message: string; push?: boolean; createPr?: boolean }
  | { action: "sync" }
  | { action: "pr" };

const PENDING_DELAY_MS = 500;
const MAX_PENDING_ROUNDS = 240;

/**
 * Runs `project.gitAction`. A long job answers `{pending: true}`: the call is sent again under the SAME command
 * id until the desktop has its answer (the data layer's `useLinkMutation` drops the id after any reply, so the
 * id is held here for the whole job).
 */
export function useRunGitAction(projectId: string) {
  const client = useLinkConnection();
  return useCallback(
    async (input: GitActionInput): Promise<{ message?: string; url?: string }> => {
      const id = makeCommandId();
      const params = { projectId, ...input };
      for (let round = 0; round < MAX_PENDING_ROUNDS; round += 1) {
        const result = (await client.request("project.gitAction", params, { id })) as { pending?: boolean; message?: string; url?: string };
        if (!result.pending) return result;
        await new Promise((resolve) => setTimeout(resolve, PENDING_DELAY_MS));
      }
      throw new Error("The desktop is still working on this. Check the Git status in a moment.");
    },
    [client, projectId],
  );
}
