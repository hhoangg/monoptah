// Typed registries for the two seams of workstreams.md 3.2 that are not components: `send-guard`
// (async hook, filled by U-16) and `session-capabilities` (predicate, filled by U-16). Same
// contract as the slots: registered once from a `features/<name>/register.ts`, a second
// registration throws, and the default is a no-op (allow, and every capability available).

/** What the user is about to send. `target` is the session target (`{kind: "local" | "cloud" ...}`), opaque in Wave 0. */
export interface SendGuardContext {
  desktopId: string;
  sessionId: string;
  target?: { kind: string } | null;
}

/** The notice shown when the desktop refuses a send with `wake_required` although the guard allowed it (a stale cached state). */
export const WAKE_REQUIRED_NOTICE = "This instance is stopped. Sending wakes it, and a running instance is billable until it stops.";

/**
 * `confirm` shows a notice (for example a cost notice) and sends only if the user agrees. `wake: true` marks a
 * notice that asks to wake a stopped cloud instance: the send after the confirmation then carries `wake: true`.
 */
export type SendGuardResult =
  | { kind: "allow" }
  | { kind: "confirm"; notice: string; wake?: true }
  | { kind: "refuse"; reason: string };

export type SendGuard = (context: SendGuardContext) => Promise<SendGuardResult>;

/** Session actions a non-local session may hide (Delete, Files, Changes, Git). */
export const SESSION_CAPABILITIES = ["delete", "files", "changes", "git"] as const;
export type SessionCapability = (typeof SESSION_CAPABILITIES)[number];

export interface SessionCapabilityContext {
  target?: { kind: string } | null;
  /** The desktop's advertised capability names (`state.capabilities`). */
  desktopCapabilities: readonly string[];
}

export type SessionCapabilityPredicate = (capability: SessionCapability, context: SessionCapabilityContext) => boolean;

const allowAll: SendGuard = () => Promise.resolve({ kind: "allow" });
const everything: SessionCapabilityPredicate = () => true;

let sendGuard: SendGuard | undefined;
let capabilityPredicate: SessionCapabilityPredicate | undefined;

export function registerSendGuard(guard: SendGuard): void {
  if (sendGuard) throw new Error('Hook "send-guard" is already registered');
  sendGuard = guard;
}

/** Asks the registered guard, or allows when none is registered. */
export function runSendGuard(context: SendGuardContext): Promise<SendGuardResult> {
  return (sendGuard ?? allowAll)(context);
}

export function registerSessionCapabilities(predicate: SessionCapabilityPredicate): void {
  if (capabilityPredicate) throw new Error('Hook "session-capabilities" is already registered');
  capabilityPredicate = predicate;
}

/** True when the session may offer `capability`; true for everything when no predicate is registered. */
export function sessionCan(capability: SessionCapability, context: SessionCapabilityContext): boolean {
  return (capabilityPredicate ?? everything)(capability, context);
}

/** Drops both registrations. For tests. */
export function clearHooks(): void {
  sendGuard = undefined;
  capabilityPredicate = undefined;
}
