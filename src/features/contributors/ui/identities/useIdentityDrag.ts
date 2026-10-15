import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
} from "react";
import {
  setGrabbing,
  suppressTextSelection,
} from "../../../../shared/lib/drag";

/** Pointer travel before a press becomes a drag, so plain clicks keep working. */
const THRESHOLD = 4;
/** Elements carrying this attribute accept a drop; the value is the person id. */
export const DROP_ATTR = "data-person-drop";

/** Present only while an identity is being dragged. */
export type IdentityDrag = {
  key: string;
  x: number;
  y: number;
  /** Person whose drop zone is under the pointer, if any. */
  overPersonId: string | null;
};

function personAt(x: number, y: number): string | null {
  const zone = document.elementFromPoint(x, y)?.closest(`[${DROP_ATTR}]`);
  return zone?.getAttribute(DROP_ATTR) ?? null;
}

/**
 * Pointer-based drag of an identity onto a person. HTML5 drag-and-drop does not
 * reach the DOM in the Tauri webview (its native drop handler takes the drag),
 * so this follows the pointer itself, like the tab and pane drags do.
 */
export function useIdentityDrag({
  onDrop,
}: {
  onDrop: (key: string, personId: string) => void;
}) {
  const [drag, setDrag] = useState<IdentityDrag | null>(null);
  const onDropRef = useRef(onDrop);
  onDropRef.current = onDrop;
  const cleanup = useRef<(() => void) | null>(null);

  useEffect(() => () => cleanup.current?.(), []);

  const start = useCallback(
    (key: string, fromPersonId: string, event: ReactPointerEvent) => {
      if (event.button !== 0) return;
      if ((event.target as HTMLElement | null)?.closest("button, input, a")) {
        return;
      }
      cleanup.current?.();

      const { pointerId, clientX: startX, clientY: startY } = event;
      const handle = event.currentTarget as HTMLElement;
      let active = false;
      let restoreSelection: (() => void) | null = null;

      const finish = (commit: boolean, x: number, y: number) => {
        window.removeEventListener("pointermove", onMove);
        window.removeEventListener("pointerup", onUp);
        window.removeEventListener("pointercancel", onCancel);
        window.removeEventListener("keydown", onKey, true);
        window.removeEventListener("blur", onCancel);
        try {
          if (handle.hasPointerCapture?.(pointerId)) {
            handle.releasePointerCapture(pointerId);
          }
        } catch {
          /* already released */
        }
        if (active) {
          setGrabbing(false);
          restoreSelection?.();
        }
        cleanup.current = null;
        setDrag(null);
        if (!active || !commit) return;
        const target = personAt(x, y);
        if (target && target !== fromPersonId) onDropRef.current(key, target);
      };

      function onMove(move: PointerEvent) {
        if (move.pointerId !== pointerId) return;
        if (!active) {
          if (
            Math.hypot(move.clientX - startX, move.clientY - startY) < THRESHOLD
          ) {
            return;
          }
          active = true;
          restoreSelection = suppressTextSelection();
          setGrabbing(true);
          try {
            handle.setPointerCapture?.(pointerId);
          } catch {
            /* window listeners still track the gesture */
          }
        }
        move.preventDefault();
        setDrag({
          key,
          x: move.clientX,
          y: move.clientY,
          overPersonId: personAt(move.clientX, move.clientY),
        });
      }
      function onUp(up: PointerEvent) {
        if (up.pointerId !== pointerId) return;
        finish(true, up.clientX, up.clientY);
      }
      function onCancel() {
        finish(false, 0, 0);
      }
      function onKey(keyEvent: KeyboardEvent) {
        if (keyEvent.key !== "Escape") return;
        keyEvent.preventDefault();
        keyEvent.stopPropagation();
        finish(false, 0, 0);
      }

      cleanup.current = () => finish(false, 0, 0);
      window.addEventListener("pointermove", onMove);
      window.addEventListener("pointerup", onUp);
      window.addEventListener("pointercancel", onCancel);
      window.addEventListener("keydown", onKey, true);
      window.addEventListener("blur", onCancel);
    },
    [],
  );

  return { drag, start };
}
