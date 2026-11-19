// The context-aware bottom action of the Changes list and the Git screen: Commit N files / Stage All /
// Publish Branch / Sync / PR #n / Create PR. Anything that leaves the machine (publish, sync, PR, a pushing
// commit) is offered only when `canPush` (capability gitPush, control device) and always goes through a
// confirm dialog naming the remote and branch, plus a second one for the default branch (F-66).
import { useState } from "react";
import { Button } from "@/ui/button";
import { ConfirmDialog } from "@/ui/ConfirmDialog";
import { Icon, type IconName } from "@/ui/Icon";
import { Notice } from "@/ui/Notice";
import { CommitDialog } from "./CommitDialog";
import { barAction, canGenerate, nextSteps, errorText, safePrUrl, sections, type BarAction, type BarKind, type ChangedFile, type GitStatus } from "./gitModel";
import { useRunGitAction } from "./useGit";
import { usePullRequestOpener } from "./usePullRequestOpener";

const ICONS: Record<BarKind, IconName> = { commit: "check", stageAll: "add", publish: "send", sync: "send", viewPr: "branch", createPr: "branch" };

type Push =
  | { kind: "publish" | "sync" | "createPr" }
  | { kind: "commit"; message: string };

export interface GitActionBarProps {
  projectId: string;
  files: readonly ChangedFile[];
  status: GitStatus | undefined;
  canPush: boolean;
  /** Stages everything (optimistic in the list). */
  onStageAll?: () => void;
  stagingAll?: boolean;
  /** After anything that changed the repository, to read it again. */
  onChanged: () => void;
}

/** "Sync" for a branch with an upstream, "Publish Branch" without. */
function pushVerb(push: Push): string {
  switch (push.kind) {
    case "publish":
      return "Publish";
    case "sync":
      return "Sync";
    case "createPr":
      return "Create a pull request for";
    case "commit":
      return "Commit and push";
  }
}

export function GitActionBar({ projectId, files, status, canPush, onStageAll, stagingAll = false, onChanged }: GitActionBarProps) {
  const run = useRunGitAction(projectId);
  const [commitOpen, setCommitOpen] = useState(false);
  const [push, setPush] = useState<Push | null>(null);
  const [step, setStep] = useState<"push" | "default">("push");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [prUrl, setPrUrl] = useState<string | null>(null);
  const pullRequest = usePullRequestOpener();

  const { staged } = sections(files);
  const bar = barAction({ changed: files.length, staged: staged.length, status, canPush });
  // Publish, Sync and the pull request stay reachable with a dirty tree (F-66); the main button may already be one of them.
  const secondary = nextSteps({ changed: files.length, status, canPush }).filter((step) => step.kind !== bar?.kind);
  const additions = staged.reduce((sum, file) => sum + file.additions, 0);
  const deletions = staged.reduce((sum, file) => sum + file.deletions, 0);
  const branch = status?.branch ?? "this branch";
  const onDefault = !!status?.branch && status.branch === status.defaultBranch;

  const ask = (next: Push) => {
    setError(null);
    setStep("push");
    setPush(next);
  };

  const press = (action: BarAction | null = bar) => {
    if (!action) return;
    setError(null);
    if (action.kind === "stageAll") {
      if (onStageAll) onStageAll();
      else void stageEverything();
    }
    else if (action.kind === "commit") setCommitOpen(true);
    else if (action.kind === "viewPr") {
      if (!pullRequest.open(status?.pr?.url)) setError("This pull request link is not a safe https address.");
    } else ask({ kind: action.kind });
  };

  const stageEverything = async () => {
    setBusy(true);
    try {
      await run({ action: "stageAll" });
      onChanged();
    } catch (reason) {
      setError(`Could not stage: ${errorText(reason)}`);
    } finally {
      setBusy(false);
    }
  };

  const confirmPush = async () => {
    if (!push) return;
    // The default branch needs a second, explicit confirmation before anything is sent.
    if (onDefault && step === "push") {
      setStep("default");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      if (push.kind === "commit") await run({ action: "commit", message: push.message, push: true });
      else if (push.kind === "createPr") {
        const result = await run({ action: "pr" });
        const url = safePrUrl(result.url);
        if (url) setPrUrl(url);
        else setError("The pull request was created, but the desktop sent a link that is not a safe https address.");
      } else await run({ action: "sync" });
      setPush(null);
      setCommitOpen(false);
      onChanged();
    } catch (reason) {
      setError(errorText(reason));
    } finally {
      setBusy(false);
    }
  };

  const target = push ? `${pushVerb(push)} "${branch}" on the remote?` : "";

  return (
    <div className="flex flex-col gap-3" data-testid="git-action-bar">
      {pullRequest.sheet}
      {error && !push ? <Notice variant="error">{error}</Notice> : null}
      {prUrl ? (
        <Notice title="Pull request ready">
          <Button type="button" variant="outline" size="sm" className="mt-2" onClick={() => pullRequest.open(prUrl)}>
            <Icon name="externalLink" size={16} />
            Open pull request
          </Button>
        </Notice>
      ) : null}
      {bar ? (
        <Button type="button" size="lg" disabled={stagingAll || busy} aria-busy={stagingAll} onClick={() => press()}>
          <Icon name={ICONS[bar.kind]} size={18} />
          {bar.label}
        </Button>
      ) : null}
      {secondary.map((step) => (
        <Button key={step.kind} type="button" size="lg" variant="outline" disabled={stagingAll || busy} onClick={() => press(step)}>
          <Icon name={ICONS[step.kind]} size={18} />
          {step.label}
        </Button>
      ))}
      <CommitDialog
        open={commitOpen}
        onOpenChange={setCommitOpen}
        staged={staged.length}
        additions={additions}
        deletions={deletions}
        status={status}
        canGenerate={canGenerate(status)}
        canPush={canPush}
        secondary={secondary}
        onSecondary={(step) => press(step)}
        run={run}
        onCommitted={onChanged}
        onCommitAndPush={(message) => ask({ kind: "commit", message })}
      />
      <ConfirmDialog
        open={push !== null}
        onOpenChange={(next) => (next ? undefined : setPush(null))}
        title={step === "default" ? `Push to the default branch "${branch}"?` : target}
        description={
          step === "default"
            ? `"${branch}" is the default branch of this repository. Pushing to it publishes your commits to everyone who uses it.`
            : `This sends your work on branch "${branch}" to the remote repository${status?.upstream ? " it tracks" : " as a new upstream branch"}. Other people can see it.`
        }
        confirmLabel={step === "default" ? "Push to default branch" : push?.kind === "createPr" ? "Create PR" : "Push"}
        destructive={step === "default"}
        busy={busy}
        error={error}
        onConfirm={() => void confirmPush()}
      />
    </div>
  );
}
