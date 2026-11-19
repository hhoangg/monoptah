// The host elements of the pane views (see TabbedChat.tsx): one detached `div` per pane, created on first use and kept
// while the pane is on screen. A pane view renders into its host through a portal and the layout moves the host into
// the slot of the pane, so a split or a closed neighbour re-parents the DOM instead of remounting the pane. React
// state (a composer draft, an open menu) survives that; the browser's own DOM state does not, so `moveInto` puts back
// the scroll offsets and the keyboard focus that a move resets.
export class PaneHosts {
  private readonly hosts = new Map<string, HTMLElement>();

  /** The host of a pane (`display: contents`, so it adds no box of its own). */
  of(id: string): HTMLElement {
    let host = this.hosts.get(id);
    if (!host) {
      host = document.createElement("div");
      host.style.display = "contents";
      this.hosts.set(id, host);
    }
    return host;
  }

  /** Forgets the hosts of panes that are not in `ids` (closed, or hidden while the window is too narrow). */
  retain(ids: readonly string[]): void {
    for (const id of [...this.hosts.keys()]) if (!ids.includes(id)) this.hosts.delete(id);
  }

  private readonly pending = new Map<HTMLElement, Snapshot>();

  /** Notes the scroll offsets inside a host and its focused element. A slot calls this as it unmounts, before the browser takes the host out of the document and resets both. */
  snapshot(host: HTMLElement): void {
    const scrolled: Snapshot["scrolled"] = [];
    for (const element of host.querySelectorAll("*")) if (element.scrollTop !== 0 || element.scrollLeft !== 0) scrolled.push([element, element.scrollTop, element.scrollLeft]);
    const focused = document.activeElement instanceof HTMLElement && host.contains(document.activeElement) ? document.activeElement : null;
    this.pending.set(host, { scrolled, focused });
  }

  /** Moves a host into `slot`, keeping the scroll offsets of everything inside it and the focused element. */
  moveInto(host: HTMLElement, slot: HTMLElement): void {
    if (host.parentNode === slot) return;
    if (!this.pending.has(host)) this.snapshot(host);
    const { scrolled, focused } = this.pending.get(host) as Snapshot;
    this.pending.delete(host);
    slot.appendChild(host);
    for (const [element, top, left] of scrolled) {
      element.scrollTop = top;
      element.scrollLeft = left;
    }
    focused?.focus({ preventScroll: true });
  }
}

interface Snapshot {
  scrolled: Array<[Element, number, number]>;
  focused: HTMLElement | null;
}
