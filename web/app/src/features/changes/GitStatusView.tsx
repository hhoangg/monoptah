// F-67: the git status of the project: branch, ahead/behind, upstream and PR, polled every 10 s, with the
// same context-aware action as the Changes list.
import { Link } from "@tanstack/react-router";
import { useLinkDesktopId } from "@/link";
import { Button } from "@/ui/button";
import { DelayedSpinner } from "@/ui/DelayedSpinner";
import { Icon } from "@/ui/Icon";
import { Notice } from "@/ui/Notice";
import { GitActionBar } from "./GitActionBar";
import { errorText, safePrUrl } from "./gitModel";
import { usePullRequestOpener } from "./usePullRequestOpener";
import { useCanPush, useChangesQuery, useGitQuery, useReadOnlyDevice } from "./useGit";

/** Label above the value in a narrow pane, side by side from 28rem; long values (branch names) wrap instead of being cut. */
function Row({ label, title, children }: { label: string; title?: string; children: React.ReactNode }) {
  return (
    <div className="flex min-h-11 flex-col items-start gap-0.5 border-b border-stroke px-4 py-2 last:border-b-0 @md:flex-row @md:items-center @md:justify-between @md:gap-3">
      <dt className="shrink-0 text-label text-muted-text">{label}</dt>
      <dd title={title} className="min-w-0 text-label wrap-anywhere @md:text-right">
        {children}
      </dd>
    </div>
  );
}

export function GitStatusView({ projectId }: { projectId: string }) {
  const git = useGitQuery(projectId);
  const changes = useChangesQuery(projectId);
  const readOnly = useReadOnlyDevice();
  const canPush = useCanPush();
  const pullRequest = usePullRequestOpener();
  const desktopId = useLinkDesktopId();

  if (git.isError && !git.data) {
    return (
      <Notice variant="error" className="m-4">
        {errorText(git.error)}
      </Notice>
    );
  }
  const status = git.data;
  if (!status) return <DelayedSpinner label="Loading git status" className="m-6" />;
  const prUrl = safePrUrl(status.pr?.url);

  return (
    <div className="@container flex flex-col gap-4 p-4" data-testid="git-status">
      {pullRequest.sheet}
      <dl className="rounded-card border border-stroke">
        <Row label="Branch" title={status.branch}>
          <span className="inline-flex items-start gap-2">
            <Icon name="branch" size={16} className="mt-0.5 shrink-0" />
            {status.branch}
          </span>
        </Row>
        <Row label="Default branch" title={status.defaultBranch}>{status.defaultBranch}</Row>
        {status.remoteName ? <Row label="Remote">{status.remoteName}</Row> : null}
        <Row label="Upstream">{status.remote ? (status.upstream ? "Tracking a remote branch" : "Not published") : "No remote"}</Row>
        <Row label="Ahead / behind">
          {status.ahead} ahead, {status.behind} behind
        </Row>
        <Row label="Changed files">
          {status.files > 0 ? (
            <Link to="/d/$desktopId/p/$projectId/changes/$" params={{ desktopId, projectId, _splat: "" }} className="underline hover:text-brand-text">
              {status.files}
            </Link>
          ) : (
            <span className="text-muted-text">{status.files}</span>
          )}
        </Row>
        <Row label="Pull request">
          {status.pr ? (
            prUrl ? (
              <Button type="button" variant="link" size="sm" onClick={() => pullRequest.open(prUrl)}>
                PR #{status.pr.number}
              </Button>
            ) : (
              `PR #${status.pr.number}`
            )
          ) : (
            <span className="text-muted-text">None yet</span>
          )}
        </Row>
      </dl>
      {readOnly ? (
        <Notice>This device is read-only.</Notice>
      ) : (
        <GitActionBar
          projectId={projectId}
          files={changes.data?.files ?? []}
          status={status}
          canPush={canPush}
          onChanged={() => {
            void git.refetch();
            void changes.refetch();
          }}
        />
      )}
    </div>
  );
}
