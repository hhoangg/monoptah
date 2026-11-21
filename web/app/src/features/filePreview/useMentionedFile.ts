// The query behind the preview: one file of one conversation, read to the end (and, for html, with its images).
import { useQuery } from "@tanstack/react-query";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useLinkConnection, useLinkDesktopId, type LinkRequestError } from "@/link";
import { useCapability } from "@/version";
import { imageMime, readAsset, type MentionContent, type MentionRequester, type MentionSource, type ReadOptions } from "./mentionedFile";
import { transferReader } from "./transfer/reader";

export type PreviewData =
  | { kind: "text" | "markdown"; meta: MentionContent["meta"]; text: string }
  | { kind: "html"; meta: MentionContent["meta"]; text: string }
  | { kind: "image"; meta: MentionContent["meta"]; mime: string; chunks: Uint8Array[] }
  /** `chunks` is set when the desktop sent the file (encrypted transfer): it can be saved, not previewed. */
  | { kind: "binary"; meta: MentionContent["meta"]; chunks?: Uint8Array[] };

/** Bytes of the download received, and the total. */
export interface DownloadProgress {
  received: number;
  total: number;
}

/** `chat.mentionedFile` of `conversationId` over the link of the nearest provider. */
export function useMentionRequester(conversationId: string): MentionRequester {
  const connection = useLinkConnection();
  return useCallback(
    (params, signal) => connection.request("chat.mentionedFile", { conversationId, ...params }, signal ? { signal } : {}),
    [connection, conversationId],
  );
}

/**
 * What the previews read through: encrypted transfers (`file.prepare`) when the desktop advertises `fileTransfer`, else
 * the paged `chat.mentionedFile` of older desktops.
 */
export function useMentionSource(conversationId: string): MentionSource {
  const connection = useLinkConnection();
  const desktopId = useLinkDesktopId();
  const request = useMentionRequester(conversationId);
  const transfer = useCapability("fileTransfer");
  return useMemo<MentionSource>(() => {
    if (!transfer) return request;
    return {
      read: transferReader({
        desktopId,
        deviceId: () => connection.deviceId(),
        prepare: (params, signal) => connection.request("file.prepare", { conversationId, ...params }, signal ? { signal } : {}),
      }),
    };
  }, [transfer, request, connection, desktopId, conversationId]);
}

async function load(source: MentionSource, path: string, sourceText: boolean, signal: AbortSignal, onProgress: ReadOptions["onProgress"], onWaiting: (waiting: boolean) => void): Promise<PreviewData> {
  // A busy desktop (its places for `file.prepare` may still be held by work an earlier preview started) is waited for, not shown as an error.
  const content = await readAsset(source, { path, ...(sourceText ? { source: true } : {}) }, { signal, onWaiting, ...(onProgress ? { onProgress } : {}) });
  const { meta } = content;
  switch (meta.kind) {
    case "text":
    case "markdown":
      return { kind: meta.kind, meta, text: content.text };
    case "html": {
      // The images are read by the preview itself, after it shows the page.
      return { kind: "html", meta, text: content.text };
    }
    case "image": {
      const mime = imageMime(meta.mime);
      return mime ? { kind: "image", meta, mime, chunks: content.chunks } : { kind: "binary", meta };
    }
    case "binary":
      return content.chunks.length > 0 ? { kind: "binary", meta, chunks: content.chunks } : { kind: "binary", meta };
  }
}

/** `source`: ask for the raw text of an html or markdown file (a jump to a line wants the lines, not the render). */
export function useMentionedFile(conversationId: string, path: string, source = false, enabled = true) {
  const desktopId = useLinkDesktopId();
  const mention = useMentionSource(conversationId);
  const [progress, setProgress] = useState<DownloadProgress | null>(null);
  // True while the file itself waits for a busy or rate limiting desktop.
  const [waiting, setWaiting] = useState(false);
  const query = useQuery<PreviewData, LinkRequestError>({
    queryKey: ["link", desktopId, "mentionedFile", conversationId, path, source],
    queryFn: ({ signal }) => {
      setProgress(null);
      setWaiting(false);
      return load(mention, path, source, signal, (received, total) => setProgress({ received, total }), setWaiting);
    },
    enabled,
    networkMode: "always",
    retry: false,
    staleTime: 0,
    // The bytes of a file are held for a moment only: closing the preview lets them go.
    gcTime: 30_000,
    structuralSharing: false,
    refetchOnWindowFocus: false,
    refetchOnReconnect: false,
  });
  return { ...query, progress, waiting };
}

/**
 * An object URL for the html preview document only (it carries its own CSP and is shown in a `sandbox=""` iframe).
 * Bytes from the desktop that are not that document (images) never get a blob: URL: they use data: URIs, because a
 * blob: URL is an app-origin address that a crafted page could be navigated to. Made once for the life of the component (the parts are fixed per mount: callers key the
 * component by the data) and revoked after it unmounts. The revoke waits a tick, so a StrictMode unmount followed by an
 * immediate remount keeps its URL.
 */
export function useBlobUrl(parts: BlobPart[], type: string): string {
  const [url] = useState(() => URL.createObjectURL(new Blob(parts, { type })));
  const pending = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  useEffect(() => {
    clearTimeout(pending.current);
    return () => {
      pending.current = setTimeout(() => URL.revokeObjectURL(url), 0);
    };
  }, [url]);
  return url;
}
