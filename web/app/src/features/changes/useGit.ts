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

interface ProjectGitState {
  /** Command ids held per call signature until the call succeeds. */
  ids: Map<string, string>;
  /** The end of the calls queued for this project: the next call waits for it. */
  tail: Promise<unknown>;
}

// One state per connection and project, so every runner of the project (the staging list, the action bar, the
// commit dialog) shares one id map and one queue: a generate or commit never overtakes a stage call in flight.
const projectStates = new WeakMap<object, Map<string, ProjectGitState>>();

function projectState(client: object, projectId: string): ProjectGitState {
  let byProject = projectStates.get(client);
  if (!byProject) {
    byProject = new Map();
    projectStates.set(client, byProject);
  }
  let state = byProject.get(projectId);
  if (!state) {
    state = { ids: new Map(), tail: Promise.resolve() };
    byProject.set(projectId, state);
  }
  return state;
}

/**
 * Runs `project.gitAction`. Calls for one project (on one connection) run one after the other. A long job
 * answers `{pending: true}`: the call is sent again under the SAME command id until the desktop has its answer.
 * The id belongs to the call's signature (like `Mutator` in `data/mutations.ts`): it is kept until the call
 * succeeds, so a retry after a failure or a Stop is the same command to the desktop. The polling ends when
 * `options.signal` aborts and when the component unmounts.
 */
export function useRunGitAction(projectId: string) {
  const client = useLinkConnection();
  const lifetime = useRef<AbortController | null>(null);
  useEffect(() => {
    const controller = new AbortController();
    lifetime.current = controller;
    return () => controller.abort();
  }, []);
  return useCallback(
    (input: GitActionInput, options: GitActionOptions = {}): Promise<{ message?: string; url?: string }> => {
      const state = projectState(client, projectId);
      const params = { projectId, ...input };
      const signature = JSON.stringify(params);
      // One signal for the caller's and the component's lifetime; its listeners go when the call settles.
      const controller = new AbortController();
      const abort = () => controller.abort();
      const sources = [options.signal, lifetime.current?.signal].filter((source): source is AbortSignal => !!source);
      for (const source of sources) {
        if (source.aborted) controller.abort();
        else source.addEventListener("abort", abort, { once: true });
      }
      const { signal } = controller;
      const execute = async (): Promise<{ message?: string; url?: string }> => {
        let id = state.ids.get(signature);
        if (!id) {
          id = makeCommandId();
          state.ids.set(signature, id);
          if (state.ids.size > MAX_HELD_IDS) {
            const oldest = state.ids.keys().next().value;
            if (oldest !== undefined) state.ids.delete(oldest);
          }
        }
        for (let round = 0; round < MAX_PENDING_ROUNDS; round += 1) {
          if (signal.aborted) throw abortedError();
          const result = (await client.request("project.gitAction", params, { id, signal })) as { pending?: boolean; message?: string; url?: string };
          if (!result.pending) {
            state.ids.delete(signature);
            return result;
          }
          await sleep(PENDING_DELAY_MS, signal);
        }
        throw new Error("The desktop is still working on this. Check the Git status in a moment.");
      };
      const task = state.tail.then(execute);
      state.tail = task.catch(() => undefined);
      return task.finally(() => {
        for (const source of sources) source.removeEventListener("abort", abort);
      });
    },
    [client, projectId],
  );
}
