import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { setApiTransport } from "@/features/accounts/api";
import { getLinkStore, resetLinkStore, useLink } from "@/link/store";
import type * as LinkModule from "@/link";
import type * as KeysModule from "@/link/keys";

const h = vi.hoisted(() => ({ ensureDevice: vi.fn(), getDeviceKeyInfo: vi.fn() }));
vi.mock("@/link", async (importOriginal) => {
  const actual = await importOriginal<typeof LinkModule>();
  return {
    ...actual,
    ensureDevice: h.ensureDevice,
    useLinkDesktopId: () => "d1",
    useLinkState: (selector: Parameters<typeof useLink>[1] = (state) => state) => useLink("d1", selector),
  };
});
vi.mock("@/link/keys", async (importOriginal) => ({ ...(await importOriginal<typeof KeysModule>()), getDeviceKeyInfo: h.getDeviceKeyInfo }));

import { setBlockingLayerShown } from "./blockingLayer";
import { DesktopSwitcher } from "./DesktopSwitcher";
import { createFakeServer, desktop, grant, type FakeServer } from "./testServer";
import { mountSlot } from "./testSlotMount";

let server: FakeServer;
let restore: (() => void) | undefined;

function install() {
  server = createFakeServer({
    desktops: [
      desktop({ id: "d1", name: "Studio Mac", online: true }),
      desktop({ id: "d2", name: "Linux box", color: null, online: false }),
      desktop({ id: "d3", name: "Office PC", online: true }),
      desktop({ id: "d4", name: "Old laptop", online: false }),
    ],
    grants: {
      d1: [grant({ deviceId: "dev1", state: "confirmed" })],
      d2: [],
      d3: [grant({ deviceId: "dev1", state: "pending", permission: null, confirmedAt: null })],
      d4: [grant({ deviceId: "dev1", state: "revoked" })],
    },
  });
  restore = setApiTransport(server.transport);
}

beforeEach(() => {
  localStorage.clear();
  resetLinkStore("d1");
  getLinkStore("d1").setState({ status: "ready", desktopName: "Studio Mac", presence: "online" });
  h.ensureDevice.mockResolvedValue({ deviceId: "dev1" });
  h.getDeviceKeyInfo.mockResolvedValue({ deviceId: "dev1", publicKey: new Uint8Array(32).fill(7) });
  install();
});
afterEach(() => {
  restore?.();
  vi.clearAllMocks();
});

describe("desktop switcher", () => {
  it("is not rendered while any blocking layer shows, including the requesting one", async () => {
    mountSlot(<DesktopSwitcher />);
    await screen.findByRole("button", { name: /Switch desktop/ });
    act(() => setBlockingLayerShown("d1", true));
    expect(screen.queryByRole("button", { name: /Switch desktop/ })).toBeNull();
    act(() => setBlockingLayerShown("d1", false));
    expect(await screen.findByRole("button", { name: /Switch desktop/ })).toBeInTheDocument();
  });

  it("shows the current desktop in a slim bar and records it as last used", async () => {
    mountSlot(<DesktopSwitcher />);
    const trigger = await screen.findByRole("button", { name: "Switch desktop, current: Studio Mac" });
    expect(within(trigger).getByText("Online")).toBeInTheDocument();
    expect(JSON.parse(localStorage.getItem("monoptah.desktops.lastUsed") ?? "[]")).toEqual(["d1"]);
  });

  it("lists desktops by last use with online status, marks the current one and links to the others", async () => {
    localStorage.setItem("monoptah.desktops.lastUsed", JSON.stringify(["d3", "d1"]));
    mountSlot(<DesktopSwitcher />);
    await userEvent.click(await screen.findByRole("button", { name: /Switch desktop/ }));
    const dialog = await screen.findByRole("dialog", { name: "Switch desktop" });
    await within(dialog).findByRole("link", { name: "Open Linux box" });
    const items = within(dialog).getAllByRole("listitem");
    // d1 was just opened (front), then d3, then the never opened ones in list order.
    expect(items.map((item) => within(item).getAllByRole("link")[0]?.getAttribute("aria-label"))).toEqual([
      "Studio Mac, current desktop",
      "Open Office PC",
      "Open Linux box",
      "Open Old laptop",
    ]);
    expect(within(items[0] as HTMLElement).getByRole("link").getAttribute("aria-current")).not.toBeNull();
    expect(within(items[1] as HTMLElement).getByRole("link").getAttribute("aria-current")).toBeNull();
    expect(within(items[2] as HTMLElement).getByText("Offline")).toBeInTheDocument();
    expect(within(items[1] as HTMLElement).getByText("Online")).toBeInTheDocument();
    expect(within(dialog).getByRole("link", { name: "Open Linux box" })).toHaveAttribute("href", "/d/d2");
    expect(within(dialog).getByRole("link", { name: "Manage desktops" })).toHaveAttribute("href", "/desktops");
  });

  it("offers Request access only where this device has no grant, and posts it for that desktop only", async () => {
    mountSlot(<DesktopSwitcher />);
    await userEvent.click(await screen.findByRole("button", { name: /Switch desktop/ }));
    const dialog = await screen.findByRole("dialog", { name: "Switch desktop" });
    const request = await within(dialog).findByRole("button", { name: "Request access to Linux box" });
    expect(within(dialog).getAllByRole("button", { name: /Request access/ })).toHaveLength(1);
    expect(await within(dialog).findByText("Waiting for confirmation on the desktop")).toBeInTheDocument();
    expect(within(dialog).getByText("Access revoked")).toBeInTheDocument();
    // Reading the grants is not requesting them.
    expect(server.calls.filter((call) => call.method === "POST")).toHaveLength(0);

    await userEvent.click(request);
    await waitFor(() => expect(server.calls.filter((call) => call.method === "POST")).toHaveLength(1));
    expect(server.calls.find((call) => call.method === "POST")).toMatchObject({ path: "/api/desktops/d2/grants", body: { deviceId: "dev1" } });
    expect(localStorage.getItem("monoptah.desktops.grantRequested.d2.dev1")).toBe("1");
    await waitFor(() => expect(within(dialog).queryByRole("button", { name: /Request access/ })).toBeNull());
  });

  it("hides while a blocking link-state screen shows", async () => {
    mountSlot(<DesktopSwitcher />);
    await screen.findByRole("button", { name: /Switch desktop/ });
    act(() => getLinkStore("d1").setState({ status: "closed", blocked: "key_changed" }));
    await waitFor(() => expect(screen.queryByRole("button", { name: /Switch desktop/ })).toBeNull());
    act(() => getLinkStore("d1").setState({ status: "pending", blocked: null }));
    expect(screen.queryByRole("button", { name: /Switch desktop/ })).toBeNull();
    act(() => getLinkStore("d1").setState({ status: "ready" }));
    expect(await screen.findByRole("button", { name: /Switch desktop/ })).toBeInTheDocument();
  });
});
