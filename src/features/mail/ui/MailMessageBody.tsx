import { useEffect, useMemo, useRef, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Loader } from "../../../shared/ui/icons";
import { SecondaryButton } from "../../../shared/ui/SecondaryButton";
import { mailAttachmentBytes, mailRemoteImage } from "../model/mail";
import { CSS_URL, cidKey, cidReferences } from "../model/mailCid";
import type { ImagePolicy, MessageRow } from "../model/mailTypes";

/**
 * The sandbox must never gain `allow-scripts`. `allow-same-origin` is here
 * only so the host can read the document's height and catch link clicks. That
 * is safe solely because nothing inside the frame can execute: with both flags
 * a framed script could remove its own sandbox and, with `frame-src 'self'` in
 * the app CSP, reach `invoke`. Do not add a flag to fix a rendering problem.
 */
export const FRAME_SANDBOX = "allow-same-origin";
/** A second wall: whatever the sanitiser misses cannot load or connect. */
export const FRAME_CSP =
  "default-src 'none'; img-src data: blob:; style-src 'unsafe-inline'";
/** Taller mail scrolls inside the frame instead of stretching the pane. */
export const MAX_FRAME_HEIGHT = 640;
const MIN_FRAME_HEIGHT = 48;
const IMAGE_FETCH_CONCURRENCY = 4;

// Emails assume a white page, so the frame stays light whatever the app theme.
const FRAME_STYLE =
  "html,body{margin:0;padding:0}" +
  "body{background:white;color:black;font:13px/1.5 system-ui,sans-serif;" +
  "overflow-wrap:anywhere;padding:12px}" +
  "img{max-width:100%;height:auto}";

const ABSOLUTE_HTTP = /^https?:\/\//i;

const parse = (html: string) =>
  new DOMParser().parseFromString(html, "text/html");

/**
 * Absolute http(s) URLs the sanitiser blocked. A relative or protocol-relative
 * value would resolve against the app origin, so it is skipped, not fetched.
 */
export function remoteImageUrls(html: string): string[] {
  const urls = new Set<string>();
  for (const img of parse(html).querySelectorAll("img[data-mono-src]")) {
    const value = img.getAttribute("data-mono-src") ?? "";
    if (ABSOLUTE_HTTP.test(value)) urls.add(value);
  }
  return [...urls];
}

/** Blocked images that can actually be fetched; Rust's count includes the rest. */
export function loadableImageCount(html: string): number {
  let n = 0;
  for (const img of parse(html).querySelectorAll("img[data-mono-src]")) {
    if (ABSOLUTE_HTTP.test(img.getAttribute("data-mono-src") ?? "")) n += 1;
  }
  return n;
}

/**
 * The document handed to `srcdoc`. Only blob/data sources survive on images,
 * so no sender-controlled URL can reach the frame even if the sanitiser let
 * one through. Re-serialising could in principle mutate markup, which is why
 * the sandbox and CSP, not this function, are what is relied on.
 */
export function buildFrameDocument(
  html: string,
  remote: ReadonlyMap<string, string>,
  cid: ReadonlyMap<string, string>,
): string {
  const doc = parse(html);
  // Layer 1 already drops these; removing them here keeps this layer from
  // depending on layer 1 being perfect.
  for (const el of doc.querySelectorAll("base,meta,link,style,script"))
    el.remove();
  const produced = new Set([...remote.values(), ...cid.values()]);
  for (const img of doc.querySelectorAll("img")) {
    const blocked = img.getAttribute("data-mono-src");
    const proxied = blocked === null ? undefined : remote.get(blocked);
    if (proxied) {
      img.setAttribute("src", proxied);
    }
    // Blocked or loaded, the original URL has no business inside the frame.
    img.removeAttribute("data-mono-src");
    const src = img.getAttribute("src") ?? "";
    if (/^cid:/i.test(src)) {
      const resolved = cid.get(cidKey(src.slice(4)));
      if (resolved) img.setAttribute("src", resolved);
      else img.removeAttribute("src");
    } else if (src && !/^data:/i.test(src) && !produced.has(src)) {
      img.removeAttribute("src");
    }
    img.removeAttribute("srcset");
  }
  for (const el of doc.querySelectorAll("[style]")) {
    const style = (el.getAttribute("style") ?? "").replace(
      CSS_URL,
      (whole, _quote, value: string) => {
        if (/^cid:/i.test(value)) {
          const resolved = cid.get(cidKey(value.slice(4)));
          return resolved ? `url(${resolved})` : "none";
        }
        return /^data:/i.test(value) ? whole : "none";
      },
    );
    el.setAttribute("style", style);
  }
  return (
    '<!doctype html><html><head><meta charset="utf-8">' +
    `<meta http-equiv="Content-Security-Policy" content="${FRAME_CSP}">` +
    `<style>${FRAME_STYLE}</style></head><body>${doc.body.innerHTML}</body></html>`
  );
}

/** `fetch_remote_image` returns bytes only; `<img>` sniffs, but not SVG. */
function sniffImageType(bytes: Uint8Array): string {
  const at = (i: number) => bytes[i] ?? 0;
  if (at(0) === 0x89 && at(1) === 0x50 && at(2) === 0x4e) return "image/png";
  if (at(0) === 0xff && at(1) === 0xd8 && at(2) === 0xff) return "image/jpeg";
  if (at(0) === 0x47 && at(1) === 0x49 && at(2) === 0x46) return "image/gif";
  if (at(0) === 0x52 && at(8) === 0x57 && at(9) === 0x45) return "image/webp";
  return "application/octet-stream";
}

type Resolved = {
  urls: ReadonlyMap<string, string>;
  failed: number;
  pending: boolean;
};
const IDLE: Resolved = { urls: new Map(), failed: 0, pending: false };

/** Loads each key to a blob URL, a few at a time, and revokes them on cleanup. */
function useBlobUrls(
  keys: readonly string[],
  load: (key: string) => Promise<Blob>,
  active: boolean,
): Resolved {
  const [state, setState] = useState<Resolved>(IDLE);
  const loadRef = useRef(load);
  loadRef.current = load;
  const identity = keys.join("\n");

  useEffect(() => {
    if (!active || keys.length === 0) {
      setState(IDLE);
      return;
    }
    let cancelled = false;
    const made: string[] = [];
    const urls = new Map<string, string>();
    let failed = 0;
    let next = 0;
    setState({ urls, failed: 0, pending: true });
    const worker = async () => {
      while (next < keys.length && !cancelled) {
        const key = keys[next++];
        try {
          const blob = await loadRef.current(key);
          if (cancelled) return;
          const url = URL.createObjectURL(blob);
          made.push(url);
          urls.set(key, url);
        } catch {
          failed += 1;
        }
      }
    };
    void Promise.all(
      Array.from(
        { length: Math.min(IMAGE_FETCH_CONCURRENCY, keys.length) },
        worker,
      ),
    ).then(() => {
      // A fresh Map: the in-flight one is mutated, and state needs a new identity.
      if (!cancelled) setState({ urls: new Map(urls), failed, pending: false });
    });
    return () => {
      cancelled = true;
      for (const url of made) URL.revokeObjectURL(url);
    };
    // `identity` stands in for the keys array, which is rebuilt every render.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [active, identity]);

  return state;
}

const URL_IN_TEXT = /https?:\/\/[^\s<>"]+/gi;

/** Plain text with URLs made clickable. Built as React nodes, never as HTML. */
export function linkifyText(text: string): (string | { url: string })[] {
  const parts: (string | { url: string })[] = [];
  let last = 0;
  for (const m of text.matchAll(URL_IN_TEXT)) {
    const url = m[0].replace(/[.,;:!?)\]'"]+$/, "");
    if (m.index > last) parts.push(text.slice(last, m.index));
    parts.push({ url });
    last = m.index + url.length;
  }
  if (last < text.length) parts.push(text.slice(last));
  return parts;
}

const openExternal = (href: string) => {
  if (/^(https?:|mailto:)/i.test(href)) {
    void openUrl(href).catch(() => undefined);
  }
};

export function PlainTextBody({ text }: { text: string }) {
  return (
    <div className="whitespace-pre-wrap break-words px-3 py-2 text-[13px] leading-relaxed text-content/85">
      {linkifyText(text).map((part, i) =>
        typeof part === "string" ? (
          part
        ) : (
          <a
            key={i}
            href={part.url}
            rel="noopener noreferrer nofollow"
            className="text-accent underline-offset-2 hover:underline"
            onClick={(event) => {
              event.preventDefault();
              openExternal(part.url);
            }}
          >
            {part.url}
          </a>
        ),
      )}
    </div>
  );
}

/** HTML-only mail has no `bodyText`, so the toggle would show nothing. */
export const canTogglePlain = (message: MessageRow) =>
  Boolean(message.bodyHtml?.trim()) && Boolean(message.bodyText?.trim());

type Props = {
  message: MessageRow;
  plain?: boolean;
  /** The source's policy; "never" removes the show-images actions. */
  imagePolicy: ImagePolicy;
  /** The user allowed this sender earlier in the session. */
  senderAllowed: boolean;
  senderLabel: string;
  onAllowSender: () => void;
  /** Kept by the parent so collapsing the card does not forget the choice. */
  messageAllowed: boolean;
  onAllowMessage: () => void;
};

export function MailMessageBody({
  message,
  plain = false,
  imagePolicy,
  senderAllowed,
  senderLabel,
  onAllowSender,
  messageAllowed,
  onAllowMessage,
}: Props) {
  const html = message.bodyHtml?.trim() ? message.bodyHtml : null;
  const showText = html === null || (plain && message.bodyText !== null);
  const blocked = message.remoteImagesBlocked;
  const imagesOff = imagePolicy === "never";
  const allowed =
    !imagesOff && (messageAllowed || senderAllowed || imagePolicy === "always");

  const frameHtml = showText ? null : html;
  const remoteKeys = useMemo(
    () => (frameHtml ? remoteImageUrls(frameHtml) : []),
    [frameHtml],
  );
  const loadable = useMemo(
    () => (frameHtml ? loadableImageCount(frameHtml) : 0),
    [frameHtml],
  );
  const cidKeys = useMemo(() => {
    if (!frameHtml) return [];
    const wanted = new Set(cidReferences(frameHtml));
    return message.attachments
      .filter(
        (a) =>
          a.contentId !== null &&
          a.mimeType.startsWith("image/") &&
          wanted.has(cidKey(a.contentId)),
      )
      .map((a) => cidKey(a.contentId as string));
  }, [frameHtml, message.attachments]);

  const remote = useBlobUrls(
    remoteKeys,
    async (url) => {
      // Rust fetches it: no cookies, no Referer, and its internal-address guard.
      const bytes = await mailRemoteImage(message.id, url);
      return new Blob([bytes], { type: sniffImageType(new Uint8Array(bytes)) });
    },
    allowed,
  );
  const inline = useBlobUrls(
    cidKeys,
    async (key) => {
      const att = message.attachments.find(
        (a) => a.contentId !== null && cidKey(a.contentId) === key,
      );
      if (!att) throw new Error("missing attachment");
      const bytes = await mailAttachmentBytes(att.id);
      return new Blob([bytes], { type: att.mimeType });
    },
    true,
  );

  const srcDoc = useMemo(
    () =>
      frameHtml ? buildFrameDocument(frameHtml, remote.urls, inline.urls) : "",
    [frameHtml, remote.urls, inline.urls],
  );

  const frameRef = useRef<HTMLIFrameElement>(null);
  const detach = useRef<(() => void) | null>(null);
  const [height, setHeight] = useState(MIN_FRAME_HEIGHT);

  // Fires on every srcdoc change, so listeners are re-attached to the new document.
  const onFrameLoad = () => {
    detach.current?.();
    detach.current = null;
    const frame = frameRef.current;
    const doc = frame?.contentDocument;
    if (!frame || !doc?.body) return;
    const body = doc.body;
    const measure = () =>
      setHeight(Math.max(MIN_FRAME_HEIGHT, Math.ceil(body.scrollHeight)));
    // Links never navigate the frame; they open through the opener plugin.
    const onClick = (event: Event) => {
      const target = event.target as Element | null;
      const anchor = target?.closest?.("a[href]");
      if (!anchor) return;
      event.preventDefault();
      openExternal(anchor.getAttribute("href") ?? "");
    };
    doc.addEventListener("click", onClick);
    // Images arrive after the document's own load, and each can change the height.
    doc.addEventListener("load", measure, true);
    const Observer =
      (frame.contentWindow as typeof globalThis | null)?.ResizeObserver ??
      globalThis.ResizeObserver;
    const observer = Observer ? new Observer(measure) : null;
    observer?.observe(body);
    measure();
    detach.current = () => {
      doc.removeEventListener("click", onClick);
      doc.removeEventListener("load", measure, true);
      observer?.disconnect();
    };
  };
  useEffect(() => () => detach.current?.(), []);

  const unloadable = remote.failed;

  return (
    <div>
      {!showText && blocked > 0 && !allowed ? (
        <div className="m-3 flex flex-col gap-2 rounded-md border border-content/10 bg-content/5 px-3 py-2 text-[12px]">
          <div className="text-content">
            {blocked} remote {blocked === 1 ? "image" : "images"} blocked
          </div>
          {loadable < blocked ? (
            <div className="text-content/50">
              {loadable === 0
                ? "None of them can be loaded."
                : `${blocked - loadable} of them cannot be loaded.`}
            </div>
          ) : null}
          {imagesOff ? (
            <div className="text-content/50">
              Remote images are turned off for this account.
            </div>
          ) : loadable === 0 ? null : (
            <>
              <div className="text-content/50">
                Loading images tells the sender you opened this message, when,
                and from which IP address. Monoptah fetches them without cookies
                or a Referer, but that is all it hides.
              </div>
              <div className="flex flex-wrap gap-2">
                <SecondaryButton onClick={onAllowMessage}>
                  Show images
                </SecondaryButton>
                <SecondaryButton onClick={onAllowSender}>
                  Show from {senderLabel} this session
                </SecondaryButton>
              </div>
            </>
          )}
        </div>
      ) : null}
      {!showText && allowed && remote.pending ? (
        <div className="flex items-center gap-2 px-3 py-2 text-[12px] text-content/50">
          <Loader className="size-3 animate-spin" />
          Loading images
        </div>
      ) : null}
      {!showText && allowed && unloadable > 0 ? (
        <div className="px-3 py-2 text-[12px] text-content/50">
          {unloadable} {unloadable === 1 ? "image" : "images"} could not be
          loaded.
        </div>
      ) : null}
      {showText ? (
        message.bodyText?.trim() ? (
          <PlainTextBody text={message.bodyText} />
        ) : (
          <p className="px-3 py-2 text-[12px] text-content/50">
            This message has no content.
          </p>
        )
      ) : (
        <iframe
          ref={frameRef}
          title={`Message from ${senderLabel}`}
          sandbox={FRAME_SANDBOX}
          srcDoc={srcDoc}
          referrerPolicy="no-referrer"
          onLoad={onFrameLoad}
          style={{ height: Math.min(height, MAX_FRAME_HEIGHT) }}
          className="block w-full border-0 bg-white"
        />
      )}
    </div>
  );
}
