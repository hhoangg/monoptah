// Copied from src/features/sessions/model/session.ts at ca73ac2b
// The `HarnessId`, `Attachment` and `RuntimeMode` unions only. The wire types live in wire.ts and are re-exported here.
//
// `full-access` rule (security.md 7, architecture.md 7.1): the desktop may REPORT `full-access`
// on a session card or in `models.list`, but no web code path may SEND it. Everything the web
// offers or sends goes through SENDABLE_RUNTIME_MODES and `toSendableRuntimeMode`.
import {
  HARNESS_IDS,
  HarnessSchema,
  RuntimeModeSchema,
  SendableRuntimeModeSchema,
  type Harness,
  type RuntimeMode,
  type SendableRuntimeMode,
} from "./wire.ts";

export { AttachmentSchema, AttachmentsSchema, SessionCardSchema } from "./methods.ts";
export type { Attachment, SessionCard } from "./methods.ts";
export { HARNESS_IDS, HarnessSchema, RuntimeModeSchema, SendableRuntimeModeSchema };
export type { Harness, RuntimeMode, SendableRuntimeMode };

/** The desktop's harness id (the same 11 ids as mobile `Harness`). */
export type HarnessId = Harness;

/** Every mode the desktop may report, in picker order. Includes `full-access`: display only. */
export const REPORTED_RUNTIME_MODES: readonly RuntimeMode[] = RuntimeModeSchema.options;

/** Modes the web may offer and send. Never contains `full-access`. */
export const SENDABLE_RUNTIME_MODES: readonly SendableRuntimeMode[] = SendableRuntimeModeSchema.options;

export const DEFAULT_RUNTIME_MODE: SendableRuntimeMode = "supervised";

export const RUNTIME_MODE_LABEL: Record<RuntimeMode, string> = {
  supervised: "Supervised",
  "auto-accept-edits": "Auto-accept edits",
  auto: "Auto",
  "full-access": "Full access",
};

export const RUNTIME_MODE_HINT: Record<RuntimeMode, string> = {
  supervised: "Ask before commands and file changes.",
  "auto-accept-edits": "Auto-approve edits, ask before other actions.",
  auto: "An AI reviewer can approve or deny actions.",
  "full-access": "Set on the desktop. This browser cannot turn it on.",
};

export function isSendableRuntimeMode(mode: unknown): mode is SendableRuntimeMode {
  return SendableRuntimeModeSchema.safeParse(mode).success;
}

/**
 * The mode to put on the wire for a reported mode. `full-access` (and anything unknown) has no
 * sendable form, so it maps to null and the caller leaves the session's mode unchanged.
 */
export function toSendableRuntimeMode(mode: RuntimeMode | string | undefined): SendableRuntimeMode | null {
  return isSendableRuntimeMode(mode) ? mode : null;
}

export function isHarnessId(value: unknown): value is HarnessId {
  return HarnessSchema.safeParse(value).success;
}
