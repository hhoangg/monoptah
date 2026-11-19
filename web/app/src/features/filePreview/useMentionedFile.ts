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

async function load(request: MentionRequester, path: string, signal: AbortSignal): Promise<PreviewData> {
  const content = await readMention(request, { path }, { signal });
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

export function useMentionedFile(conversationId: string, path: string, enabled = true) {
  const desktopId = useLinkDesktopId();
  const request = useMentionRequester(conversationId);
  return useQuery<PreviewData, LinkRequestError>({
    queryKey: ["link", desktopId, "mentionedFile", conversationId, path],
    queryFn: ({ signal }) => load(request, path, signal),
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
 * An object URL for `parts`, made once for the life of the component (the parts are fixed per mount: callers key the
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
