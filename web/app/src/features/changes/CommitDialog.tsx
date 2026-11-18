// F-66: the commit sheet. A message, "Write a message" (the desktop writes it; "Stop writing" drops the
// answer on this device through a generation counter), Commit (never pushes), and "Commit and push" only
// when the desktop reports Remote push in Git.
import { useRef, useState } from "react";
import { Button } from "@/ui/button";
import { Icon } from "@/ui/Icon";
import { Notice } from "@/ui/Notice";
import { ResponsiveOverlay } from "@/ui/ResponsiveOverlay";
import { errorText, formatCount, type GitStatus } from "./gitModel";
import type { GitActionInput } from "./useGit";

type Busy = "generate" | "commit" | null;

export interface CommitDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  staged: number;
  additions: number;
  deletions: number;
  status: GitStatus | undefined;
  canGenerate: boolean;
  /** Offers "Commit and push" (Remote push in Git is on). */
  canPush: boolean;
  run: (input: GitActionInput) => Promise<{ message?: string; url?: string }>;
  /** After a commit, to read the repository again. */
  onCommitted: () => void;
  /** Asks to push with the commit: the caller confirms (remote, branch, default branch) and sends it. */
  onCommitAndPush: (message: string) => void;
}

export function CommitDialog({ open, onOpenChange, staged, additions, deletions, status, canGenerate, canPush, run, onCommitted, onCommitAndPush }: CommitDialogProps) {
  const [message, setMessage] = useState("");
  const [busy, setBusy] = useState<Busy>(null);
  const [error, setError] = useState<string | null>(null);
  // Bumped to drop a generation in flight; the desktop finishes it unseen.
  const generation = useRef(0);

  const hasMessage = message.trim().length > 0;
  const idle = busy === null;

  const generate = async () => {
    if (busy === "generate") {
      generation.current += 1;
      setBusy(null);
      return;
    }
    if (!idle || staged === 0) return;
    const mine = ++generation.current;
    setBusy("generate");
    setError(null);
    try {
      const result = await run({ action: "generate" });
      if (generation.current === mine && result.message) setMessage(result.message);
    } catch (reason) {
      if (generation.current === mine) setError(errorText(reason));
    } finally {
      if (generation.current === mine) setBusy(null);
    }
  };

  const commit = async () => {
    if (!idle || !hasMessage) return;
    setBusy("commit");
    setError(null);
    try {
      // No `push` and no `createPr`: this call stays a control action.
      await run({ action: "commit", message });
      setMessage("");
      onOpenChange(false);
      onCommitted();
    } catch (reason) {
      setError(errorText(reason));
    } finally {
      setBusy(null);
    }
  };

  const footer = (
    <>
      <Button type="button" variant="ghost" onClick={() => onOpenChange(false)} disabled={busy === "commit"}>
        Cancel
      </Button>
      {canGenerate ? (
        <Button type="button" variant="outline" onClick={() => void generate()} disabled={busy === "commit" || staged === 0} aria-busy={busy === "generate"}>
          <Icon name={busy === "generate" ? "close" : "sparkle"} size={16} />
          {busy === "generate" ? "Stop writing" : "Write a message"}
        </Button>
      ) : null}
      {canPush && status?.remote ? (
        <Button type="button" variant="outline" disabled={!idle || !hasMessage} onClick={() => onCommitAndPush(message)}>
          Commit and push
        </Button>
      ) : null}
      <Button type="button" disabled={!idle || !hasMessage} aria-busy={busy === "commit"} onClick={() => void commit()}>
        Commit
      </Button>
    </>
  );

  return (
    <ResponsiveOverlay
      open={open}
      onOpenChange={onOpenChange}
      title="Commit"
      description={`${status?.branch ? `${status.branch}: ` : ""}${staged} ${staged === 1 ? "file" : "files"} staged, +${formatCount(additions)} -${formatCount(deletions)}`}
      footer={footer}
    >
      <label className="flex flex-col gap-2 text-label font-medium">
        Commit message
        <textarea
          value={message}
          onChange={(event) => setMessage(event.target.value)}
          rows={5}
          placeholder={busy === "generate" ? "Writing a message..." : "Describe the change"}
          className="min-h-28 w-full resize-y rounded-row border border-stroke bg-fill px-4 py-3 text-row font-normal text-content placeholder:text-muted-text"
        />
      </label>
      {error ? (
        <Notice variant="error" className="mt-3">
          {error}
        </Notice>
      ) : null}
    </ResponsiveOverlay>
  );
}
