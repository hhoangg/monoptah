// WebSocket close codes of the relay (architecture.md 4.5) as data, with the
// reaction of each side. The table is exported to vectors.json.

export type CloseAction =
  /** Reconnect with exponential backoff (3.3 "Resync and reconnect"). */
  | "reconnect_backoff"
  /** Reconnect immediately, once (deploy). */
  | "reconnect_once"
  /** Retry a single time, then fall back to backoff. */
  | "retry_once"
  /** Reconnect after a fresh Noise handshake. */
  | "reconnect_new_handshake"
  /** Wait 60 s, then reconnect. */
  | "backoff_60s"
  /** Device only: go to /login. */
  | "login"
  /** Stop reconnecting. */
  | "stop";

export interface CloseReaction {
  action: CloseAction;
  /** User-facing copy, when the reaction shows something. */
  message?: string;
}

export interface CloseCodeEntry {
  code: number;
  name: string;
  meaning: string;
  device: CloseReaction;
  desktop: CloseReaction;
}

const DESKTOP_REVOKED: CloseReaction = { action: "stop", message: "Web app: access revoked, pair again" };

export const CLOSE_CODES = [
  {
    code: 1000,
    name: "normal",
    meaning: "Normal closure",
    device: { action: "reconnect_backoff" },
    desktop: { action: "reconnect_backoff" },
  },
  {
    code: 1001,
    name: "going_away",
    meaning: "Going away (deploy)",
    device: { action: "reconnect_once" },
    desktop: { action: "reconnect_once" },
  },
  {
    code: 1008,
    name: "policy",
    meaning: "Policy violation",
    device: { action: "stop", message: "Connection refused" },
    desktop: { action: "stop", message: "Connection refused" },
  },
  {
    code: 1009,
    name: "message_too_big",
    meaning: "Message too big (a bug; no retry)",
    device: { action: "stop", message: "Connection error" },
    desktop: { action: "stop", message: "Connection error" },
  },
  {
    code: 4001,
    name: "unauthorized",
    meaning: "Unauthorized (cookie or token invalid)",
    device: { action: "login" },
    desktop: DESKTOP_REVOKED,
  },
  {
    code: 4003,
    name: "revoked",
    meaning: "Revoked (grant, device, desktop or token)",
    device: { action: "stop", message: "Access revoked" },
    desktop: DESKTOP_REVOKED,
  },
  {
    code: 4008,
    name: "rate_limited",
    meaning: "Rate limited",
    device: { action: "backoff_60s" },
    desktop: { action: "backoff_60s" },
  },
  {
    code: 4009,
    name: "replaced",
    meaning: "Replaced by a newer connection",
    device: { action: "retry_once" },
    desktop: { action: "retry_once" },
  },
  {
    code: 4010,
    name: "room_quota",
    meaning: "Room quota exceeded",
    device: { action: "backoff_60s" },
    desktop: { action: "backoff_60s" },
  },
  {
    code: 4011,
    name: "blocked",
    meaning: "Account blocked",
    device: { action: "stop", message: "Account blocked" },
    desktop: DESKTOP_REVOKED,
  },
  {
    code: 4012,
    name: "protocol_error",
    meaning: "Protocol error",
    device: { action: "reconnect_new_handshake" },
    desktop: { action: "reconnect_new_handshake" },
  },
] as const satisfies readonly CloseCodeEntry[];

export type CloseCode = (typeof CLOSE_CODES)[number]["code"];

/** Named constants for the codes the relay itself sends. */
export const CloseCodes = {
  NORMAL: 1000,
  GOING_AWAY: 1001,
  POLICY: 1008,
  MESSAGE_TOO_BIG: 1009,
  UNAUTHORIZED: 4001,
  REVOKED: 4003,
  RATE_LIMITED: 4008,
  REPLACED: 4009,
  ROOM_QUOTA: 4010,
  BLOCKED: 4011,
  PROTOCOL_ERROR: 4012,
} as const satisfies Record<string, CloseCode>;

export function closeEntry(code: number): CloseCodeEntry | undefined {
  return CLOSE_CODES.find((entry) => entry.code === code);
}

/**
 * What a client does after a close. An unlisted code (for example 1006, an
 * abnormal drop) is treated like 1000: reconnect with backoff.
 */
export function closeReaction(code: number, role: "device" | "desktop" = "device"): CloseReaction {
  const entry = closeEntry(code);
  if (!entry) return { action: "reconnect_backoff" };
  return entry[role];
}
