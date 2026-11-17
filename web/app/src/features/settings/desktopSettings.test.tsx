import { QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { setApiTransport, type ApiTransport } from "@/features/accounts/api";
import { createMockServer } from "@/features/accounts/mock/transport";
import { createMockState } from "@/features/accounts/mock/state";
import { createQueryClient } from "@/lib/queryClient";
import { createAppRouter } from "@/router";
import { clearSlots, registerSlot } from "@/ui/slots";
import { DesktopLinkError, forgetDesktopLocally, onForgetDesktop, provideDesktopLink, resetDesktopLinks } from "./desktopLink";
import { FirstRunPicker, pickerSeenKey } from "./FirstRunPicker";
import { createMockDesktopLink, type MockDesktopLink } from "./mockDesktopLink";

const DESKTOP = "desktop-dev-1";
const SETTINGS = `/d/${DESKTOP}/settings`;

let restore: (() => void) | undefined;
let requests: Array<{ method: string; url: string; body: unknown }> = [];
let links: MockDesktopLink[] = [];

function installApi() {
  const server = createMockServer({ state: createMockState({ signedIn: true }) });
  const transport: ApiTransport = (url, init) => {
    requests.push({ method: init.method ?? "GET", url, body: typeof init.body === "string" ? (JSON.parse(init.body) as unknown) : undefined });
    return server.transport(url, init);
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
  it("shows the switches as not loaded, sends nothing on a flip, and saves all values on an explicit Save", async () => {
    const user = userEvent.setup();
    provideMockLink();
    mount(SETTINGS);
    const toggle = await screen.findByRole("switch", { name: "Agent finished" });
    expect(screen.getByText("Current settings not loaded")).toBeInTheDocument();
    await user.click(toggle);
    expect(screen.getByRole("switch", { name: "Agent finished" })).not.toBeChecked();
    expect(requests.filter((request) => request.method === "PUT")).toHaveLength(0);

    await user.click(screen.getByRole("button", { name: "Save these settings" }));
    await waitFor(() => expect(screen.queryByText("Current settings not loaded")).toBeNull());
    const put = requests.filter((request) => request.method === "PUT");
    expect(put).toHaveLength(1);
    expect(put[0]?.url).toBe(`/api/push/prefs/${DESKTOP}`);
    expect(put[0]?.body).toEqual({ enabled: true, categories: { agentFinished: false, agentInput: true, monoMessages: true } });

    // Known now: a flip saves at once, from the last saved result.
    await user.click(screen.getByRole("switch", { name: "Notifications from this desktop" }));
    await waitFor(() => expect(screen.getByRole("switch", { name: "Agent finished" })).toBeDisabled());
    expect(requests.filter((request) => request.method === "PUT")[1]?.body).toEqual({ enabled: false, categories: { agentFinished: false, agentInput: true, monoMessages: true } });
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

  it("shows the desktop's refusal when an item outside the granted selection is checked", async () => {
    const user = userEvent.setup();
    const link = provideMockLink();
    mount(SETTINGS);
    // Narrow the Monos to none first: the catalog still lists the Mono, so checking it is a widening.
    let dialog = await openPicker(user);
    await user.click(await within(dialog).findByRole("checkbox", { name: /Example Mono/ }));
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(selectionOf(link)?.monos).toEqual([]);

    dialog = await openPicker(user);
    const mono = await within(dialog).findByRole("checkbox", { name: /Example Mono/ });
    expect(mono).not.toBeChecked();
    await user.click(mono);
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    const alert = await within(dialog).findByRole("alert");
    expect(alert).toHaveTextContent("only lets this browser see fewer");
    expect(alert).toHaveTextContent("A selection can only be narrowed from the web");
    expect(selectionOf(link)?.monos).toEqual([]);
    expect(screen.getByRole("dialog", { name: "Projects on this device" })).toBeInTheDocument();
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
