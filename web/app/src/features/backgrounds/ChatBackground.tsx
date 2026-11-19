import { cn } from "@/lib/utils";
import { useStoredBackground } from "./useBackground";

export interface ChatBackgroundProps {
  /** The project whose stored background to show; null (a chat with no project) shows nothing. */
  projectId: string | null | undefined;
  className?: string;
}

/** The classes of the container the background is mounted in: `isolate` keeps its `-z-10` inside, so it never slips behind the page. */
export const BACKGROUND_CONTAINER_CLASS = "relative isolate";

/**
 * The project's stored background (F-90) for every chat view: the New session page, an existing session and a Mono
 * chat. Mount it as the first child of a container with `BACKGROUND_CONTAINER_CLASS`, behind the content. It ignores
 * pointer events and is hidden from assistive tech. Renders nothing when the project has none.
 *
 * The photo is drawn at the opacity the user chose, under a scrim of the page background colour at 75%. The scrim
 * follows the theme (light in light mode, dark in dark mode), so transcript text, bubbles and the composer keep their
 * contrast on top of a photo in both themes; the photo stays visible, mostly at the pane edges and in the gaps. At 50%
 * the muted text of the transcript measured about 2.7:1 over a dark photo (light theme) or a bright one (dark theme).
 */
export function ChatBackground({ projectId, className }: ChatBackgroundProps) {
  const loaded = useStoredBackground(projectId ?? "");
  if (!projectId || !loaded) return null;
  const { record, url } = loaded;
  return (
    <>
      <img
        src={url}
        alt=""
        aria-hidden
        draggable={false}
        data-testid="chat-background"
        style={{ opacity: record.opacity }}
        className={cn("pointer-events-none absolute inset-0 -z-10 size-full object-cover object-top select-none", className)}
      />
      <div aria-hidden data-testid="chat-background-scrim" className="pointer-events-none absolute inset-0 -z-10 bg-background/75" />
    </>
  );
}
