import { QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type * as KeysModule from "@/link/keys";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { setApiTransport, type ApiTransport } from "@/features/accounts/api";
import { createMockServer } from "@/features/accounts/mock/transport";
import { createMockState } from "@/features/accounts/mock/state";
import { createQueryClient } from "@/lib/queryClient";
import { REFUSED_COPY } from "./ProjectsPicker";
import { createAppRouter } from "@/router";
import { clearSlots, registerSlot } from "@/ui/slots";
import { DesktopLinkError, forgetDesktopLocally, onForgetDesktop, provideDesktopLink, resetDesktopLinks } from "./desktopLink";
import { FirstRunPicker, pickerSeenKey } from "./FirstRunPicker";
import { createMockDesktopLink, type MockDesktopLink } from "./mockDesktopLink";

// The widening flow asks for a passkey (re-auth) and for this browser's device id; neither exists in jsdom.
const webauthn = vi.hoisted(() => ({ createPasskey: vi.fn(), getPasskey: vi.fn() }));
vi.mock("@/features/accounts/webauthn", () => webauthn);
const keys = vi.hoisted(() => ({ getDeviceKeyInfo: vi.fn() }));
vi.mock("@/link/keys", async (importOriginal) => ({ ...(await importOriginal<typeof KeysModule>()), ...keys }));
const ASSERTION = { status: "ok", response: { id: "cred", rawId: "cred", type: "public-key", response: {}, clientExtensionResults: {} } };

const DESKTOP = "desktop-dev-1";
const SETTINGS = `/d/${DESKTOP}/settings`;

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
function provideMockLink(options: { online?: boolean } = {}): MockDesktopLink {
  const link = createMockDesktopLink(options);
  links.push(link);
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
  await user.click(await screen.findByRole("button", { name: /Projects on this device/ }));
  return screen.findByRole("dialog", { name: "Projects on this device" });
}

beforeEach(() => {
  requests = [];
  window.localStorage.clear();
  webauthn.getPasskey.mockResolvedValue(ASSERTION);
  keys.getDeviceKeyInfo.mockResolvedValue({ deviceId: "dev-this", publicKey: new Uint8Array(32) });
  installApi();
});
afterEach(() => {
  restore?.();
  for (const link of links) link.close();
  links = [];
  resetDesktopLinks();
  clearSlots();
  window.history.replaceState(null, "", "/");
});

describe("/d/:desktopId/settings", () => {
  it("renders one h1, the groups and the desktop-settings-sections slot with the desktop id", async () => {
    // The router imports features/sessions/register, which registers the default-model row (U-07): replace it.
    clearSlots();
    registerSlot("desktop-settings-sections", ({ desktopId }) => <p>Slot for {desktopId}</p>);
    const link = provideMockLink();
    mount(SETTINGS);
    await screen.findByRole("heading", { level: 1, name: "Desktop settings" });
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    for (const title of ["Status", "Notifications", "Projects", "Connection", "Remove"]) {
      expect(screen.getByRole("heading", { level: 2, name: title })).toBeInTheDocument();
    }
    expect(screen.getByText("Fake Studio Mac", { selector: "span" })).toBeInTheDocument();
    expect(screen.getByText(`Slot for ${DESKTOP}`)).toBeInTheDocument();
    await connected(link);
    expect(await screen.findByText("Connected")).toBeInTheDocument();
    expect(await screen.findByText("2 projects, 1 Mono")).toBeInTheDocument();
  });

  it("shows a clear not-connected state without a link provider", async () => {
    mount(SETTINGS);
    await screen.findByRole("heading", { level: 1, name: "Desktop settings" });
    expect(screen.getAllByText("Not connected").length).toBeGreaterThan(0);
    expect(screen.queryByRole("button", { name: /Projects on this device/ })).toBeNull();
    expect(screen.queryByRole("button", { name: /Reconnect/ })).toBeNull();
    expect(screen.getByRole("button", { name: /Remove this desktop from this device/ })).toBeInTheDocument();
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
  function mountFirstRun() {
    registerSlot("first-run-picker", FirstRunPicker);
    return mount(SETTINGS);
  }

  it("Choose later makes no set call, keeps the selection and sets the flag; the picker shows once", async () => {
    const user = userEvent.setup();
    const link = provideMockLink();
    const first = mountFirstRun();
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
    mountFirstRun();
    await screen.findByRole("heading", { level: 1, name: "Desktop settings" });
    await connected(link);
    expect(screen.queryByRole("dialog", { name: "Projects on this device" })).toBeNull();
  });

  it("does not show without a link provider and leaves the flag unset", async () => {
    mountFirstRun();
    await screen.findByRole("heading", { level: 1, name: "Desktop settings" });
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(window.localStorage.getItem(pickerSeenKey(DESKTOP))).toBeNull();
  });
});

describe("remove this desktop", () => {
  it("confirms, calls device.leave, runs the forget hook and goes to /desktops", async () => {
    const user = userEvent.setup();
    const link = provideMockLink();
    const forgotten = vi.fn();
    onForgetDesktop(forgotten);
    const { router } = mount(SETTINGS);
    await connected(link);
    await user.click(await screen.findByRole("button", { name: /Remove this desktop from this device/ }));
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent("Remove this desktop from this device?");
    expect(link.calls.some((call) => call.method === "device.leave")).toBe(false);
    await user.click(within(dialog).getByRole("button", { name: "Remove" }));
    await waitFor(() => expect(router.state.location.pathname).toBe("/desktops"));
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
    await user.click(await screen.findByRole("button", { name: /Remove this desktop from this device/ }));
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent("revoke this device's access on your account, use Desktops");
    await user.click(within(dialog).getByRole("button", { name: "Remove" }));
    await waitFor(() => expect(router.state.location.pathname).toBe("/desktops"));
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
    await user.click(await screen.findByRole("button", { name: /Remove this desktop from this device/ }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).queryByRole("button", { name: "Remove from this device anyway" })).toBeNull();
    await user.click(within(dialog).getByRole("button", { name: "Remove" }));
    await within(dialog).findByRole("alert");
    expect(forgotten).not.toHaveBeenCalled();
    expect(router.state.location.pathname).toBe(SETTINGS);

    // Never trapped: the local forget works without the desktop and clears the picker flag (M11).
    await user.click(within(dialog).getByRole("button", { name: "Remove from this device anyway" }));
    await waitFor(() => expect(router.state.location.pathname).toBe("/desktops"));
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
