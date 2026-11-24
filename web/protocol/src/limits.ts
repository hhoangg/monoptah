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
/**
 * Methods that answer after work the desktop does for the request, with the time the client waits (15 s more than the host,
 * `reply_wait` in host.rs, so the host's answer, a `busy` included, always arrives first). `file.prepare` hashes a file and uploads it to R2 first (64 MiB on a slow uplink).
 */
export const REQUEST_TIMEOUT_BY_METHOD: Readonly<Record<string, number>> = {
  "file.prepare": 5 * 60_000 + 45_000,
  // A long poll: the desktop answers within `waitMs` (at most TERMINAL_READ_MAX_WAIT_MS), plus 10 s of slack. The client takes the
  // larger of this and REQUEST_TIMEOUT_MS; the terminal controller also aborts a single read after `waitMs + 10 s` on its own.
  "terminal.read": 20_000 + 10_000,
};
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
/** Raw bytes of one emoji image the desktop answers to `mono.emoji`; larger is `too_large`. */
export const MAX_MONO_EMOJI_BYTES = 4 * 1024 * 1024;
/** Most emoji picks the desktop sends on one Mono transcript text item. */
export const MONO_EMOJI_MAX_PICKS = 64;
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

// --- Encrypted file transfers (security.md 7 "File transfers", crates/monocode-core file_transfer::crypto) ---

/** Plaintext bytes of every sealed chunk but the last. */
export const TRANSFER_CHUNK_BYTES = 1024 * 1024;
/** The GCM tag every sealed chunk carries. */
export const TRANSFER_TAG_BYTES = 16;
/** Largest file the desktop seals. */
export const TRANSFER_MAX_PLAINTEXT_BYTES = 64 * 1024 * 1024;
/** Prefix of the R2 keys the lifecycle rule expires (docs/deploy-runbook.md). */
export const TRANSFER_PREFIX = "transfers/";
/** An object lives one day in R2 (lifecycle rule). */
export const TRANSFER_RETENTION_SECONDS = 24 * 60 * 60;

// --- Web terminal (security.md "Terminal"; features.md N-26). The desktop enforces every limit; the web mirrors them. ---

/** Length of the one-time code the desktop shows (31 character alphabet, case-insensitive on input). */
export const TERMINAL_CODE_LENGTH = 6;
/** Wrong codes one challenge allows before it is used up (`reauth_required` follows). */
export const TERMINAL_CODE_ATTEMPTS = 3;
/** A challenge lives this long on the desktop (display only: the desktop answers `expiresInMs`). */
export const TERMINAL_CODE_TTL_MS = 60_000;
/** Idle limit of a shell (default, display only: the open result carries the desktop's value). */
export const TERMINAL_IDLE_TIMEOUT_MS = 15 * 60_000;
/** Longest life of a shell (default, display only). */
export const TERMINAL_MAX_DURATION_MS = 8 * 60 * 60_000;
/** The desktop keeps a shell this long after the link drops; a reattach within it finds the same PTY. */
export const TERMINAL_GRACE_MS = 30_000;
/** A killed terminal stays readable this long, so the final message arrives; then `not_found`. */
export const TERMINAL_KILLED_READABLE_MS = 60_000;
/** Largest `subdir` of `terminal.open`. */
export const TERMINAL_SUBDIR_MAX = 512;
/** Window size limits of `terminal.open` and `terminal.resize`. */
export const TERMINAL_COLS_MIN = 10;
export const TERMINAL_COLS_MAX = 500;
export const TERMINAL_ROWS_MIN = 2;
export const TERMINAL_ROWS_MAX = 200;
/** `waitMs` of `terminal.read`: default and largest (the desktop clamps). */
export const TERMINAL_READ_WAIT_DEFAULT_MS = 15_000;
export const TERMINAL_READ_MAX_WAIT_MS = 20_000;
/** Slack added to `waitMs` for the client side timeout of one read. */
export const TERMINAL_READ_TIMEOUT_SLACK_MS = 10_000;
/** `maxBytes` of `terminal.read`: default and largest. */
export const TERMINAL_READ_MAX_BYTES = 32_768;
/** Largest decoded `data` of one `terminal.write`. */
export const TERMINAL_WRITE_MAX_BYTES = 8_192;
/** The web sends at most this many bytes per write (well under the limit). */
export const TERMINAL_WRITE_CHUNK_BYTES = 4_096;
/** Keystrokes typed within this window go out in one write. */
export const TERMINAL_WRITE_FLUSH_MS = 50;
/** Least time between the end of a read and the next while data keeps flowing (relay limit: 40 msg/s sustained). */
export const TERMINAL_READ_MIN_GAP_MS = 75;
/** Input waiting for the one write in flight is capped here; more is dropped with a hint. */
export const TERMINAL_PENDING_INPUT_MAX_BYTES = 64 * 1024;
/** Bytes handed to the screen per slice, so a huge burst cannot freeze the tab. */
export const TERMINAL_RENDER_SLICE_BYTES = 16 * 1024;
