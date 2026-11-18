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

describe("/desktops list", () => {
  it("shows online and offline desktops with last seen, version, pending badge and both links", async () => {
    install({
      desktops: [
        desktop({ id: "d1", name: "Studio Mac", online: true, pendingCount: 2 }),
        desktop({ id: "d2", name: "Linux box", color: null, online: false, os: "Linux", appVersion: "1.3.0", lastSeenAt: NOW - 3 * 3600 }),
      ],
    });
    mountApp("/desktops");
    await screen.findByRole("heading", { level: 1, name: "Desktops" });
    const studio = (await screen.findByRole("link", { name: "Open Studio Mac" })).closest("li") as HTMLElement;
    expect(within(studio).getByText("Online")).toBeInTheDocument();
    expect(within(studio).getByText("2 to confirm")).toBeInTheDocument();
    expect(within(studio).getByText(/macOS, v1\.4\.0/)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Open Studio Mac" })).toHaveAttribute("href", "/d/d1");
    expect(screen.getByRole("link", { name: "Manage Studio Mac" })).toHaveAttribute("href", "/desktops/d1");

    const linux = screen.getByRole("link", { name: "Open Linux box" }).closest("li") as HTMLElement;
    expect(within(linux).getByText("Offline")).toBeInTheDocument();
    expect(within(linux).getByText(/Last seen 3 hours ago/)).toBeInTheDocument();
    expect(within(linux).queryByText(/to confirm/)).toBeNull();
  });

  it("explains how to pair when there is no desktop", async () => {
    install();
    mountApp("/desktops");
    expect(await screen.findByText("No desktop is paired yet.")).toBeInTheDocument();
    expect(screen.getByText(/Settings > Web app > Pair/)).toBeInTheDocument();
    expect(screen.getByLabelText("Paste a pairing link")).toBeInTheDocument();
  });

  it("renders desktop names as text only", async () => {
    install({ desktops: [desktop({ name: "<img src=x onerror=alert(1)>‮evil" })] });
    mountApp("/desktops");
    const link = await screen.findByRole("link", { name: /^Open / });
    expect(link.textContent).toContain("<img src=x onerror=alert(1)>evil");
    expect(document.querySelector("img")).toBeNull();
  });
});

describe("paste a pairing link", () => {
  it("opens /pair/:code and keeps #fp for a same-origin link", async () => {
    install();
    const { router } = mountApp("/desktops");
    const field = await screen.findByLabelText("Paste a pairing link");
    await userEvent.type(field, `${window.location.origin}/pair/ABCD2345#fp=${FP_A}`);
    await userEvent.click(screen.getByRole("button", { name: "Continue" }));
    await waitFor(() => expect(router.state.location.pathname).toBe("/pair/ABCD2345"));
    expect(router.state.location.hash).toContain(`fp=${FP_A}`);
  });

  it("accepts a bare code", async () => {
    install();
    const { router } = mountApp("/desktops");
    await userEvent.type(await screen.findByLabelText("Paste a pairing link"), "abcd2345");
    await userEvent.click(screen.getByRole("button", { name: "Continue" }));
    await waitFor(() => expect(router.state.location.pathname).toBe("/pair/ABCD2345"));
  });

  it("rejects a link of another origin and stays on the page", async () => {
    install();
    const { router } = mountApp("/desktops");
    await userEvent.type(await screen.findByLabelText("Paste a pairing link"), `https://evil.example/pair/ABCD2345#fp=${FP_A}`);
    await userEvent.click(screen.getByRole("button", { name: "Continue" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(/not a pairing link or code of this app/);
    expect(router.state.location.pathname).toBe("/desktops");
  });
});

describe("This account's devices", () => {
  it("lists name, fingerprint in display form, last seen and marks this device", async () => {
    keys.getDeviceKeyInfo.mockResolvedValue({ keyId: "k", deviceId: "dev1", publicKey: new Uint8Array(32) });
    install({ devices: [device({ id: "dev2", name: "Firefox on Linux", fingerprint: FP_B, lastSeenAt: null }), device({ id: "dev1", name: "Chrome on macOS", lastSeenAt: NOW - 120 })] });
    mountApp("/desktops");
    const heading = await screen.findByRole("heading", { level: 2, name: "This account's devices" });
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
    mountApp("/desktops");
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
    mountApp("/desktops");
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
    mountApp("/desktops");
    expect(await screen.findByText("<script>alert(1)</script>")).toBeInTheDocument();
    expect(document.querySelector("script")).toBeNull();
  });
});
