// Error codes. Two independent vocabularies:
//  - DesktopErrorCode: the additive `code` on desktop link responses
//    (architecture.md 5.5, workstreams.md 2.1). The human `error` string stays;
//    `code` gives the web client a stable key.
//  - ApiErrorCode: the `{error, code}` body of Worker `/api/*` failures
//    (architecture.md 4, security.md 3-4).
import "./zod-config.ts";
import { z } from "zod";

// --- Desktop link ---

export const DESKTOP_ERROR_CODES = [
  "busy",
  "forbidden",
  "read_only",
  "not_selected",
  "path_denied",
  "revoked",
  "disabled",
  "too_large",
  "unsupported",
  "unsupported_target",
  "starting",
  "auto_stopping",
  "instance_busy",
  "not_pending",
  "needs_desktop",
  "wake_required",
] as const;
export type DesktopErrorCode = (typeof DESKTOP_ERROR_CODES)[number];
export const DesktopErrorCodeSchema = z.enum(DESKTOP_ERROR_CODES);

/**
 * Codes in the ERROR relay frame body `{code}` (architecture.md 4.4, 5.6).
 * Additive: clients show a generic message for a code they do not know.
 */
export const RELAY_ERROR_CODES = [
  /** No desktop socket in the room (4.4). */
  "desktop_offline",
  /** The desktop refuses an older prologue (5.6). */
  "unsupported_version",
] as const;
export type RelayErrorCode = (typeof RELAY_ERROR_CODES)[number];

// --- Worker API ---

export const API_ERROR_CODES = [
  /** Malformed body or parameter (400). */
  "invalid",
  /** Registration with a bad, used, expired or paused invite: deliberately generic (4.2, security.md 4). */
  "invalid_invite",
  /** No valid session or token (401). */
  "unauthorized",
  /** Valid session but not allowed (403): wrong owner, wrong role, blocked. */
  "forbidden",
  /** A passkey re-auth within the last 5 minutes is needed (403). */
  "fresh_required",
  /** A `recovery_only` session may only add a passkey (403). */
  "recovery_only",
  /** The account is blocked (403). */
  "blocked",
  /** Missing `X-Monoptah-CSRF: 1` (403, security.md 3.4). */
  "csrf",
  /** `Origin` header missing or not RP_ORIGIN (403, security.md 3.4). */
  "bad_origin",
  /** A non-GET cookie route got a Content-Type other than `application/json` (415, security.md 3.4). */
  "unsupported_media_type",
  /** 429, with Retry-After (security.md 3.5). */
  "rate_limited",
  /** 404. */
  "not_found",
  /** State conflict (409): handle taken, last passkey, already approved. */
  "conflict",
  /** Per-account quota reached (4.4). */
  "quota_exceeded",
  /** Invites paused or MAX_USERS reached (4.4, security.md 4). */
  "invites_paused",
  /** The pairing code or challenge expired or was already used (6.4). */
  "expired",
  /** Cannot delete the last passkey (4.2). */
  "last_passkey",
  /** 501 stub (workstreams.md 2.1 contract e). */
  "unsupported",
  /** Unexpected failure (500). */
  "internal",
] as const;
export type ApiErrorCode = (typeof API_ERROR_CODES)[number];
export const ApiErrorCodeSchema = z.enum(API_ERROR_CODES);

/** Body of every failed `/api/*` response. */
export const ApiErrorSchema = z.object({
  error: z.string(),
  code: ApiErrorCodeSchema,
});
export type ApiError = z.infer<typeof ApiErrorSchema>;
