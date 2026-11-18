// W-06 and W-08: where a session runs and in which state, as text. The list card shows it only for a
// session that is not local (a local session shows nothing, which keeps the list calm); the header chip
// is live from `useCloud` and falls back to the catalog card.
import type { SessionCard } from "@monoptah/protocol";
import { useCloud } from "@/data";
import { cn } from "@/lib/utils";
import { Badge } from "@/ui/badge";
import { useCapability } from "@/version";
import { isNonLocalKind, kindLabel, placementOf, stateLabel, type Placement } from "../model";
import { useLiveCloudState } from "./liveState";

/** Pure view: "GitHub codespace" and "Available" as text. Local or unknown placements render nothing. */
export function CloudChip({ placement, className }: { placement: Placement | null; className?: string }) {
  if (!placement || !isNonLocalKind(placement.kind)) return null;
  const state = placement.state ? stateLabel(placement.state) : null;
  return (
    <Badge variant="outline" data-testid="cloud-chip" data-state={placement.state ?? "none"} className={cn("max-w-full gap-1 truncate font-normal", className)}>
      <span className="truncate">{kindLabel(placement.kind)}</span>
      {state ? <span aria-hidden="true">-</span> : null}
      {state ? <span>{state}</span> : null}
    </Badge>
  );
}

/** `session-card-badge` slot: the catalog card's own target and state. */
export function SessionCardBadge({ session }: { session: SessionCard }) {
  const cloud = useCapability("cloud");
  if (!cloud) return null;
  return <CloudChip placement={placementOf(session)} />;
}

/** `session-header-chip` slot: live state from the conversation's `cloud` field. */
export function SessionHeaderChip({ session }: { session: SessionCard }) {
  const enabled = useCapability("cloud");
  const cloud = useCloud(session.id);
  const placement = placementOf(session, cloud);
  useLiveCloudState(session.id, placement?.state);
  if (!enabled) return null;
  return <CloudChip placement={placement} />;
}
