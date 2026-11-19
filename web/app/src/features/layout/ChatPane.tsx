// The chat pane of the classic layout: everything right of the sidebars (and left of a docked right panel), edge to
// edge, with no card, border or centering of its own. A route screen (a session, a Mono chat, the New session page, a
// project tool) fills it; the screens keep a readable column for their own text.
//
// This is the one place to extend the chat area: a session tab strip goes above `children` in this flex column, and
// split panes replace the single `children` with a row or column of panes, each a `min-h-0 min-w-0 flex-1` flex column.
import type { ReactNode } from "react";

export function ChatPane({ children }: { children?: ReactNode }) {
  return (
    <div data-testid="chat-pane" className="@container relative flex min-h-0 min-w-0 flex-1 flex-col overflow-y-auto">
      {children}
    </div>
  );
}
