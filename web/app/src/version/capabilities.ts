// Capability handling (W-22, features.md N-23). The desktop advertises `capabilities` in the encrypted
// `state` message; the web hides what is not advertised. Unknown names are ignored.
import { KNOWN_CAPABILITIES, type KnownCapability } from "@monoptah/protocol";
import { useLinkState } from "@/link/LinkProvider";

/** The user-facing feature name of every capability, used in "Update Monoptah on your desktop to use <feature>". */
export const FEATURES: Readonly<Record<KnownCapability, string>> = {
  approvals: "Approvals",
  cloud: "Cloud sessions",
  cloudStart: "Start cloud sessions",
  cloudMove: "Move sessions to the cloud",
  gitPush: "Push from Git",
  monoTools: "Mono tools",
  monoFiles: "Mono files",
  mentionedFiles: "File previews",
  mentionedFiles2: "File previews of files made by commands",
  fileTransfer: "Opening any file from the web",
};

export type { KnownCapability };

export function isKnownCapability(name: string): name is KnownCapability {
  return (KNOWN_CAPABILITIES as readonly string[]).includes(name);
}

/** Pure check: true only when the desktop advertised `name`. Unknown advertised names never matter. */
export function isCapabilityAvailable(capabilities: readonly string[], name: KnownCapability): boolean {
  return isKnownCapability(name) && capabilities.includes(name);
}

/** Whether the desktop of the nearest `LinkProvider` advertises `name` (re-renders on a live `state` re-send). */
export function useCapability(name: KnownCapability): boolean {
  return useLinkState((state) => isCapabilityAvailable(state.capabilities, name));
}
