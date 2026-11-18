// Shared vocabulary of the cloud UI (U-16 to U-19): the labels of target kinds, cloud states and step
// statuses, the human names of the startup and move steps, and small helpers over `SessionCard` and
// the `cloud` field. Every state is a word (never colour only). `unknown` reads "Not connected".
import type { CloudField, CloudState, MoveStepId, SessionCard, StartupStepId, StepStatus, TargetKind } from "@monoptah/protocol";
import { useLinkState } from "@/link";

export const KIND_LABELS: Readonly<Record<TargetKind, string>> = {
  local: "Local",
  sandbox: "Docker sandbox",
  codespace: "GitHub codespace",
  remote: "Remote machine",
};

export const STATE_LABELS: Readonly<Record<CloudState, string>> = {
  available: "Available",
  starting: "Starting",
  stopping: "Stopping",
  stopped: "Stopped",
  unavailable: "Unavailable",
  unknown: "Not connected",
};

export const STARTUP_STEP_LABELS: Readonly<Record<StartupStepId, string>> = {
  create: "Create the instance",
  provision: "Provision resources",
  buildContainer: "Build the container",
  blockingCommands: "Run setup commands",
  configureCodespace: "Configure the codespace",
  connect: "Connect to the instance",
  openProject: "Open the project",
  claudeSetup: "Set up Claude",
  lifecycleCommands: "Run lifecycle commands",
};

export const MOVE_STEP_LABELS: Readonly<Record<MoveStepId, string>> = {
  snapshot: "Snapshot the workspace",
  push: "Push the code",
  workspace: "Prepare the workspace",
  upload: "Upload the session",
  restoreCode: "Restore the code",
  restoreConversation: "Restore the conversation",
  flip: "Switch the session over",
};

export const STEP_STATUS_LABELS: Readonly<Record<StepStatus, string>> = {
  pending: "Pending",
  running: "Running",
  done: "Done",
  failed: "Failed",
  skipped: "Skipped",
};

/** Kinds that bill (an instance that costs money while it runs). `remote` is a monocode-host machine. */
export type BillableKind = "codespace" | "sandbox";

export const isBillableKind = (kind: string | undefined | null): kind is BillableKind => kind === "codespace" || kind === "sandbox";

/** A session on anything but this desktop (an absent target means local). */
export const isNonLocalKind = (kind: string | undefined | null): boolean => !!kind && kind !== "local";

export function kindLabel(kind: string): string {
  return (KIND_LABELS as Record<string, string>)[kind] ?? "Cloud";
}

export function stateLabel(state: string): string {
  return (STATE_LABELS as Record<string, string>)[state] ?? "Not connected";
}

/** Where a session runs and in which state; the live `cloud` field wins over the catalog card. */
export interface Placement {
  kind: string;
  state: CloudState | undefined;
}

export function placementOf(session: Pick<SessionCard, "target" | "cloudState"> | undefined, cloud?: CloudField): Placement | null {
  const kind = cloud?.kind ?? session?.target?.kind;
  if (!kind) return null;
  return { kind, state: cloud?.state ?? session?.target?.state ?? session?.cloudState };
}

/** Every kind and state the live `cloud` field and the catalog card report; the send guard checks all of them. */
export function placementSources(
  session: Pick<SessionCard, "target" | "cloudState"> | undefined,
  cloud?: CloudField,
  /** False while the catalog is still the persisted copy (not yet refreshed by a live reply). */
  catalogFresh = true,
): { kinds: string[]; states: CloudState[] } {
  const kinds = [cloud?.kind, session?.target?.kind].filter((kind): kind is NonNullable<typeof kind> => !!kind);
  const states = [cloud?.state, session?.target?.state, session?.cloudState].filter((state): state is CloudState => !!state);
  // A persisted card can be days old: with no live `cloud` field to confirm it, a cloud card is not trusted as
  // `available` (or anything else). `unknown` makes the send guard ask with the wake cost notice.
  if (!catalogFresh && !cloud && kinds.some((kind) => kind === "codespace" || kind === "sandbox")) states.push("unknown");
  return { kinds: [...new Set<string>(kinds)], states: [...new Set(states)] };
}

/** True when the device may change things (link permission `control`). Other devices see controls disabled. */
export function useCanControl(): boolean {
  return useLinkState((state) => state.permission === "control");
}
