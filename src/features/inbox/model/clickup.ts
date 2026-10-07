import { invoke } from "@tauri-apps/api/core";
import { recordInboxSelfActivity } from "./inboxSelfActivity";

export type ClickUpWorkspace = {
  id: string;
  name: string;
};

/** ClickUp spaces play the role Jira projects do: the unit users hide. */
export type ClickUpSpace = {
  id: string;
  name: string;
};

export type ClickUpIssue = {
  provider: "clickup";
  kind: "clickup";
  /** Raw ClickUp task id. The only id the details, thread and comment commands accept. */
  id: string;
  /** Display string only: the custom id when the workspace has them, else `#<id>`. */
  identifier: string;
  number: number;
  title: string;
  url: string;
  state: string;
  /** Normalized by the backend: `new`, `indeterminate` or `done`. */
  stateType: string;
  updatedAt: string;
  labels: { name: string; color: string }[];
  assignees: { login: string; avatarUrl?: string }[];
  draft: boolean;
  repo: string;
  /** Space id. */
  teamId: string;
  /** Space name. */
  teamName: string;
  /** List name. */
  projectPath: string;
};

export type ClickUpIssueDetails = {
  body: string;
  author: string;
  authorAvatarUrl?: string;
};

export type ClickUpIssueComment = {
  id: string;
  kind: string;
  author: string;
  authorAvatarUrl?: string;
  body: string;
  createdAt: string;
  url: string;
  state: string;
  path: string;
  line: number | null;
  resolved: boolean;
  threadId: string;
  replies: ClickUpIssueComment[];
};

export type ClickUpIssueThread = {
  comments: ClickUpIssueComment[];
  truncated: boolean;
  reviewDecision: string;
  baseRefName: string;
  headRefName: string;
};

export type ClickUpStatus = {
  connected: boolean;
  teamId: string;
  teamName: string;
  /** Cached by the backend when the token was saved, so reading it is offline. */
  username: string;
};

const SPACE_IDS_KEY = "monocode.clickupHiddenSpaces";
export const CLICKUP_CHANGE_EVENT = "monocode:clickup-change";

// Keyed by the raw task id: the REST paths take it, and a custom id is rejected.
const detailsByTaskId = new Map<string, ClickUpIssueDetails>();
const threadByTaskId = new Map<string, ClickUpIssueThread>();
const threadInflight = new Map<string, Promise<ClickUpIssueThread>>();

let cacheGeneration = 0;

export function clearClickUpCache() {
  cacheGeneration += 1;
  detailsByTaskId.clear();
  threadByTaskId.clear();
  threadInflight.clear();
}

function requireTaskId(taskId: string): string {
  const id = taskId.trim();
  if (!id) throw new Error("Missing ClickUp task");
  return id;
}

export function clickupConnected(): Promise<ClickUpStatus> {
  return invoke<ClickUpStatus>("clickup_status");
}

export async function saveClickUpConfig(config: {
  token: string;
  teamId?: string | null;
}): Promise<ClickUpStatus> {
  const status = await invoke<ClickUpStatus>("clickup_set_config", {
    token: config.token.trim(),
    teamId: config.teamId?.trim() || null,
  });
  clearClickUpCache();
  return status;
}

export async function disconnectClickUp(): Promise<ClickUpStatus> {
  const status = await invoke<ClickUpStatus>("clickup_set_config", {
    token: "",
    teamId: null,
  });
  clearClickUpCache();
  return status;
}

export function listClickUpWorkspaces(): Promise<ClickUpWorkspace[]> {
  return invoke<ClickUpWorkspace[]>("clickup_list_workspaces");
}

export function listClickUpSpaces(): Promise<ClickUpSpace[]> {
  return invoke<ClickUpSpace[]>("clickup_list_spaces");
}

/**
 * Drops hidden ids whose space no longer exists. The backend rejects any space
 * id it cannot use, so a stale preference must never reach a request.
 */
export function reconcileHiddenClickUpSpaceIds(
  spaces: readonly ClickUpSpace[],
  hiddenIds: readonly string[],
): string[] {
  const live = new Set(spaces.map((space) => space.id));
  return hiddenIds.filter((id) => live.has(id));
}

/**
 * `null` means do not filter by space. `[]` means every known space is hidden.
 * Every returned id comes from `spaces`, never from the stored preference.
 */
export function clickupSpaceIdsForFetch(
  spaces: readonly ClickUpSpace[],
  hiddenIds: readonly string[],
): string[] | null {
  const hidden = new Set(reconcileHiddenClickUpSpaceIds(spaces, hiddenIds));
  if (hidden.size === 0) return null;
  return spaces.map((space) => space.id).filter((id) => !hidden.has(id));
}

/**
 * The backend has no assignee filter, so "assigned to me" is matched locally
 * against the username it cached with the token. Without a username nothing
 * can match: showing the whole workspace would mislead the user.
 */
export function clickupIssuesAssignedTo(
  issues: readonly ClickUpIssue[],
  username: string,
): ClickUpIssue[] {
  const me = username.trim().toLowerCase();
  if (!me) return [];
  return issues.filter((issue) =>
    issue.assignees.some((person) => person.login.trim().toLowerCase() === me),
  );
}

export function listClickUpIssues(query: {
  includeClosed: boolean;
  spaceIds: string[];
  limit?: number;
}): Promise<ClickUpIssue[]> {
  return invoke<ClickUpIssue[]>("clickup_list_issues", {
    spaceIds: query.spaceIds,
    includeClosed: query.includeClosed,
    limit: query.limit,
  });
}

export function peekClickUpIssueDetails(
  taskId: string,
): ClickUpIssueDetails | null {
  return detailsByTaskId.get(taskId) ?? null;
}

export async function clickupIssueDetails(
  taskId: string,
): Promise<ClickUpIssueDetails> {
  const id = requireTaskId(taskId);
  const generation = cacheGeneration;
  const details = await invoke<ClickUpIssueDetails>("clickup_issue_details", {
    taskId: id,
  });
  if (generation === cacheGeneration) detailsByTaskId.set(id, details);
  return details;
}

export function peekClickUpIssueThread(
  taskId: string,
): ClickUpIssueThread | null {
  return threadByTaskId.get(taskId) ?? null;
}

export async function clickupIssueThread(
  taskId: string,
  options?: { force?: boolean },
): Promise<ClickUpIssueThread> {
  const id = requireTaskId(taskId);
  if (options?.force) {
    threadByTaskId.delete(id);
    threadInflight.delete(id);
  }
  const pending = threadInflight.get(id);
  if (pending) return pending;
  const generation = cacheGeneration;
  const promise = invoke<ClickUpIssueThread>("clickup_issue_thread", {
    taskId: id,
  })
    .then((thread) => {
      if (
        generation === cacheGeneration &&
        threadInflight.get(id) === promise
      ) {
        threadByTaskId.set(id, thread);
      }
      return thread;
    })
    .finally(() => {
      if (threadInflight.get(id) === promise) threadInflight.delete(id);
    });
  threadInflight.set(id, promise);
  return promise;
}

export async function clickupIssueComment(
  issue: { id: string },
  body: string,
): Promise<string> {
  const id = requireTaskId(issue.id);
  const url = await invoke<string>("clickup_issue_comment", {
    taskId: id,
    body: body.trim(),
  });
  threadByTaskId.delete(id);
  threadInflight.delete(id);
  recordInboxSelfActivity({ provider: "clickup", kind: "clickup", id });
  return url;
}

export function loadHiddenClickUpSpaceIds(): string[] {
  try {
    const raw = localStorage.getItem(SPACE_IDS_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw) as unknown;
    if (!Array.isArray(parsed)) return [];
    return parsed.filter(
      (id): id is string => typeof id === "string" && id.length > 0,
    );
  } catch {
    return [];
  }
}

export function saveHiddenClickUpSpaceIds(ids: string[]) {
  try {
    localStorage.setItem(SPACE_IDS_KEY, JSON.stringify(ids));
  } catch {
    // private mode / quota
  }
  notifyClickUpChange();
}

export function notifyClickUpChange() {
  if (typeof window === "undefined") return;
  window.dispatchEvent(new Event(CLICKUP_CHANGE_EVENT));
}
