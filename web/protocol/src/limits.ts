// Protocol constants. Every number cites the document section it comes from
// (paths are relative to web/docs). Changing a value here is a contract change:
// bump PROTOCOL_VERSION when the wire meaning changes (workstreams.md 2.3).

// --- Versions (architecture.md 5.6) ---

/** `proto` of the `state` message (5.3, 5.5, 5.6). */
export const PROTOCOL_VERSION = 1;
/** Byte 0 of the outer relay frame (5.1). */
export const FRAME_VERSION = 1;
/** `"version": 1` of the JSON application envelope (5.5, enforced by the host at host.rs:641). */
export const ENVELOPE_VERSION = 1;
/** Start of the Noise prologue: `"monoptah-web/1" || desktopId || deviceId || channel` (5.3). */
export const PROLOGUE_PREFIX = "monoptah-web/1";

// --- Outer relay frame (5.1, 4.4) ---

/** version(1) + type(1) + channel(2) + deviceId(16) (5.1). */
export const FRAME_HEADER_SIZE = 20;
/** Largest body: the largest Noise message (5.1, 5.2). */
export const MAX_FRAME_BODY = 65_535;
/** Smallest frame: a header with an empty body, such as BYE (4.4: "20 to 65,555 bytes"). */
export const MIN_FRAME_SIZE = FRAME_HEADER_SIZE;
/** Largest frame: 20 header + 65,535 body (4.4 "Max WebSocket message"). */
export const MAX_FRAME_SIZE = FRAME_HEADER_SIZE + MAX_FRAME_BODY;

// --- Noise plaintext and chunking (5.2) ---

/** 65,535 Noise message minus the 16-byte ChaChaPoly tag (5.2). Includes the 1-byte plaintext `kind`. */
export const MAX_NOISE_PLAINTEXT = 65_519;
/** The ChaChaPoly tag (5.2). */
export const NOISE_TAG_SIZE = 16;
/** Reassembled request limit (5.2: the web transport lowers it to 8 MiB). */
export const MAX_REQUEST_BYTES = 8 * 1024 * 1024;
/** Reassembled response limit (5.2). */
export const MAX_RESPONSE_BYTES = 8 * 1024 * 1024;
/** Messages in reassembly per session (5.2). */
export const MAX_REASSEMBLY_IN_FLIGHT = 4;
/** A reassembly must finish within this, else the session ends with BYE (5.2). */
export const REASSEMBLY_TIMEOUT_MS = 60_000;

// --- Timing (5.5, 3.3, 5.4) ---

/** Client request timeout (3.3 step 5; desktop handler is 30 s). */
export const REQUEST_TIMEOUT_MS = 35_000;
/** Desktop handler timeout, for reference (3.3 step 5). */
export const DESKTOP_HANDLER_TIMEOUT_MS = 30_000;
/** Watch heartbeat period on the web link (5.5; 10 s on QUIC). */
export const HEARTBEAT_INTERVAL_MS = 25_000;
/** The client treats this much silence as a dead link (3.3 step 5). */
export const DEAD_AFTER_MS = 75_000;
/** Client WebSocket text `ping` period (3.3 step 6). */
export const PING_INTERVAL_MS = 30_000;
/** Presence needs a keepalive ping within this (4.4 "Presence"). */
export const PRESENCE_TIMEOUT_MS = 90_000;
/** Rekey after this many messages (5.4). */
export const REKEY_AFTER_MESSAGES = 2 ** 16;
/** Rekey after this long (5.4). */
export const REKEY_AFTER_MS = 60 * 60 * 1000;
/** A session ends after this idle time (5.4). */
export const SESSION_IDLE_LIMIT_MS = 12 * 60 * 60 * 1000;

// --- Per-room quotas (4.4) ---

export const ROOM_QUOTAS = {
  /** A new desktop socket replaces the old one, which closes 4009. */
  desktopSockets: 1,
  /** Device sockets per room; a new one closes 4010. */
  deviceSocketsPerRoom: 16,
  /** Device sockets per device id (several tabs). */
  deviceSocketsPerDevice: 4,
  /** Device inbound rate: sustained msg/s, burst, bytes/s. */
  deviceRate: { messagesPerSecond: 40, burst: 100, bytesPerSecond: 1024 * 1024 },
  /** Desktop outbound rate. */
  desktopRate: { messagesPerSecond: 200, bytesPerSecond: 4 * 1024 * 1024 },
  /** Close 4008 after this many breaches within the window. */
  rateBreachesBeforeClose: 3,
  rateBreachWindowMs: 10_000,
  /** The only text frame the DO accepts (auto-response); other text closes 4012. */
  textFramePing: "ping",
  textFramePong: "pong",
} as const;

// --- Per-account defaults (4.4; self-hosters may change them) ---

export const ACCOUNT_QUOTAS = {
  maxDesktopsPerUser: 5,
  maxDevicesPerUser: 10,
  /** `invites_granted` default (4.3, security.md 4). */
  invitesPerUser: 3,
} as const;

// --- Desktop host limits (5.5) ---

/** Pending requests host-wide (host.rs:57). */
export const HOST_MAX_PENDING = 16;
/** Open watches per device (host.rs:58). */
export const HOST_MAX_WATCHES_PER_DEVICE = 4;
/** Longest command id (host.rs:698-701). */
export const MAX_COMMAND_ID_LENGTH = 128;
/** A pending device is dropped after this without desktop confirmation (5.3). */
export const PENDING_CONFIRMATION_TIMEOUT_MS = 5 * 60 * 1000;
/** Attachments per message (mobile_host.rs:37). */
export const MAX_ATTACHMENTS = 10;
/** Message text limit (mobile_host.rs:38). */
export const MAX_MESSAGE_CHARS = 100_000;

// --- Pairing (6.4) ---

/** Length of the user code. */
export const USER_CODE_LENGTH = 8;
/** Alphabet without 0, O, 1, I, L (6.4). 31 characters. */
export const USER_CODE_ALPHABET = "23456789ABCDEFGHJKMNPQRSTUVWXYZ";
/** A pairing request lives 10 minutes (6.4). */
export const PAIRING_TTL_SECONDS = 600;
/** Desktop poll interval (6.4). */
export const PAIRING_POLL_INTERVAL_SECONDS = 3;
