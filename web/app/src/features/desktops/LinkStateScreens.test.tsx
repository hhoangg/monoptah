import { act, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { setApiTransport } from "@/features/accounts/api";
import { displayFingerprint } from "@/link/fingerprint";
import { getLinkStore, resetLinkStore, useLink } from "@/link/store";
import type * as LinkModule from "@/link";
import type { LinkState } from "@/link";

const h = vi.hoisted(() => ({ reconnect: vi.fn(), ensureDevice: vi.fn() }));
vi.mock("@/link", async (importOriginal) => {
  const actual = await importOriginal<typeof LinkModule>();
  return {
    ...actual,
    ensureDevice: h.ensureDevice,
    useLinkDesktopId: () => "d1",
    useLinkConnection: () => ({ reconnect: h.reconnect }),
    useLinkState: (selector: Parameters<typeof useLink>[1] = (state) => state) => useLink("d1", selector),
  };
});

import { LinkStateScreens } from "./LinkStateScreens";
import { createFakeServer, desktop, FP_A, FP_B, grant, type FakeServer } from "./testServer";
import { mountSlot } from "./testSlotMount";

const MARKER = "monoptah.desktops.grantRequested.d1.dev1";
const GRANT_PATH = "/api/desktops/d1/grants";
/** The first-open race: the relay closed with 4003 before the link was ever pending or ready. */
const RACE: Partial<LinkState> = { status: "closed", blocked: "revoked", revoked: true, lastError: { code: "close_4003", closeCode: 4003 } };
let server: FakeServer;
let restore: (() => void) | undefined;

function install(options: Parameters<typeof createFakeServer>[0] = {}) {
  server = createFakeServer({ desktops: [desktop({ id: "d1" }), desktop({ id: "d2", name: "Other" })], ...options });
  restore = setApiTransport(server.transport);
}

function setLink(patch: Partial<LinkState>) {
  act(() => getLinkStore("d1").setState(patch));
}

function grantPosts(): Array<{ path: string; body: unknown }> {
  return server.calls.filter((call) => call.method === "POST" && call.path.endsWith("/grants")).map(({ path, body }) => ({ path, body }));
}

beforeEach(() => {
  localStorage.clear();
  resetLinkStore("d1");
  h.ensureDevice.mockResolvedValue({ deviceId: "dev1" });
  // The real reconnect clears the block and starts over.
  h.reconnect.mockImplementation(() => getLinkStore("d1").setState({ blocked: null, revoked: false, lastError: null, status: "connecting" }));
});
afterEach(() => {
  restore?.();
  vi.clearAllMocks();
});

describe("lazy grant request", () => {
  it("requests the grant once for the opened desktop only and never reconnects after the 4003", async () => {
    install();
    setLink(RACE);
    mountSlot(<LinkStateScreens />);
    await screen.findByRole("alertdialog", { name: "Access to this desktop was revoked" });
    expect(grantPosts()).toEqual([{ path: GRANT_PATH, body: { deviceId: "dev1" } }]);
    expect(localStorage.getItem(MARKER)).toBe("1");
    expect(h.reconnect).not.toHaveBeenCalled();
    // Not even a look at the other desktop's grants.
    expect(server.calls.some((call) => call.path.includes("/d2"))).toBe(false);
  });

  it("shows a neutral requesting state, never the revoked screen, while the request is in flight", async () => {
    install();
    let release: () => void = () => undefined;
    const gate = new Promise<void>((resolve) => (release = resolve));
    const inner = server.transport;
    restore?.();
    restore = setApiTransport(async (url, init) => {
      if (url.endsWith("/grants") && init.method === "POST") await gate;
      return inner(url, init);
    });
    setLink(RACE);
    mountSlot(<LinkStateScreens />);
    // Not at once: the layer waits 250 ms so a fast answer never flashes it.
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(await screen.findByRole("alertdialog", { name: "Requesting access..." })).toBeInTheDocument();
    expect(screen.queryByText("Access to this desktop was revoked")).toBeNull();
    release();
    // The grant exists and the relay still said 4003: a real revoke, and no reconnect.
    await screen.findByRole("alertdialog", { name: "Access to this desktop was revoked" });
    expect(h.reconnect).not.toHaveBeenCalled();
  });

  it("shows the revoked screen on a 409 and never reconnects", async () => {
    install();
    server.conflicts.add(`POST ${GRANT_PATH}`);
    setLink(RACE);
    mountSlot(<LinkStateScreens />);
    const dialog = await screen.findByRole("alertdialog", { name: "Access to this desktop was revoked" });
    expect(dialog).toHaveTextContent(/forget this device/i);
    expect(dialog).toHaveTextContent(/on every desktop/);
    expect(dialog).toHaveTextContent(/compare the fingerprints on the confirmation screen with the ones in the dialog on your desktop/i);
    expect(dialog).toHaveTextContent("Monoptah");
    expect(dialog).not.toHaveTextContent(/My desktops/);
    expect(screen.getByRole("link", { name: "Go to desktops" })).toHaveAttribute("href", "/settings/desktops");
    expect(screen.queryByRole("button", { name: /try again|reconnect|retry/i })).toBeNull();
    expect(h.reconnect).not.toHaveBeenCalled();
    expect(grantPosts()).toHaveLength(1);
  });

  it("with a pending grant and a first-open 4003, shows the revoked screen and never reconnects", async () => {
    install({ grants: { d1: [grant({ deviceId: "dev1", state: "pending", permission: null, confirmedAt: null })] } });
    setLink(RACE);
    mountSlot(<LinkStateScreens />);
    await screen.findByRole("alertdialog", { name: "Access to this desktop was revoked" });
    setLink(RACE);
    await new Promise((resolve) => setTimeout(resolve, 30));
    expect(h.reconnect).not.toHaveBeenCalled();
    expect(grantPosts()).toHaveLength(1);
  });

  it("skips the POST on a later open when the marker is set", async () => {
    install();
    localStorage.setItem(MARKER, "1");
    setLink({ status: "connecting" });
    mountSlot(<LinkStateScreens />);
    await waitFor(() => expect(h.ensureDevice).toHaveBeenCalled());
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(grantPosts()).toHaveLength(0);
    expect(h.reconnect).not.toHaveBeenCalled();
  });

  it("re-checks once with a POST when the marker is set but the link reports revoked", async () => {
    install({ grants: { d1: [grant({ deviceId: "dev1", state: "confirmed" })] } });
    localStorage.setItem(MARKER, "1");
    setLink(RACE);
    mountSlot(<LinkStateScreens />);
    await screen.findByRole("alertdialog", { name: "Access to this desktop was revoked" });
    expect(grantPosts()).toHaveLength(1);
    expect(h.reconnect).not.toHaveBeenCalled();
  });

  it("shows the revoked screen when the re-check answers 409", async () => {
    install();
    server.conflicts.add(`POST ${GRANT_PATH}`);
    localStorage.setItem(MARKER, "1");
    setLink(RACE);
    mountSlot(<LinkStateScreens />);
    await screen.findByRole("alertdialog", { name: "Access to this desktop was revoked" });
    expect(h.reconnect).not.toHaveBeenCalled();
  });

  it("offers a retry when the request itself failed, and never reconnects after it worked", async () => {
    install();
    const inner = server.transport;
    let failing = true;
    restore?.();
    restore = setApiTransport(async (url, init) => {
      if (failing && url.endsWith("/grants") && init.method === "POST") throw new TypeError("offline");
      return inner(url, init);
    });
    setLink(RACE);
    mountSlot(<LinkStateScreens />);
    await screen.findByRole("alertdialog", { name: "Could not request access" });
    failing = false;
    await userEvent.click(screen.getByRole("button", { name: "Try again" }));
    await screen.findByRole("alertdialog", { name: "Access to this desktop was revoked" });
    expect(h.reconnect).not.toHaveBeenCalled();
  });
});

describe("revoked block after the first open", () => {
  it("never reconnects and never POSTs for a revoked block without a close code", async () => {
    install();
    localStorage.setItem(MARKER, "1");
    setLink({ status: "closed", blocked: "revoked", revoked: true });
    mountSlot(<LinkStateScreens />);
    await screen.findByRole("alertdialog", { name: "Access to this desktop was revoked" });
    await new Promise((resolve) => setTimeout(resolve, 30));
    expect(h.reconnect).not.toHaveBeenCalled();
    expect(grantPosts()).toHaveLength(0);
  });

  it("treats a revoke after the link was ready as final: no reconnect, no re-check POST", async () => {
    install();
    localStorage.setItem(MARKER, "1");
    setLink({ status: "ready", permission: "control" });
    mountSlot(<LinkStateScreens />);
    await waitFor(() => expect(h.ensureDevice).toHaveBeenCalled());
    setLink({ status: "closed", blocked: "revoked", revoked: true, lastError: { code: "close_4003", closeCode: 4003 } });
    await screen.findByRole("alertdialog", { name: "Access to this desktop was revoked" });
    await new Promise((resolve) => setTimeout(resolve, 30));
    expect(h.reconnect).not.toHaveBeenCalled();
    expect(grantPosts()).toHaveLength(0);
  });

  it("after a successful POST and a ready link, a later 4003 is final", async () => {
    install();
    setLink({ status: "connecting" });
    mountSlot(<LinkStateScreens />);
    await waitFor(() => expect(localStorage.getItem(MARKER)).toBe("1"));
    setLink({ status: "pending" });
    setLink({ status: "closed", blocked: "revoked", revoked: true, lastError: { code: "close_4003", closeCode: 4003 } });
    await screen.findByRole("alertdialog", { name: "Access to this desktop was revoked" });
    expect(h.reconnect).not.toHaveBeenCalled();
    expect(grantPosts()).toHaveLength(1);
  });

  it("never reconnects after a 4003 on first open, also under strict mode", async () => {
    install();
    setLink(RACE);
    mountSlot(<LinkStateScreens />, "/d/d1", { strict: true });
    await screen.findByRole("alertdialog", { name: "Access to this desktop was revoked" });
    await new Promise((resolve) => setTimeout(resolve, 30));
    expect(h.reconnect).not.toHaveBeenCalled();
    expect(getLinkStore("d1").getState()).toMatchObject({ blocked: "revoked", revoked: true });
  });

  it("shows nothing for a block kept from an earlier visit while the connection is idle", async () => {
    install();
    localStorage.setItem(MARKER, "1");
    setLink({ status: "idle", blocked: "key_changed" });
    mountSlot(<LinkStateScreens />);
    await waitFor(() => expect(h.ensureDevice).toHaveBeenCalled());
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });

  it.each([404, 403])("shows the revoked screen when the grant POST answers %i for the device", async (status) => {
    install();
    const inner = server.transport;
    restore?.();
    restore = setApiTransport(async (url, init) => {
      if (url.endsWith("/grants") && init.method === "POST") return new Response(JSON.stringify({ error: "x", code: status === 404 ? "not_found" : "forbidden" }), { status, headers: { "Content-Type": "application/json" } });
      return inner(url, init);
    });
    setLink(RACE);
    mountSlot(<LinkStateScreens />);
    await screen.findByRole("alertdialog", { name: "Access to this desktop was revoked" });
    expect(screen.queryByText("Could not request access")).toBeNull();
    expect(h.reconnect).not.toHaveBeenCalled();
  });

  it.each([404, 403])("shows no revoked screen when the grant POST answers %i and the link is not refused (a deleted or unowned desktop)", async (status) => {
    install();
    const inner = server.transport;
    restore?.();
    let posts = 0;
    restore = setApiTransport(async (url, init) => {
      if (url.endsWith("/grants") && init.method === "POST") {
        posts += 1;
        return new Response(JSON.stringify({ error: "x", code: status === 404 ? "not_found" : "forbidden" }), { status, headers: { "Content-Type": "application/json" } });
      }
      return inner(url, init);
    });
    setLink({ status: "connecting" });
    mountSlot(<LinkStateScreens />);
    await waitFor(() => expect(posts).toBe(1));
    await new Promise((resolve) => setTimeout(resolve, 300));
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(screen.queryByText("Access to this desktop was revoked")).toBeNull();
    expect(getLinkStore("d1").getState()).toMatchObject({ revoked: false });
    expect(h.reconnect).not.toHaveBeenCalled();
  });

  it("never renders the requesting layer when the request answers fast", async () => {
    install();
    let everShown = false;
    const observer = new MutationObserver(() => {
      if (document.querySelector('[role="alertdialog"]')?.textContent?.includes("Requesting access")) everShown = true;
    });
    observer.observe(document.body, { childList: true, subtree: true });
    setLink(RACE);
    mountSlot(<LinkStateScreens />);
    await waitFor(() => expect(localStorage.getItem(MARKER)).toBe("1"));
    await new Promise((resolve) => setTimeout(resolve, 300));
    observer.disconnect();
    expect(everShown).toBe(false);
  });
});

describe("blocking screens", () => {
  beforeEach(() => {
    install();
    localStorage.setItem(MARKER, "1");
  });

  it("shows the pending screen with both fingerprints and the waiting message, and leaves when ready", async () => {
    setLink({ status: "pending", desktopName: "Studio Mac", desktopFingerprint: FP_A, deviceFingerprint: FP_B });
    mountSlot(<LinkStateScreens />);
    const dialog = await screen.findByRole("alertdialog", { name: "Confirm this device on your desktop" });
    expect(dialog).toHaveTextContent("Desktop fingerprint");
    expect(dialog).toHaveTextContent(displayFingerprint(FP_A));
    expect(dialog).toHaveTextContent("This device's fingerprint");
    expect(dialog).toHaveTextContent(displayFingerprint(FP_B));
    expect(dialog).toHaveTextContent(/Waiting for confirmation on Studio Mac/);
    expect(dialog).toHaveTextContent(/updates by itself/);
    expect(screen.getByRole("link", { name: "Go to desktops" })).toHaveAttribute("href", "/settings/desktops");
    setLink({ status: "ready", permission: "control" });
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
  });

  it("renders the desktop name as text only", async () => {
    setLink({ status: "pending", desktopName: "<img src=x onerror=alert(1)>", desktopFingerprint: FP_A, deviceFingerprint: FP_B });
    mountSlot(<LinkStateScreens />);
    const dialog = await screen.findByRole("alertdialog");
    expect(dialog.querySelector("img")).toBeNull();
    expect(dialog).toHaveTextContent("<img src=x onerror=alert(1)>");
  });

  it("key_changed explains the verify flow, shows the pinned fingerprint and has no trust control", async () => {
    setLink({ status: "closed", blocked: "key_changed", desktopFingerprint: FP_A });
    mountSlot(<LinkStateScreens />);
    const dialog = await screen.findByRole("alertdialog", { name: "Desktop key changed" });
    expect(dialog).toHaveTextContent(displayFingerprint(FP_A));
    expect(dialog).toHaveTextContent(/Verify on another device/);
    expect(screen.getByRole("link", { name: "Manage this desktop" })).toHaveAttribute("href", "/settings/desktops/d1");
    expect(screen.queryByRole("button", { name: /trust/i })).toBeNull();
    expect(screen.queryByRole("link", { name: /trust/i })).toBeNull();
    expect(screen.queryByRole("button", { name: /try again/i })).toBeNull();
  });

  it("account_blocked and policy have no retry", async () => {
    setLink({ status: "closed", blocked: "account_blocked" });
    mountSlot(<LinkStateScreens />);
    await screen.findByRole("alertdialog", { name: "This account is blocked" });
    expect(screen.queryByRole("button")).toBeNull();
    setLink({ blocked: "policy" });
    await screen.findByRole("alertdialog", { name: "This desktop closed the connection" });
    expect(screen.queryByRole("button")).toBeNull();
    expect(screen.getByRole("link", { name: "Go to desktops" })).toHaveAttribute("href", "/settings/desktops");
  });

  it("desktop_refused offers Try again, which reconnects", async () => {
    setLink({ status: "closed", blocked: "desktop_refused" });
    mountSlot(<LinkStateScreens />);
    await screen.findByRole("alertdialog", { name: "This desktop did not accept this device" });
    expect(screen.getByRole("link", { name: "Go to desktops" })).toHaveAttribute("href", "/settings/desktops");
    await userEvent.click(screen.getByRole("button", { name: "Try again" }));
    expect(h.reconnect).toHaveBeenCalledTimes(1);
  });

  it.each(["login_required", "web_outdated", "desktop_outdated"] as const)("leaves %s to the layout", async (blocked) => {
    setLink({ status: "closed", blocked });
    mountSlot(<LinkStateScreens />);
    await waitFor(() => expect(h.ensureDevice).toHaveBeenCalled());
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });

  it.each(["revoked", "account_blocked", "key_changed", "desktop_refused", "policy"] as const)("has no trust-the-new-key control on the %s screen", async (blocked) => {
    server.conflicts.add(`POST ${GRANT_PATH}`);
    setLink({ status: "closed", blocked, revoked: blocked === "revoked" });
    mountSlot(<LinkStateScreens />);
    await screen.findByRole("alertdialog");
    expect(screen.queryByRole("button", { name: /trust/i })).toBeNull();
    expect(screen.queryByRole("link", { name: /trust/i })).toBeNull();
  });
});
