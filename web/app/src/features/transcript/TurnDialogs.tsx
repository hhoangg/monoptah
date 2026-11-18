// The two dialogs a transcript opens: the error details of a notice (F-41) and a turn's token metrics (F-38).
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/ui/dialog";
import { Button } from "@/ui/button";
import { Icon } from "@/ui/Icon";
import { turnMetricsSheet } from "./format";
import { sessionErrorContent } from "./sessionError";
import type { TurnMetrics } from "./turnItems";

export function SessionErrorDialog({ message, open, onOpenChange }: { message: string; open: boolean; onOpenChange: (open: boolean) => void }) {
  const content = sessionErrorContent(message);
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent aria-describedby={undefined} data-testid="session-error-dialog">
        <DialogHeader className="items-center gap-3 text-center">
          <span className="grid size-16 place-items-center rounded-full bg-surface text-brand">
            <Icon name={content.recovery ? "lock" : "chat"} size={28} />
          </span>
          <DialogTitle>{content.title}</DialogTitle>
        </DialogHeader>
        <div className="max-h-[40dvh] overflow-y-auto">
          <p className="text-small whitespace-pre-wrap text-muted-text select-text wrap-anywhere">{content.message}</p>
          {content.recovery ? (
            <div className="mt-4 flex items-start gap-3 rounded-row bg-surface p-3">
              <Icon name="computer" size={20} className="mt-0.5 shrink-0 text-muted-text" />
              <div>
                <p className="text-small font-semibold">Switch accounts</p>
                <p className="text-small text-muted-text">{content.recovery}</p>
              </div>
            </div>
          ) : null}
        </div>
        <Button onClick={() => onOpenChange(false)}>Got it</Button>
      </DialogContent>
    </Dialog>
  );
}

export function TurnMetricsDialog({
  metrics,
  elapsedMs,
  open,
  onOpenChange,
}: {
  metrics: TurnMetrics;
  elapsedMs: number | null;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const { highlights, tokens } = turnMetricsSheet(metrics, elapsedMs);
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent aria-describedby={undefined} data-testid="turn-metrics-dialog">
        <DialogHeader>
          <DialogTitle>Turn metrics</DialogTitle>
        </DialogHeader>
        {highlights.length > 0 ? (
          <dl className="grid grid-cols-3 gap-2">
            {highlights.map((stat) => (
              <div key={stat.label} className="rounded-row bg-surface p-3">
                <dt className="text-caption text-muted-text">{stat.label}</dt>
                <dd className="text-h font-semibold">
                  {stat.value}
                  {stat.unit ? <span className="ml-0.5 text-caption font-normal text-muted-text">{stat.unit}</span> : null}
                </dd>
              </div>
            ))}
          </dl>
        ) : null}
        {tokens.length > 0 ? (
          <dl className="divide-y divide-list-divider">
            {tokens.map((stat) => (
              <div key={stat.label} className="flex justify-between py-2 text-small">
                <dt className="text-muted-text">{stat.label}</dt>
                <dd className="font-medium tabular-nums">{stat.value}</dd>
              </div>
            ))}
          </dl>
        ) : null}
      </DialogContent>
    </Dialog>
  );
}

/** Shown when the clipboard refuses the copy (no permission, insecure context): the text to select by hand (F-38). */
export function CopyFallbackDialog({ text, open, onOpenChange }: { text: string; open: boolean; onOpenChange: (open: boolean) => void }) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent data-testid="copy-fallback-dialog">
        <DialogHeader>
          <DialogTitle>Copy the response</DialogTitle>
          <DialogDescription>Your browser did not allow copying. Select the text below and copy it.</DialogDescription>
        </DialogHeader>
        <textarea
          readOnly
          value={text}
          aria-label="Response text"
          rows={10}
          onFocus={(event) => event.currentTarget.select()}
          className="w-full resize-none rounded-row border border-stroke bg-surface p-3 text-small text-content"
        />
      </DialogContent>
    </Dialog>
  );
}
