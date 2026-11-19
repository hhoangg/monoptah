// The chat pane of the classic layout: everything right of the sidebars (and left of a docked right panel), edge to
// edge, with no card, border or centering of its own. A route screen (a session, a Mono chat, the New session page, a
// project tool) fills it; the screens keep a readable column for their own text.
//
// From 640px the pane is the tabbed chat area (tabs/TabbedChat.tsx): a tab strip per pane, split panes, and the route
// screen in the focused pane when the URL is not a session or a Mono. Below 640px it is the plain route screen:
// single-session navigation, no tabs and no split.
import type { ReactNode } from "react";
import { useMediaQuery } from "@/ui/useMediaQuery";
import { TabbedChat } from "./tabs/TabbedChat";

const TABS_QUERY = "(min-width: 640px)";

export function ChatPane({ desktopId, children }: { desktopId: string; children?: ReactNode }) {
  const tabbed = useMediaQuery(TABS_QUERY);
  return (
    <div data-testid="chat-pane" className={`@container relative flex min-h-0 min-w-0 flex-1 flex-col ${tabbed ? "overflow-hidden" : "overflow-y-auto"}`}>
      {tabbed ? <TabbedChat desktopId={desktopId}>{children}</TabbedChat> : children}
    </div>
  );
}
