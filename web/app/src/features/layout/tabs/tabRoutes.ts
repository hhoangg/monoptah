// The URL of a tab, and the tab of a URL. The URL stays the source of truth of the focused pane: `/d/:desktopId/s/:id`
// is a session tab, `/d/:desktopId/monos/:id` a Mono chat tab. Every other route (New session, Files, Changes, Git, the
// Mono profile, settings) is not a tab: it shows in the focused pane in place of its active tab.
import { monoTab, parseTab, sessionTab, type TabKey } from "./paneTree";

const SESSION = /^\/d\/[^/]+\/s\/([^/]+)\/?$/;
const MONO = /^\/d\/[^/]+\/monos\/([^/]+)\/?$/;

function decode(value: string): string | null {
  try {
    return decodeURIComponent(value);
  } catch {
    return null;
  }
}

export function tabOfPath(pathname: string): TabKey | null {
  const session = SESSION.exec(pathname)?.[1];
  if (session) {
    const id = decode(session);
    return id ? sessionTab(id) : null;
  }
  const mono = MONO.exec(pathname)?.[1];
  if (mono) {
    const id = decode(mono);
    return id ? monoTab(id) : null;
  }
  return null;
}

/** Router `navigate` options for a tab. */
export function tabLocation(desktopId: string, key: TabKey) {
  const tab = parseTab(key);
  if (!tab) return null;
  return tab.kind === "session"
    ? ({ to: "/d/$desktopId/s/$sessionId", params: { desktopId, sessionId: tab.id } } as const)
    : ({ to: "/d/$desktopId/monos/$monoId", params: { desktopId, monoId: tab.id } } as const);
}
