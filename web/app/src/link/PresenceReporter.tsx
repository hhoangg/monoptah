// Renders nothing: reports what the page shows to the desktop (see presence.ts). A component of its own so that the
// status changes it follows re-render only this leaf, never the screen next to it.
import type { PresenceTarget } from "@monoptah/protocol";
import { usePresence } from "./presence.ts";

export function PresenceReporter({ kind, id }: { kind: PresenceTarget["kind"]; id: string }) {
  usePresence(kind, id);
  return null;
}
