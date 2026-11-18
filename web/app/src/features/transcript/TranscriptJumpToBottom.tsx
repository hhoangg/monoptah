// Copied from src/features/sessions/ui/TranscriptJumpToBottom.tsx at ca73ac2b
import { useState, useSyncExternalStore } from "react";
import { Glass } from "@/ui/Glass";
import { ChevronDown } from "@/ui/icons";

/** The reader counts as "away from the end" once the gap to the bottom exceeds this (F-33). */
export const JUMP_TO_BOTTOM_THRESHOLD_PX = 160;

/** True when the scroller is more than the threshold above its end. */
export function isAwayFromBottom(
  scroller: Pick<HTMLElement, "scrollHeight" | "scrollTop" | "clientHeight">,
  threshold = JUMP_TO_BOTTOM_THRESHOLD_PX,
): boolean {
  return scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight > threshold;
}

/** Scroll events update the button without rerendering the session pane. */
export function useTranscriptJumpVisibility() {
  const [visibility] = useState(() => {
    let visible = false;
    const listeners = new Set<() => void>();
    return {
      getSnapshot: () => visible,
      subscribe: (listener: () => void) => {
        listeners.add(listener);
        return () => {
          listeners.delete(listener);
        };
      },
      setVisible: (next: boolean) => {
        if (visible === next) return;
        visible = next;
        for (const listener of listeners) listener();
      },
    };
  });
  return visibility;
}

export function TranscriptJumpToBottom({
  visibility,
  onJump,
}: {
  visibility: ReturnType<typeof useTranscriptJumpVisibility>;
  onJump: () => void;
}) {
  const visible = useSyncExternalStore(visibility.subscribe, visibility.getSnapshot, visibility.getSnapshot);
  if (!visible) return null;
  return (
    <div className="pointer-events-none absolute inset-x-0 bottom-2 z-30 flex justify-center">
      <Glass
        pressable
        className="pointer-events-auto size-9 rounded-full"
      >
        <button
          type="button"
          title="Jump to latest"
          aria-label="Jump to latest"
          data-jump-to-bottom
          onClick={onJump}
          className="grid size-full place-items-center rounded-full text-content focus-visible:outline-2 focus-visible:outline-brand"
        >
          <ChevronDown size={18} strokeWidth={2} />
        </button>
      </Glass>
    </div>
  );
}
