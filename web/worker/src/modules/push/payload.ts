// The notification payload (architecture.md 8): the generic line, or the desktop's title and short preview when it sent them. Nothing here is logged or stored.
import type { PushPayload } from "@monoptah/protocol";
import { toBase64Url } from "../../lib/crypto.ts";

export const MAX_NAME_CHARS = 64;
export const MAX_PREVIEW_CHARS = 180;
/** Well under the ~4 KiB Web Push limit (RFC 8030) and the one 4096-byte record of `webpush.ts`. */
export const MAX_CONTENT_PAYLOAD_BYTES = 3072;
/** The generic line, as before content existed. */
export const MAX_PAYLOAD_BYTES = 1024;
/** Ids that end up in the URL and the tag. */
export const SAFE_ID = /^[A-Za-z0-9_.:-]{1,128}$/;

const encoder = new TextEncoder();

export type NotifyStatus = "finished" | "needs_input" | "message";

const BODY_SUFFIX: Record<NotifyStatus, string> = {
  finished: "Finished",
  needs_input: "Needs your input",
  message: "New message",
};

export interface PayloadInput {
  desktopId: string;
  desktopName: string;
  project: string;
  status: NotifyStatus;
  /** Absent for a Mono's own chat, which has no session. Then `monoId` is required. */
  sessionId?: string | undefined;
  monoId?: string | undefined;
  habitRun?: boolean | undefined;
  /** Desktop-provided content: both set, or the generic line is used. */
  title?: string | undefined;
  preview?: string | undefined;
}

/**
 * What notices collapse on: the session id, or `mono:<monoId>` for a Mono's own chat (no session id). It is the
 * payload `tag`, the `Topic` source and part of the dedupe key.
 */
export function targetKey(input: Pick<PayloadInput, "sessionId" | "monoId">): string {
  if (input.sessionId !== undefined) return input.sessionId;
  if (input.monoId === undefined) throw new Error("Push target needs a session id or a mono id");
  return `mono:${input.monoId}`;
}

/** The click URL, built here only: a Mono message opens the chat, everything else (habit runs too) the session. */
export function notificationUrl(input: Pick<PayloadInput, "desktopId" | "sessionId" | "monoId" | "habitRun">): string {
  const desktop = encodeURIComponent(input.desktopId);
  if (input.sessionId === undefined) {
    if (input.monoId === undefined) throw new Error("Push target needs a session id or a mono id");
    return `/d/${desktop}/monos/${encodeURIComponent(input.monoId)}`;
  }
  if (input.monoId !== undefined && input.habitRun !== true) return `/d/${desktop}/monos/${encodeURIComponent(input.monoId)}`;
  return `/d/${desktop}/s/${encodeURIComponent(input.sessionId)}`;
}

function truncate(value: string, chars: number): string {
  return Array.from(value).slice(0, chars).join("");
}

/** One line, whitespace collapsed, at most `chars` characters with an ellipsis. The desktop already did this: this is the second check. */
function oneLine(value: string, chars: number): string {
  const text = value.replace(/\s+/g, " ").trim();
  const letters = Array.from(text);
  return letters.length <= chars ? text : `${letters.slice(0, chars - 1).join("").trimEnd()}\u2026`;
}

/** The JSON bytes of the payload. Names are cut to 64 characters, then further if the cap needs it: 1 KiB for the generic line, 3 KiB with content. */
export function buildPayload(input: PayloadInput): Uint8Array {
  const url = notificationUrl(input);
  const title = input.title === undefined ? "" : oneLine(input.title, MAX_NAME_CHARS);
  const preview = input.preview === undefined ? "" : oneLine(input.preview, MAX_PREVIEW_CHARS);
  const withContent = title !== "" && preview !== "";
  for (let limit = MAX_NAME_CHARS; limit >= 1; limit = Math.floor(limit / 2)) {
    const payload: PushPayload = {
      v: 1,
      t: withContent ? truncate(title, limit) : truncate(input.desktopName, limit),
      b: withContent ? preview : `${truncate(input.project, limit)}: ${BODY_SUFFIX[input.status]}`,
      url,
      tag: targetKey(input),
    };
    const bytes = encoder.encode(JSON.stringify(payload));
    if (bytes.length < (withContent ? MAX_CONTENT_PAYLOAD_BYTES : MAX_PAYLOAD_BYTES)) return bytes;
  }
  throw new Error("Push payload over its size cap");
}

/** `Topic` header: first 32 base64url characters of SHA-256(`value`), so notices of one target collapse. */
export async function topicFor(value: string): Promise<string> {
  return toBase64Url(new Uint8Array(await crypto.subtle.digest("SHA-256", encoder.encode(value)))).slice(0, 32);
}
