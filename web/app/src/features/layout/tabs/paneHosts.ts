// The host elements of the pane views (see TabbedChat.tsx): one detached `div` per pane, created on first use and kept
// while the pane is on screen. A pane view renders into its host through a portal and the layout moves the host into
// the slot of the pane, so a split or a closed neighbour re-parents the DOM instead of remounting the pane.
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
}
