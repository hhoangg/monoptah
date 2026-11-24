// F-76: a Mono's profile at `/d/:desktopId/monos/:monoId/profile`, as the desktop's details sidebar has it:
// who it is and the model it runs on, then its day, its contributions, what it does next, its habits, the
// documents it wrote and the projects it works in. Read-only and allowed for read-only devices; Edit is a
// control action. A refresh button replaces pull-to-refresh.
import { Link } from "@tanstack/react-router";
import { Fragment, useState } from "react";
import { useCatalog } from "@/data";
import { useMonoTools } from "@/features/sessions/workspaceFilter";
import { useReadOnly } from "@/features/sessions/WorkspaceFrame";
import { ProjectMascot } from "@/features/sessions/ProjectMascot";
import { safeHexColor } from "@/lib/color";
import { ContributionGraph } from "@/features/visuals/charts";
import { Button } from "@/ui/button";
import { DelayedSpinner } from "@/ui/DelayedSpinner";
import { HarnessIcon } from "@/ui/HarnessIcon";
import { Icon } from "@/ui/Icon";
import { Notice } from "@/ui/Notice";
import { MonoDocumentRows } from "./MonoDocumentRows";
import { MonoAvatar } from "./MonoAvatar";
import { BACK_LINK_CLASS, BackIcon, Group, MonoPage, RefreshButton } from "./MonoPage";
import { useMonoProfile, type MonoProfile } from "./profile";
import { ResetMonoDialog, useCanResetMono } from "./ResetMonoDialog";

const DOCUMENTS_SHOWN = 5;

export function MonoProfileScreen({ desktopId, monoId }: { desktopId: string; monoId: string }) {
  const catalog = useCatalog().data;
  const readOnly = useReadOnly();
  const monoTools = useMonoTools();
  const mono = catalog?.monos.find((candidate) => candidate.id === monoId);
  const { profile, loading, error, refreshing, refresh } = useMonoProfile(monoId);
  const assigned = (catalog?.projects ?? []).filter((project) => mono?.projectIds?.includes(project.id));
  const working = profile?.state === "working" || mono?.status === "working";
  const canReset = useCanResetMono();
  const [resetting, setResetting] = useState(false);

  return (
    <MonoPage
      testId="mono-profile"
      title="Profile"
      back={
        <Link to="/d/$desktopId/monos/$monoId" params={{ desktopId, monoId }} aria-label="Back" className={BACK_LINK_CLASS}>
          <BackIcon />
        </Link>
      }
      actions={<RefreshButton onRefresh={refresh} refreshing={refreshing} />}
    >
      <section aria-label="Identity" className="flex flex-col items-center gap-2 py-2 text-center">
        <MonoAvatar monoId={monoId} name={mono?.name ?? "Mono"} mascot={mono?.mascot} color={mono?.color} working={working} size={88} />
        <h2 className="max-w-full truncate text-headline font-semibold">{mono?.name ?? "Mono"}</h2>
        {profile?.status ? <p className="text-small text-muted-text">{profile.status}</p> : null}
        {mono?.model ? (
          <p className="flex items-center gap-1.5 text-small text-muted-text">
            {mono.harness ? <HarnessIcon harness={mono.harness} className="size-3.5" /> : null}
            <span>{mono.model}</span>
          </p>
        ) : null}
        {readOnly || !monoTools ? null : (
          <Button asChild variant="outline" size="sm">
            <Link to="/d/$desktopId/monos/$monoId/profile/edit" params={{ desktopId, monoId }}>
              <Icon name="edit" size={16} />
              Edit
            </Link>
          </Button>
        )}
      </section>

      {profile ? (
        <Overview desktopId={desktopId} monoId={monoId} name={mono?.name ?? "Mono"} profile={profile} />
      ) : error ? (
        <Notice variant="error">{error}</Notice>
      ) : loading ? (
        <DelayedSpinner label="Loading the profile" className="m-6" />
      ) : null}

      {catalog?.monoFiles === false ? null : (
        <Group title="Mind">
          <ul>
            {(["soul", "memory"] as const).map((file) => (
              <li key={file}>
                <Link to="/d/$desktopId/monos/$monoId/profile/file/$name" params={{ desktopId, monoId, name: file }} search={{ mind: file }} className="flex min-h-12 items-center gap-3 rounded-row px-2 hover:bg-selection">
                  <Icon name={file === "soul" ? "sparkle" : "note"} size={18} />
                  <span className="min-w-0 flex-1 truncate text-row">{file === "soul" ? "Soul" : "Memory"}</span>
                  <Icon name="chevronRight" size={16} className="text-muted-text" />
                </Link>
              </li>
            ))}
          </ul>
        </Group>
      )}

      <Group title="Projects">
        {assigned.length ? (
          <ul>
            {assigned.map((project) => (
              <li key={project.id} className="flex min-h-11 items-center gap-3 px-2 text-row">
                <ProjectMascot project={project.id} color={safeHexColor(project.color, "var(--accent-color)")} name={project.mascot ?? null} size={20} />
                <span className="min-w-0 flex-1 truncate">{project.name}</span>
              </li>
            ))}
          </ul>
        ) : (
          <p className="px-2 py-3 text-small text-muted-text">No projects. Without one it works from your home folder.</p>
        )}
      </Group>

      {canReset ? (
        <Group title="Conversation">
          <div className="flex flex-col items-start gap-2 p-2">
            <p className="text-small text-muted-text">Deletes every message of this chat. Its soul, memory and habits are kept.</p>
            <Button type="button" variant="destructive" size="sm" onClick={() => setResetting(true)}>
              <Icon name="refresh" size={16} />
              Reset conversation
            </Button>
          </div>
        </Group>
      ) : null}
      {resetting ? <ResetMonoDialog desktopId={desktopId} monoId={monoId} name={mono?.name ?? "Mono"} onOpenChange={setResetting} /> : null}
    </MonoPage>
  );
}

function Tile({ value, label }: { value: React.ReactNode; label: string }) {
  return (
    <div className="flex flex-col gap-0.5 rounded-card bg-surface px-3 py-2.5">
      <span className="text-[20px] leading-7 font-semibold tabular-nums">{value}</span>
      <span className="text-meta text-muted-text">{label}</span>
    </div>
  );
}

function Overview({ desktopId, monoId, name, profile }: { desktopId: string; monoId: string; name: string; profile: MonoProfile }) {
  const { today, contributions } = profile;
  return (
    <>
      <section aria-label="Today" className="flex flex-col gap-1.5">
        <h2 className="px-1 text-label font-medium text-muted-text">Today</h2>
        <div className="grid grid-cols-2 gap-2 min-[30rem]:grid-cols-4">
          <Tile value={today.merged} label="PRs merged" />
          <Tile
            value={
              <>
                <span className="text-additions">+{today.additions.toLocaleString()}</span> <span className="text-deletions">-{today.deletions.toLocaleString()}</span>
              </>
            }
            label="Lines changed"
          />
          <Tile value={today.sessions} label="Sessions" />
          <Tile value={`${today.habitsRan}/${today.habitsTotal}`} label="Habits run" />
        </div>
      </section>

      <Group title="Contributions">
        <div className="flex flex-col gap-2 p-2">
          <p className="flex flex-wrap items-baseline gap-x-2">
            <span className="text-[20px] leading-7 font-semibold tabular-nums">{contributions.total.toLocaleString()}</span>
            <span className="text-small text-muted-text">in the last 6 months</span>
            {contributions.streak > 1 ? <span className="text-small text-[color:var(--accent-color)]">{contributions.streak}-day streak</span> : null}
          </p>
          {contributions.levels.length > 0 ? <ContributionGraph levels={contributions.levels} /> : null}
        </div>
      </Group>

      <Group title="Up next">
        {profile.upNext.length ? (
          <ul>
            {profile.upNext.map((habit) => (
              <li key={habit.id}>
                <Link to="/d/$desktopId/monos/$monoId/profile/habit/$id" params={{ desktopId, monoId, id: habit.id }} className="flex min-h-12 items-center gap-3 rounded-row px-2 hover:bg-selection">
                  <Icon name={habit.running ? "terminal" : "automation"} size={18} />
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-row">{habit.name}</span>
                    <span className="block truncate text-meta text-muted-text">{habit.schedule}</span>
                  </span>
                  <span className="text-meta text-muted-text">{habit.when}</span>
                </Link>
              </li>
            ))}
          </ul>
        ) : (
          <p className="px-2 py-3 text-small text-muted-text">Nothing scheduled. Ask {name} to put recurring work on a habit.</p>
        )}
      </Group>

      {profile.habits.length ? (
        <Group title="Habits">
          <ul>
            {profile.habits.map((habit) => (
              <Fragment key={habit.id}>
                <li>
                  <Link to="/d/$desktopId/monos/$monoId/profile/habit/$id" params={{ desktopId, monoId, id: habit.id }} className="flex min-h-12 items-center gap-3 rounded-row px-2 hover:bg-selection">
                    <Icon name="automation" size={18} />
                    <span className="min-w-0 flex-1 truncate text-row">{habit.name}</span>
                    <span className="text-meta text-muted-text">{habit.enabled ? habit.schedule : "Paused"}</span>
                  </Link>
                </li>
              </Fragment>
            ))}
          </ul>
        </Group>
      ) : null}

      <Group title="Documents">
        <MonoDocumentRows desktopId={desktopId} monoId={monoId} documents={profile.documents} limit={DOCUMENTS_SHOWN} />
      </Group>
    </>
  );
}
