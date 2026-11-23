import { QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type * as KeysModule from "@/link/keys";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { setApiTransport, type ApiTransport } from "@/features/accounts/api";
import { createMockServer } from "@/features/accounts/mock/transport";
import { createMockState } from "@/features/accounts/mock/state";
import { clearLinkStores, getLinkStore } from "@/link/store";
import { createQueryClient } from "@/lib/queryClient";
import { REFUSED_COPY } from "./ProjectsPicker";
import { createAppRouter } from "@/router";
import { clearSlots, registerSlot } from "@/ui/slots";
import { DesktopLinkError, forgetDesktopLocally, onForgetDesktop, provideDesktopLink, resetDesktopLinks } from "./desktopLink";
import { UPDATE_POLL_MS } from "./DesktopUpdateSection";
import { markGrantRequested, wasGrantRequested } from "@/features/desktops/storage";
import { FirstRunPicker, pickerSeenKey } from "./FirstRunPicker";
import { createMockDesktopLink, type MockDesktopLink } from "./mockDesktopLink";

// The widening flow asks for a passkey (re-auth) and for this browser's device id; neither exists in jsdom.
const webauthn = vi.hoisted(() => ({ createPasskey: vi.fn(), getPasskey: vi.fn() }));
vi.mock("@/features/accounts/webauthn", () => webauthn);
const keys = vi.hoisted(() => ({ getDeviceKeyInfo: vi.fn() }));
vi.mock("@/link/keys", async (importOriginal) => ({ ...(await importOriginal<typeof KeysModule>()), ...keys }));
const ASSERTION = { status: "ok", response: { id: "cred", rawId: "cred", type: "public-key", response: {}, clientExtensionResults: {} } };

const DESKTOP = "desktop-dev-1";
const SETTINGS = `/settings/desktops/${DESKTOP}`;
const THIS_DEVICE = "dev-this";

let restore: (() => void) | undefined;
let requests: Array<{ method: string; url: string; body: unknown }> = [];
let links: MockDesktopLink[] = [];

let mockState: ReturnType<typeof createMockState>;

/** Tickets the fake selection-ticket route handed out, and the answer it gives next (default: a ticket once fresh). */
let tickets: string[] = [];
let ticketAnswer: { status: number; code: string } | null = null;

function installApi() {
  mockState = createMockState({ signedIn: true });
  const server = createMockServer({ state: mockState });
  let fresh = false;
  tickets = [];
  ticketAnswer = null;
  const reply = (status: number, body: unknown) => new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
  const transport: ApiTransport = async (url, init) => {
    requests.push({ method: init.method ?? "GET", url, body: typeof init.body === "string" ? (JSON.parse(init.body) as unknown) : undefined });
    if (/^\/api\/desktops\/[^/]+\/grants\/[^/]+\/selection-ticket$/.test(url)) {
      if (ticketAnswer) return reply(ticketAnswer.status, { error: ticketAnswer.code, code: ticketAnswer.code });
      // Like the real route: a fresh session is needed (the app re-authenticates once and retries).
      if (!fresh) return reply(403, { error: "fresh_required", code: "fresh_required" });
      const ticket = `ticket-${tickets.length + 1}`;
      tickets.push(ticket);
      return reply(200, { ticket, expiresAt: 1_900_000_000 });
    }
    const response = await server.transport(url, init);
    if (url === "/api/auth/reauth/verify" && response.ok) fresh = true;
    return response;
  };
  restore = setApiTransport(transport);
}

/** Provides a mock desktop link for every desktop id and keeps the links for inspection. */
function provideMockLink(options: { online?: boolean; permission?: "read" | "control" } = {}): MockDesktopLink {
  const link = createMockDesktopLink(options);
  links.push(link);
  // The page attaches to the link only for a desktop this browser asked access to before (see DesktopSettingsPage).
  markGrantRequested(DESKTOP, THIS_DEVICE);
  provideDesktopLink(() => link);
  return link;
}

/** Waits until the first handshake ended: connected, or offline for a desktop that is not attached. */
async function connected(link: MockDesktopLink) {
  await waitFor(() => expect(["connected", "offline"]).toContain(link.status));
}

function mount(url: string) {
  const queryClient = createQueryClient();
  const router = createAppRouter({ history: createMemoryHistory({ initialEntries: [url] }), queryClient });
  render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return { queryClient, router };
}

function selectionOf(link: MockDesktopLink) {
  return [...link.desktop.devices.values()][0]?.selection;
}

function setCalls(link: MockDesktopLink) {
  return link.calls.filter((call) => call.method === "device.selection" && call.params && Object.keys(call.params).length > 0);
}

async function openPicker(user: ReturnType<typeof userEvent.setup>) {
  await user.click(await screen.findByRole("button", { name: /Choose projects/ }));
  return screen.findByRole("dialog", { name: "Projects on this device" });
}

beforeEach(() => {
  requests = [];
  window.localStorage.clear();
  webauthn.getPasskey.mockResolvedValue(ASSERTION);
  keys.getDeviceKeyInfo.mockClear();
  keys.getDeviceKeyInfo.mockResolvedValue({ deviceId: THIS_DEVICE, publicKey: new Uint8Array(32) });
  installApi();
  getLinkStore(DESKTOP).setState({ permission: "control", desktopVersion: "1.4.0" });
});
afterEach(() => {
  clearLinkStores();
  restore?.();
  for (const link of links) link.close();
  links = [];
  resetDesktopLinks();
  clearSlots();
  window.history.replaceState(null, "", "/");
});

describe("/settings/desktops/:id", () => {
  it("renders one h1, the groups and the desktop-settings-sections slot with the desktop id", async () => {
    // The router imports features/sessions/register, which registers the default-model row (U-07): replace it.
    clearSlots();
    registerSlot("desktop-settings-sections", ({ desktopId }) => <p>Slot for {desktopId}</p>);
    const link = provideMockLink();
    mount(SETTINGS);
    await screen.findByRole("heading", { level: 1, name: "Fake Studio Mac" });
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    for (const title of ["General", "Notifications", "Projects visible in this browser", "Desktop app update", "Access", "Troubleshoot", "Remove"]) {
      expect(screen.getByRole("heading", { level: 2, name: title })).toBeInTheDocument();
    }
    for (const title of ["Status", "About", "Name", "Color", "Avatar"]) {
      expect(screen.getByRole("heading", { level: 3, name: title })).toBeInTheDocument();
    }
    // The settings nav lists the desktop too: look at the status card of the page.
    expect(within(document.getElementById("desktop-status") as HTMLElement).getByText("Fake Studio Mac", { selector: "span" })).toBeInTheDocument();
    expect(await screen.findByText(`Slot for ${DESKTOP}`)).toBeInTheDocument();
    await connected(link);
    expect(await screen.findByText("Connected")).toBeInTheDocument();
    expect(await screen.findByText("2 projects, 1 Mono")).toBeInTheDocument();
  });

  it("does not connect a desktop this browser never asked access to, and shows a Models placeholder", async () => {
    const link = createMockDesktopLink();
    links.push(link);
    const factory = vi.fn(() => link);
    provideDesktopLink(factory);
    const subscribe = vi.spyOn(link, "subscribe");
    clearSlots();
    registerSlot("desktop-settings-sections", () => <p>Models slot</p>);
    mount(SETTINGS);
    await screen.findByRole("heading", { level: 1, name: "Fake Studio Mac" });
    // The device is known by now; with no grant marker the page still stays off the link.
    await waitFor(() => expect(keys.getDeviceKeyInfo).toHaveBeenCalled());
    expect(wasGrantRequested(DESKTOP, THIS_DEVICE)).toBe(false);
    expect(factory).not.toHaveBeenCalled();
    expect(subscribe).not.toHaveBeenCalled();
    expect(screen.queryByText("Models slot")).toBeNull();
    const models = document.getElementById("desktop-models") as HTMLElement;
    expect(within(models).getByRole("heading", { name: "Models" })).toBeInTheDocument();
    expect(within(models).getByText("Needs a connection to the desktop")).toBeInTheDocument();
    expect(screen.getAllByText("Not connected").length).toBeGreaterThan(0);
  });

  it("shows neither the not-connected notice nor the placeholders while this browser's device is still loading", async () => {
    let resolveDevice: (value: { deviceId: string; publicKey: Uint8Array }) => void = () => undefined;
    keys.getDeviceKeyInfo.mockReturnValue(new Promise((resolve) => (resolveDevice = resolve)));
    const link = createMockDesktopLink();
    links.push(link);
    provideDesktopLink(() => link);
    mount(SETTINGS);
    await screen.findByRole("heading", { level: 1, name: "Fake Studio Mac" });
    await waitFor(() => expect(keys.getDeviceKeyInfo).toHaveBeenCalled());
    expect(screen.queryByText("Not connected")).toBeNull();
    expect(screen.queryByText("Needs a connection to the desktop")).toBeNull();
    expect(screen.queryByRole("heading", { name: "Models" })).toBeNull();
    expect(within(document.getElementById("desktop-update") as HTMLElement).getAllByText("Checking...").length).toBeGreaterThan(0);
    // The device is known and this browser never used the desktop: now the notice and the placeholders are right.
    resolveDevice({ deviceId: THIS_DEVICE, publicKey: new Uint8Array(32) });
    expect((await screen.findAllByText("Not connected")).length).toBeGreaterThan(0);
    expect(within(document.getElementById("desktop-models") as HTMLElement).getByRole("heading", { name: "Models" })).toBeInTheDocument();
    expect(within(document.getElementById("desktop-projects") as HTMLElement).getByText("Needs a connection to the desktop")).toBeInTheDocument();
  });

  it("connects and mounts the Models slot once this browser asked access before", async () => {
    clearSlots();
    registerSlot("desktop-settings-sections", () => <p>Models slot</p>);
    const link = provideMockLink();
    mount(SETTINGS);
    expect(await screen.findByText("Models slot")).toBeInTheDocument();
    await connected(link);
    expect(await screen.findByText("Connected")).toBeInTheDocument();
  });

  it("shows a clear not-connected state without a link provider", async () => {
    // A browser that used this desktop before, so the page would attach: only the missing provider leaves it unlinked.
    markGrantRequested(DESKTOP, THIS_DEVICE);
    mount(SETTINGS);
    await screen.findByRole("heading", { level: 1, name: "Fake Studio Mac" });
    // The page shows "Checking..." until this browser's device is read; wait for that, then the state is final.
    expect((await screen.findAllByText("Not connected")).length).toBeGreaterThan(0);
    expect(screen.queryByRole("button", { name: /Choose projects/ })).toBeNull();
    expect(screen.queryByRole("button", { name: /Reconnect/ })).toBeNull();
    expect(screen.getByRole("button", { name: /Forget in this browser/ })).toBeInTheDocument();
  });

  it("Reconnect runs the handshake again and refreshes workspace.list", async () => {
    const user = userEvent.setup();
    const link = provideMockLink();
    mount(SETTINGS);
    await connected(link);
    await screen.findByText("2 projects, 1 Mono");
    expect(link.handshakes).toBe(1);
    const before = link.calls.filter((call) => call.method === "workspace.list").length;

    await user.click(screen.getByRole("button", { name: /Reconnect/ }));
    await screen.findByText("Reconnected and refreshed from the desktop.");

    expect(link.handshakes).toBe(2);
    expect(link.status).toBe("connected");
    expect(link.calls.filter((call) => call.method === "workspace.list").length).toBeGreaterThan(before);
  });
});

describe("notifications", () => {
  it("loads the defaults of a desktop that never saved any as real values, and saves a flip at once", async () => {
    const user = userEvent.setup();
    provideMockLink();
    mount(SETTINGS);
    const toggle = await screen.findByRole("switch", { name: "Agent finished" });
    await waitFor(() => expect(toggle).toBeEnabled());
    expect(requests.some((request) => request.method === "GET" && request.url === `/api/push/prefs/${DESKTOP}`)).toBe(true);
    expect(toggle).toBeChecked();
    expect(screen.queryByText("Current settings not loaded")).toBeNull();
    expect(screen.queryByRole("button", { name: "Save these settings" })).toBeNull();

    await user.click(toggle);
    await waitFor(() => expect(requests.filter((request) => request.method === "PUT")).toHaveLength(1));
    const put = requests.filter((request) => request.method === "PUT");
    expect(put[0]?.url).toBe(`/api/push/prefs/${DESKTOP}`);
    expect(put[0]?.body).toEqual({ enabled: true, categories: { agentFinished: false, agentInput: true, monoMessages: true } });

    await waitFor(() => expect(screen.getByRole("switch", { name: "Notifications from this desktop" })).toBeEnabled());
    await user.click(screen.getByRole("switch", { name: "Notifications from this desktop" }));
    await waitFor(() => expect(screen.getByRole("switch", { name: "Agent finished" })).toBeDisabled());
    expect(requests.filter((request) => request.method === "PUT")[1]?.body).toEqual({ enabled: false, categories: { agentFinished: false, agentInput: true, monoMessages: true } });
  });

  it("shows the stored values of a desktop that saved them", async () => {
    provideMockLink();
    mockState.prefs.set(DESKTOP, { enabled: true, categories: { agentFinished: false, agentInput: true, monoMessages: false } });
    mount(SETTINGS);
    const toggle = await screen.findByRole("switch", { name: "Agent finished" });
    await waitFor(() => expect(toggle).not.toBeChecked());
    expect(screen.getByRole("switch", { name: "Messages from Monos" })).not.toBeChecked();
    expect(screen.getByRole("switch", { name: "Approvals and questions" })).toBeChecked();
  });
});

describe("projects picker", () => {
  it("narrows: Save sends only the narrower project list", async () => {
    const user = userEvent.setup();
    const link = provideMockLink();
    mount(SETTINGS);
    const dialog = await openPicker(user);
    const beta = await within(dialog).findByRole("checkbox", { name: /Example Beta/ });
    expect(beta).toBeChecked();
    expect(within(dialog).getByRole("button", { name: "Save" })).toBeDisabled();
    await user.click(beta);
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Projects on this device" })).toBeNull());
    expect(setCalls(link)).toEqual([{ method: "device.selection", params: { projects: ["proj-alpha"] } }]);
    expect(selectionOf(link)).toEqual({ projects: ["proj-alpha"], monos: null });
    // The row now counts what the desktop lists for this device.
    await screen.findByText("1 project, 1 Mono");
  });

  it("lists a project that is not shared as Not shared, with no id and no path", async () => {
    const user = userEvent.setup();
    const link = provideMockLink();
    const device = [...link.desktop.devices.values()][0];
    if (device) device.selection = { projects: ["proj-alpha"], monos: null };
    mount(SETTINGS);
    const dialog = await openPicker(user);
    const beta = await within(dialog).findByRole("checkbox", { name: /Example Beta/ });
    expect(beta).not.toBeChecked();
    expect(beta).toBeEnabled();
    expect(within(dialog).getByRole("checkbox", { name: /Example Alpha/ })).toBeChecked();
    expect(within(dialog).getAllByText("Not shared")).toHaveLength(1);
    expect(within(dialog).getAllByText("Shared").length).toBeGreaterThanOrEqual(2);
    expect(dialog).not.toHaveTextContent("proj-beta");
    expect(dialog).not.toHaveTextContent("/example/beta");
    expect(within(dialog).getByRole("checkbox", { name: "All projects, including new ones" })).not.toBeChecked();
    expect(within(dialog).getByRole("checkbox", { name: "All Monos, including new ones" })).toBeChecked();
    expect(dialog).not.toHaveTextContent("never widen");
  });

  it("narrowing sends no ticket and never opens the passkey prompt", async () => {
    const user = userEvent.setup();
    const link = provideMockLink();
    mount(SETTINGS);
    const dialog = await openPicker(user);
    await user.click(await within(dialog).findByRole("checkbox", { name: /Example Beta/ }));
    expect(within(dialog).getByRole("button", { name: "Save" })).toBeEnabled();
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(setCalls(link)).toEqual([{ method: "device.selection", params: { projects: ["proj-alpha"] } }]);
    expect(requests.some((request) => request.url.endsWith("/selection-ticket"))).toBe(false);
    expect(webauthn.getPasskey).not.toHaveBeenCalled();
  });

  it("widening asks for a ticket through the passkey prompt, then sends the refs with the ticket", async () => {
    const user = userEvent.setup();
    const link = provideMockLink();
    const device = [...link.desktop.devices.values()][0];
    if (device) device.selection = { projects: ["proj-alpha"], monos: null };
    mount(SETTINGS);
    const dialog = await openPicker(user);
    await user.click(await within(dialog).findByRole("checkbox", { name: /Example Beta/ }));
    await user.click(within(dialog).getByRole("button", { name: "Confirm and save" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());

    expect(webauthn.getPasskey).toHaveBeenCalledTimes(1);
    expect(tickets).toEqual(["ticket-1"]);
    const ticketRequests = requests.filter((request) => request.url.endsWith("/selection-ticket"));
    expect(ticketRequests.map((request) => request.url)).toEqual([`/api/desktops/${DESKTOP}/grants/dev-this/selection-ticket`, `/api/desktops/${DESKTOP}/grants/dev-this/selection-ticket`]);
    const sets = setCalls(link);
    expect(sets).toHaveLength(1);
    const params = sets[0]?.params as { projects: string[]; addProjects: string[]; ticket: string };
    expect(params.projects).toEqual(["proj-alpha"]);
    expect(params.addProjects).toHaveLength(1);
    expect(params.ticket).toBe("ticket-1");
    // The ref is opaque: it must not carry the id or the path.
    expect(JSON.stringify(params)).not.toContain("proj-beta");
    expect(selectionOf(link)?.projects).toEqual(["proj-alpha", "proj-beta"]);
    // The catalog and the settings row follow what the browser now sees.
    await screen.findByText("2 projects, 1 Mono");
  });

  it("a browser with read access cannot check an unshared project and is told why", async () => {
    const user = userEvent.setup();
    const link = provideMockLink();
    const device = [...link.desktop.devices.values()][0];
    if (device) {
      device.permission = "read";
      device.selection = { projects: ["proj-alpha"], monos: null };
    }
    mount(SETTINGS);
    const dialog = await openPicker(user);
    const beta = await within(dialog).findByRole("checkbox", { name: /Example Beta/ });
    expect(beta).toBeDisabled();
    expect(within(dialog).getByRole("checkbox", { name: "All projects, including new ones" })).toBeDisabled();
    expect(dialog).toHaveTextContent("Only a browser with Control access can add projects. You can still remove some.");
    // Removing still works, without a ticket.
    await user.click(within(dialog).getByRole("checkbox", { name: /Example Alpha/ }));
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(setCalls(link)).toEqual([{ method: "device.selection", params: { projects: [] } }]);
    expect(webauthn.getPasskey).not.toHaveBeenCalled();
  });

  it("All projects, including new ones sends projects: null with the ticket", async () => {
    const user = userEvent.setup();
    const link = provideMockLink();
    const device = [...link.desktop.devices.values()][0];
    if (device) device.selection = { projects: ["proj-alpha"], monos: null };
    mount(SETTINGS);
    const dialog = await openPicker(user);
    const all = await within(dialog).findByRole("checkbox", { name: "All projects, including new ones" });
    await user.click(all);
    expect(within(dialog).getByRole("checkbox", { name: /Example Beta/ })).toBeDisabled();
    await user.click(within(dialog).getByRole("button", { name: "Confirm and save" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(setCalls(link)).toEqual([{ method: "device.selection", params: { projects: null, ticket: "ticket-1" } }]);
    expect(selectionOf(link)?.projects).toBeNull();
  });

  it("turning All projects off sends the shared list without a ticket", async () => {
    const user = userEvent.setup();
    const link = provideMockLink();
    mount(SETTINGS);
    const dialog = await openPicker(user);
    await user.click(await within(dialog).findByRole("checkbox", { name: "All projects, including new ones" }));
    await user.click(within(dialog).getByRole("checkbox", { name: /Example Beta/ }));
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(setCalls(link)).toEqual([{ method: "device.selection", params: { projects: ["proj-alpha"] } }]);
    expect(webauthn.getPasskey).not.toHaveBeenCalled();
  });

  it("stops without sending anything when the passkey prompt is cancelled", async () => {
    const user = userEvent.setup();
    const link = provideMockLink();
    const device = [...link.desktop.devices.values()][0];
    if (device) device.selection = { projects: ["proj-alpha"], monos: null };
    webauthn.getPasskey.mockResolvedValue({ status: "cancelled" });
    mount(SETTINGS);
    const dialog = await openPicker(user);
    await user.click(await within(dialog).findByRole("checkbox", { name: /Example Beta/ }));
    await user.click(within(dialog).getByRole("button", { name: "Confirm and save" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("Passkey confirmation was cancelled");
    expect(setCalls(link)).toHaveLength(0);
    expect(selectionOf(link)?.projects).toEqual(["proj-alpha"]);
  });

  it.each([
    ["fresh_required", "could not confirm your passkey"],
    ["read_only", "Only a browser with Control access can add projects"],
    ["not_found", "The desktop's list changed. Refresh and try again."],
    ["forbidden", "The desktop did not allow this change"],
    ["revoked", "no longer has access to this desktop"],
  ])("maps the desktop code %s to clear copy", async (code, copy) => {
    const user = userEvent.setup();
    const link = provideMockLink();
    const device = [...link.desktop.devices.values()][0];
    if (device) device.selection = { projects: ["proj-alpha"], monos: null };
    const original = link.call.bind(link) as (method: string, params?: unknown) => Promise<unknown>;
    vi.spyOn(link, "call").mockImplementation(((method: string, params?: unknown) =>
      method === "device.selection" && params && Object.keys(params).length > 0 ? Promise.reject(new DesktopLinkError("desktop said so", code)) : original(method, params)) as never);
    mount(SETTINGS);
    const dialog = await openPicker(user);
    await user.click(await within(dialog).findByRole("checkbox", { name: /Example Beta/ }));
    await user.click(within(dialog).getByRole("button", { name: "Confirm and save" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(copy);
    expect(screen.getByRole("dialog", { name: "Projects on this device" })).toBeInTheDocument();
  });

  it("shows clear copy when the Worker refuses the ticket (403)", async () => {
    const user = userEvent.setup();
    const link = provideMockLink();
    const device = [...link.desktop.devices.values()][0];
    if (device) device.selection = { projects: ["proj-alpha"], monos: null };
    mount(SETTINGS);
    const dialog = await openPicker(user);
    await user.click(await within(dialog).findByRole("checkbox", { name: /Example Beta/ }));
    ticketAnswer = { status: 403, code: "forbidden" };
    await user.click(within(dialog).getByRole("button", { name: "Confirm and save" }));
    await waitFor(() => expect(within(dialog).getByRole("alert")).toHaveTextContent("does not have Control access"));
    expect(setCalls(link)).toHaveLength(0);
  });

  it("an older desktop without `available` keeps the narrow-only list and its copy", async () => {
    const user = userEvent.setup();
    const link = provideMockLink();
    const original = link.call.bind(link) as (method: string, params?: unknown) => Promise<unknown>;
    vi.spyOn(link, "call").mockImplementation((async (method: string, params?: unknown) => {
      const result = (await original(method, params)) as Record<string, unknown>;
      if (method !== "device.selection") return result;
      const { projects, monos } = result;
      return { projects, monos };
    }) as never);
    mount(SETTINGS);
    const dialog = await openPicker(user);
    const beta = await within(dialog).findByRole("checkbox", { name: /Example Beta/ });
    expect(dialog).toHaveTextContent("never widen it");
    expect(within(dialog).queryByText("Shared")).toBeNull();
    expect(within(dialog).queryByRole("checkbox", { name: /including new ones/ })).toBeNull();
    expect(dialog).toHaveTextContent("/example/beta");
    await user.click(beta);
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(setCalls(link)).toEqual([{ method: "device.selection", params: { projects: ["proj-alpha"] } }]);
    expect(webauthn.getPasskey).not.toHaveBeenCalled();
  });

  it("an older desktop that refuses with forbidden shows the refused copy and its message", async () => {
    const user = userEvent.setup();
    const link = provideMockLink();
    const original = link.call.bind(link) as (method: string, params?: unknown) => Promise<unknown>;
    vi.spyOn(link, "call").mockImplementation((async (method: string, params?: unknown) => {
      if (method === "device.selection" && params && Object.keys(params).length > 0) throw new DesktopLinkError("widening is refused", "forbidden");
      const result = (await original(method, params)) as Record<string, unknown>;
      if (method !== "device.selection") return result;
      const { projects, monos } = result;
      return { projects, monos };
    }) as never);
    mount(SETTINGS);
    const dialog = await openPicker(user);
    await user.click(await within(dialog).findByRole("checkbox", { name: /Example Beta/ }));
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(`${REFUSED_COPY} (Desktop said: widening is refused)`);
  });

  it("keeps granted ids the desktop no longer lists when narrowing", async () => {
    const user = userEvent.setup();
    const link = provideMockLink();
    const device = [...link.desktop.devices.values()][0];
    if (device) device.selection = { projects: ["proj-alpha", "proj-beta", "proj-closed"], monos: null };
    mount(SETTINGS);
    const dialog = await openPicker(user);
    await user.click(await within(dialog).findByRole("checkbox", { name: /Example Beta/ }));
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(setCalls(link)).toEqual([{ method: "device.selection", params: { projects: ["proj-alpha", "proj-closed"] } }]);
  });

  it("Refresh desktop reloads the catalog without a set call", async () => {
    const user = userEvent.setup();
    const link = provideMockLink();
    mount(SETTINGS);
    const dialog = await openPicker(user);
    await within(dialog).findByRole("checkbox", { name: /Example Beta/ });
    const before = link.calls.filter((call) => call.method === "workspace.list").length;
    await user.click(within(dialog).getByRole("button", { name: /Refresh desktop/ }));
    await waitFor(() => expect(link.calls.filter((call) => call.method === "workspace.list").length).toBeGreaterThan(before));
    expect(setCalls(link)).toHaveLength(0);
  });
});

describe("first run", () => {
  // The first-run picker slot is rendered by the /d/:desktopId layout only, so these tests mount a /d route. The probe
  // slot proves the layout rendered, so "no dialog" is not true just because nothing mounted yet.
  async function mountFirstRun() {
    registerSlot("first-run-picker", FirstRunPicker);
    registerSlot("desktop-switcher", () => <p>Layout probe</p>);
    const mounted = mount(`/d/${DESKTOP}`);
    await screen.findByText("Layout probe");
    return mounted;
  }

  it("Choose later makes no set call, keeps the selection and sets the flag; the picker shows once", async () => {
    const user = userEvent.setup();
    const link = provideMockLink();
    const first = await mountFirstRun();
    await connected(link);
    const dialog = await screen.findByRole("dialog", { name: "Projects on this device" });
    expect(within(dialog).queryByRole("button", { name: "Cancel" })).toBeNull();
    await user.click(await within(dialog).findByRole("button", { name: "Choose later" }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Projects on this device" })).toBeNull());
    expect(setCalls(link)).toHaveLength(0);
    expect(selectionOf(link)).toEqual({ projects: null, monos: null });
    expect(window.localStorage.getItem(pickerSeenKey(DESKTOP))).toBe("1");

    first.queryClient.clear();
    cleanup();
    clearSlots();
    await mountFirstRun();
    await connected(link);
    expect(screen.queryByRole("dialog", { name: "Projects on this device" })).toBeNull();
  });

  it("does not show without a link provider and leaves the flag unset", async () => {
    await mountFirstRun();
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(window.localStorage.getItem(pickerSeenKey(DESKTOP))).toBeNull();
  });
});

describe("remove this desktop", () => {
  it("confirms, calls device.leave, runs the forget hook and goes to /settings/desktops", async () => {
    const user = userEvent.setup();
    const link = provideMockLink();
    const forgotten = vi.fn();
    onForgetDesktop(forgotten);
    const { router } = mount(SETTINGS);
    await connected(link);
    await user.click(await screen.findByRole("button", { name: /Forget in this browser/ }));
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent("Remove this desktop from this device?");
    expect(link.calls.some((call) => call.method === "device.leave")).toBe(false);
    await user.click(within(dialog).getByRole("button", { name: "Remove" }));
    await waitFor(() => expect(router.state.location.pathname).toBe("/settings/desktops"));
    expect(link.calls.some((call) => call.method === "device.leave")).toBe(true);
    expect(forgotten).toHaveBeenCalledWith(DESKTOP);
    expect(link.desktop.devices.size).toBe(0);
  });

  it("forgets locally without device.leave when the desktop is offline", async () => {
    const user = userEvent.setup();
    installOfflineAccount();
    const link = provideMockLink({ online: false });
    const forgotten = vi.fn();
    onForgetDesktop(forgotten);
    const { router } = mount(SETTINGS);
    await connected(link);
    await user.click(await screen.findByRole("button", { name: /Forget in this browser/ }));
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent("revoke this device's access on your account, use Desktops");
    await user.click(within(dialog).getByRole("button", { name: "Remove" }));
    await waitFor(() => expect(router.state.location.pathname).toBe("/settings/desktops"));
    expect(link.calls.some((call) => call.method === "device.leave")).toBe(false);
    expect(forgotten).toHaveBeenCalledWith(DESKTOP);
  });

  it("does not forget when device.leave fails, but offers to remove from this device anyway", async () => {
    const user = userEvent.setup();
    const link = provideMockLink();
    const forgotten = vi.fn();
    onForgetDesktop(forgotten);
    window.localStorage.setItem(pickerSeenKey(DESKTOP), "1");
    vi.spyOn(link, "call").mockRejectedValue(new DesktopLinkError("closed", "closed"));
    const { router } = mount(SETTINGS);
    await user.click(await screen.findByRole("button", { name: /Forget in this browser/ }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).queryByRole("button", { name: "Remove from this device anyway" })).toBeNull();
    await user.click(within(dialog).getByRole("button", { name: "Remove" }));
    await within(dialog).findByRole("alert");
    expect(forgotten).not.toHaveBeenCalled();
    expect(router.state.location.pathname).toBe(SETTINGS);

    // Never trapped: the local forget works without the desktop and clears the picker flag (M11).
    await user.click(within(dialog).getByRole("button", { name: "Remove from this device anyway" }));
    await waitFor(() => expect(router.state.location.pathname).toBe("/settings/desktops"));
    expect(forgotten).toHaveBeenCalledWith(DESKTOP);
    expect(window.localStorage.getItem(pickerSeenKey(DESKTOP))).toBeNull();
  });

  it("keeps keyboard focus on the Reconnect button while it runs", async () => {
    const user = userEvent.setup();
    const link = provideMockLink();
    mount(SETTINGS);
    await connected(link);
    const row = await screen.findByRole("button", { name: /Reconnect/ });
    await user.click(row);
    expect(screen.getByRole("button", { name: /Reconnect/ })).toBeInTheDocument();
    await waitFor(() => expect(screen.getByRole("button", { name: /Reconnect/ })).not.toHaveAttribute("aria-disabled"));
  });
});

describe("forget hook registry", () => {
  it("is a no-op without handlers and runs every handler", async () => {
    await expect(forgetDesktopLocally("x")).resolves.toBeUndefined();
    const a = vi.fn();
    const b = vi.fn();
    const offA = onForgetDesktop(a);
    onForgetDesktop(b);
    await forgetDesktopLocally("x");
    offA();
    await forgetDesktopLocally("y");
    expect(a).toHaveBeenCalledTimes(1);
    expect(b).toHaveBeenCalledTimes(2);
  });
});

function installOfflineAccount() {
  restore?.();
  const state = createMockState({ signedIn: true });
  const desktop = state.desktops[0];
  if (desktop) desktop.online = false;
  const server = createMockServer({ state });
  restore = setApiTransport(server.transport);
}

describe("Desktop app update", () => {
  const updateCalls = (link: MockDesktopLink) => link.calls.filter((call) => call.method === "desktop.update");

  it("shows both versions and, for a Control device, updates after a confirmation that lists the running sessions", async () => {
    const user = userEvent.setup();
    const link = provideMockLink();
    link.desktop.world.latestVersion = "1.5.0";
    mount(SETTINGS);
    expect(await screen.findByRole("heading", { level: 2, name: "Desktop app update" })).toBeInTheDocument();
    expect(await screen.findByText("1.5.0")).toBeInTheDocument();
    expect(screen.getByText("1.4.0")).toBeInTheDocument();

    await user.click(await screen.findByRole("button", { name: "Update and restart" }));
    const dialog = await screen.findByRole("dialog", { name: "Update and restart the desktop?" });
    expect(within(dialog).getByText("Run the example checks")).toBeInTheDocument();
    expect(updateCalls(link)).toHaveLength(0);

    await user.click(within(dialog).getByRole("button", { name: "Update and restart" }));
    expect(await screen.findByText("Desktop is updating and restarting")).toBeInTheDocument();
    expect(updateCalls(link).map((call) => call.params)).toEqual([{ confirm: true }]);
    expect(screen.queryByRole("dialog")).toBeNull();

    // The app relaunches: the link drops and comes back, and the new version shows.
    link.desktop.world.completeUpdate();
    await link.reconnect();
    await waitFor(() => expect(screen.queryByText("Desktop is updating and restarting")).toBeNull());
    await waitFor(() => expect(screen.queryByText("1.4.0")).toBeNull());
    expect(screen.getAllByText("1.5.0")).toHaveLength(2);
    expect(screen.queryByRole("button", { name: "Update and restart" })).toBeNull();
  });

  async function startUpdate(user: ReturnType<typeof userEvent.setup>) {
    await user.click(await screen.findByRole("button", { name: "Update and restart" }));
    const dialog = await screen.findByRole("dialog", { name: "Update and restart the desktop?" });
    await user.click(within(dialog).getByRole("button", { name: "Update and restart" }));
    expect(await screen.findByText("Desktop is updating and restarting")).toBeInTheDocument();
  }

  it("says the connection is down while the desktop restarts, then shows the new version once it is back", async () => {
    const user = userEvent.setup();
    const link = provideMockLink();
    link.desktop.world.latestVersion = "1.5.0";
    mount(SETTINGS);
    await screen.findByText("1.5.0");
    await startUpdate(user);
    expect(screen.getByText("The desktop is downloading and installing the update.")).toBeInTheDocument();

    link.hold();
    expect(await screen.findByText(/The connection is down while the desktop restarts/)).toBeInTheDocument();
    link.hold({ offline: true });
    expect(screen.getByText(/The connection is down while the desktop restarts/)).toBeInTheDocument();
    expect(screen.getByText("Desktop is updating and restarting")).toBeInTheDocument();

    link.desktop.world.completeUpdate();
    await link.restore();
    await waitFor(() => expect(screen.queryByText("Desktop is updating and restarting")).toBeNull());
    await waitFor(() => expect(screen.getAllByText("1.5.0")).toHaveLength(2));
    expect(screen.queryByText("1.4.0")).toBeNull();
    expect(screen.queryByText(/The connection is down/)).toBeNull();
  });

  it("polls the status while the link stays up and shows a failure before the restart, then allows trying again", async () => {
    const user = userEvent.setup();
    const link = provideMockLink();
    link.desktop.world.latestVersion = "1.5.0";
    link.desktop.world.failNextUpdate = "disk full";
    mount(SETTINGS);
    await screen.findByText("1.5.0");
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    try {
      await startUpdate(user);
      const statusReads = () => link.calls.filter((call) => call.method === "desktop.updateStatus").length;
      const before = statusReads();
      await vi.advanceTimersByTimeAsync(UPDATE_POLL_MS - 100);
      expect(statusReads()).toBe(before);
      await vi.advanceTimersByTimeAsync(200);
      expect(await screen.findByText("Update to 1.5.0 failed: disk full")).toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
    expect(screen.queryByText("Desktop is updating and restarting")).toBeNull();
    expect(link.status).toBe("connected");

    // Trying again clears the old failure and starts a new install.
    await user.click(screen.getByRole("button", { name: "Update and restart" }));
    const dialog = await screen.findByRole("dialog", { name: "Update and restart the desktop?" });
    await user.click(within(dialog).getByRole("button", { name: "Update and restart" }));
    expect(await screen.findByText("Desktop is updating and restarting")).toBeInTheDocument();
    expect(screen.queryByText("Update to 1.5.0 failed: disk full")).toBeNull();
    expect(link.desktop.world.updatesStarted).toHaveLength(2);
  });

  it("shows a failure the desktop already reported on a plain status load", async () => {
    const link = provideMockLink();
    link.desktop.world.latestVersion = "1.5.0";
    link.desktop.world.lastUpdateFailure = "Update to 1.5.0 failed: disk full";
    mount(SETTINGS);
    expect(await screen.findByText("Update to 1.5.0 failed: disk full")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Update and restart" })).toBeInTheDocument();
  });

  it("asks again with the returned sessions when the desktop answers confirmation_required", async () => {
    const user = userEvent.setup();
    const link = provideMockLink();
    link.desktop.world.latestVersion = "1.5.0";
    link.desktop.world.session("sess-local-approval").busy = false;
    mount(SETTINGS);
    await user.click(await screen.findByRole("button", { name: "Update and restart" }));
    // A session starts running after the status was read.
    link.desktop.world.session("sess-local-approval").busy = true;
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).queryByText("Run the example checks")).toBeNull();

    await user.click(within(dialog).getByRole("button", { name: "Update and restart" }));
    expect(await within(dialog).findByText("Run the example checks")).toBeInTheDocument();
    expect(updateCalls(link).map((call) => call.params)).toEqual([{}]);

    await user.click(within(dialog).getByRole("button", { name: "Update and restart" }));
    expect(await screen.findByText("Desktop is updating and restarting")).toBeInTheDocument();
    expect(updateCalls(link).map((call) => call.params)).toEqual([{}, { confirm: true }]);
  });

  it("a read-only device sees the versions and a note, but no button", async () => {
    getLinkStore(DESKTOP).setState({ permission: "read" });
    const link = provideMockLink({ permission: "read" });
    link.desktop.world.latestVersion = "1.5.0";
    mount(SETTINGS);
    expect(await screen.findByText("1.5.0")).toBeInTheDocument();
    expect(screen.getByText("Control access is needed to update the desktop from here.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Update and restart" })).toBeNull();
    expect(updateCalls(link)).toHaveLength(0);
  });

  it("offers no button when the desktop is up to date", async () => {
    provideMockLink();
    mount(SETTINGS);
    await waitFor(() => expect(screen.getAllByText("1.4.0")).toHaveLength(2));
    expect(screen.queryByRole("button", { name: "Update and restart" })).toBeNull();
  });

  it("without a connection the card says it needs one", async () => {
    mount(SETTINGS);
    expect(await screen.findByRole("heading", { level: 2, name: "Desktop app update" })).toBeInTheDocument();
    // The Models placeholder says it too, so each card is checked on its own. The cards say "Checking..." until the device is read.
    await waitFor(() => expect(within(document.getElementById("desktop-update")!).getByText("Needs a connection to the desktop")).toBeInTheDocument());
    expect(within(document.getElementById("desktop-update")!).getByText("Needs a connection to the desktop")).toBeInTheDocument();
    expect(within(document.getElementById("desktop-projects")!).getByText("Needs a connection to the desktop")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Update and restart" })).toBeNull();
  });
});

describe("page structure", () => {
  const ANCHORS = [
    "desktop-general",
    "desktop-status",
    "desktop-about",
    "desktop-name",
    "desktop-color",
    "desktop-avatar",
    "desktop-models",
    "desktop-notifications",
    "desktop-projects",
    "desktop-update",
    "desktop-access",
    "desktop-troubleshoot",
    "desktop-remove",
  ];

  it("renders the cards in the requested order, each with its settings anchor", async () => {
    provideMockLink();
    mount(SETTINGS);
    await screen.findByRole("heading", { level: 1, name: "Fake Studio Mac" });
    const nodes = ANCHORS.map((anchor) => document.getElementById(anchor));
    nodes.forEach((node, index) => expect(node, ANCHORS[index]).not.toBeNull());
    for (let index = 1; index < nodes.length; index += 1) {
      const before = nodes[index - 1] as HTMLElement;
      const after = nodes[index] as HTMLElement;
      expect(before.compareDocumentPosition(after) & Node.DOCUMENT_POSITION_FOLLOWING, `${ANCHORS[index - 1]} before ${ANCHORS[index]}`).toBeTruthy();
    }
  });

  it("groups status, about, name, color and avatar as blocks of the General card, in that order", async () => {
    provideMockLink();
    mount(SETTINGS);
    await screen.findByRole("heading", { level: 1, name: "Fake Studio Mac" });
    const general = document.getElementById("desktop-general") as HTMLElement;
    expect(within(general).getByRole("heading", { level: 2, name: "General" })).toBeInTheDocument();
    const blocks = ["desktop-status", "desktop-about", "desktop-name", "desktop-color", "desktop-avatar"].map((id) => document.getElementById(id) as HTMLElement);
    blocks.forEach((block) => expect(general.contains(block)).toBe(true));
    expect(blocks.map((block) => block.querySelector("h3")?.textContent)).toEqual(["Status", "About", "Name", "Color", "Avatar"]);
    expect(blocks.every((block) => block.className.includes("scroll-mt-"))).toBe(true);
    // The avatar block keeps its footnote as in-card text, and no block is a card of its own.
    expect(within(blocks[4] as HTMLElement).getByText(/Shown in the desktop list on the left/)).toBeInTheDocument();
    expect(general.querySelectorAll('[data-slot="settings-card"]')).toHaveLength(1);
  });

  it("offers Open desktop and a way back to all desktops", async () => {
    mount(SETTINGS);
    await screen.findByRole("heading", { level: 1, name: "Fake Studio Mac" });
    expect(screen.getByRole("link", { name: "Open desktop" })).toHaveAttribute("href", `/d/${DESKTOP}`);
    // The nav repeats "All desktops": the back link is the one in the page header.
    const header = screen.getByRole("heading", { level: 1 }).closest("header") as HTMLElement;
    expect(within(header).getByRole("link", { name: "All desktops" })).toHaveAttribute("href", "/settings/desktops");
  });

  it("shows the not-found state for a desktop that is not on the account", async () => {
    mount("/settings/desktops/nope");
    expect(await screen.findByRole("heading", { level: 1, name: "Desktop not found" })).toBeInTheDocument();
    expect(screen.getByText("It may have been revoked, or it belongs to another account.")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Back to desktops" })).toHaveAttribute("href", "/settings/desktops");
  });

  it("the Remove card has two clearly scoped actions, each with its own unchanged dialog", async () => {
    const user = userEvent.setup();
    provideMockLink();
    mount(SETTINGS);
    const card = (await screen.findByRole("heading", { level: 2, name: "Remove" })).closest("section") as HTMLElement;
    expect(within(card).getByText("This browser stops using this desktop. It stays on your account.")).toBeInTheDocument();
    expect(within(card).getByText("Removes this desktop and every device's access from your account, in every browser. Needs your passkey.")).toBeInTheDocument();

    await user.click(within(card).getByRole("button", { name: /Forget in this browser/ }));
    let dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByRole("heading", { name: "Remove this desktop from this device?" })).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Remove" })).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());

    await user.click(within(card).getByRole("button", { name: /Revoke from account/ }));
    dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByRole("heading", { name: "Revoke Fake Studio Mac?" })).toBeInTheDocument();
    expect(dialog).toHaveTextContent("Fake Studio Mac and every device's access to it are removed from your account. You will be asked to confirm with your passkey.");
    expect(within(dialog).getByRole("button", { name: "Revoke desktop" })).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(requests.some((request) => request.method === "DELETE")).toBe(false);
  });

  it.each([`/d/${DESKTOP}/settings`, `/desktops/${DESKTOP}`])("%s lands on this page", async (url) => {
    const { router } = mount(url);
    await screen.findByRole("heading", { level: 1, name: "Fake Studio Mac" });
    expect(router.state.location.pathname).toBe(SETTINGS);
  });
});
