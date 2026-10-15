import { LAYER } from "../../../../shared/lib/layers";
import type { IdentityDrag } from "./useIdentityDrag";

/** The identity label that follows the pointer during a drag. */
export function DragGhost({ drag }: { drag: IdentityDrag }) {
  return (
    <div
      aria-hidden
      data-identity-ghost
      style={{
        position: "fixed",
        left: drag.x + 12,
        top: drag.y + 12,
        zIndex: LAYER.popover,
      }}
      className="pointer-events-none max-w-64 truncate rounded-md border border-accent/60 bg-background-base px-2 py-1 font-mono text-[12px] text-content shadow-xl"
    >
      {drag.key}
    </div>
  );
}
