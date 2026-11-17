// The notification payload (architecture.md 8): metadata only, never a title, assistant text or transcript.
import type { PushPayload } from "@monoptah/protocol";
import { toBase64Url } from "../../lib/crypto.ts";

export const MAX_NAME_CHARS = 64;
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
  sessionId: string;
  monoId?: string | undefined;
  habitRun?: boolean | undefined;
}

/** The click URL, built here only: a Mono message opens the chat, everything else (habit runs too) the session. */
export function notificationUrl(input: Pick<PayloadInput, "desktopId" | "sessionId" | "monoId" | "habitRun">): string {
  const desktop = encodeURIComponent(input.desktopId);
  if (input.monoId !== undefined && input.habitRun !== true) return `/d/${desktop}/monos/${encodeURIComponent(input.monoId)}`;
  return `/d/${desktop}/s/${encodeURIComponent(input.sessionId)}`;
}

function truncate(value: string, chars: number): string {
  return Array.from(value).slice(0, chars).join("");
}

/** The JSON bytes of the payload. Names are cut to 64 characters, then further if the 1 KiB cap needs it. */
export function buildPayload(input: PayloadInput): Uint8Array {
  const url = notificationUrl(input);
  for (let limit = MAX_NAME_CHARS; limit >= 1; limit = Math.floor(limit / 2)) {
    const payload: PushPayload = {
      v: 1,
      t: truncate(input.desktopName, limit),
      b: `${truncate(input.project, limit)}: ${BODY_SUFFIX[input.status]}`,
      url,
      tag: input.sessionId,
    };
    const bytes = encoder.encode(JSON.stringify(payload));
    if (bytes.length < MAX_PAYLOAD_BYTES) return bytes;
  }
  throw new Error("Push payload over 1 KiB");
}

/** `Topic` header: first 32 base64url characters of SHA-256(sessionId), so notices of one session collapse. */
export async function topicFor(sessionId: string): Promise<string> {
  return toBase64Url(new Uint8Array(await crypto.subtle.digest("SHA-256", encoder.encode(sessionId)))).slice(0, 32);
}
