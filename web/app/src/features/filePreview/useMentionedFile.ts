// The query behind the preview: one file of one conversation, read to the end (and, for html, with its images).
import { useQuery } from "@tanstack/react-query";
import { useCallback, useEffect, useRef, useState } from "react";
import { useLinkConnection, useLinkDesktopId, type LinkRequestError } from "@/link";
import { imageMime, loadHtmlDocument, readMention, type MentionContent, type MentionRequester } from "./mentionedFile";

export type PreviewData =
  | { kind: "text" | "markdown"; meta: MentionContent["meta"]; text: string }
  | { kind: "html"; meta: MentionContent["meta"]; document: string; limited: boolean }
  | { kind: "image"; meta: MentionContent["meta"]; mime: string; chunks: Uint8Array[] }
  | { kind: "binary"; meta: MentionContent["meta"] };

/** `chat.mentionedFile` of `conversationId` over the link of the nearest provider. */
export function useMentionRequester(conversationId: string): MentionRequester {
  const connection = useLinkConnection();
  return useCallback(
    (params, signal) => connection.request("chat.mentionedFile", { conversationId, ...params }, signal ? { signal } : {}),
    [connection, conversationId],
  );
}

async function load(request: MentionRequester, path: string, source: boolean, signal: AbortSignal): Promise<PreviewData> {
  const content = await readMention(request, { path, ...(source ? { source } : {}) }, { signal });
  const { meta } = content;
  switch (meta.kind) {
    case "text":
    case "markdown":
      return { kind: meta.kind, meta, text: content.text };
    case "html": {
      const { document, limited } = await loadHtmlDocument(request, path, content, signal);
      return { kind: "html", meta, document, limited };
    }
    case "image": {
      const mime = imageMime(meta.mime);
      return mime ? { kind: "image", meta, mime, chunks: content.chunks } : { kind: "binary", meta };
    }
    case "binary":
      return { kind: "binary", meta };
  }
}

/** `source`: ask for the raw text of an html or markdown file (a jump to a line wants the lines, not the render). */
export function useMentionedFile(conversationId: string, path: string, source = false, enabled = true) {
  const desktopId = useLinkDesktopId();
  const request = useMentionRequester(conversationId);
  return useQuery<PreviewData, LinkRequestError>({
    queryKey: ["link", desktopId, "mentionedFile", conversationId, path, source],
    queryFn: ({ signal }) => load(request, path, source, signal),
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
