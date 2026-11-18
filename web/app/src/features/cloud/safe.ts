// Text that the desktop or a cloud provider chose (instance and session names, branch and machine names,
// startup errors, auto-stop reasons, stop warnings, refusal messages) is untrusted. It is rendered as plain
// React text, never as markup; this also caps its length and removes bidi controls and other hidden
// characters, so a name cannot pose as another one and a long text cannot flood the screen.
import { safeLabel } from "@/features/accounts/pair/untrusted";

export const MAX_NAME_CHARS = 64;
export const MAX_MESSAGE_CHARS = 200;

/** A name (instance, session, branch, machine or size). Empty text reads "Unnamed". */
export function safeName(text: string, fallback = "Unnamed"): string {
  return safeLabel(text, MAX_NAME_CHARS, fallback);
}

/** A message, reason or warning of the desktop. Empty text reads `fallback`. */
export function safeMessage(text: string, fallback = ""): string {
  return safeLabel(text, MAX_MESSAGE_CHARS, fallback);
}
