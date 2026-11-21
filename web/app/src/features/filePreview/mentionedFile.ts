// Reading a file an assistant message mentions (`chat.mentionedFile`, contract in web/docs/features.md F-45).
// Pages are followed until the file is complete; an html document gets its images as data: URIs, read a few at a time
// (the desktop's allowance for one device) and asked again while it is busy. A page has no total cap on its assets: only
// one file is limited (40 MiB), and an asset that cannot be shown gets a visible placeholder. Nothing here touches the
// DOM, and the html is only ever handled as a string: it is never parsed into the app's document.
import type { ResultOf } from "@monoptah/protocol";
import { hasControl } from "@/features/markdown/fileLinks";
import { Gate, PREPARE_CONCURRENCY } from "./assetGate.ts";
import { TransferError } from "./transfer/client.ts";

export type MentionedPage = ResultOf<"chat.mentionedFile">;
/** `chat.mentionedFile` over the link of one conversation. */
export type MentionRequester = (params: { path: string; asset?: string; offset?: number; source?: boolean }, signal?: AbortSignal) => Promise<MentionedPage>;

/** The most one file may weigh to be shown in a preview (a binary file is only offered for download). There is no total per page. */
export const MAX_PREVIEW_BYTES = 40 * 1024 * 1024;

/** One file is over the per-file limit: it is not shown, and that is not an error of the desktop. */
export class PreviewLimitError extends Error {
  constructor() {
    super("Preview limit reached");
    this.name = "PreviewLimitError";
  }
}

const BASE64 = /^[A-Za-z0-9+/]*={0,2}$/;

export function decodeBase64(data: string): Uint8Array {
  if (!BASE64.test(data) || data.length % 4 === 1) throw new Error("The desktop sent a damaged file");
  const binary = atob(data);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

export function encodeBase64(bytes: Uint8Array): string {
  let out = "";
  // A multiple of 3 bytes per slice, so no slice but the last carries padding.
  for (let i = 0; i < bytes.length; i += 0x6000) {
    let binary = "";
    const end = Math.min(bytes.length, i + 0x6000);
    for (let j = i; j < end; j++) binary += String.fromCharCode(bytes[j] as number);
    out += btoa(binary);
  }
  return out;
}

const IMAGE_MIMES = new Set(["image/png", "image/jpeg", "image/gif", "image/webp", "image/svg+xml", "image/avif", "image/bmp", "image/x-icon", "image/vnd.microsoft.icon"]);

/** `mime` when it is an image type the preview shows, else null (the file then counts as not viewable). */
export function imageMime(mime: string | undefined): string | null {
  const value = mime?.toLowerCase().split(";")[0]?.trim();
  return value && IMAGE_MIMES.has(value) ? value : null;
}

export interface ReadOptions {
  signal?: AbortSignal;
  /** Bytes of the download received so far, and the total (encrypted transfers only). */
  onProgress?: (received: number, total: number) => void;
}

/** Reads one file or asset to its end. Two implementations: paged `chat.mentionedFile`, and `file.prepare` with an encrypted object. */
export type MentionReader = (params: { path: string; asset?: string; source?: boolean }, options?: ReadOptions) => Promise<MentionContent>;

/** What the previews read through: a paged requester (older desktops) or a reader. */
export type MentionSource = MentionRequester | { read: MentionReader };

export function asReader(source: MentionSource): MentionReader {
  return typeof source === "function" ? (params, options) => readMention(source, params, options) : source.read;
}

export interface MentionContent {
  /** The first page: path, name, size, kind, mime, assets, project. `data` is empty. */
  meta: MentionedPage;
  /** Text of `utf8` pages joined. */
  text: string;
  /** Raw bytes of `base64` pages, one array per page. */
  chunks: Uint8Array[];
  /** Bytes of the stream received. */
  length: number;
}

/** Follows `nextOffset` until the file is complete. */
export async function readMention(request: MentionRequester, params: { path: string; asset?: string; source?: boolean }, options: ReadOptions = {}): Promise<MentionContent> {
  const { signal } = options;
  let offset = 0;
  let meta: MentionedPage | undefined;
  let text = "";
  const chunks: Uint8Array[] = [];
  let length = 0;
  for (;;) {
    signal?.throwIfAborted();
    const page = await request({ ...params, ...(offset > 0 ? { offset } : {}) }, signal);
    if (page.offset !== offset) throw new Error("The desktop sent a page out of order");
    if (!meta) {
      meta = { ...page, data: "" };
      // Stop before following the pages of a file over the per-file limit.
      if (page.total > MAX_PREVIEW_BYTES) throw new PreviewLimitError();
    }
    if (page.encoding === "utf8") {
      text += page.data;
      length += page.data.length;
    } else if (page.encoding === "base64") {
      const bytes = decodeBase64(page.data);
      chunks.push(bytes);
      length += bytes.length;
    }
    if (length > page.total || length > MAX_PREVIEW_BYTES) throw new PreviewLimitError();
    if (page.nextOffset === undefined) break;
    if (page.nextOffset <= offset || page.nextOffset > page.total) throw new Error("The desktop sent a bad page offset");
    offset = page.nextOffset;
  }
  return { meta, text, chunks, length };
}

/**
 * The Content-Security-Policy the sandboxed document carries itself: images and fonts inline only, nothing from the net.
 * `blob:` is deliberately not allowed: the frame is `sandbox=""` (an opaque origin), and neither Chromium nor WebKit lets
 * such a frame load a blob: URL made by the app (measured, see security.md), so images reach it as `data:` URIs.
 */
export const PREVIEW_CSP = "default-src 'none'; img-src data:; style-src-attr 'unsafe-inline'; font-src data:; media-src data:";
const PREVIEW_META = `<meta http-equiv="Content-Security-Policy" content="${PREVIEW_CSP}">`;
/**
 * What a placeholder becomes when its asset could not be loaded (missing, refused, too large, desktop busy): a small grey
 * picture that says so, never a broken or empty image. An svg inside `<img>` runs no script.
 */
export const ASSET_PLACEHOLDER_URI = `data:image/svg+xml;base64,${btoa(
  '<svg xmlns="http://www.w3.org/2000/svg" width="240" height="90" viewBox="0 0 240 90"><rect width="240" height="90" fill="#e6e8eb" stroke="#9aa0a6" stroke-dasharray="6 4"/><text x="120" y="50" font-family="sans-serif" font-size="14" fill="#4b5563" text-anchor="middle">Image not loaded</text></svg>',
)}`;
/** What an image shows while its asset is still being read. */
export const ASSET_LOADING_URI = `data:image/svg+xml;base64,${btoa(
  '<svg xmlns="http://www.w3.org/2000/svg" width="240" height="90" viewBox="0 0 240 90"><rect width="240" height="90" fill="#eef0f2" stroke="#c4c9cf"/><text x="120" y="50" font-family="sans-serif" font-size="14" fill="#6b7280" text-anchor="middle">Loading image...</text></svg>',
)}`;
const PLACEHOLDER = /monoptah-asset:(\d+)/g;

/** `data:<image mime>;base64,<payload>`: only an allowed image type and base64 characters, so it cannot close a quote. */
export function imageDataUri(mime: string, bytes: Uint8Array): string | null {
  const allowed = imageMime(mime);
  return allowed ? `data:${allowed};base64,${encodeBase64(bytes)}` : null;
}

/**
 * The document the sandboxed iframe loads, as the parts of a Blob: the desktop's sanitized html with each
 * `monoptah-asset:<i>` replaced by what `shown` holds for it (a data URI as a string, or as a Blob that already holds
 * one), behind a CSP meta. A Blob part is not copied into the JS heap, so a page of many large images is assembled
 * without ever being one huge string. The meta goes after a leading doctype, so the page keeps standards mode, and before
 * everything else of the document. Anything `shown` lacks becomes the "Image not loaded" placeholder.
 */
export function buildPreviewParts(html: string, shown: ReadonlyMap<number, string | Blob>): BlobPart[] {
  const doctype = /^\s*<!doctype[^>]*>/i.exec(html);
  const rest = doctype ? html.slice(doctype[0].length) : html;
  const parts: BlobPart[] = [doctype ? doctype[0] : "", PREVIEW_META];
  let at = 0;
  for (const match of rest.matchAll(PLACEHOLDER)) {
    parts.push(rest.slice(at, match.index), shown.get(Number(match[1])) ?? ASSET_PLACEHOLDER_URI);
    at = match.index + match[0].length;
  }
  parts.push(rest.slice(at));
  return parts;
}

/** [`buildPreviewParts`] as one string, for documents that hold only small data URIs. String work only. */
export function buildPreviewDocument(html: string, dataUris: ReadonlyMap<number, string>): string {
  return buildPreviewParts(html, dataUris).join("");
}

/** Assets read at once: the desktop answers a device with at most 3 `file.prepare` in flight and refuses the next. */
export const ASSET_CONCURRENCY = PREPARE_CONCURRENCY;
/** Pause after a transient failure (no answer in time, a failed download); after the last one the asset gets its placeholder. */
export const TRANSIENT_RETRY_DELAYS_MS = [1_000, 3_000, 6_000];
/** The pause after `busy` without a hint from the desktop grows from the first value to the last; with a hint it is the hint, within these bounds. */
export const BUSY_BACKOFF_MS = { first: 500, last: 15_000, min: 500, max: 60_000 };

/** Gates of the older desktops' paged requesters (the encrypted transfers gate their own `file.prepare`). */
const legacyGates = new WeakMap<object, Gate>();

function sleep(ms: number, signal?: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    const done = () => {
      signal?.removeEventListener("abort", stop);
      resolve();
    };
    const stop = () => {
      clearTimeout(timer);
      reject(signal?.reason);
    };
    const timer = setTimeout(done, ms);
    signal?.addEventListener("abort", stop, { once: true });
  });
}

function codeOf(error: unknown): string | undefined {
  return typeof error === "object" && error !== null ? ((error as { code?: unknown }).code as string | undefined) : undefined;
}

/** The desktop is busy or limits how often files are opened: waiting makes it pass, so the read waits as long as the preview is open. */
function isBusy(error: unknown): boolean {
  return codeOf(error) === "busy";
}

/** Failures that often pass on a second try: no answer in time, the desktop link dropping, a download that failed on the network. */
function isTransient(error: unknown): boolean {
  if (error instanceof TransferError) return error.problem === "network";
  // Codes the link itself raises: no answer in time, a link that dropped or is not ready again yet.
  const code = codeOf(error);
  if (code === "timeout" || code === "session_ended" || code === "not_ready") return true;
  // The host words these two without a `code` (host.rs `Failure::from`), so the text is all there is to tell them by.
  const message = error instanceof Error ? error.message : "";
  return code === undefined && (message === "Desktop did not answer" || message === "Desktop is unavailable");
}

function busyDelay(error: unknown, attempt: number): number {
  const hint = typeof error === "object" && error !== null ? (error as { retryAfter?: unknown }).retryAfter : undefined;
  const { first, last, min, max } = BUSY_BACKOFF_MS;
  if (typeof hint === "number" && hint > 0) return Math.min(max, Math.max(min, hint * 1000));
  return Math.min(last, first * 2 ** attempt);
}

/**
 * Reads one file (an asset of a page or markdown file, or the file itself) through `source`, never giving up while it can still pass:
 *  - `busy` (a full queue, or the desktop's rate limit, which says `retryAfter`) is waited out for as long as the preview
 *    is open (`signal` not aborted); `onWaiting` tells the caller so it can show "waiting for the desktop";
 *  - a transient failure is asked again after a pause, a few times;
 *  - anything else (not found, refused, too large) is thrown at once.
 * A paged requester (older desktop) is also limited to [`ASSET_CONCURRENCY`] at a time per requester.
 */
export async function readAsset(source: MentionSource, params: { path: string; asset?: string; source?: boolean }, options: ReadOptions & { onWaiting?: (waiting: boolean) => void } = {}): Promise<MentionContent> {
  const { onWaiting, ...readOptions } = options;
  const read = asReader(source);
  let gate: Gate | undefined;
  if (typeof source === "function") {
    gate = legacyGates.get(source) ?? new Gate(ASSET_CONCURRENCY);
    legacyGates.set(source, gate);
  }
  let busyAttempt = 0;
  let transientAttempt = 0;
  for (;;) {
    try {
      return await (gate ? gate.run(() => read(params, readOptions), readOptions.signal) : read(params, readOptions));
    } catch (error) {
      let delay: number | undefined;
      if (isBusy(error)) delay = busyDelay(error, busyAttempt++);
      else if (isTransient(error)) delay = TRANSIENT_RETRY_DELAYS_MS[transientAttempt++];
      if (delay === undefined || readOptions.signal?.aborted) throw error;
      const waiting = isBusy(error);
      if (waiting) onWaiting?.(true);
      try {
        await sleep(delay, readOptions.signal);
      } finally {
        if (waiting) onWaiting?.(false);
      }
    }
  }
}

/** What the preview of an html page shows now: the document with each image loaded, loading or failed. */
export interface HtmlAssetsView {
  /** The page as a Blob (text/html): the images are parts of it, never held as one string. */
  document: Blob;
  /** Images that could not be had, shown as the "Image not loaded" placeholder. */
  skipped: number;
  /** Images still being read (or waiting for a busy desktop). */
  pending: number;
  /** Some image waits for a busy or rate limiting desktop. */
  waiting: boolean;
}

/** One image a page names, for the list of its images. */
export interface HtmlAssetInfo {
  /** The key as the desktop lists it (`img/a.png`). */
  path: string;
  state: "loading" | "loaded" | "failed";
  /** Only when loaded: the image's own bytes (type = its image mime) and their count. */
  blob?: Blob;
  size?: number;
}

export interface HtmlAssetsLoad {
  /** How many images failed, are pending or wait for the desktop, and the list of them: cheap, for a screen that updates at once. */
  status(): Omit<HtmlAssetsView, "document"> & { assets: HtmlAssetInfo[] };
  /** The document as it is now. Built on demand (cheap: a Blob of parts), so a caller that updates the screen less often pays for fewer builds. */
  view(): HtmlAssetsView;
  /** Settles when every image is loaded or failed, or after `dispose`; never rejects. */
  done: Promise<void>;
  /** Stops reading and lets the images go. */
  dispose(): void;
}

const HTML_TYPE = "text/html;charset=utf-8";

function viewOf(text: string, used: readonly number[], shown: ReadonlyMap<number, string | Blob>, failed: ReadonlySet<number>, pending: number, waiting: boolean): HtmlAssetsView {
  const parts = new Map<number, string | Blob>();
  for (const index of used) parts.set(index, shown.get(index) ?? (failed.has(index) ? ASSET_PLACEHOLDER_URI : ASSET_LOADING_URI));
  return { document: new Blob(buildPreviewParts(text, parts), { type: HTML_TYPE }), skipped: failed.size, pending, waiting };
}

function usedAssets(page: { text: string; assets: readonly string[] }): number[] {
  return [...new Set([...page.text.matchAll(PLACEHOLDER)].map((match) => Number(match[1])))].filter((index) => index < page.assets.length);
}

/** The page before any image is read: every image the page names shows its loading placeholder. */
export function pendingHtmlView(page: { text: string; assets: readonly string[] }): HtmlAssetsView {
  const used = usedAssets(page);
  return viewOf(page.text, used, new Map(), new Set(), used.length, false);
}

/**
 * The images of an html page, read in the background. The document is usable at once (`view()`): each image shows a loading
 * placeholder until its asset arrives, then the image, or the "Image not loaded" placeholder when it cannot be had.
 * Assets are read [`ASSET_CONCURRENCY`] at a time with no cap on how many or how big in all; a busy desktop is waited for.
 * A loaded image is kept as a Blob holding its data URI (the sandboxed frame loads no blob: URL, see [`PREVIEW_CSP`]), so
 * the bytes sit outside the JS heap and the document is assembled from them without copying.
 * `onChange` is called after every image settles and when the waiting state changes.
 */
export function loadHtmlAssets(
  source: MentionSource,
  path: string,
  page: { text: string; assets: readonly string[] },
  options: { signal?: AbortSignal; onChange?: () => void } = {},
): HtmlAssetsLoad {
  const controller = new AbortController();
  if (options.signal?.aborted) controller.abort();
  else options.signal?.addEventListener("abort", () => controller.abort(), { once: true });
  const loaded = new Map<number, Blob>();
  const raw = new Map<number, Blob>();
  const failed = new Set<number>();
  const used = usedAssets(page);
  let next = 0;
  let waiting = 0;
  const changed = () => {
    if (!controller.signal.aborted) options.onChange?.();
  };
  const tellWaiting = (now: boolean) => {
    waiting += now ? 1 : -1;
    changed();
  };
  const status = () => ({
    skipped: failed.size,
    pending: used.length - loaded.size - failed.size,
    waiting: waiting > 0,
    assets: used.map((index): HtmlAssetInfo => {
      const blob = raw.get(index);
      const path = page.assets[index] as string;
      return blob ? { path, state: "loaded", blob, size: blob.size } : { path, state: failed.has(index) ? "failed" : "loading" };
    }),
  });
  const view = (): HtmlAssetsView => viewOf(page.text, used, loaded, failed, used.length - loaded.size - failed.size, waiting > 0);
  const worker = async () => {
    while (next < used.length && !controller.signal.aborted) {
      const index = used[next++] as number;
      try {
        const asset = await readAsset(source, { path, asset: page.assets[index] as string }, { signal: controller.signal, onWaiting: tellWaiting });
        const uri = asset.meta.kind === "image" ? imageDataUri(asset.meta.mime ?? "", concat(asset.chunks, asset.length)) : null;
        if (controller.signal.aborted) return;
        if (uri) {
          loaded.set(index, new Blob([uri]));
          // The image's own bytes, for opening it in the viewer and saving it: the ones already loaded, never asked again.
          raw.set(index, new Blob(asset.chunks as BlobPart[], { type: imageMime(asset.meta.mime) ?? "application/octet-stream" }));
        } else failed.add(index);
      } catch {
        if (controller.signal.aborted) return;
        // A missing, refused or too large image shows its placeholder; the rest of the page still shows.
        failed.add(index);
      }
      changed();
    }
  };
  const done = Promise.all(Array.from({ length: Math.min(ASSET_CONCURRENCY, used.length) }, worker)).then(() => undefined);
  return {
    view,
    status,
    done,
    dispose() {
      controller.abort();
      loaded.clear();
      raw.clear();
    },
  };
}

/** The text of a Blob. */
export function blobText(blob: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result));
    reader.onerror = () => reject(reader.error);
    reader.readAsText(blob);
  });
}

/** [`loadHtmlAssets`] run to the end, as one string (for tests and small pages: a big page is better kept as the Blob). */
export async function loadHtmlDocument(
  source: MentionSource,
  path: string,
  content: MentionContent,
  signal?: AbortSignal,
  /** Called with true while some asset waits for a busy or rate limiting desktop, and false when none does. */
  onWaiting?: (waiting: boolean) => void,
): Promise<{ document: string; skipped: number }> {
  const load = loadHtmlAssets(source, path, { text: content.text, assets: content.meta.assets ?? [] }, { ...(signal ? { signal } : {}), onChange: () => onWaiting?.(load.view().waiting) });
  await load.done;
  if (signal?.aborted) {
    load.dispose();
    throw signal.reason;
  }
  const { document, skipped } = load.view();
  return { document: await blobText(document), skipped };
}

export function concat(chunks: readonly Uint8Array[], length: number): Uint8Array {
  if (chunks.length === 1 && chunks[0]?.length === length) return chunks[0];
  const out = new Uint8Array(length);
  let at = 0;
  for (const chunk of chunks) {
    out.set(chunk, at);
    at += chunk.length;
  }
  return out;
}

const IMAGE_EXTENSIONS = new Set(["png", "jpg", "jpeg", "gif", "webp", "svg", "avif", "bmp", "ico"]);

/**
 * The key of an image reference of a markdown image, exactly as the desktop lists it in `assets`
 * (`normalize_reference` in mentioned_file/html.rs): relative (`a.png`, `../a.png`), absolute (`/p/a.png`) or `file://`
 * (empty or `localhost` host), with `?query` and `#fragment` cut and percent-decoding applied once. Whether the desktop
 * reads it is its decision (inside the project of the file, else the file's folder); this only drops what is never an image path.
 */
export function assetPath(source: string): string | null {
  let value = source.trim().replace(/[?#][\s\S]*$/, "");
  try {
    value = decodeURIComponent(value);
  } catch {
    return null;
  }
  if (/^file:\/\//i.test(value)) {
    value = value.slice(7);
    if (/^localhost\//i.test(value)) value = value.slice(9);
    if (!value.startsWith("/")) return null;
  }
  if (!value || value.length > 1024 || value.startsWith("~") || value.includes("\\") || value.includes(":") || hasControl(value)) return null;
  const parts = value.split("/").filter((part) => part !== "." && part !== "");
  if (parts.length === 0) return null;
  const key = parts.join("/");
  const extension = key.slice(key.lastIndexOf(".") + 1).toLowerCase();
  if (!key.includes(".") || !IMAGE_EXTENSIONS.has(extension)) return null;
  return value.startsWith("/") ? `/${key}` : key;
}

export function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB"];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit++;
  }
  return `${value >= 10 ? Math.round(value) : Math.round(value * 10) / 10} ${units[unit]}`;
}

export type PreviewProblemKind = "denied" | "not_mentioned" | "too_large" | "not_found" | "busy" | "not_selected" | "update_desktop" | "failed" | "expired" | "signed_out" | "damaged";

export interface PreviewProblem {
  kind: PreviewProblemKind;
  title: string;
  detail: string;
  /** Worth a "Try again" button. */
  retry: boolean;
}

export const HIDDEN_PREVIEW_MESSAGE = "This file is hidden by Monoptah";

function updateDesktop(): PreviewProblem {
  return { kind: "update_desktop", title: "Update Monoptah on your desktop", detail: "Your desktop is too old to open this file from the web. Update it, then try again.", retry: false };
}

export interface PreviewProblemOptions {
  /**
   * Whether the desktop advertises `mentionedFiles2` (a shell command or its output counts as evidence, a file made during
   * the conversation too). An older desktop refuses such a file with the same `forbidden` as an unmentioned one, so there
   * the refusal reads as "update the desktop", not as "not in this conversation". Defaults to true.
   */
  desktopCurrent?: boolean;
  /**
   * Whether the desktop advertises `fileTransfer` (files open through encrypted objects, no mention needed). Then a
   * `forbidden` means the file is outside the projects shared with this read-only device. Defaults to false.
   */
  fileTransfer?: boolean;
}

/** The short reason shown for a failed preview: not found, outside the conversation, blocked, or the desktop needs an update. */
export function previewProblem(error: unknown, options: PreviewProblemOptions = {}): PreviewProblem {
  if (error instanceof PreviewLimitError) {
    return { kind: "too_large", title: "This file is too large to preview", detail: "It is more than the preview can load here. Open it on your desktop instead.", retry: false };
  }
  if (error instanceof TransferError) {
    switch (error.problem) {
      case "gone":
        return { kind: "expired", title: "This file is no longer available", detail: "It expired on the server. Try again to send it from your desktop once more.", retry: true };
      case "signed_out":
        return { kind: "signed_out", title: "You are signed out", detail: "Sign in again, then open the file.", retry: false };
      case "damaged":
        return { kind: "damaged", title: "This file could not be verified", detail: "What arrived did not match what your desktop sent, so it was not shown. Try again.", retry: true };
      case "too_large":
        return { kind: "too_large", title: "This file is too large to preview", detail: "Open it on your desktop instead.", retry: false };
      case "network":
        return { kind: "failed", title: "This file could not be downloaded", detail: "Check your connection and try again.", retry: true };
    }
  }
  const code = typeof error === "object" && error !== null && "code" in error ? (error as { code?: unknown }).code : undefined;
  switch (code) {
    case "path_denied":
      return { kind: "denied", title: HIDDEN_PREVIEW_MESSAGE, detail: "Monoptah keeps this kind of file (for example secrets) off your devices.", retry: false };
    case "forbidden":
      if (options.desktopCurrent === false) return updateDesktop();
      if (options.fileTransfer) {
        return { kind: "not_mentioned", title: "This device cannot open this file", detail: "A read-only device opens files of the projects shared with it. Give it Control access on your desktop to open any file.", retry: false };
      }
      // A desktop without `fileTransfer` also refuses a file nothing in this conversation named, which a newer desktop opens for a Control device.
      return { kind: "not_mentioned", title: "This file is outside this conversation", detail: "Only files the assistant named, and that its work in this conversation touched, can be opened here.", retry: false };
    case "unsupported":
      return updateDesktop();
    case "too_large":
      return { kind: "too_large", title: "This file is too large to preview", detail: "Open it on your desktop instead.", retry: false };
    case "not_found":
      return { kind: "not_found", title: "This file could not be found", detail: "It may have been moved or deleted on your desktop.", retry: false };
    case "busy":
      return { kind: "busy", title: "Your desktop is busy right now", detail: "Too many files were opened in a short time. Wait a moment and try again.", retry: true };
    case "not_selected":
      return { kind: "not_selected", title: "Not on this device's projects", detail: "This conversation is not shared with this device.", retry: false };
    default:
      return { kind: "failed", title: "This file could not be opened", detail: "Check the connection to your desktop and try again.", retry: true };
  }
}
