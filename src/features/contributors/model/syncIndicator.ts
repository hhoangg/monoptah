import { SYNC_STEP_LABEL, isLive } from "./syncEvents";
import type { SyncEvent } from "./syncEvents";
import type { SyncJobs } from "./syncJobs";

/** What the footer indicator shows for the jobs of every project. */
export type SyncIndicatorSummary = {
  /** Combined progress of the running jobs (each weighs the same), or null with none. */
  percent: number | null;
  /** The first job that stopped with an error and was not dismissed. */
  failed: SyncEvent | null;
  /** The job a click opens: the failure when there is one, else the first running job. */
  target: SyncEvent;
  /** One line per job, for the tooltip. */
  title: string;
};

const folderOf = (path: string) => {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? path;
};

const nameOf = (event: SyncEvent) => event.project || folderOf(event.root);

function progressLine(event: SyncEvent): string {
  const prefix = `Syncing contributors · ${nameOf(event)} · `;
  if (event.state === "queued") return `${prefix}Queued`;
  const label = SYNC_STEP_LABEL[event.step];
  if (event.total <= 0) return `${prefix}${label}`;
  const count = (n: number) => n.toLocaleString("en-US");
  return `${prefix}${label} ${count(event.done)}/${count(event.total)}`;
}

const failureLine = (event: SyncEvent) =>
  `Contributors sync stopped · ${nameOf(event)} · ${event.error ?? "The sync failed."}`;

/** Null when nothing runs and nothing failed: the indicator is not shown. */
export function summarizeSync(jobs: SyncJobs): SyncIndicatorSummary | null {
  const all = Object.values(jobs);
  const live = all.filter(isLive);
  const failed = all.find((event) => event.state === "error") ?? null;
  const target = failed ?? live[0];
  if (!target) return null;
  const percent =
    live.length === 0
      ? null
      : Math.round(
          live.reduce((sum, event) => sum + event.percent, 0) / live.length,
        );
  const lines = [
    ...(failed ? [failureLine(failed)] : []),
    ...live.map(progressLine),
  ];
  return {
    percent: percent === null ? null : Math.min(100, Math.max(0, percent)),
    failed,
    target,
    title: lines.join("\n"),
  };
}
