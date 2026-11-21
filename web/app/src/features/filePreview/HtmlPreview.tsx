// An html report in a sandbox. The document is the desktop's sanitized html; the page shows at once, with a loading
// placeholder where an image is still being read, and each image is swapped in as it arrives (`loadHtmlAssets`, see
// mentionedFile.ts: the images are Blob parts of the document, not one huge string). It is loaded from a blob: URL into an iframe with `sandbox=""`: no
// script, no same-origin, no popups, no top navigation, no forms. The app CSP allows `frame-src blob:` for exactly this.
// The html is never parsed into the app's DOM (no innerHTML, no srcdoc).
//
// The sandboxed frame cannot be reached from here (no script runs in it), so a new image means a new document: updates
// are batched (at most one reload per `UPDATE_EVERY_MS`) and the last one comes when the final image settles.
import { useEffect, useRef, useState } from "react";
import { Button } from "@/ui/button";
import { blobBytes, SaveButton, safeDownloadName } from "./download";
import { ImageLightbox, type LightboxImage } from "./ImageLightbox";
import { formatSize, imageMime, loadHtmlAssets, pendingHtmlView, type HtmlAssetInfo, type HtmlAssetsLoad, type HtmlAssetsView, type MentionSource } from "./mentionedFile";

/** The shortest time between two reloads of the frame while images arrive. */
export const UPDATE_EVERY_MS = 700;

/** The blob: URL of `html`: a new one (and the old one revoked a moment later) whenever the document changes. */
function useDocumentUrl(html: Blob): string {
  const [url, setUrl] = useState(() => URL.createObjectURL(html));
  const made = useRef({ html, url });
  const pending = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  useEffect(() => {
    if (made.current.html === html) return;
    const previous = made.current.url;
    const next = URL.createObjectURL(html);
    made.current = { html, url: next };
    setUrl(next);
    // After the new document has replaced it in the frame.
    setTimeout(() => URL.revokeObjectURL(previous), 5_000);
  }, [html]);
  useEffect(() => {
    clearTimeout(pending.current);
    return () => {
      const last = made.current.url;
      // A moment later, so a StrictMode unmount followed by an immediate remount keeps its URL.
      pending.current = setTimeout(() => URL.revokeObjectURL(last), 0);
    };
  }, []);
  return url;
}

/** The images a page names before any is read: all loading. */
function pendingAssets(page: { text: string; assets: readonly string[] }): HtmlAssetInfo[] {
  const used = [...new Set([...page.text.matchAll(/monoptah-asset:(\d+)/g)].map((match) => Number(match[1])))].filter((index) => index < page.assets.length);
  return used.map((index) => ({ path: page.assets[index] as string, state: "loading" as const }));
}

/** A small hash of `text`, to tell one page from another without keeping both. */
function hashOf(text: string): string {
  let hash = 2166136261;
  for (let i = 0; i < text.length; i++) hash = Math.imul(hash ^ text.charCodeAt(i), 16777619) >>> 0;
  return `${text.length}:${hash}`;
}

/** Hover (pointer entering the frame) counts as the reader's only after the page has been on screen this long. */
const HOVER_GRACE_MS = 800;
/** Where a touch cannot be seen (nothing inside the frame reaches the app), the page stops reloading by itself after this. */
const COARSE_AUTO_WINDOW_MS = 3_000;

/**
 * Another page (other text or images) is another preview: it starts from its own loading page, with nothing carried over,
 * so the old page's images and counts never show for the new one.
 */
export function HtmlPreview(props: { source: MentionSource; path: string; name: string; text: string; assets: readonly string[] }) {
  return <HtmlPreviewOf key={`${props.path}\0${hashOf(props.text)}\0${props.assets.join("\0")}`} {...props} />;
}

function HtmlPreviewOf({ source, path, name, text, assets }: { source: MentionSource; path: string; name: string; text: string; assets: readonly string[] }) {
  const [view, setView] = useState<HtmlAssetsView>(() => pendingHtmlView({ text, assets }));
  // The counts and the waiting notice follow every change at once; only the document waits for its turn.
  const [showImages, setShowImages] = useState(false);
  const [opened, setOpened] = useState<LightboxImage | null>(null);
  const [status, setStatus] = useState<ReturnType<HtmlAssetsLoad["status"]>>({ skipped: 0, pending: 0, waiting: false, assets: pendingAssets({ text, assets }) });
  // Images settled that the frame does not show yet (once the reader has taken over the scroll position).
  const [ready, setReady] = useState(0);
  const frame = useRef<HTMLIFrameElement>(null);
  const mountedAt = useRef(0);
  const firstLoadAt = useRef<number | null>(null);
  const touched = useRef(false);
  const hovering = useRef(false);
  const control = useRef({ onLoad: () => undefined as void, show: () => undefined as void });
  // Seen from here: the pointer over the frame, or focus moving into it. A scroll or a touch inside it is not (no event leaves the frame).
  useEffect(() => {
    const element = frame.current;
    if (!element) return;
    const hover = () => {
      hovering.current = true;
      if (firstLoadAt.current !== null && Date.now() - firstLoadAt.current >= HOVER_GRACE_MS) touched.current = true;
    };
    const leave = () => {
      hovering.current = false;
    };
    const focus = () => {
      if (document.activeElement === element) touched.current = true;
    };
    element.addEventListener("pointerenter", hover);
    element.addEventListener("pointerover", hover);
    element.addEventListener("pointerleave", leave);
    window.addEventListener("blur", focus);
    return () => {
      element.removeEventListener("pointerenter", hover);
      element.removeEventListener("pointerover", hover);
      element.removeEventListener("pointerleave", leave);
      window.removeEventListener("blur", focus);
    };
  }, []);
  useEffect(() => {
    mountedAt.current = Date.now();
    let timer: ReturnType<typeof setTimeout> | undefined;
    let frameReady = false;
    let dirty = false;
    let last = Date.now();
    let settled = 0;
    let shown = 0;
    const coarse = typeof window.matchMedia === "function" && window.matchMedia("(pointer: coarse)").matches;
    // Each new document makes the frame start from the top, so it replaces the one shown only while the reader has not
    // taken over (hovered or focused the frame, or on a touch screen after a short while): the first quick swaps and the
    // last one when everything settled. After that a button offers the new document, and it replaces the frame once, when pressed.
    const mayReload = () => !touched.current && !(coarse && Date.now() - mountedAt.current > COARSE_AUTO_WINDOW_MS);
    const showNow = () => {
      dirty = false;
      frameReady = false;
      last = Date.now();
      shown = settled;
      setReady(0);
      setView(load.view());
    };
    // A new document only replaces the frame's once the frame has loaded the one before (WebKit drops a blob: navigation
    // started while another is still loading), and at most once per UPDATE_EVERY_MS. A frame that never reports its load
    // is not waited for beyond 3 s.
    const apply = () => {
      timer = undefined;
      if (!dirty) return;
      if (!mayReload()) return setReady(settled - shown);
      if (!frameReady && Date.now() - last < 3_000) return schedule(300);
      showNow();
    };
    function schedule(wait = Math.max(0, last + UPDATE_EVERY_MS - Date.now())) {
      dirty = true;
      if (timer === undefined) timer = setTimeout(apply, wait);
    }
    const load = loadHtmlAssets(source, path, { text, assets }, {
      onChange: () => {
        const now = load.status();
        setStatus(now);
        settled = now.assets.length - now.pending;
        schedule();
      },
    });
    control.current = {
      onLoad: () => {
        if (firstLoadAt.current === null) {
          firstLoadAt.current = Date.now();
          // A pointer that was already over the frame when the page appeared counts once the grace is over.
          setTimeout(() => {
            if (hovering.current) touched.current = true;
          }, HOVER_GRACE_MS);
        }
        frameReady = true;
        if (dirty) schedule();
      },
      show: () => {
        if (dirty) showNow();
      },
    };
    return () => {
      clearTimeout(timer);
      load.dispose();
    };
  }, [source, path, text, assets]);
  const url = useDocumentUrl(view.document);
  const notLoaded = status.skipped === 1 ? "1 image could not be loaded and is shown as a placeholder." : `${status.skipped} images could not be loaded and are shown as placeholders.`;
  return (
    <div className="relative flex min-h-0 min-w-0 flex-1 flex-col gap-2" data-testid="preview-html-frame">
      {/* One compact row for everything about the page's images: the list, and short status chips (the long text is in their title). It never wraps, so the frame keeps its height. */}
      {status.assets.length > 0 ? (
        <div className="flex h-9 shrink-0 items-center gap-2 overflow-hidden whitespace-nowrap" data-testid="preview-html-controls">
          <Button type="button" variant="outline" size="sm" className="shrink-0" aria-expanded={showImages} data-testid="preview-html-images-toggle" onClick={() => setShowImages((now) => !now)}>
            {`Images (${status.assets.length})`}
          </Button>
          <div className="flex min-w-0 flex-1 items-center gap-2 overflow-hidden text-small text-muted-text">
            {status.waiting ? (
              <span data-testid="file-preview-waiting" className="truncate" title="Waiting for your desktop. It is busy or limiting how fast files open; the images load when it is ready.">
                Waiting...
              </span>
            ) : null}
            {status.skipped > 0 ? (
              <span data-testid="preview-skipped-notice" className="truncate" title={notLoaded}>
                {`${status.skipped} not loaded`}
              </span>
            ) : null}
            {ready > 0 ? (
              <span data-testid="preview-html-ready" className="truncate" title="Images that arrived after you started reading. Showing them reloads the page from the top.">
                {`${ready} ready`}
              </span>
            ) : null}
          </div>
          {ready > 0 ? (
            <Button type="button" variant="outline" size="sm" className="shrink-0" data-testid="preview-html-ready-show" onClick={() => control.current.show()}>
              Show
            </Button>
          ) : null}
        </div>
      ) : null}
      {showImages ? <ImagesList assets={status.assets} onOpen={setOpened} /> : null}
      <iframe
        data-testid="preview-html"
        title={`Preview of ${name}`}
        src={url}
        sandbox=""
        ref={frame}
        onLoad={() => control.current.onLoad()}
        referrerPolicy="no-referrer"
        className="block min-h-0 w-full min-w-0 max-w-full flex-1 rounded-row border border-stroke bg-[#fff] [color-scheme:light]"
      />
      <ImageLightbox image={opened} onClose={() => setOpened(null)} />
    </div>
  );
}

/**
 * The page's images as a list over the frame (it does not push the frame down). A click inside the sandboxed frame cannot reach
 * the app (no script runs there, and the sandbox and CSP stay as they are), so each image is offered here: open it in the zoom
 * viewer or save it, from the bytes already loaded for the page.
 */
function ImagesList({ assets, onOpen }: { assets: HtmlAssetInfo[]; onOpen: (image: LightboxImage) => void }) {
  return (
    <ul
      className="absolute top-10 right-0 left-0 z-10 max-h-56 divide-y divide-stroke overflow-auto rounded-card border border-stroke bg-popover text-popover-foreground"
      data-testid="preview-html-images-list"
    >
      {assets.map((asset) => {
        const name = safeDownloadName(asset.path, "image");
        const blob = asset.state === "loaded" ? asset.blob : undefined;
        return (
          <li key={asset.path} data-testid="preview-html-image-row" data-state={asset.state} className="flex min-w-0 items-center gap-2 px-2 py-1.5">
            <span className="min-w-0 flex-1 truncate text-small" title={asset.path}>
              {name}
            </span>
            <span className="shrink-0 text-small text-muted-text">{asset.state === "loaded" ? formatSize(asset.size ?? 0) : asset.state === "failed" ? "Not loaded" : "Loading..."}</span>
            {blob ? (
              <>
                {imageMime(blob.type) ? (
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    data-testid="preview-html-image-open"
                    onClick={() => void blobBytes(blob).then((chunks) => onOpen({ name, mime: blob.type, chunks: chunks as Uint8Array[] }))}
                  >
                    View
                  </Button>
                ) : null}
                <SaveButton name={name} parts={() => blobBytes(blob)} testId="preview-html-image-download" />
              </>
            ) : null}
          </li>
        );
      })}
    </ul>
  );
}
