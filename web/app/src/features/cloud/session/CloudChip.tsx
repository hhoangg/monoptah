// W-06 and W-08: where a session runs and in which state, as text. The list card shows it only for a
// session that is not local (a local session shows nothing, which keeps the list calm); the header chip
// is live from `useCloud` and falls back to the catalog card.
import { useState } from "react";
import type { SessionCard } from "@monoptah/protocol";
import { useCatalog, useCloud } from "@/data";
import { cn } from "@/lib/utils";
import { Badge } from "@/ui/badge";
import { useCapability } from "@/version";
import { isNonLocalKind, kindLabel, placementOf, placementSources, stateLabel, type Placement } from "../model";
import { useLivePlacement } from "./liveState";

/** Pure view: "GitHub codespace" and "Available" as text. Local or unknown placements render nothing. */
export function CloudChip({ placement, className }: { placement: Placement | null; className?: string }) {
  if (!placement || !isNonLocalKind(placement.kind)) return null;
  const state = placement.state ? stateLabel(placement.state) : null;
  return (
    <Badge variant="outline" data-testid="cloud-chip" data-state={placement.state ?? "none"} className={cn("min-w-0 max-w-full gap-1 overflow-hidden font-normal", className)}>
      {/* The kind gives way first and ends in an ellipsis; the state and the dash never clip mid-word. */}
      <span className="min-w-0 truncate">{kindLabel(placement.kind)}</span>
      {state ? <span aria-hidden="true" className="shrink-0">-</span> : null}
      {state ? <span className="shrink-0">{state}</span> : null}
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
export function SessionHeaderChip({ desktopId, session }: { desktopId: string; session: SessionCard }) {
  const enabled = useCapability("cloud");
  const cloud = useCloud(session.id);
  const placement = placementOf(session, cloud);
  const catalogFresh = useCatalog().dataUpdatedAt > 0;
  useLivePlacement(desktopId, session.id, placementSources(session, cloud, catalogFresh));
  const visible = placement !== null && isNonLocalKind(placement.kind);
  const text = visible ? (placement.state ? `${kindLabel(placement.kind)}: ${stateLabel(placement.state)}` : kindLabel(placement.kind)) : "";
  // Announce a change, not the first text: the region mounts empty and fills only after the text changed.
  const [seen, setSeen] = useState({ text, changed: false });
  if (seen.text !== text) setSeen({ text, changed: true });
  if (!enabled) return null;
  return (
    <>
      <CloudChip placement={placement} />
      {/* A state change (Starting to Available) is announced, not only repainted. */}
      {visible ? (
        <span role="status" aria-live="polite" className="sr-only">
          {seen.changed ? text : ""}
        </span>
      ) : null}
    </>
  );
}
