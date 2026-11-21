// JSON application envelope inside MSG plaintexts (architecture.md 5.5): the
// existing mobile-link shapes plus `rid`, a client-chosen u32 correlation id.
import "./zod-config.ts";
import { z } from "zod";
import { ENVELOPE_VERSION, MAX_COMMAND_ID_LENGTH } from "./limits.ts";
import { DesktopErrorCodeSchema } from "./errors.ts";
import { StateMessageSchema } from "./state.ts";
import { WatchFrameSchema } from "./watch.ts";

export const RidSchema = z.number().int().min(0).max(0xffff_ffff);

// --- Device -> desktop ---

/** `id` is required for app methods and at most 128 chars (host.rs:698-701). */
export const EnvelopeRequestSchema = z.object({
  version: z.literal(ENVELOPE_VERSION),
  rid: RidSchema,
  id: z.string().min(1).max(MAX_COMMAND_ID_LENGTH),
  method: z.string().min(1),
  params: z.record(z.string(), z.unknown()),
  /** Only for `conversation.watch`. */
  watch: z.literal(true).optional(),
});
export type EnvelopeRequest = z.infer<typeof EnvelopeRequestSchema>;

/** Stops the watch opened with the same `rid`. */
export const CancelSchema = z.object({ rid: RidSchema, cancel: z.literal(true) });
export type Cancel = z.infer<typeof CancelSchema>;

/** What a visible, focused page shows: a session transcript or a Mono chat. */
export const PresenceTargetSchema = z.object({ kind: z.enum(["session", "mono"]), id: z.string().min(1).max(256) });
export type PresenceTarget = z.infer<typeof PresenceTargetSchema>;

/**
 * `{t: "presence", version, target}`: sent on route, visibility and focus changes and every 25 s while visible and
 * focused. `target: null` means the page is hidden, unfocused or off a chat. It has no `rid` and no answer, so a
 * desktop that does not know it drops it. The desktop keeps it in memory per session and treats it as stale after
 * 50 s; it only decides whether a web push is sent for that target (never persisted, never sent to the Worker).
 */
export const PresenceMessageSchema = z.object({
  t: z.literal("presence"),
  version: z.literal(ENVELOPE_VERSION),
  target: PresenceTargetSchema.nullable(),
});
export type PresenceMessage = z.infer<typeof PresenceMessageSchema>;

export const DeviceMessageSchema = z.union([EnvelopeRequestSchema, CancelSchema, PresenceMessageSchema]);
export type DeviceMessage = z.infer<typeof DeviceMessageSchema>;

// --- Desktop -> device ---

/** `{rid, ok: true, watching: true}`: a watch was accepted. */
export const WatchingSchema = z.object({ rid: RidSchema, ok: z.literal(true), watching: z.literal(true) });
/** `{rid, ok: true, heartbeat: true}`: every 25 s on an open watch. */
export const HeartbeatSchema = z.object({ rid: RidSchema, ok: z.literal(true), heartbeat: z.literal(true) });
/** `{rid, ok: false, error, code?}`: the human string stays, `code` is the stable key. */
export const ErrorResponseSchema = z.object({
  rid: RidSchema,
  ok: z.literal(false),
  error: z.string(),
  /** Kept a string: a newer desktop may add codes. Narrow with `DesktopErrorCodeSchema.safeParse`. */
  code: z.string().optional(),
  /** Seconds after which the same request may pass (a rate limit or a full queue); only with a `code`. */
  retryAfter: z.number().int().nonnegative().optional(),
});
/** `{rid, ok: true, result}`: a response, or a watch frame when `rid` is an open watch. */
export const OkResponseSchema = z.object({ rid: RidSchema, ok: z.literal(true), result: z.unknown() });
/** An OK response whose `result` is a watch frame. */
export const WatchFrameResponseSchema = z.object({ rid: RidSchema, ok: z.literal(true), result: WatchFrameSchema });

export type Watching = z.infer<typeof WatchingSchema>;
export type Heartbeat = z.infer<typeof HeartbeatSchema>;
export type ErrorResponse = z.infer<typeof ErrorResponseSchema>;
export type OkResponse = z.infer<typeof OkResponseSchema>;

/** Order matters: the specific `ok` shapes first, the generic `result` response last. */
export const EnvelopeResponseSchema = z.union([WatchingSchema, HeartbeatSchema, ErrorResponseSchema, OkResponseSchema]);
export type EnvelopeResponse = z.infer<typeof EnvelopeResponseSchema>;

/** Everything the desktop sends: the `state` message (no `rid`) or a response. */
export const DesktopMessageSchema = z.union([StateMessageSchema, EnvelopeResponseSchema]);
export type DesktopMessage = z.infer<typeof DesktopMessageSchema>;

export type ResponseKind = "watching" | "heartbeat" | "error" | "result";

export function responseKind(response: EnvelopeResponse): ResponseKind {
  if (!response.ok) return "error";
  if ("watching" in response) return "watching";
  if ("heartbeat" in response) return "heartbeat";
  return "result";
}

/** The `code` of an error response when it is one this build knows. */
export function knownErrorCode(response: ErrorResponse): z.infer<typeof DesktopErrorCodeSchema> | undefined {
  const parsed = DesktopErrorCodeSchema.safeParse(response.code);
  return parsed.success ? parsed.data : undefined;
}

// --- JSON <-> bytes ---

const textEncoder = new TextEncoder();
const textDecoder = new TextDecoder("utf-8", { fatal: true, ignoreBOM: false });

export function encodeEnvelope(message: DeviceMessage | DesktopMessage): Uint8Array {
  return textEncoder.encode(JSON.stringify(message));
}

/** Parses the bytes of a MSG payload as JSON. The caller validates with the matching schema. */
export function decodeEnvelopeJson(bytes: Uint8Array): unknown {
  return JSON.parse(textDecoder.decode(bytes));
}

/** Convenience: decode a MSG payload sent by the desktop. Throws on invalid input. */
export function parseDesktopMessage(bytes: Uint8Array): DesktopMessage {
  return DesktopMessageSchema.parse(decodeEnvelopeJson(bytes));
}

/** Convenience: decode a MSG payload sent by a device. Throws on invalid input. */
export function parseDeviceMessage(bytes: Uint8Array): DeviceMessage {
  return DeviceMessageSchema.parse(decodeEnvelopeJson(bytes));
}
