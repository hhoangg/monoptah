// F-67: the git status of the project: branch, ahead/behind, upstream and PR, polled every 10 s, with the
// same context-aware action as the Changes list.
import { Button } from "@/ui/button";
import { DelayedSpinner } from "@/ui/DelayedSpinner";
import { Icon } from "@/ui/Icon";
import { Notice } from "@/ui/Notice";
import { GitActionBar } from "./GitActionBar";
import { errorText, openPullRequest, safePrUrl } from "./gitModel";
import { useCanPush, useChangesQuery, useGitQuery, useReadOnlyDevice } from "./useGit";

function Row({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex min-h-11 items-center justify-between gap-3 border-b border-stroke px-4 last:border-b-0">
      <dt className="text-label text-muted-text">{label}</dt>
      <dd className="min-w-0 truncate text-label">{children}</dd>
    </div>
  );
}

export function GitStatusView({ projectId }: { projectId: string }) {
  const git = useGitQuery(projectId);
  const changes = useChangesQuery(projectId);
  const readOnly = useReadOnlyDevice();
  const canPush = useCanPush();

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
    <div className="flex flex-col gap-4 p-4" data-testid="git-status">
      <dl className="overflow-hidden rounded-card border border-stroke">
        <Row label="Branch">
          <span className="inline-flex items-center gap-2">
            <Icon name="branch" size={16} />
            {status.branch}
          </span>
        </Row>
        <Row label="Default branch">{status.defaultBranch}</Row>
        {status.remoteName ? <Row label="Remote">{status.remoteName}</Row> : null}
        <Row label="Upstream">{status.remote ? (status.upstream ? "Tracking a remote branch" : "Not published") : "No remote"}</Row>
        <Row label="Ahead / behind">
          {status.ahead} ahead, {status.behind} behind
        </Row>
        <Row label="Changed files">{status.files}</Row>
        <Row label="Pull request">
          {status.pr ? (
            prUrl ? (
              <Button type="button" variant="link" size="sm" onClick={() => openPullRequest(prUrl)}>
                PR #{status.pr.number}
              </Button>
            ) : (
              `PR #${status.pr.number}`
            )
          ) : (
            "None"
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
