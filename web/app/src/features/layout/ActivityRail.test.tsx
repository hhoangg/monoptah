import { act, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useParams } from "@tanstack/react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { setApiTransport } from "@/features/accounts/api";
import { createFakeServer, desktop, grant } from "@/features/desktops/testServer";
import { mountSlot } from "@/features/desktops/testSlotMount";
import { getLinkStore, resetLinkStore, useLink } from "@/link/store";
import type * as LinkModule from "@/link";
import type * as KeysModule from "@/link/keys";

const h = vi.hoisted(() => ({ cloud: true, getDeviceKeyInfo: vi.fn() }));
vi.mock("@/link", async (importOriginal) => {
  const actual = await importOriginal<typeof LinkModule>();
  return {
    ...actual,
    useLinkState: (selector: Parameters<typeof useLink>[1] = (state) => state) => useLink("d1", selector),
  };
});
vi.mock("@/link/keys", async (importOriginal) => ({ ...(await importOriginal<typeof KeysModule>()), getDeviceKeyInfo: h.getDeviceKeyInfo }));
vi.mock("@/version", () => ({ useCapability: () => h.cloud }));

import { ActivityRail } from "./ActivityRail";

let restore: (() => void) | undefined;

function Rail() {
  const { desktopId } = useParams({ strict: false }) as { desktopId: string };
  return <ActivityRail desktopId={desktopId} />;
}

beforeEach(() => {
  h.cloud = true;
  h.getDeviceKeyInfo.mockResolvedValue({ deviceId: "dev1", publicKey: new Uint8Array(32).fill(7) });
  localStorage.clear();
  resetLinkStore("d1");
  getLinkStore("d1").setState({ status: "ready", desktopName: "Studio Mac", presence: "online" });
  const server = createFakeServer({
    desktops: [
      desktop({ id: "d1", name: "Studio Mac", online: true, color: "#459cf7" }),
      desktop({ id: "d2", name: "Linux box", color: null, online: false }),
      desktop({ id: "d3", name: "Office PC", online: true, color: "#ea603e" }),
    ],
    grants: {
      d1: [grant({ deviceId: "dev1", state: "confirmed" })],
      d2: [grant({ deviceId: "dev1", state: "confirmed" })],
      d3: [grant({ deviceId: "dev1", state: "confirmed" })],
    },
  });
  restore = setApiTransport(server.transport);
});
afterEach(() => {
  restore?.();
  vi.clearAllMocks();
});

describe("activity rail", () => {
  it("lists the paired desktops as avatars with initials, a status dot and the current one marked", async () => {
    mountSlot(<Rail />);
    const list = await screen.findByRole("list", { name: "Desktops" });
    await within(list).findByRole("link", { name: "Office PC, online" });
    const links = within(list).getAllByRole("link");
    expect(links.map((link) => link.getAttribute("aria-label"))).toEqual([
      "Studio Mac, online, current desktop",
      "Linux box, offline",
      "Office PC, online",
    ]);
    expect(links.map((link) => link.textContent)).toEqual(["SM", "LB", "OP"]);
    expect(links[0]).toHaveAttribute("aria-current", "page");
    expect(links[1]).not.toHaveAttribute("aria-current");
    expect(links[0]).toHaveStyle({ backgroundColor: "#459cf7" });
    expect(links[0]!.querySelector("[data-status]")).toHaveAttribute("data-status", "online");
    expect(links[1]!.querySelector("[data-status]")).toHaveAttribute("data-status", "offline");
    // The indicator bar sits beside the current desktop only.
    expect(within(list).getAllByTestId("rail-indicator")).toHaveLength(1);
    expect(list.querySelector("[data-testid=rail-indicator]")?.parentElement).toContainElement(links[0]!);
    // Every target is 44px on touch.
    for (const link of links) expect(link.className).toContain("size-11");
  });

  it("shows the full name and the status in a tooltip on hover", async () => {
    mountSlot(<Rail />);
    const link = await screen.findByRole("link", { name: "Linux box, offline" });
    await userEvent.hover(link);
    const tip = await screen.findByRole("tooltip");
    expect(tip).toHaveTextContent("Linux box");
    expect(tip).toHaveTextContent("Offline");
  });

  it("switches desktop on click and moves the current marker", async () => {
    const { router } = mountSlot(<Rail />);
    await userEvent.click(await screen.findByRole("link", { name: "Office PC, online" }));
    expect(router.state.location.pathname).toBe("/d/d3");
    expect(await screen.findByRole("link", { name: "Office PC, online, current desktop" })).toHaveAttribute("aria-current", "page");
    expect(screen.getByRole("link", { name: "Studio Mac, online" })).not.toHaveAttribute("aria-current");
  });

  it("moves between desktops with the arrow keys and opens one with Enter", async () => {
    const { router } = mountSlot(<Rail />);
    const first = await screen.findByRole("link", { name: "Studio Mac, online, current desktop" });
    await screen.findByRole("link", { name: "Linux box, offline" });
    const user = userEvent.setup();
    act(() => first.focus());
    await user.keyboard("{ArrowDown}");
    expect(screen.getByRole("link", { name: "Linux box, offline" })).toHaveFocus();
    await user.keyboard("{ArrowUp}{ArrowUp}");
    expect(screen.getByRole("link", { name: "Office PC, online" })).toHaveFocus();
    await user.keyboard("{Enter}");
    expect(router.state.location.pathname).toBe("/d/d3");
  });

  it("is one tab stop (roving tabindex), and Home and End jump to the first and last desktop", async () => {
    mountSlot(<Rail />);
    const first = await screen.findByRole("link", { name: "Studio Mac, online, current desktop" });
    const last = await screen.findByRole("link", { name: "Office PC, online" });
    const middle = screen.getByRole("link", { name: "Linux box, offline" });
    // The open desktop is the tab stop until another one takes focus.
    expect([first, middle, last].map((link) => link.getAttribute("tabindex"))).toEqual(["0", "-1", "-1"]);
    const user = userEvent.setup();
    act(() => first.focus());
    await user.keyboard("{End}");
    expect(last).toHaveFocus();
    expect([first, middle, last].map((link) => link.getAttribute("tabindex"))).toEqual(["-1", "-1", "0"]);
    await user.keyboard("{Home}");
    expect(first).toHaveFocus();
    expect(first.getAttribute("tabindex")).toBe("0");
  });

  it("shows this device's grant on each desktop (aria-label, tooltip, lock badge) and still opens it to request access", async () => {
    restore?.();
    restore = setApiTransport(
      createFakeServer({
        desktops: [
          desktop({ id: "d1", name: "Studio Mac", online: true }),
          desktop({ id: "d2", name: "Waiting box", online: true }),
          desktop({ id: "d3", name: "Revoked box", online: true }),
          desktop({ id: "d4", name: "New box", online: false }),
          desktop({ id: "d5", name: "Fine box", online: true }),
        ],
        grants: {
          d1: [grant({ deviceId: "dev1", state: "confirmed" })],
          d2: [grant({ deviceId: "dev1", state: "pending", permission: null, confirmedAt: null })],
          d3: [grant({ deviceId: "dev1", state: "revoked" })],
          d4: [],
          d5: [grant({ deviceId: "dev1", state: "confirmed" })],
        },
      }).transport,
    );
    const { router } = mountSlot(<Rail />);
    const pending = await screen.findByRole("link", { name: "Waiting box, online, waiting for confirmation on the desktop" });
    const revoked = await screen.findByRole("link", { name: "Revoked box, online, access revoked" });
    const none = await screen.findByRole("link", { name: "New box, offline, no access yet, open it to request" });
    expect([pending, revoked, none].map((link) => link.getAttribute("data-grant"))).toEqual(["pending", "revoked", "none"]);
    for (const link of [pending, revoked, none]) expect(within(link).getByTestId("rail-lock")).toBeInTheDocument();
    // A desktop with access, and the open one, carry no cue.
    const fine = await screen.findByRole("link", { name: "Fine box, online" });
    expect(within(fine).queryByTestId("rail-lock")).toBeNull();
    expect(within(screen.getByRole("link", { name: "Studio Mac, online, current desktop" })).queryByTestId("rail-lock")).toBeNull();
    await userEvent.hover(none);
    expect(await screen.findByRole("tooltip")).toHaveTextContent("no access yet, open it to request");
    // Opening it navigates: the link-state screen of that desktop sends the request.
    await userEvent.click(none);
    expect(router.state.location.pathname).toBe("/d/d4");
  });

  it("scrolls when there are many desktops", async () => {
    mountSlot(<Rail />);
    const list = await screen.findByRole("list", { name: "Desktops" });
    expect(list.className).toContain("overflow-y-auto");
    expect(list.className).toContain("min-h-0");
  });

  it("puts Cloud, Desktop settings and the account avatar at the bottom, in that order", async () => {
    mountSlot(<Rail />);
    const actions = await screen.findByTestId("rail-actions");
    const order = [...actions.querySelectorAll("a, button")].map((el) => el.getAttribute("aria-label"));
    expect(order).toEqual(["Cloud", "Desktop settings", "Account"]);
    expect(within(actions).getByRole("link", { name: "Cloud" })).toHaveAttribute("href", "/d/d1/cloud");
    expect(within(actions).getByRole("link", { name: "Desktop settings" })).toHaveAttribute("href", "/d/d1/settings");
  });

  it("leaves Cloud out when the desktop has no cloud capability", async () => {
    h.cloud = false;
    mountSlot(<Rail />);
    const actions = await screen.findByTestId("rail-actions");
    expect([...actions.querySelectorAll("a, button")].map((el) => el.getAttribute("aria-label"))).toEqual(["Desktop settings", "Account"]);
  });

  it("opens the account menu with All desktops and Install app, and has no monitor or download icon on the rail", async () => {
    mountSlot(<Rail />);
    const rail = await screen.findByTestId("activity-rail");
    expect(rail.querySelector('[data-icon="computer"], [data-icon="download"]')).toBeNull();
    await userEvent.click(within(rail).getByRole("button", { name: "Account" }));
    const menu = await screen.findByRole("menu");
    expect(within(menu).getByRole("menuitem", { name: "Account settings" })).toHaveAttribute("href", "/settings");
    expect(within(menu).getByRole("menuitem", { name: "All desktops" })).toHaveAttribute("href", "/desktops");
    expect(within(menu).getByRole("menuitem", { name: "Install app" })).toHaveAttribute("href", "/install");
  });

  it("still shows the open desktop from the live link when the list does not have it", async () => {
    restore?.();
    restore = setApiTransport(createFakeServer({ desktops: [] }).transport);
    mountSlot(<Rail />);
    expect(await screen.findByRole("link", { name: "Studio Mac, online, current desktop" })).toBeInTheDocument();
  });
});
