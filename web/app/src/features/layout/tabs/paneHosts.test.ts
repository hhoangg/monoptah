import { describe, expect, it } from "vitest";
import { PaneHosts } from "./paneHosts";

describe("PaneHosts", () => {
  it("makes one host per pane and forgets the ones that are not retained", () => {
    const hosts = new PaneHosts();
    const first = hosts.of("p1");
    expect(hosts.of("p1")).toBe(first);
    hosts.retain(["p2"]);
    expect(hosts.of("p1")).not.toBe(first);
  });

  it("does not keep the snapshot of a dropped host: moving it later reads the DOM as it is then", () => {
    const hosts = new PaneHosts();
    const host = hosts.of("p1");
    const child = document.createElement("div");
    host.appendChild(child);
    document.body.appendChild(host);
    child.scrollTop = 50;
    hosts.snapshot(host);
    child.scrollTop = 10;
    hosts.retain([]);
    const slot = document.createElement("div");
    document.body.appendChild(slot);
    hosts.moveInto(host, slot, true);
    expect(child.scrollTop).toBe(10);
    host.remove();
    slot.remove();
  });

  it("restores the focus only when asked to", () => {
    const hosts = new PaneHosts();
    const host = hosts.of("p1");
    const input = document.createElement("input");
    host.appendChild(input);
    document.body.appendChild(host);
    const slot = document.createElement("div");
    document.body.appendChild(slot);
    input.focus();
    hosts.snapshot(host);
    input.blur();
    hosts.moveInto(host, slot, false);
    expect(input).not.toHaveFocus();
    hosts.snapshot(host);
    hosts.moveInto(host, document.body, true);
    host.remove();
    slot.remove();
  });
});
