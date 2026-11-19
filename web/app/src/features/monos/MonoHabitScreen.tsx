// F-77: one habit at `/d/:desktopId/monos/:monoId/profile/habit/:id`, read-only: when it runs and whether it is on,
// what it is told to do (Markdown through the safe renderer) and how its latest runs went. A refresh button
// replaces pull-to-refresh.
import { Link } from "@tanstack/react-router";
import { Markdown } from "@/features/markdown/Markdown";
import { DelayedSpinner } from "@/ui/DelayedSpinner";
import { Notice } from "@/ui/Notice";
import { BACK_LINK_CLASS, BackIcon, Group, MonoPage, RefreshButton } from "./MonoPage";
import { useMonoProfile } from "./profile";

function duration(ms: number): string {
  const seconds = Math.round(ms / 1000);
  return seconds < 60 ? `${seconds}s` : `${Math.floor(seconds / 60)}m ${seconds % 60}s`;
}

function Line({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex min-h-11 items-center justify-between gap-3 px-2">
      <dt className="text-row">{label}</dt>
      <dd className="min-w-0 truncate text-row text-muted-text">{value}</dd>
    </div>
  );
}

export function MonoHabitScreen({ desktopId, monoId, habitId }: { desktopId: string; monoId: string; habitId: string }) {
  const { profile, loading, error, refreshing, refresh } = useMonoProfile(monoId);
  const habit = profile?.habits.find((candidate) => candidate.id === habitId);
  return (
    <MonoPage
      testId="mono-habit"
      title={habit?.name ?? "Habit"}
      back={
        <Link to="/d/$desktopId/monos/$monoId/profile" params={{ desktopId, monoId }} aria-label="Back" className={BACK_LINK_CLASS}>
          <BackIcon />
        </Link>
      }
      actions={<RefreshButton onRefresh={refresh} refreshing={refreshing} />}
    >
      {!profile ? (
        error ? (
          <Notice variant="error">{error}</Notice>
        ) : loading ? (
          <DelayedSpinner label="Loading the habit" className="m-6" />
        ) : null
      ) : !habit ? (
        <p className="py-6 text-center text-body text-muted-text">This habit is no longer there.</p>
      ) : (
        <>
          <div className="rounded-card bg-surface px-2.5 py-1.5">
            <dl>
              <Line label="Schedule" value={habit.schedule} />
              <Line label={habit.running ? "Running" : "Next run"} value={habit.enabled || habit.running ? habit.when : "Paused"} />
              {habit.lastOutcome ? <Line label="Last run" value={habit.lastOutcome} /> : null}
            </dl>
          </div>
          {habit.lastError ? <Notice variant="error">{habit.lastError}</Notice> : null}
          <Group title="Instructions">
            <div className="p-2">
              <Markdown text={habit.instructions} />
            </div>
          </Group>
          <Group title="Recent runs">
            {habit.runs.length ? (
              <ul className="divide-y divide-stroke">
                {habit.runs.map((run, index) => (
                  <li key={`${index}:${run.at}`} className="flex flex-col gap-1 px-2 py-2.5">
                    <div className="flex items-baseline justify-between gap-3">
                      <span className="text-row">{run.at > 0 ? new Date(run.at).toLocaleString([], { dateStyle: "medium", timeStyle: "short" }) : "Unknown time"}</span>
                      <span className="text-meta text-muted-text">
                        {run.outcome}
                        {run.durationMs ? ` · ${duration(run.durationMs)}` : ""}
                      </span>
                    </div>
                    {run.error ? <p className="line-clamp-3 text-small text-deletions">{run.error}</p> : null}
                    {run.report ? <p className="line-clamp-4 text-small text-muted-text">{run.report}</p> : null}
                  </li>
                ))}
              </ul>
            ) : (
              <p className="px-2 py-3 text-small text-muted-text">It hasn&apos;t run yet.</p>
            )}
          </Group>
        </>
      )}
    </MonoPage>
  );
}
