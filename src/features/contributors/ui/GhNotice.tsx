import { CircleAlert, Loader, RefreshCw } from "../../../shared/ui/icons";
import { SecondaryButton } from "../../../shared/ui/SecondaryButton";

/**
 * Explains a missing Review score when GitHub data is unavailable (a warning),
 * says the synced GitHub data is incomplete (`incomplete`), or shows the
 * note as information when GitHub answered.
 */
export function GhNotice({
  available = false,
  incomplete = false,
  note,
  retrying = false,
  onRetry,
}: {
  /** GitHub data was read; `note` is then a hint, not a failure. */
  available?: boolean;
  /** The sync could not read all of GitHub: the button retries only that step. */
  incomplete?: boolean;
  note?: string;
  retrying?: boolean;
  onRetry: () => void;
}) {
  const reason = note?.trim().replace(/[.\s]+$/, "");
  const warning = incomplete || !available;
  return (
    <div
      role="status"
      className={`mb-4 flex items-start gap-2.5 rounded-xl border px-3.5 py-2.5 text-[12px] text-content/70 ${
        warning
          ? "border-amber-400/30 bg-amber-400/5"
          : "border-stroke bg-content/3"
      }`}
    >
      <CircleAlert
        className={`mt-0.5 size-3.5 shrink-0 ${warning ? "text-amber-400" : "text-content/45"}`}
        strokeWidth={1.75}
      />
      <p className="min-w-0 flex-1 leading-relaxed">
        {incomplete ? (
          <>
            GitHub data is incomplete for this period.
            {reason ? ` ${reason}.` : ""}
            {available ? "" : " Review points are not counted."}
          </>
        ) : available ? (
          note?.trim()
        ) : (
          <>
            Review points are not counted because GitHub data is unavailable
            {reason ? `: ${reason}` : ""}. PRs are inferred from git history.
          </>
        )}
      </p>
      <SecondaryButton onClick={onRetry} disabled={retrying}>
        {retrying ? (
          <Loader className="size-3 animate-spin" strokeWidth={1.75} />
        ) : (
          <RefreshCw className="size-3" strokeWidth={1.75} />
        )}
        {incomplete ? "Retry GitHub" : "Retry"}
      </SecondaryButton>
    </div>
  );
}
