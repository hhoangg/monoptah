import { AlertCircle } from "../../../shared/ui/icons";
import { asSentence } from "../model/mailText";
import type { MailStatus } from "../model/mailTypes";

/** Why there is nothing to list yet, when the cause is the setup. */
export type SourceSetup = "no-source" | "all-off" | "no-mailbox";

/** Null while status loads, and once there is a mailbox to list from. */
export function sourceSetup(status: MailStatus | null): SourceSetup | null {
  if (!status) return null;
  if (status.sources.length === 0) return "no-source";
  const enabled = status.sources.filter((s) => s.enabled);
  if (enabled.length === 0) return "all-off";
  if (enabled.every((s) => s.mailboxes.length === 0)) return "no-mailbox";
  return null;
}

export const SETUP_COPY: Record<SourceSetup, { title: string; hint: string }> =
  {
    "no-source": {
      title: "No mail source yet",
      hint: "Add an AWS source in Settings, under Mail, and Monoptah will list its mail here.",
    },
    "all-off": {
      title: "Every mail source is turned off",
      hint: "Turn one on in Settings, under Mail, to see its mail here.",
    },
    "no-mailbox": {
      title: "Your sources have no mailboxes",
      hint: "Add a mailbox to a source in Settings, under Mail.",
    },
  };

/**
 * A failing source does not empty the list: the mail already synced is still
 * there and still readable. This says so, so a quiet inbox is not mistaken for
 * a healthy one. Disabled sources do not sync, so their old error is not news.
 */
export function MailSourceErrors({ status }: { status: MailStatus | null }) {
  const failing =
    status?.sources.filter((s) => s.enabled && s.lastError !== null) ?? [];
  if (failing.length === 0) return null;
  return (
    <div
      role="alert"
      className="flex shrink-0 flex-col gap-1 border-b border-stroke bg-red-400/5 px-3 py-2 text-[12px]"
    >
      {failing.map((source) => (
        <p key={source.id} className="flex items-start gap-1.5 text-red-400/90">
          <AlertCircle className="mt-0.5 size-3 shrink-0" strokeWidth={1.75} />
          <span className="min-w-0 break-words">
            {source.label} could not sync: {asSentence(source.lastError ?? "")}{" "}
            Mail already downloaded is still shown, and may be out of date.
          </span>
        </p>
      ))}
    </div>
  );
}
