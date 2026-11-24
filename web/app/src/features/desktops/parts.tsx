// Small shared pieces of the desktop screens. Owner: WS-M (U-01).
import { Badge } from "@/ui/badge";
import { cn } from "@/lib/utils";
import { displayFingerprint } from "@/link/fingerprint";
import { isApiError } from "@/features/accounts/api";
import { fingerprintLines } from "@/features/accounts/pair/fingerprint";
import { settingsErrorMessage } from "@/features/settings/copy";
import { resolveDesktopColor } from "./colors";
import { presenceLabel } from "./format";

/**
 * A round color marker. The style only ever gets a palette hex (colors.ts), never the stored string.
 * Without a chosen color it is a hollow ring, so it never reads as a grey "offline" dot next to the presence badge.
 */
export function ColorDot({ color, className }: { color: string | null; className?: string }) {
  if (color === null) return <span aria-hidden="true" className={cn("inline-block size-3 shrink-0 rounded-pill border border-muted-text", className)} />;
  return <span aria-hidden="true" className={cn("inline-block size-3 shrink-0 rounded-pill", className)} style={{ backgroundColor: resolveDesktopColor(color).value }} />;
}

/** The small online/offline dot. Offline is muted-text, never faint, so it stays readable on the top bar. */
export function PresenceDot({ online, className }: { online: boolean; className?: string }) {
  return <span aria-hidden="true" className={cn("size-1.5 rounded-pill", online ? "bg-additions" : "bg-muted-text", className)} />;
}

export function PresenceBadge({ online }: { online: boolean }) {
  return (
    <Badge variant={online ? "default" : "outline"} className="gap-1">
      <PresenceDot online={online} />
      {presenceLabel(online)}
    </Badge>
  );
}

/** A fingerprint in its display form (8 groups of 4), as text only. */
export function Fingerprint({ hex, className }: { hex: string; className?: string }) {
  return <span className={cn("font-mono text-meta break-words text-content", className)}>{displayFingerprint(hex)}</span>;
}

/** The one presentation of a fingerprint to compare: two lines of four groups, mono, never wrapped inside a line. */
export function FingerprintLines({ hex, className }: { hex: string; className?: string }) {
  const [lineOne, lineTwo] = fingerprintLines(hex);
  return (
    <span className={cn("block font-mono text-label whitespace-nowrap text-content tabular-nums", className)}>
      <span className="block">{lineOne} </span>
      <span className="block">{lineTwo}</span>
    </span>
  );
}

/** One line for a failed desktop call. */
export function desktopErrorMessage(error: unknown): string {
  if (isApiError(error, "not_found")) return "That item no longer exists. Reload the page to see the current list.";
  if (isApiError(error, "conflict")) return "That change is no longer possible, because the access was revoked.";
  return settingsErrorMessage(error);
}
