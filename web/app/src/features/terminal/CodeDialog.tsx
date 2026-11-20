// "Enter the code shown on your desktop": the re-auth of every open. The desktop shows a one-time code on its own screen
// (never in the browser); the user reads it there and types it here. Attempts and expiry are the desktop's; this dialog
// shows them and never sends a code of the wrong length.
import { useState, type FormEvent } from "react";
import { Button } from "@/ui/button";
import { DelayedSpinner } from "@/ui/DelayedSpinner";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/ui/dialog";
import { Input } from "@/ui/input";
import { Notice } from "@/ui/Notice";
import { CODE_RESTART_NOTICE } from "./copy";
import type { TerminalSnapshot } from "./controller";
import { clockText, useNow } from "./time";

export interface CodeDialogProps {
  snapshot: TerminalSnapshot;
  /** Where the shell will open, for the description. */
  where: string;
  onSubmit: (code: string) => void;
  onNewCode: () => void;
  onCancel: () => void;
}

export function CodeDialog({ snapshot, where, onSubmit, onNewCode, onCancel }: CodeDialogProps) {
  const { phase, challenge, codeExpired, notice } = snapshot;
  const open = phase === "starting" || phase === "code" || phase === "verifying";
  const verifying = phase === "verifying";
  const [code, setCode] = useState("");
  // A fresh challenge (a restart after a used-up code) starts with an empty field: remember which one the text is for.
  const [codeFor, setCodeFor] = useState<string | undefined>(undefined);
  const challengeId = challenge?.challengeId;
  if (codeFor !== challengeId) {
    setCodeFor(challengeId);
    setCode("");
  }
  const now = useNow(1000, phase === "code");
  const length = challenge?.codeLength ?? 6;
  const left = challenge ? Math.max(challenge.expiresAt - now, 0) : 0;
  const expired = codeExpired || (challenge !== null && phase === "code" && left <= 0);

  function submit(event: FormEvent): void {
    event.preventDefault();
    if (!verifying && !expired) onSubmit(code);
  }

  return (
    <Dialog open={open} onOpenChange={(next) => (!next && !verifying ? onCancel() : undefined)}>
      <DialogContent data-testid="terminal-code-dialog" showClose={!verifying} onInteractOutside={(event) => event.preventDefault()} aria-describedby="terminal-code-help">
        <form onSubmit={submit} className="grid gap-4">
          <DialogHeader>
            <DialogTitle>Enter the code shown on your desktop</DialogTitle>
            <DialogDescription id="terminal-code-help">
              Monoptah on your desktop shows a one-time code. Type it here to open a shell in {where}. The code is never sent to this page.
            </DialogDescription>
          </DialogHeader>
          {phase === "starting" || challenge === null ? (
            <p className="flex items-center gap-2 text-small text-muted-text" role="status">
              Asking your desktop for a code
              <DelayedSpinner label="Waiting" />
            </p>
          ) : (
            <div className="grid gap-2">
              <label htmlFor="terminal-code-input" className="text-label font-medium">
                Code ({length} characters)
              </label>
              <Input
                id="terminal-code-input"
                data-testid="terminal-code-input"
                value={code}
                onChange={(event) => setCode(event.target.value.replace(/\s+/g, "").toUpperCase().slice(0, length))}
                maxLength={length}
                inputMode="text"
                autoCapitalize="characters"
                autoComplete="off"
                autoCorrect="off"
                spellCheck={false}
                autoFocus
                disabled={verifying || expired}
                className="text-center font-mono text-title tracking-[0.4em] uppercase"
              />
              <p className="text-caption text-muted-text" role="timer">
                {expired ? "The code expired." : `Expires in ${clockText(left)}.`}
              </p>
            </div>
          )}
          {notice ? <Notice variant={notice === CODE_RESTART_NOTICE ? "info" : "error"}>{notice}</Notice> : null}
          <DialogFooter>
            <Button type="button" variant="ghost" disabled={verifying} onClick={onCancel}>
              Cancel
            </Button>
            {expired ? (
              <Button type="button" onClick={onNewCode} data-testid="terminal-new-code">
                Get a new code
              </Button>
            ) : (
              <Button type="submit" disabled={verifying || challenge === null || code.length !== length} aria-busy={verifying} data-testid="terminal-code-submit">
                Open terminal
                {verifying ? <DelayedSpinner label="Checking" /> : null}
              </Button>
            )}
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
