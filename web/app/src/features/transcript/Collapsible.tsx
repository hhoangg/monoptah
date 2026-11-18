import type { ReactNode } from "react";
import { cn } from "@/lib/utils";

/** Height animation by grid rows (no measuring). Closed content is `inert` and hidden from assistive tech. */
export function Collapsible({ open, children }: { open: boolean; children: ReactNode }) {
  return (
    <div
      className={cn("grid transition-[grid-template-rows] duration-200 ease-out motion-reduce:transition-none", open ? "grid-rows-[1fr]" : "grid-rows-[0fr]")}
      aria-hidden={!open}
      inert={!open}
    >
      <div className="min-h-0 overflow-hidden">{children}</div>
    </div>
  );
}
