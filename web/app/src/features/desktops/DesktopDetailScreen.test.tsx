import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { IDBFactory } from "fake-indexeddb";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { setApiTransport } from "@/features/accounts/api";
import { base64UrlEncode } from "@/link/base64url";
import { fingerprintLines } from "@/features/accounts/pair/fingerprint";
import { displayFingerprint, fingerprint } from "@/link/fingerprint";
import { checkPin } from "@/link/pins";
import type * as KeysModule from "@/link/keys";
import type * as PersistModule from "@/data/persist";
import type * as AvatarImageModule from "./avatarImage";

const webauthn = vi.hoisted(() => ({ createPasskey: vi.fn(), getPasskey: vi.fn() }));
vi.mock("@/features/accounts/webauthn", () => webauthn);
const keys = vi.hoisted(() => ({ getDeviceKeyInfo: vi.fn(), forgetDeviceKey: vi.fn() }));
vi.mock("@/link/keys", async (importOriginal) => ({ ...(await importOriginal<typeof KeysModule>()), ...keys }));

const persist = vi.hoisted(() => ({ clearPersistedCache: vi.fn() }));
vi.mock("@/data/persist", async (importOriginal) => ({ ...(await importOriginal<typeof PersistModule>()), ...persist }));

const avatarImage = vi.hoisted(() => ({ prepare: vi.fn() }));
vi.mock("./avatarImage", async (importOriginal) => ({ ...(await importOriginal<typeof AvatarImageModule>()), prepareAvatarImage: avatarImage.prepare }));

import { createFakeServer, desktop, grant, FP_A, FP_B, type FakeServer } from "./testServer";
import { mountApp } from "./testMount";

const ASSERTION = { status: "ok", response: { id: "cred", rawId: "cred", type: "public-key", response: {}, clientExtensionResults: {} } };
let server: FakeServer;
let restore: (() => void) | undefined;

function install(options: Parameters<typeof createFakeServer>[0] = {}) {
  server = createFakeServer({ desktops: [desktop()], grants: { d1: [grant()] }, ...options });
  restore = setApiTransport(server.transport);
}

function row(name: string): HTMLElement {
  return screen.getByText(name, { selector: "span" }).closest("li") as HTMLElement;
}

const KEY = new Uint8Array(32).fill(5);
const OTHER_KEY = new Uint8Array(32).fill(6);

beforeEach(() => {
  globalThis.indexedDB = new IDBFactory();
  persist.clearPersistedCache.mockResolvedValue(undefined);
  webauthn.getPasskey.mockResolvedValue(ASSERTION);
  keys.getDeviceKeyInfo.mockResolvedValue(null);
});
afterEach(() => {
  restore?.();
  vi.clearAllMocks();
});

describe("/desktops/:id", () => {
  it("shows the header, status, version and the open link", async () => {
    install({ desktops: [desktop({ lastSeenAt: null })] });
    mountApp("/desktops/d1");
    expect(await screen.findByRole("heading", { level: 1, name: "Studio Mac" })).toBeInTheDocument();
    expect(screen.getByText("macOS, v1.4.0")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Open desktop" })).toHaveAttribute("href", "/d/d1");
    expect(screen.getByRole("link", { name: "Open the install guide" })).toHaveAttribute("href", "/install");
  });

  it("never shows the server's fingerprint claim: it computes one from the key and says it is not verified yet", async () => {
    install({ publicKeys: { d1: base64UrlEncode(KEY) }, fingerprintClaims: { d1: FP_B } });
    mountApp("/desktops/d1");
    expect(await screen.findByText("Reported by the server, not yet verified on this device")).toBeInTheDocument();
    expect(await screen.findByText(displayFingerprint(fingerprint(KEY)))).toBeInTheDocument();
    expect(screen.queryByText(displayFingerprint(FP_B))).toBeNull();
    expect(screen.queryByText("Key changed since this device pinned it")).toBeNull();
  });

  it("shows the pinned fingerprint of this device when the directory key matches it", async () => {
    await checkPin("d1", KEY);
    install({ publicKeys: { d1: base64UrlEncode(KEY) }, fingerprintClaims: { d1: FP_B } });
    mountApp("/desktops/d1");
    expect(await screen.findByText("Pinned on this device")).toBeInTheDocument();
    expect(screen.getByText(displayFingerprint(fingerprint(KEY)))).toBeInTheDocument();
    expect(screen.queryByText("Key changed since this device pinned it")).toBeNull();
    expect(screen.queryByText(displayFingerprint(FP_B))).toBeNull();
  });

  it("warns when the directory key differs from the pin, with the verify guidance", async () => {
    await checkPin("d1", OTHER_KEY);
    install({ publicKeys: { d1: base64UrlEncode(KEY) }, fingerprintClaims: { d1: fingerprint(OTHER_KEY) } });
    mountApp("/desktops/d1");
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Key changed since this device pinned it");
    expect(alert).toHaveTextContent(/Verify on another device/);
    expect(alert).toHaveTextContent(displayFingerprint(fingerprint(KEY)));
    // The pin is shown as the trusted one, and the lying claim is never presented as a match.
    expect(screen.getByText("Pinned on this device")).toBeInTheDocument();
    // Both fingerprints are shown as two comparable lines each.
    for (const line of fingerprintLines(fingerprint(OTHER_KEY))) expect(screen.getAllByText(line)).toHaveLength(1);
    for (const line of fingerprintLines(fingerprint(KEY))) expect(within(alert).getByText(line)).toBeInTheDocument();
  });

  it("says so when the key cannot be read", async () => {
    install();
    mountApp("/desktops/d1");
    expect(await screen.findByText("The key of this desktop could not be read.")).toBeInTheDocument();
  });

  it("shows loading then a not-found state for an unknown desktop", async () => {
    install();
    mountApp("/desktops/nope");
    expect(await screen.findByRole("heading", { level: 1, name: "Desktop not found" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Back to desktops" })).toHaveAttribute("href", "/desktops");
  });

  it("renames with the trimmed name and blocks an empty one", async () => {
    install();
    mountApp("/desktops/d1");
    const field = await screen.findByLabelText("Desktop name");
    await userEvent.clear(field);
    expect(await screen.findByText("Enter a name.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Save name" })).toBeDisabled();
    await userEvent.type(field, "  Office  ");
    await userEvent.click(screen.getByRole("button", { name: "Save name" }));
    await waitFor(() => expect(server.calls.find((call) => call.method === "PATCH")).toMatchObject({ body: { name: "Office" } }));
    expect(await screen.findByText("Name saved.")).toBeInTheDocument();
  });

  it("sets the color from the palette", async () => {
    install();
    mountApp("/desktops/d1");
    const group = await screen.findByRole("radiogroup", { name: "Desktop color" });
    expect(within(group).getByRole("radio", { name: "Blue" })).toBeChecked();
    await userEvent.click(within(group).getByRole("radio", { name: "Green" }));
    await waitFor(() => expect(server.calls.some((call) => call.method === "PATCH" && JSON.stringify(call.body) === JSON.stringify({ color: "#39c363" }))).toBe(true));
    await waitFor(() => expect(within(group).getByRole("radio", { name: "Green" })).toBeChecked());
  });

  it("lists devices with state, permission and the requested row, and shows only the valid actions", async () => {
    install({
      grants: {
        d1: [
          grant({ deviceId: "a", deviceName: "Control device", permission: "control" }),
          grant({ deviceId: "b", deviceName: "Read device", permission: "read" }),
          grant({ deviceId: "c", deviceName: "Raising device", permission: "read", requestedPermission: "control" }),
          grant({ deviceId: "d", deviceName: "New device", state: "pending", permission: null, confirmedAt: null }),
          grant({ deviceId: "e", deviceName: "Old device", state: "revoked", permission: null, confirmedAt: null }),
        ],
      },
    });
    mountApp("/desktops/d1");
    await screen.findByText("Control device");
    const control = row("Control device");
    expect(within(control).getByText("Confirmed")).toBeInTheDocument();
    expect(within(control).getByText("Permission: Control, confirmed", { exact: false })).toBeInTheDocument();
    expect(within(control).getByRole("button", { name: /Lower .* to read only/ })).toBeInTheDocument();
    expect(within(control).queryByRole("button", { name: /Request control/ })).toBeNull();

    const read = row("Read device");
    expect(within(read).getByText("Permission: Read only", { exact: false })).toBeInTheDocument();
    expect(within(read).getByRole("button", { name: /Request control/ })).toBeInTheDocument();
    expect(within(read).queryByRole("button", { name: /Lower/ })).toBeNull();

    const raising = row("Raising device");
    expect(within(raising).getByText("Requested: Control, confirm on the desktop.")).toBeInTheDocument();
    expect(within(raising).queryByRole("button", { name: /Request control/ })).toBeNull();

    const pending = row("New device");
    expect(within(pending).getByText("Pending confirmation")).toBeInTheDocument();
    expect(within(pending).getByText("Waiting for confirmation on the desktop")).toBeInTheDocument();
    expect(within(pending).queryByRole("button", { name: /Lower|Request control/ })).toBeNull();

    const revoked = row("Old device");
    expect(within(revoked).getByText("Revoked")).toBeInTheDocument();
    expect(within(revoked).queryByRole("button")).toBeNull();
  });

  it("lowering sends requestedPermission read, raising sends control", async () => {
    install({ grants: { d1: [grant({ deviceId: "a", deviceName: "Control device", permission: "control" }), grant({ deviceId: "b", deviceName: "Read device", permission: "read" })] } });
    mountApp("/desktops/d1");
    await userEvent.click(await screen.findByRole("button", { name: "Lower Control device to read only" }));
    await waitFor(() => expect(server.calls.some((call) => call.method === "PUT" && call.path.endsWith("/grants/a") && JSON.stringify(call.body) === JSON.stringify({ requestedPermission: "read" }))).toBe(true));
    await userEvent.click(screen.getByRole("button", { name: "Request control for Read device" }));
    await waitFor(() => expect(server.calls.some((call) => call.method === "PUT" && call.path.endsWith("/grants/b") && JSON.stringify(call.body) === JSON.stringify({ requestedPermission: "control" }))).toBe(true));
    expect(await screen.findByText("Requested: Control, confirm on the desktop.")).toBeInTheDocument();
  });

  it("marks this device", async () => {
    keys.getDeviceKeyInfo.mockResolvedValue({ keyId: "k", deviceId: "dev1", publicKey: new Uint8Array(32) });
    install({ grants: { d1: [grant({ deviceId: "dev1", fingerprint: FP_A })] } });
    mountApp("/desktops/d1");
    await waitFor(() => expect(within(row("Chrome on macOS")).getByText("This device")).toBeInTheDocument());
  });

  it("revokes access only after a confirm dialog that names the device, and re-authenticates when asked", async () => {
    install();
    mountApp("/desktops/d1");
    await userEvent.click(await screen.findByRole("button", { name: "Revoke access for Chrome on macOS" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByRole("heading", { name: "Revoke access for Chrome on macOS?" })).toBeInTheDocument();
    expect(dialog).toHaveTextContent(`Device fingerprint: ${displayFingerprint(FP_A)}`);
    expect(server.calls.some((call) => call.method === "PUT")).toBe(false);
    await userEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(server.calls.some((call) => call.method === "PUT")).toBe(false);

    await userEvent.click(screen.getByRole("button", { name: "Revoke access for Chrome on macOS" }));
    await userEvent.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Revoke access" }));
    await waitFor(() => expect(server.grants.d1?.[0]?.state).toBe("revoked"));
    expect(server.reauths).toBe(1);
    expect(webauthn.getPasskey).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(within(row("Chrome on macOS")).getByText("Revoked")).toBeInTheDocument());
  });

  it("keeps the grant and shows the soft notice when the passkey prompt is closed", async () => {
    webauthn.getPasskey.mockResolvedValue({ status: "cancelled" });
    install();
    mountApp("/desktops/d1");
    await userEvent.click(await screen.findByRole("button", { name: "Revoke access for Chrome on macOS" }));
    await userEvent.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Revoke access" }));
    expect(await screen.findByText(/passkey prompt was closed/)).toBeInTheDocument();
    expect(server.grants.d1?.[0]?.state).toBe("confirmed");
  });

  it("revokes the desktop behind a confirm dialog naming it, then returns to /desktops", async () => {
    install();
    const { router } = mountApp("/desktops/d1");
    await userEvent.click(await screen.findByRole("button", { name: "Revoke desktop" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByRole("heading", { name: "Revoke Studio Mac?" })).toBeInTheDocument();
    expect(server.calls.some((call) => call.method === "DELETE")).toBe(false);
    await userEvent.click(within(dialog).getByRole("button", { name: "Revoke desktop" }));
    await waitFor(() => expect(router.state.location.pathname).toBe("/desktops"));
    expect(server.reauths).toBe(1);
    expect(server.desktops).toHaveLength(0);
    expect(persist.clearPersistedCache).toHaveBeenCalledWith("d1");
    // No flash of "Desktop not found": the deleted desktop is never fetched again, and the page never showed it.
    expect(screen.queryByRole("heading", { name: "Desktop not found" })).toBeNull();
    const deleteAt = server.calls.findIndex((call) => call.method === "DELETE");
    expect(server.calls.slice(deleteAt + 1).some((call) => call.method === "GET" && call.path === "/api/desktops/d1")).toBe(false);
  });

  it("follows a name that changed elsewhere while the field is untouched, and keeps edits otherwise", async () => {
    install();
    const { queryClient } = mountApp("/desktops/d1");
    const field = await screen.findByLabelText("Desktop name");
    expect(field).toHaveValue("Studio Mac");
    server.desktops[0]!.name = "Renamed elsewhere";
    await queryClient.invalidateQueries({ queryKey: ["desktops", "detail", "d1"] });
    await waitFor(() => expect(screen.getByLabelText("Desktop name")).toHaveValue("Renamed elsewhere"));

    await userEvent.type(screen.getByLabelText("Desktop name"), " v2");
    server.desktops[0]!.name = "Renamed again";
    await queryClient.invalidateQueries({ queryKey: ["desktops", "detail", "d1"] });
    await screen.findByRole("heading", { level: 1, name: "Renamed again" });
    expect(screen.getByLabelText("Desktop name")).toHaveValue("Renamed elsewhere v2");
  });

  it("keeps a device's name and its badges together in one row group", async () => {
    keys.getDeviceKeyInfo.mockResolvedValue({ keyId: "k", deviceId: "dev1", publicKey: new Uint8Array(32) });
    install();
    mountApp("/desktops/d1");
    const badge = await screen.findByText("Confirmed");
    await waitFor(() => expect(within(row("Chrome on macOS")).getByText("This device").parentElement).toBe(badge.parentElement));
  });

  it("renders a hostile device name as text only", async () => {
    install({ grants: { d1: [grant({ deviceName: "<b onclick=x>pwn</b>" })] } });
    mountApp("/desktops/d1");
    expect(await screen.findByText("<b onclick=x>pwn</b>")).toBeInTheDocument();
    expect(document.querySelector("b")).toBeNull();
  });
});

describe("/desktops/:id avatar", () => {
  const patches = () => server.calls.filter((call) => call.method === "PATCH");

  it("saves a chosen emoji on the account and shows it in the preview", async () => {
    install();
    const user = userEvent.setup();
    mountApp("/desktops/d1");
    const group = await screen.findByRole("radiogroup", { name: "Avatar emoji" });
    expect(screen.getByTestId("avatar-preview")).toHaveTextContent("SM");
    await user.click(within(group).getByRole("radio", { name: "Emoji \u{1F680}" }));
    await waitFor(() => expect(patches().at(-1)?.body).toEqual({ avatar: { kind: "emoji", emoji: "\u{1F680}" } }));
    await waitFor(() => expect(within(screen.getByTestId("avatar-preview")).getByTestId("avatar-emoji")).toHaveTextContent("\u{1F680}"));
    expect(within(group).getByRole("radio", { name: "Emoji \u{1F680}" })).toBeChecked();
  });

  it("uploads a prepared image (only the re-encoded bytes are sent) and can go back to initials", async () => {
    install();
    avatarImage.prepare.mockResolvedValue({ mime: "image/webp", data: "UklGRgAAAABXRUJQ" });
    const user = userEvent.setup();
    mountApp("/desktops/d1");
    const file = new File(["x"], "me.png", { type: "image/png" });
    await user.upload(await screen.findByLabelText("Avatar image file"), file);
    await waitFor(() => expect(patches().at(-1)?.body).toEqual({ avatar: { kind: "image", mime: "image/webp", data: "UklGRgAAAABXRUJQ" } }));
    expect(avatarImage.prepare).toHaveBeenCalledWith(file);
    expect(await screen.findByTestId("avatar-image")).toHaveAttribute("src", expect.stringMatching(/^\/api\/desktops\/d1\/avatar\?v=[0-9a-f]{16}$/));
    await user.click(screen.getByRole("button", { name: "Use initials" }));
    await waitFor(() => expect(patches().at(-1)?.body).toEqual({ avatar: null }));
    await waitFor(() => expect(screen.getByTestId("avatar-preview")).toHaveTextContent("SM"));
  });

  it("shows why a refused picture was refused and sends nothing", async () => {
    install();
    const { AvatarImageError } = await import("./avatarImage");
    avatarImage.prepare.mockRejectedValue(new AvatarImageError("unsupported"));
    const user = userEvent.setup({ applyAccept: false });
    mountApp("/desktops/d1");
    await user.upload(await screen.findByLabelText("Avatar image file"), new File(["<svg/>"], "logo.svg", { type: "image/svg+xml" }));
    expect(await screen.findByText("Use a PNG, JPEG or WebP image.")).toBeInTheDocument();
    expect(patches()).toHaveLength(0);
  });

  it("offers Use initials only when there is an avatar to clear", async () => {
    install({ desktops: [desktop({ avatar: null })] });
    mountApp("/desktops/d1");
    expect(await screen.findByRole("button", { name: "Use initials" })).toBeDisabled();
  });
});
