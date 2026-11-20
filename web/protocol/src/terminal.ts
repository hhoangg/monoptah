// The web terminal wire contract (security.md "Terminal", features.md N-26): params and results of `terminal.open`,
// `terminal.read`, `terminal.write`, `terminal.resize` and `terminal.close`. The desktop (crates/monocode-core) enforces
// everything here again; the web validates first so a bad value never leaves the browser.
import "./zod-config.ts";
import { z } from "zod";
import {
  TERMINAL_COLS_MAX,
  TERMINAL_COLS_MIN,
  TERMINAL_READ_MAX_BYTES,
  TERMINAL_READ_MAX_WAIT_MS,
  TERMINAL_ROWS_MAX,
  TERMINAL_ROWS_MIN,
  TERMINAL_SUBDIR_MAX,
  TERMINAL_WRITE_MAX_BYTES,
} from "./limits.ts";

/** Why a session ended (`exited.reason` of `terminal.read`). */
export const TERMINAL_EXIT_REASONS = ["exit", "closed", "revoked", "idle", "max", "toggle_off", "killed", "grace", "desktop"] as const;
export type TerminalExitReason = (typeof TERMINAL_EXIT_REASONS)[number];

/**
 * True for a relative folder inside the project: not empty, no leading slash or backslash, no drive letter, no NUL and no `..`
 * segment (either separator). The desktop checks again against the real file system; this is the client side screen.
 */
export function isSafeTerminalSubdir(value: string): boolean {
  if (value.length === 0 || value.length > TERMINAL_SUBDIR_MAX) return false;
  if (value.startsWith("/") || value.startsWith("\\") || /^[a-zA-Z]:/.test(value) || value.includes("\0")) return false;
  return !value.split(/[\\/]+/).includes("..");
}

const TerminalId = z.string().min(1).max(256);
/** Base64 (standard alphabet, padded) of at most `bytes` raw bytes. */
function base64Of(bytes: number) {
  return z
    .string()
    .max(Math.ceil(bytes / 3) * 4)
    .regex(/^[A-Za-z0-9+/]*={0,2}$/)
    .refine((value) => base64DecodedLength(value) <= bytes, `at most ${bytes} bytes`);
}

/** Bytes a padded base64 string decodes to (a length that is not a multiple of 4 counts its partial group). */
export function base64DecodedLength(value: string): number {
  const padding = value.endsWith("==") ? 2 : value.endsWith("=") ? 1 : 0;
  return Math.floor(((value.length - padding) * 3) / 4);
}
const Cols = z.number().int().min(TERMINAL_COLS_MIN).max(TERMINAL_COLS_MAX);
const Rows = z.number().int().min(TERMINAL_ROWS_MIN).max(TERMINAL_ROWS_MAX);

export const TerminalOpenParamsSchema = z.object({
  projectId: z.string().min(1).max(256),
  subdir: z.string().max(TERMINAL_SUBDIR_MAX).refine(isSafeTerminalSubdir, "subdir must be a relative path without ..").optional(),
  cols: Cols,
  rows: Rows,
  /** Single-use ticket from the Worker (fresh passkey re-auth), bound to this device and `projectId`. The desktop consumes it. */
  ticket: z.string().min(1).max(256),
});
export type TerminalOpenParams = z.infer<typeof TerminalOpenParamsSchema>;

/** The shell is running. `cwd` is a display label only. */
export const TerminalOpenedSchema = z.looseObject({
  terminalId: TerminalId,
  cols: z.number().int().positive(),
  rows: z.number().int().positive(),
  cwd: z.string(),
  idleTimeoutMs: z.number().int().nonnegative(),
  maxDurationMs: z.number().int().nonnegative(),
});
export type TerminalOpened = z.output<typeof TerminalOpenedSchema>;

export const TerminalOpenResultSchema = TerminalOpenedSchema;
export type TerminalOpenResult = TerminalOpened;

export const TerminalReadParamsSchema = z.object({
  terminalId: TerminalId,
  /** The first output byte not yet received: it acknowledges everything before it. */
  from: z.number().int().min(0),
  waitMs: z.number().int().min(0).max(TERMINAL_READ_MAX_WAIT_MS).optional(),
  maxBytes: z.number().int().min(1).max(TERMINAL_READ_MAX_BYTES).optional(),
});

export const TerminalExitedSchema = z.looseObject({ reason: z.string(), code: z.number().int().optional() });
export type TerminalExited = z.output<typeof TerminalExitedSchema>;

export const TerminalReadResultSchema = z.looseObject({
  from: z.number().int().min(0),
  next: z.number().int().min(0),
  /** Base64 of the raw PTY bytes; empty when the wait ran out. */
  data: z.string(),
  exited: TerminalExitedSchema.nullable(),
});
export type TerminalReadResult = z.output<typeof TerminalReadResultSchema>;

export const TerminalWriteParamsSchema = z.object({ terminalId: TerminalId, data: base64Of(TERMINAL_WRITE_MAX_BYTES) });
export const TerminalResizeParamsSchema = z.object({ terminalId: TerminalId, cols: Cols, rows: Rows });
export const TerminalCloseParamsSchema = z.object({ terminalId: TerminalId });
