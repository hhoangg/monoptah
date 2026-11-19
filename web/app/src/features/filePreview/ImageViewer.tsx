// An image from the desktop in a viewer with zoom: buttons, Ctrl/Cmd + wheel (a trackpad pinch sends it) and a
// two-finger pinch. The bytes become a blob: URL shown through `<img>` only, also for svg (never inline, object or
// iframe), so an svg cannot run anything.
import { useEffect, useRef, useState } from "react";
import { Icon } from "@/ui/Icon";
import { cn } from "@/lib/utils";
import { useBlobUrl } from "./useMentionedFile";

const MIN_WIDTH = 24;
const MAX_WIDTH = 30_000;
const STEP = 1.25;
const clamp = (width: number) => Math.min(MAX_WIDTH, Math.max(MIN_WIDTH, width));

const BUTTON = "inline-flex min-h-11 min-w-11 items-center justify-center rounded-pill px-3 text-label text-content hover:bg-selection pointer-fine:min-h-9 pointer-fine:min-w-9";

export function ImageViewer({ chunks, mime, name, className }: { chunks: BlobPart[]; mime: string; name: string; className?: string }) {
  const url = useBlobUrl(chunks, mime);
  /** `null` is "fit": the image as wide as the pane and no taller than it. */
  const [width, setWidth] = useState<number | null>(null);
  const pane = useRef<HTMLDivElement>(null);
  const image = useRef<HTMLImageElement>(null);
  const pointers = useRef(new Map<number, { x: number; y: number }>());
  const pinch = useRef<{ distance: number; width: number } | null>(null);

  const shownWidth = () => width ?? (image.current?.getBoundingClientRect().width || pane.current?.clientWidth || image.current?.naturalWidth || 300);
  const zoomBy = (factor: number) => setWidth(clamp(shownWidth() * factor));

  // A trackpad pinch arrives as a wheel event with ctrlKey; it must not zoom the whole page.
  useEffect(() => {
    const element = pane.current;
    if (!element) return;
    const onWheel = (event: WheelEvent) => {
      if (!event.ctrlKey && !event.metaKey) return;
      event.preventDefault();
      setWidth(clamp(shownWidth() * Math.exp(-event.deltaY * 0.01)));
    };
    element.addEventListener("wheel", onWheel, { passive: false });
    return () => element.removeEventListener("wheel", onWheel);
  });

  const distance = () => {
    const [a, b] = [...pointers.current.values()];
    return a && b ? Math.hypot(a.x - b.x, a.y - b.y) : 0;
  };

  return (
    <div className={cn("flex min-h-0 flex-1 flex-col gap-2", className)} data-testid="preview-image-viewer">
      <div role="toolbar" aria-label="Image zoom" className="flex shrink-0 items-center gap-1">
        <button type="button" aria-label="Zoom out" data-testid="preview-zoom-out" className={BUTTON} onClick={() => zoomBy(1 / STEP)}>
          <Icon name="minus" size={18} />
        </button>
        <button type="button" aria-label="Zoom in" data-testid="preview-zoom-in" className={BUTTON} onClick={() => zoomBy(STEP)}>
          <Icon name="add" size={18} />
        </button>
        <button type="button" aria-label="Fit to window" data-testid="preview-zoom-fit" className={BUTTON} onClick={() => setWidth(null)}>
          Fit
        </button>
        <button
          type="button"
          aria-label="Actual size"
          data-testid="preview-zoom-actual"
          className={BUTTON}
          onClick={() => setWidth(image.current?.naturalWidth ? clamp(image.current.naturalWidth) : null)}
        >
          100%
        </button>
      </div>
      <div
        ref={pane}
        data-testid="preview-image-pane"
        className="flex min-h-0 flex-1 touch-pan-x touch-pan-y overflow-auto rounded-row border border-stroke bg-fill"
        onPointerDown={(event) => {
          pointers.current.set(event.pointerId, { x: event.clientX, y: event.clientY });
          if (pointers.current.size === 2) pinch.current = { distance: distance(), width: shownWidth() };
        }}
        onPointerMove={(event) => {
          if (!pointers.current.has(event.pointerId)) return;
          pointers.current.set(event.pointerId, { x: event.clientX, y: event.clientY });
          const start = pinch.current;
          if (pointers.current.size === 2 && start && start.distance > 0) setWidth(clamp((start.width * distance()) / start.distance));
        }}
        onPointerUp={(event) => {
          pointers.current.delete(event.pointerId);
          pinch.current = null;
        }}
        onPointerCancel={(event) => {
          pointers.current.delete(event.pointerId);
          pinch.current = null;
        }}
      >
        {/* A blob: URL made here from the bytes; never a URL from the message. */}
        <img
          ref={image}
          src={url}
          alt={name}
          data-testid="preview-image"
          data-zoom={width === null ? "fit" : String(Math.round(width))}
          draggable={false}
          className="m-auto block max-w-none object-contain"
          style={width === null ? { width: "100%", maxHeight: "100%" } : { width }}
        />
      </div>
    </div>
  );
}
