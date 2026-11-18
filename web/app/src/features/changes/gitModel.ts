// Pure helpers of the Changes and Git screens: sections, the context-aware bottom action, status letters
// and PR URL validation. The bottom action follows mobile/src/features/changes/ChangesView.tsx `barAction`.
import type { ResultOf } from "@monoptah/protocol";
import { leafName } from "@/lib/fileKind";

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

/** The URL when it is an https URL, else null (a desktop reply is data, never trusted to be a safe link). */
export function safePrUrl(value: unknown): string | null {
  if (typeof value !== "string") return null;
  try {
    const url = new URL(value);
    return url.protocol === "https:" && url.hostname !== "" ? url.href : null;
  } catch {
    return null;
  }
}

/** Opens a validated URL in a new tab; true when it was allowed. Call it from a click handler. */
export function openPullRequest(value: unknown): boolean {
  const url = safePrUrl(value);
  if (!url) return false;
  window.open(url, "_blank", "noopener,noreferrer");
  return true;
}

export function formatCount(value: number): string {
  return value >= 1000 ? `${(value / 1000).toFixed(value >= 10_000 ? 0 : 1)}k` : String(value);
}

export function errorText(reason: unknown): string {
  return reason instanceof Error && reason.message ? reason.message : "Something went wrong";
}
