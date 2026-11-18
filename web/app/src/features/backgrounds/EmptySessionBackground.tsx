import { cn } from "@/lib/utils";
import { useStoredBackground } from "./useBackground";

export interface EmptySessionBackgroundProps {
  projectId: string;
  className?: string;
}

/**
 * The project's stored background (F-90), for the empty-session screen. Absolutely positioned: mount it as the
 * first child of a `relative` container, behind the content. It ignores pointer events and is hidden from
 * assistive tech. Renders nothing when the project has none.
 */
export function EmptySessionBackground({ projectId, className }: EmptySessionBackgroundProps) {
  const loaded = useStoredBackground(projectId);
  if (!loaded) return null;
  const { record, url } = loaded;
  return (
    <img
      src={url}
      alt=""
      aria-hidden
      draggable={false}
      data-testid="empty-session-background"
      style={{ opacity: record.opacity }}
      className={cn("pointer-events-none absolute inset-0 -z-10 size-full object-cover object-top select-none", className)}
    />
  );
}
