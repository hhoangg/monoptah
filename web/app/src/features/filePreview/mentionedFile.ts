// Reading a file an assistant message mentions (`chat.mentionedFile`, contract in web/docs/features.md F-45).
// Pages are followed until the file is complete; an html document gets its images as data: URIs; the client
// stops at 64 assets and 40 MiB per preview. Nothing here touches the DOM, and the html is only ever handled as a
// string: it is never parsed into the app's document.
import type { ResultOf } from "@monoptah/protocol";
import { hasControl } from "@/features/markdown/fileLinks";

export type MentionedPage = ResultOf<"chat.mentionedFile">;
/** `chat.mentionedFile` over the link of one conversation. */
export type MentionRequester = (params: { path: string; asset?: string; offset?: number; source?: boolean }, signal?: AbortSignal) => Promise<MentionedPage>;

export const MAX_PREVIEW_ASSETS = 64;
export const MAX_PREVIEW_BYTES = 40 * 1024 * 1024;

/** What one preview has fetched so far, shared by the html assets or by the images of a markdown file. */
export class PreviewBudget {
  assets = 0;
  bytes = 0;
  /** True once something was left out because of a cap. */
  limited = false;

  /** A slot for one more asset; false (and `limited`) beyond the cap. */
  takeAsset(): boolean {
    if (this.assets >= MAX_PREVIEW_ASSETS) {
      this.limited = true;
      return false;
    }
    this.assets++;
    return true;
  }

  /** Room for `bytes` more; false (and `limited`) beyond the cap. */
  reserve(bytes: number): boolean {
    if (this.bytes + bytes > MAX_PREVIEW_BYTES) {
      this.limited = true;
      return false;
    }
    this.bytes += bytes;
    return true;
  }
}

/** A cap of the budget was hit: the asset is skipped, not an error of the desktop. */
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

/** Follows `nextOffset` until the file is complete. `budget` (assets only) is charged before the data comes. */
export async function readMention(request: MentionRequester, params: { path: string; asset?: string; source?: boolean }, options: { signal?: AbortSignal; budget?: PreviewBudget } = {}): Promise<MentionContent> {
  const { signal, budget } = options;
  if (budget && !budget.takeAsset()) throw new PreviewLimitError();
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
      // The main document counts against the same 40 MiB as the assets: stop before following its pages.
      if (page.total > MAX_PREVIEW_BYTES) throw new PreviewLimitError();
      if (budget && !budget.reserve(page.total)) throw new PreviewLimitError();
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

/** The Content-Security-Policy the sandboxed document carries itself: images and fonts inline only, nothing from the net. */
export const PREVIEW_CSP = "default-src 'none'; img-src data:; style-src-attr 'unsafe-inline'; font-src data:; media-src data:";
const PREVIEW_META = `<meta http-equiv="Content-Security-Policy" content="${PREVIEW_CSP}">`;
/** What a placeholder becomes when its asset is missing or over the cap. */
export const EMPTY_DATA_URI = "data:,";
const PLACEHOLDER = /monoptah-asset:(\d+)/g;

/** `data:<image mime>;base64,<payload>`: only an allowed image type and base64 characters, so it cannot close a quote. */
export function imageDataUri(mime: string, bytes: Uint8Array): string | null {
  const allowed = imageMime(mime);
  return allowed ? `data:${allowed};base64,${encodeBase64(bytes)}` : null;
}

/**
 * The document the sandboxed iframe loads: the desktop's sanitized html with each `monoptah-asset:<i>` replaced by
 * its data URI (or an empty one), behind a CSP meta. String work only. The meta goes after a leading doctype, so the
 * page keeps standards mode, and before everything else of the document.
 */
export function buildPreviewDocument(html: string, dataUris: ReadonlyMap<number, string>): string {
  const body = html.replace(PLACEHOLDER, (_all, index: string) => dataUris.get(Number(index)) ?? EMPTY_DATA_URI);
  const doctype = /^\s*<!doctype[^>]*>/i.exec(body);
  return doctype ? `${doctype[0]}${PREVIEW_META}${body.slice(doctype[0].length)}` : `${PREVIEW_META}${body}`;
}

/** The html and its images: every `assets[i]` fetched (4 at a time) and inlined. */
export async function loadHtmlDocument(request: MentionRequester, path: string, content: MentionContent, signal?: AbortSignal): Promise<{ document: string; limited: boolean }> {
  const budget = new PreviewBudget();
  budget.reserve(content.length);
  const assets = content.meta.assets ?? [];
  const uris = new Map<number, string>();
  const used = [...new Set([...content.text.matchAll(PLACEHOLDER)].map((match) => Number(match[1])))].filter((index) => index < assets.length);
  let next = 0;
  const worker = async () => {
    while (next < used.length) {
      const index = used[next++] as number;
      try {
        const asset = await readMention(request, { path, asset: assets[index] as string }, { budget, ...(signal ? { signal } : {}) });
        const uri = asset.meta.kind === "image" ? imageDataUri(asset.meta.mime ?? "", concat(asset.chunks, asset.length)) : null;
        if (uri) uris.set(index, uri);
      } catch (error) {
        if (signal?.aborted) throw error;
        // A missing, denied or over-the-cap image leaves its place empty; the rest of the page still shows.
      }
    }
  };
  await Promise.all(Array.from({ length: Math.min(4, used.length) }, worker));
  return { document: buildPreviewDocument(content.text, uris), limited: budget.limited };
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

/** A relative asset path of a markdown image or html reference as the desktop accepts it, or null. */
export function assetPath(source: string): string | null {
  let value = source.trim().replace(/[?#][\s\S]*$/, "");
  try {
    value = decodeURIComponent(value);
  } catch {
    return null;
  }
  if (!value || value.length > 1024 || value.includes("\\") || hasControl(value)) return null;
  if (/^[a-z][a-z0-9+.-]*:/i.test(value) || value.startsWith("/") || value.startsWith("~")) return null;
  const parts = value.split("/").filter((part) => part !== "." && part !== "");
  if (parts.length === 0 || parts.includes("..")) return null;
  return parts.join("/");
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

export type PreviewProblemKind = "denied" | "not_mentioned" | "too_large" | "not_found" | "busy" | "not_selected" | "failed";

export interface PreviewProblem {
  kind: PreviewProblemKind;
  title: string;
  detail: string;
  /** Worth a "Try again" button. */
  retry: boolean;
}

export const HIDDEN_PREVIEW_MESSAGE = "This file is hidden by Monoptah";

export function previewProblem(error: unknown): PreviewProblem {
  if (error instanceof PreviewLimitError) {
    return { kind: "too_large", title: "This file is too large to preview", detail: "It is more than the preview can load here. Open it on your desktop instead.", retry: false };
  }
  const code = typeof error === "object" && error !== null && "code" in error ? (error as { code?: unknown }).code : undefined;
  switch (code) {
    case "path_denied":
      return { kind: "denied", title: HIDDEN_PREVIEW_MESSAGE, detail: "Monoptah keeps this kind of file (for example secrets) off your devices.", retry: false };
    case "forbidden":
      return { kind: "not_mentioned", title: "This file is not mentioned in this conversation", detail: "Only files an assistant message names can be opened here.", retry: false };
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
