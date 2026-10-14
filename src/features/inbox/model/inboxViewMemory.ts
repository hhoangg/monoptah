import type { InboxSource } from "./inboxFilters";

/** What the Inbox list shows for one source, kept while the view is closed. */
export type InboxViewMemory = {
  search: string;
  selectedKey: string | null;
  scrollTop: number;
  /** Rows mounted when the view was left; null until the list grew. */
  listLimit: number | null;
};

const EMPTY: InboxViewMemory = {
  search: "",
  selectedKey: null,
  scrollTop: 0,
  listLimit: null,
};

// In memory only, like the pane width: leaving the Inbox must not lose your
// place, but a restart starts from a clean list.
const views = new Map<InboxSource, InboxViewMemory>();

export function getRememberedInboxView(source: InboxSource): InboxViewMemory {
  return { ...(views.get(source) ?? EMPTY) };
}

export function setRememberedInboxView(
  source: InboxSource,
  view: InboxViewMemory,
): void {
  views.set(source, {
    ...view,
    scrollTop:
      Number.isFinite(view.scrollTop) && view.scrollTop > 0
        ? view.scrollTop
        : 0,
  });
}

export function resetRememberedInboxViews(): void {
  views.clear();
}
