// Pure helpers of the Changes and Git screens: sections, the context-aware bottom action, status letters
// and PR URL validation. The bottom action follows mobile/src/features/changes/ChangesView.tsx `barAction`.
import type { ResultOf } from "@monoptah/protocol";
import { leafName } from "@/lib/fileKind";
import { classifyLink } from "@/features/markdown/links";

export type ChangedFile = ResultOf<"project.changes">["files"][number];
export type GitStatus = ResultOf<"project.git">;
export type DiffLine = ChangedFile["lines"][number];

/** Poll period of the changes list and the git status while their screen is visible. */
export const POLL_MS = 10_000;

export const STATUS_LETTER: Record<ChangedFile["status"], string> = { modified: "M", added: "A", deleted: "D", untracked: "U" };
export const STATUS_LABEL: Record<ChangedFile["status"], string> = { modified: "Modified", added: "Added", deleted: "Deleted", untracked: "Untracked" };

export function splitPath(relative: string): { name: string; dir: string } {
  const name = leafName(relative);
  const dir = relative.length > name.length ? relative.slice(0, relative.length - name.length).replace(/\/$/, "") : "";
  return { name, dir };
}

/** The desktop may not say; only an explicit `false` hides "Write a message" (remote projects). */
export function canGenerate(status: GitStatus | undefined): boolean {
  return (status as { canGenerate?: unknown } | undefined)?.canGenerate !== false;
}

export function isStaged(file: ChangedFile): boolean {
  return file.staged === true && file.unstaged !== true;
}

export function isPartlyStaged(file: ChangedFile): boolean {
  return file.staged === true && file.unstaged === true;
}

/** Applies the wanted (optimistic) staging of each path to the files. */
export function withWanted(files: readonly ChangedFile[], wanted: ReadonlyMap<string, boolean>): ChangedFile[] {
  return files.map((file) => {
    const staged = wanted.get(file.relative);
    return staged === undefined ? file : { ...file, staged, unstaged: !staged };
  });
}

/** Staged files gather in the first section; partly staged ones stay with the unstaged. */
export function sections(files: readonly ChangedFile[]): { staged: ChangedFile[]; unstaged: ChangedFile[] } {
  return { staged: files.filter(isStaged), unstaged: files.filter((file) => !isStaged(file)) };
}

export type BarKind = "commit" | "stageAll" | "publish" | "sync" | "viewPr" | "createPr";

export interface BarAction {
  kind: BarKind;
  label: string;
  /** A write that leaves the machine: only offered when the desktop reports Remote push in Git. */
  pushes: boolean;
}

/**
 * What the bottom button offers: commit what is staged, else stage everything, else the branch's next step.
 * The next steps (publish, sync, PR) push or open a PR, so they exist only with `canPush`.
 */
export function barAction(input: { changed: number; staged: number; status: GitStatus | undefined; canPush: boolean }): BarAction | null {
  const { changed, staged, status, canPush } = input;
  if (staged > 0) return { kind: "commit", label: `Commit ${staged} ${staged === 1 ? "file" : "files"}`, pushes: false };
  if (changed > 0) return { kind: "stageAll", label: "Stage All", pushes: false };
  if (!canPush || !status?.remote) return null;
  if (!status.upstream) return { kind: "publish", label: "Publish Branch", pushes: true };
  if (status.ahead > 0 || status.behind > 0) return { kind: "sync", label: "Sync", pushes: true };
  if (status.pr) return { kind: "viewPr", label: `PR #${status.pr.number}`, pushes: false };
  if ((status.aheadOfDefault ?? 0) > 0 && status.branch !== status.defaultBranch) return { kind: "createPr", label: "Create PR", pushes: true };
  return null;
}

/**
 * The branch's next steps (F-66), offered next to the commit button whatever the tree looks like: publish or sync,
 * and view or create the pull request. Only with `canPush`. A new PR needs a clean tree, a branch that is not the
 * default one, no divergence and commits ahead of the default branch (as on mobile).
 */
export function nextSteps(input: { changed: number; status: GitStatus | undefined; canPush: boolean }): BarAction[] {
  const { changed, status, canPush } = input;
  if (!canPush || !status?.remote) return [];
  const steps: BarAction[] = [];
  if (!status.upstream) steps.push({ kind: "publish", label: "Publish Branch", pushes: true });
  else if (status.ahead > 0 || status.behind > 0) steps.push({ kind: "sync", label: "Sync", pushes: true });
  if (status.pr) steps.push({ kind: "viewPr", label: `View PR #${status.pr.number}`, pushes: false });
  else if (
    !!status.defaultBranch &&
    status.branch !== status.defaultBranch &&
    !(status.ahead > 0 && status.behind > 0) &&
    changed === 0 &&
    (status.aheadOfDefault ?? 0) > 0 &&
    status.behind === 0
  ) {
    steps.push({ kind: "createPr", label: "Create PR", pushes: true });
  }
  return steps;
}

/**
 * The URL when it passes the same link allowlist as agent text (`classifyLink`: https, no userinfo, no whitespace or
 * non-ASCII characters), else null. A desktop reply is data, never trusted to be a safe link. Opening goes through
 * the confirm sheet (`usePullRequestOpener`), never straight to `window.open` (W13-7).
 */
export function safePrUrl(value: unknown): string | null {
  if (typeof value !== "string") return null;
  const target = classifyLink(value);
  return target?.kind === "external" && !target.insecure ? target.href : null;
}

export function formatCount(value: number): string {
  return value >= 1000 ? `${(value / 1000).toFixed(value >= 10_000 ? 0 : 1)}k` : String(value);
}

export function errorText(reason: unknown): string {
  return reason instanceof Error && reason.message ? reason.message : "Something went wrong";
}
