import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { setApiTransport } from "@/features/accounts/api";
import type * as KeysModule from "@/link/keys";
import type * as PinsModule from "@/link/pins";
import type * as PersistModule from "@/data/persist";
import { displayFingerprint } from "@/link/fingerprint";

const webauthn = vi.hoisted(() => ({ createPasskey: vi.fn(), getPasskey: vi.fn() }));
vi.mock("@/features/accounts/webauthn", () => webauthn);
const keys = vi.hoisted(() => ({ getDeviceKeyInfo: vi.fn(), forgetDeviceKey: vi.fn() }));
const pins = vi.hoisted(() => ({ forgetPins: vi.fn() }));
vi.mock("@/link/keys", async (importOriginal) => ({ ...(await importOriginal<typeof KeysModule>()), ...keys }));
vi.mock("@/link/pins", async (importOriginal) => ({ ...(await importOriginal<typeof PinsModule>()), ...pins }));

const persist = vi.hoisted(() => ({ clearPersistedCache: vi.fn() }));
vi.mock("@/data/persist", async (importOriginal) => ({ ...(await importOriginal<typeof PersistModule>()), ...persist }));

import { createFakeServer, desktop, device, FP_A, FP_B, type FakeServer } from "./testServer";
import { mountApp } from "./testMount";

let server: FakeServer;
let restore: (() => void) | undefined;
const NOW = Math.floor(Date.now() / 1000);

function install(options: Parameters<typeof createFakeServer>[0] = {}) {
  server = createFakeServer(options);
  restore = setApiTransport(server.transport);
}

beforeEach(() => {
  keys.getDeviceKeyInfo.mockResolvedValue(null);
  keys.forgetDeviceKey.mockResolvedValue(undefined);
  pins.forgetPins.mockResolvedValue(undefined);
  persist.clearPersistedCache.mockResolvedValue(undefined);
});
afterEach(() => {
  restore?.();
  vi.clearAllMocks();
});

describe("/settings/desktops list", () => {
  it("shows online and offline desktops with last seen, version, pending badge and both links", async () => {
    install({
      desktops: [
        desktop({ id: "d1", name: "Studio Mac", online: true, pendingCount: 2 }),
        desktop({ id: "d2", name: "Linux box", color: null, online: false, os: "Linux", appVersion: "1.3.0", lastSeenAt: NOW - 3 * 3600 }),
      ],
    });
    mountApp("/settings/desktops");
    await screen.findByRole("heading", { level: 1, name: "All desktops" });
    const studio = (await screen.findByRole("link", { name: /^Open Studio Mac/ })).closest("li") as HTMLElement;
    expect(within(studio).getByText("Online")).toBeInTheDocument();
    expect(within(studio).getByText("2 to confirm")).toBeInTheDocument();
    expect(within(studio).getByText(/macOS, v1\.4\.0/)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /^Open Studio Mac/ })).toHaveAttribute("href", "/d/d1");
    expect(screen.getByRole("link", { name: /^Open Studio Mac/ })).toHaveAttribute("data-row-highlight");
    expect(screen.getByRole("link", { name: "Manage Studio Mac" })).not.toHaveAttribute("data-row-highlight");
    expect(screen.getByRole("button", { name: "Rename Studio Mac" })).not.toHaveAttribute("data-row-highlight");
    expect(document.getElementById("your-desktops")).not.toBeNull();
    expect(document.getElementById("pair-desktop")).not.toBeNull();
    expect(document.getElementById("browser-access")).not.toBeNull();
    expect(screen.getByRole("heading", { level: 2, name: "Your desktops" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { level: 2, name: "Pair a desktop" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Manage Studio Mac" })).toHaveAttribute("href", "/settings/desktops/d1");

    const linux = screen.getByRole("link", { name: /^Open Linux box/ }).closest("li") as HTMLElement;
    expect(within(linux).getByText("Offline")).toBeInTheDocument();
    expect(within(linux).getByText(/Last seen 3 hours ago/)).toBeInTheDocument();
    expect(within(linux).queryByText(/to confirm/)).toBeNull();
  });

  it("renames a desktop from its row in a dialog", async () => {
    install({ desktops: [desktop({ id: "d1", name: "Studio Mac" })] });
    mountApp("/settings/desktops");
    await userEvent.click(await screen.findByRole("button", { name: "Rename Studio Mac" }));
    const dialog = await screen.findByRole("dialog");
    const field = within(dialog).getByLabelText("Desktop name");
    expect(field).toHaveValue("Studio Mac");
    await userEvent.clear(field);
    await userEvent.type(field, "  Office  ");
    await userEvent.click(within(dialog).getByRole("button", { name: "Save name" }));
    await waitFor(() => expect(server.calls.find((call) => call.method === "PATCH")).toMatchObject({ body: { name: "Office" } }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(await screen.findByRole("link", { name: /^Open Office/ })).toBeInTheDocument();
  });

  it("explains how to pair when there is no desktop", async () => {
    install();
    mountApp("/settings/desktops");
    expect(await screen.findByRole("heading", { name: "Pair your first desktop" })).toBeInTheDocument();
    expect(screen.getByText(/Settings > Web app > Pair/)).toBeInTheDocument();
    expect(screen.getByLabelText("Paste a pairing link")).toBeInTheDocument();
    expect(screen.getByText("Paste the link or the 8 character code shown on the desktop.")).toBeInTheDocument();
    expect(screen.queryByText(/^Optional/)).toBeNull();
  });

  it("renders desktop names as text only", async () => {
    install({ desktops: [desktop({ name: "<img src=x onerror=alert(1)>‮evil" })] });
    mountApp("/settings/desktops");
    const link = await screen.findByRole("link", { name: /^Open / });
    expect(link.textContent).toContain("<img src=x onerror=alert(1)>evil");
    expect(document.querySelector("img")).toBeNull();
  });
});

describe("paste a pairing link", () => {
  it("opens /pair/:code and keeps #fp for a same-origin link", async () => {
    install();
    const { router } = mountApp("/settings/desktops");
    const field = await screen.findByLabelText("Paste a pairing link");
    await userEvent.type(field, `${window.location.origin}/pair/ABCD2345#fp=${FP_A}`);
    await userEvent.click(screen.getByRole("button", { name: "Continue" }));
    await waitFor(() => expect(router.state.location.pathname).toBe("/pair/ABCD2345"));
    expect(router.state.location.hash).toContain(`fp=${FP_A}`);
  });

  it("accepts a bare code", async () => {
    install();
    const { router } = mountApp("/settings/desktops");
    await userEvent.type(await screen.findByLabelText("Paste a pairing link"), "abcd2345");
    await userEvent.click(screen.getByRole("button", { name: "Continue" }));
    await waitFor(() => expect(router.state.location.pathname).toBe("/pair/ABCD2345"));
  });

  it("rejects a link of another origin and stays on the page", async () => {
    install();
    const { router } = mountApp("/settings/desktops");
    await userEvent.type(await screen.findByLabelText("Paste a pairing link"), `https://evil.example/pair/ABCD2345#fp=${FP_A}`);
    await userEvent.click(screen.getByRole("button", { name: "Continue" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(/not a pairing link or code of this app/);
    expect(router.state.location.pathname).toBe("/settings/desktops");
  });
});

describe("Browsers with desktop access", () => {
  it("lists name, fingerprint in display form, last seen and marks this device", async () => {
    keys.getDeviceKeyInfo.mockResolvedValue({ keyId: "k", deviceId: "dev1", publicKey: new Uint8Array(32) });
    install({ devices: [device({ id: "dev2", name: "Firefox on Linux", fingerprint: FP_B, lastSeenAt: null }), device({ id: "dev1", name: "Chrome on macOS", lastSeenAt: NOW - 120 })] });
    mountApp("/settings/desktops");
    const heading = await screen.findByRole("heading", { level: 2, name: "Browsers with desktop access" });
    const section = heading.closest("section") as HTMLElement;
    const items = await within(section).findAllByRole("listitem");
    expect(within(items[0]!).getByText("Chrome on macOS")).toBeInTheDocument();
    await waitFor(() => expect(within(items[0]!).getByText("This device")).toBeInTheDocument());
    expect(within(items[0]!).getByText("a1b2 c3d4 e5f6 0718 293a 4b5c 6d7e 8f90")).toBeInTheDocument();
    expect(within(items[0]!).getByText(/Last seen 2 minutes ago/)).toBeInTheDocument();
    expect(within(items[1]!).queryByText("This device")).toBeNull();
    expect(within(items[1]!).getByText("Never used")).toBeInTheDocument();
  });

  it("forgets another device only after the confirm dialog, without touching the local key", async () => {
    keys.getDeviceKeyInfo.mockResolvedValue({ keyId: "k", deviceId: "me", publicKey: new Uint8Array(32) });
    install({ devices: [device({ id: "dev2", name: "Firefox on Linux" })] });
    mountApp("/settings/desktops");
    await userEvent.click(await screen.findByRole("button", { name: "Forget device Firefox on Linux" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText(/every desktop/)).toBeInTheDocument();
    expect(dialog).toHaveTextContent(`Device fingerprint: ${displayFingerprint(FP_A)}`);
    expect(server.calls.some((call) => call.method === "DELETE")).toBe(false);
    await userEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(server.calls.some((call) => call.method === "DELETE")).toBe(false);

    await userEvent.click(screen.getByRole("button", { name: "Forget device Firefox on Linux" }));
    await userEvent.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Forget device" }));
    await waitFor(() => expect(server.calls.find((call) => call.method === "DELETE")).toMatchObject({ path: "/api/devices/dev2" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(keys.forgetDeviceKey).not.toHaveBeenCalled();
  });

  it("forgetting this device also clears the local key and pins", async () => {
    keys.getDeviceKeyInfo.mockResolvedValue({ keyId: "k", deviceId: "dev1", publicKey: new Uint8Array(32) });
    install({ devices: [device({ id: "dev1", name: "Chrome on macOS" })] });
    mountApp("/settings/desktops");
    await waitFor(() => expect(screen.getByText("This device")).toBeInTheDocument());
    await userEvent.click(screen.getByRole("button", { name: "Forget device Chrome on macOS" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText(/browser you are using/)).toBeInTheDocument();
    expect(dialog).toHaveTextContent(/pinned desktop keys/);
    await userEvent.click(within(dialog).getByRole("button", { name: "Forget device" }));
    await waitFor(() => expect(keys.forgetDeviceKey).toHaveBeenCalledTimes(1));
    expect(pins.forgetPins).toHaveBeenCalledTimes(1);
    expect(persist.clearPersistedCache).toHaveBeenCalledWith();
  });

  it("renders device names as text only", async () => {
    install({ devices: [device({ name: "<script>alert(1)</script>" })] });
    mountApp("/settings/desktops");
    expect(await screen.findByText("<script>alert(1)</script>")).toBeInTheDocument();
    expect(document.querySelector("script")).toBeNull();
  });
});
