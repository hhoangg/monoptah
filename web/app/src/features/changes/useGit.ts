// Data hooks of the Changes and Git screens: the 10 s polled queries and the action runner.
import { useCallback, useEffect, useRef } from "react";
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
const MAX_HELD_IDS = 32;

function abortedError(): Error {
  return new Error("The request was cancelled");
}

/** Resolves after `ms`, or at once when `signal` aborts. */
function sleep(ms: number, signal: AbortSignal): Promise<void> {
  return new Promise((resolve) => {
    if (signal.aborted) return resolve();
    const done = () => {
      clearTimeout(timer);
      signal.removeEventListener("abort", done);
      resolve();
    };
    const timer = setTimeout(done, ms);
    signal.addEventListener("abort", done, { once: true });
  });
}

export interface GitActionOptions {
  /** Ends the call, and the polling of a `{pending: true}` job, when it aborts. */
  signal?: AbortSignal;
}

/**
 * Runs `project.gitAction`. A long job answers `{pending: true}`: the call is sent again under the SAME command
 * id until the desktop has its answer. The id belongs to the call's signature (like `Mutator` in
 * `data/mutations.ts`): it is kept until the call succeeds, so a retry after a failure or a Stop is the same
 * command to the desktop. The polling ends when `options.signal` aborts and when the component unmounts.
 */
export function useRunGitAction(projectId: string) {
  const client = useLinkConnection();
  const ids = useRef(new Map<string, string>());
  const lifetime = useRef<AbortController | null>(null);
  useEffect(() => {
    const controller = new AbortController();
    lifetime.current = controller;
    return () => controller.abort();
  }, []);
  return useCallback(
    async (input: GitActionInput, options: GitActionOptions = {}): Promise<{ message?: string; url?: string }> => {
      const params = { projectId, ...input };
      const signature = JSON.stringify(params);
      let id = ids.current.get(signature);
      if (!id) {
        id = makeCommandId();
        ids.current.set(signature, id);
        if (ids.current.size > MAX_HELD_IDS) {
          const oldest = ids.current.keys().next().value;
          if (oldest !== undefined) ids.current.delete(oldest);
        }
      }
      // One signal for the caller's and the component's lifetime.
      const controller = new AbortController();
      for (const source of [options.signal, lifetime.current?.signal]) {
        if (!source) continue;
        if (source.aborted) controller.abort();
        else source.addEventListener("abort", () => controller.abort(), { once: true });
      }
      const { signal } = controller;
      for (let round = 0; round < MAX_PENDING_ROUNDS; round += 1) {
        if (signal.aborted) throw abortedError();
        const result = (await client.request("project.gitAction", params, { id, signal })) as { pending?: boolean; message?: string; url?: string };
        if (!result.pending) {
          ids.current.delete(signature);
          return result;
        }
        await sleep(PENDING_DELAY_MS, signal);
      }
      throw new Error("The desktop is still working on this. Check the Git status in a moment.");
    },
    [client, projectId],
  );
}
