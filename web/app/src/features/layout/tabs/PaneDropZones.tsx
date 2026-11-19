// The drop zones over a pane while a tab is dragged: the middle moves the tab into the pane, an edge splits the pane
// and puts the tab in the new half (like dropping a tab on the edge of a pane in the native app).
import { useState, type DragEvent } from "react";
import { useTabDrag, endTabDrag, getTabDrag } from "./tabDrag";
import type { SplitDir } from "./paneTree";
import type { TabActions } from "./useTabActions";

type Zone = { kind: "center" } | { kind: "edge"; dir: SplitDir; place: "before" | "after" };

const EDGE = 0.25;

function zoneAt(event: DragEvent<HTMLElement>, allowed: (dir: SplitDir) => boolean): Zone {
  const rect = event.currentTarget.getBoundingClientRect();
  if (rect.width <= 0 || rect.height <= 0) return { kind: "center" };
  const x = (event.clientX - rect.left) / rect.width;
  const y = (event.clientY - rect.top) / rect.height;
  const candidates: Array<{ distance: number; zone: Zone }> = [
    { distance: x, zone: { kind: "edge", dir: "row", place: "before" } },
    { distance: 1 - x, zone: { kind: "edge", dir: "row", place: "after" } },
    { distance: y, zone: { kind: "edge", dir: "column", place: "before" } },
    { distance: 1 - y, zone: { kind: "edge", dir: "column", place: "after" } },
  ];
  const nearest = candidates.filter((candidate) => candidate.distance < EDGE && candidate.zone.kind === "edge" && allowed(candidate.zone.dir)).sort((a, b) => a.distance - b.distance)[0];
  return nearest?.zone ?? { kind: "center" };
}

const highlight: Record<string, string> = {
  "row:before": "inset-y-0 left-0 w-1/2",
  "row:after": "inset-y-0 right-0 w-1/2",
  "column:before": "inset-x-0 top-0 h-1/2",
  "column:after": "inset-x-0 bottom-0 h-1/2",
};

export function PaneDropZones({ desktopId, paneId, actions }: { desktopId: string; paneId: string; actions: TabActions }) {
  const drag = useTabDrag();
  const [zone, setZone] = useState<Zone | null>(null);
  if (!drag || drag.desktopId !== desktopId) return null;

  const onDragOver = (event: DragEvent<HTMLElement>) => {
    event.preventDefault();
    event.dataTransfer.dropEffect = "move";
    setZone(zoneAt(event, (dir) => actions.canSplit(dir, paneId)));
  };
  const onDrop = (event: DragEvent<HTMLElement>) => {
    event.preventDefault();
    const dragged = getTabDrag();
    const target = zoneAt(event, (dir) => actions.canSplit(dir, paneId));
    setZone(null);
    endTabDrag();
    if (!dragged) return;
    actions.drop(dragged.key, target.kind === "edge" ? { paneId, edge: { dir: target.dir, place: target.place } } : { paneId });
  };

  return (
    <div
      data-testid={`drop-zones-${paneId}`}
      onDragOver={onDragOver}
      onDragLeave={() => setZone(null)}
      onDrop={onDrop}
      className="absolute inset-0 z-30"
    >
      {zone ? (
        <div
          aria-hidden="true"
          data-zone={zone.kind === "edge" ? `${zone.dir}:${zone.place}` : "center"}
          className={`pointer-events-none absolute rounded-row border-2 border-brand bg-brand/10 ${zone.kind === "edge" ? (highlight[`${zone.dir}:${zone.place}`] ?? "") : "inset-0"}`}
        />
      ) : null}
    </div>
  );
}
