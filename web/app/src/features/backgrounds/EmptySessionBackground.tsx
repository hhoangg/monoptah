import { cn } from "@/lib/utils";
import { useStoredBackground } from "./useBackground";

export interface EmptySessionBackgroundProps {
  projectId: string;
  className?: string;
}

/** The classes of the container the background is mounted in: `isolate` keeps its `-z-10` inside, so it never slips behind the page. */
export const BACKGROUND_CONTAINER_CLASS = "relative isolate";

/**
 * The project's stored background (F-90), for the empty-session screen. Absolutely positioned: mount it as the
 * first child of a container with `BACKGROUND_CONTAINER_CLASS`, behind the content. It ignores pointer events and is hidden from
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
