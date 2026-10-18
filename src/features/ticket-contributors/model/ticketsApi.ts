import { invoke } from "@tauri-apps/api/core";
import type { Level, TicketRecord, WorkspaceScope } from "./tickets";

// Plain `invoke`, like `api.ts`: tickets are fetched and assessed by the
// local app, never on a remote machine.

/** Event the Rust ticket jobs emit on every change of a job. */
export const TICKETS_EVENT = "contributors://tickets";

/** Mirrors `TicketJobEvent` in `src-tauri/src/contributors/ticket_jobs.rs`. */
export type TicketJobEvent = {
  kind: "fetch" | "assess";
  state: "queued" | "running" | "done" | "error" | "cancelled";
  /** Links (fetch) or tickets (assess) finished with, failed ones included. */
  done: number;
  total: number;
  /** The part of `done` that did not work out. */
  failed: number;
  error: string | null;
  /** The job this event is about; `fetchTickets` and `assessTickets` return it. */
  jobId: number;
  /** Fetch jobs queued or running when the event went out. */
  pendingFetches: number;
};

/** The running or next queued job's last event, and the fetch jobs pending. */
export type TicketJobStatus = {
  event: TicketJobEvent | null;
  pendingFetches: number;
};

export type AiConfigStatus = {
  runner: "claude-code" | "api";
  model: string;
  hasApiKey: boolean;
};

/**
 * Queues a fetch of the tickets updated in `from`..`to` (both edges inclusive;
 * a bare end date means the end of its day) in each linked workspace.
 * Progress arrives as `TICKETS_EVENT`. Resolves with the id of the job.
 */
export function fetchTickets(
  links: WorkspaceScope[],
  from: string,
  to: string,
): Promise<number> {
  return invoke<number>("contributors_tickets_fetch", { links, from, to });
}

/** Queues an assessment; with `force`, tickets that already have one are redone. */
export function assessTickets(ids: string[], force: boolean): Promise<void> {
  return invoke<void>("contributors_tickets_assess", { ids, force });
}

export function cancelTicketJob(): Promise<void> {
  return invoke<void>("contributors_tickets_cancel");
}

/**
 * The latest event of the running or queued ticket job, if any, and how many
 * fetch jobs are queued or running.
 */
export function ticketJobStatus(): Promise<TicketJobStatus> {
  return invoke<TicketJobStatus>("contributors_tickets_status");
}

/**
 * Whether the stored tickets of the workspace's spaces (`null` or an empty list
 * is every space) are complete for the window `from`..`to`: completed fetches
 * cover it from its start to its end, or to now when it has not ended. A fetch
 * covers only the span from its start to when its listing began, and one of
 * narrowed spaces only those spaces.
 */
export function windowCovered(
  workspaceId: string,
  spaceIds: string[] | null,
  from: string,
  to: string,
): Promise<boolean> {
  return invoke<boolean>("contributors_tickets_window_covered", {
    workspaceId,
    spaceIds,
    from,
    to,
  });
}

/**
 * The stored tickets of the workspaces updated since `since`, whatever their
 * status, subtasks included; nothing is fetched.
 */
export function loadTickets(
  workspaceIds: string[],
  since: string,
): Promise<TicketRecord[]> {
  return invoke<TicketRecord[]>("contributors_tickets_load", {
    workspaceIds,
    since,
  });
}

/** A rough size of assessing `ids`: how many would really run and the input tokens. */
export function estimateTickets(
  ids: string[],
): Promise<{ tickets: number; inputTokens: number }> {
  return invoke<{ tickets: number; inputTokens: number }>(
    "contributors_tickets_estimate",
    { ids },
  );
}

/** A manual level for the ticket; `null` clears it. */
export function setTicketOverride(
  ticketId: string,
  level: Level | null,
  note: string | null,
): Promise<void> {
  return invoke<void>("contributors_ticket_override", {
    ticketId,
    level,
    note,
  });
}

export function getAiConfig(): Promise<AiConfigStatus> {
  return invoke<AiConfigStatus>("contributors_ai_config_get");
}

/** `apiKey` of `null` keeps the stored key; an empty string deletes it. */
export function setAiConfig(
  runner: AiConfigStatus["runner"],
  model: string,
  apiKey: string | null,
): Promise<AiConfigStatus> {
  return invoke<AiConfigStatus>("contributors_ai_config_set", {
    runner,
    model,
    apiKey,
  });
}

export function listWorkspaceSpaces(
  teamId: string,
): Promise<{ id: string; name: string }[]> {
  return invoke<{ id: string; name: string }[]>("contributors_clickup_spaces", {
    teamId,
  });
}
