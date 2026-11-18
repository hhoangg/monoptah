// Ported from mobile/src/features/models/access.ts (the closer source) with the labels and hints
// of src/features/sessions/model/session.ts. `full-access` is NOT a choice: the desktop may
// report it, the web only shows it (see FULL_ACCESS_DISPLAY) and never sends it.
import type { RuntimeMode, SendableRuntimeMode } from "@monoptah/protocol";

export interface AccessMode {
  id: SendableRuntimeMode;
  label: string;
  hint: string;
  icon: "lock" | "edit" | "sparkle";
}

/** The modes this browser may offer and send, in picker order. */
export const ACCESS_MODES: readonly AccessMode[] = [
  { id: "supervised", label: "Supervised", hint: "Ask before commands and file changes.", icon: "lock" },
  { id: "auto-accept-edits", label: "Auto-accept edits", hint: "Auto-approve edits, ask before other actions.", icon: "edit" },
  { id: "auto", label: "Auto", hint: "An AI reviewer can approve or deny actions.", icon: "sparkle" },
];

export const DEFAULT_ACCESS: SendableRuntimeMode = "supervised";

/** What a session running with the reported `full-access` shows: read only, set on the desktop. */
export const FULL_ACCESS_DISPLAY = {
  label: "Full access",
  note: "Full access (set on desktop)",
  hint: "This browser cannot turn full access on or off.",
} as const;

export function isFullAccess(mode: RuntimeMode | string | undefined): boolean {
  return mode === "full-access";
}

/** The sendable mode for a reported mode, or null when there is none (full access, unknown). */
export function sendableAccess(mode: RuntimeMode | string | undefined): SendableRuntimeMode | null {
  return ACCESS_MODES.find((item) => item.id === mode)?.id ?? null;
}

export function accessMode(id: RuntimeMode | string | undefined): AccessMode {
  return ACCESS_MODES.find((mode) => mode.id === id) ?? ACCESS_MODES[0]!;
}

