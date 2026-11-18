import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { setApiTransport } from "@/features/accounts/api";
import type * as KeysModule from "@/link/keys";

const webauthn = vi.hoisted(() => ({ createPasskey: vi.fn(), getPasskey: vi.fn() }));
vi.mock("@/features/accounts/webauthn", () => webauthn);
const keys = vi.hoisted(() => ({ getDeviceKeyInfo: vi.fn(), forgetDeviceKey: vi.fn() }));
vi.mock("@/link/keys", async (importOriginal) => ({ ...(await importOriginal<typeof KeysModule>()), ...keys }));

import { createFakeServer, desktop, grant, FP_A, type FakeServer } from "./testServer";
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

beforeEach(() => {
  webauthn.getPasskey.mockResolvedValue(ASSERTION);
  keys.getDeviceKeyInfo.mockResolvedValue(null);
});
afterEach(() => {
  restore?.();
  vi.clearAllMocks();
});

describe("/desktops/:id", () => {
  it("shows the header, status, version, fingerprint and the open link", async () => {
    install({ desktops: [desktop({ lastSeenAt: null })] });
    mountApp("/desktops/d1");
    expect(await screen.findByRole("heading", { level: 1, name: "Studio Mac" })).toBeInTheDocument();
    expect(screen.getByText("macOS, v1.4.0")).toBeInTheDocument();
    expect(screen.getByText("0f1e 2d3c 4b5a 6978 8796 a5b4 c3d2 e1f0")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Open desktop" })).toHaveAttribute("href", "/d/d1");
    expect(screen.getByRole("link", { name: "Open the install guide" })).toHaveAttribute("href", "/install");
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
  });

  it("renders a hostile device name as text only", async () => {
    install({ grants: { d1: [grant({ deviceName: "<b onclick=x>pwn</b>" })] } });
    mountApp("/desktops/d1");
    expect(await screen.findByText("<b onclick=x>pwn</b>")).toBeInTheDocument();
    expect(document.querySelector("b")).toBeNull();
  });
});
