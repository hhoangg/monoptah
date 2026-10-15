import { CircleAlert, Loader, RefreshCw } from "../../../shared/ui/icons";
import { SecondaryButton } from "../../../shared/ui/SecondaryButton";

/**
 * Explains a missing Review score when GitHub data is unavailable (a warning),
 * or shows the collector's note as information when GitHub answered.
 */
export function GhNotice({
  available = false,
  note,
  retrying = false,
  onRetry,
}: {
  /** GitHub data was read; `note` is then a hint, not a failure. */
  available?: boolean;
  note?: string;
  retrying?: boolean;
  onRetry: () => void;
}) {
  const reason = note?.trim().replace(/[.\s]+$/, "");
  return (
    <div
      role="status"
      className={`mb-4 flex items-start gap-2.5 rounded-xl border px-3.5 py-2.5 text-[12px] text-content/70 ${
        available
          ? "border-stroke bg-content/3"
          : "border-amber-400/30 bg-amber-400/5"
      }`}
    >
      <CircleAlert
        className={`mt-0.5 size-3.5 shrink-0 ${available ? "text-content/45" : "text-amber-400"}`}
        strokeWidth={1.75}
      />
      <p className="min-w-0 flex-1 leading-relaxed">
        {available ? (
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
        Retry
      </SecondaryButton>
    </div>
  );
}
