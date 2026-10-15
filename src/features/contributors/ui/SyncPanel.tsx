import { Loader, RefreshCw, Users } from "../../../shared/ui/icons";
import { SecondaryButton } from "../../../shared/ui/SecondaryButton";
import { relativeTime } from "../model/format";
import { SYNC_STEP_LABEL } from "../model/syncEvents";
import type { SyncEvent } from "../model/syncEvents";
import type { SyncEstimate } from "../model/types";

const COUNT = new Intl.NumberFormat("en-US");

const plural = (n: number, one: string, many: string) =>
  `${COUNT.format(n)} ${n === 1 ? one : many}`;

/** "About 120 commits and 9 pull requests to read." */
export function estimateText(estimate: SyncEstimate): string {
  // Commits the store already has need no reading again.
  const commits = Math.max(0, estimate.commits - estimate.storedCommits);
  const parts = [plural(commits, "commit", "commits")];
  if (estimate.prs !== null) {
    parts.push(plural(estimate.prs, "pull request", "pull requests"));
  }
  return `About ${parts.join(" and ")} to read.`;
}

/** "Syncing Commits: 3,200 of 12,000" */
export function progressText(job: SyncEvent): string {
  if (job.state === "queued") return "Sync queued";
  const step = SYNC_STEP_LABEL[job.step];
  return job.total > 0
    ? `Syncing ${step}: ${COUNT.format(job.done)} of ${COUNT.format(job.total)}`
    : `Syncing ${step}`;
}

function ProgressBar({ percent }: { percent: number }) {
  return (
    <div
      role="progressbar"
      aria-label="Sync progress"
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={percent}
      className="h-1 w-full max-w-xs overflow-hidden rounded-full bg-content/10"
    >
      <div
        className="h-full rounded-full bg-accent transition-[width]"
        style={{ width: `${percent}%` }}
      />
    </div>
  );
}

/** The page of a period the store does not have in full: nothing is shown until Sync. */
export function NotSyncedPanel({
  label,
  estimate,
  job,
  onSync,
  onCancel,
}: {
  /** "the last 7 days", "Q3 2026" */
  label: string;
  estimate: SyncEstimate | null;
  job: SyncEvent | null;
  onSync: () => void;
  onCancel: () => void;
}) {
  return (
    <div className="flex flex-col items-center gap-2 rounded-xl border border-stroke px-4 py-8 text-center">
      <Users className="size-5 text-content/35" strokeWidth={1.75} />
      <p className="text-[13px] font-medium">{`No data for ${label} yet`}</p>
      {job ? (
        <div
          role="status"
          className="flex w-full flex-col items-center gap-2 pt-1"
        >
          <p className="text-[12px] text-content/60">{progressText(job)}</p>
          <ProgressBar percent={job.state === "queued" ? 0 : job.percent} />
          <SecondaryButton onClick={onCancel}>Cancel</SecondaryButton>
        </div>
      ) : (
        <>
          {estimate ? (
            <p className="text-[12px] text-content/50">
              {estimateText(estimate)}
            </p>
          ) : null}
          <div className="pt-1">
            <SecondaryButton onClick={onSync}>
              <RefreshCw className="size-3" strokeWidth={1.75} />
              Sync
            </SecondaryButton>
          </div>
        </>
      )}
    </div>
  );
}

/** One line above a synced period: when it was synced, and a sync or top-up running. */
export function SyncedBar({
  syncedAt,
  now,
  job,
  onSync,
  onCancel,
}: {
  /** Epoch seconds. */
  syncedAt: number | null;
  now: Date;
  job: SyncEvent | null;
  onSync: () => void;
  onCancel: () => void;
}) {
  const when =
    syncedAt === null
      ? ""
      : relativeTime(new Date(syncedAt * 1000).toISOString(), now);
  return (
    <div className="mb-4 flex flex-wrap items-center gap-2.5 text-[12px] text-content/50">
      {when ? <span>{`Synced ${when}`}</span> : null}
      {job ? (
        <>
          <span role="status" className="flex items-center gap-1.5">
            <Loader className="size-3 animate-spin" strokeWidth={1.75} />
            {progressText(job)}
          </span>
          <SecondaryButton onClick={onCancel}>Cancel</SecondaryButton>
        </>
      ) : (
        <SecondaryButton onClick={onSync}>
          <RefreshCw className="size-3" strokeWidth={1.75} />
          Sync again
        </SecondaryButton>
      )}
    </div>
  );
}
