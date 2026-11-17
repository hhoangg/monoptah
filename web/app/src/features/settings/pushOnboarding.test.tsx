import { QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const push = vi.hoisted(() => ({
  subscribeToPush: vi.fn(async (_deviceId: string, _userId: string) => "sub_1"),
  hasBrowserSubscription: vi.fn(async () => true),
  forgetPushLocally: vi.fn(async () => undefined),
  unsubscribeFromPush: vi.fn(async () => undefined),
  getSavedPushSubscription: vi.fn(async (): Promise<{ deviceId: string; subscriptionId: string; userId: string | null } | null> => null),
  isPushSupported: vi.fn(() => true),
}));
vi.mock("@/features/push/subscribe", async (importOriginal) => ({ ...(await importOriginal<object>()), ...push }));
vi.mock("@/link", async (importOriginal) => ({ ...(await importOriginal<object>()), ensureDevice: vi.fn(async () => ({ deviceId: "dev_9" })) }));

import { setApiTransport, type ApiTransport } from "@/features/accounts/api";
import { signOut } from "@/features/accounts/queries";
import { createMockServer } from "@/features/accounts/mock/transport";
import { createMockState, SEED_USER_ID } from "@/features/accounts/mock/state";
import { createQueryClient } from "@/lib/queryClient";
import { createAppRouter } from "@/router";
import { pushPrefsKey } from "./pushPrefs";
import { resetPushRotationCheck } from "./usePushOnboarding";
import { deniedHelp } from "./PushOnboarding";
import { clearSlots } from "@/ui/slots";

let restore: (() => void) | undefined;
let requests: Array<{ method: string; url: string; body: unknown }> = [];
let requestPermission: ReturnType<typeof vi.fn<() => Promise<NotificationPermission>>>;

let desktopIds: string[] = [];

function installApi() {
  const state = createMockState({ signedIn: true });
  desktopIds = state.desktops.map((desktop) => desktop.id);
  const server = createMockServer({ state });
  const transport: ApiTransport = (url, init) => {
    requests.push({ method: init.method ?? "GET", url, body: typeof init.body === "string" ? (JSON.parse(init.body) as unknown) : undefined });
    return server.transport(url, init);
  };
  restore = setApiTransport(transport);
}

function stubEnvironment(options: { standalone: boolean; permission: NotificationPermission; pushManager?: boolean; result?: NotificationPermission }) {
  vi.stubGlobal("matchMedia", (query: string) => ({
    matches: options.standalone && query.includes("display-mode: standalone"),
    media: query,
    addEventListener: () => undefined,
    removeEventListener: () => undefined,
  }));
  requestPermission = vi.fn(async (): Promise<NotificationPermission> => options.result ?? "granted");
  const state = { permission: options.permission };
  vi.stubGlobal("Notification", {
    get permission() {
      return state.permission;
    },
    requestPermission: async () => {
      const result = await requestPermission();
      state.permission = result;
      return result;
    },
  });
  if (options.pushManager !== false) vi.stubGlobal("PushManager", class {});
  else vi.stubGlobal("PushManager", undefined);
}

function mount(url: string) {
  const queryClient = createQueryClient();
  const router = createAppRouter({ history: createMemoryHistory({ initialEntries: [url] }), queryClient });
  render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return { queryClient };
}

beforeEach(() => {
  resetPushRotationCheck();
  requests = [];
  window.localStorage.clear();
  push.getSavedPushSubscription.mockResolvedValue(null);
  push.isPushSupported.mockReturnValue(true);
  installApi();
});
afterEach(() => {
  cleanup();
  restore?.();
  clearSlots();
  vi.clearAllMocks();
  vi.unstubAllGlobals();
});

describe("push onboarding (P-05)", () => {
  it.each(["/settings", "/install", "/d/desktop-dev-1/settings"])("never asks for permission on load or render of %s", async (url) => {
    stubEnvironment({ standalone: true, permission: "default" });
    mount(url);
    await screen.findByRole("heading", { level: 1 });
    await screen.findAllByText(/Notifications/);
    expect(requestPermission).not.toHaveBeenCalled();
    expect(push.subscribeToPush).not.toHaveBeenCalled();
  });

  it("hides the button when the app is not installed and points to /install", async () => {
    stubEnvironment({ standalone: false, permission: "default" });
    mount("/settings");
    expect(await screen.findByText("Install Monoptah to get notifications")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Enable notifications" })).toBeNull();
    expect(screen.getByRole("link", { name: /Install Monoptah to get notifications/ })).toHaveAttribute("href", "/install");
  });

  it("hides the button without PushManager", async () => {
    stubEnvironment({ standalone: true, permission: "default", pushManager: false });
    mount("/settings");
    expect(await screen.findByText("Notifications are not available here")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Enable notifications" })).toBeNull();
  });

  it("requests permission from the click, then subscribes with the device id", async () => {
    stubEnvironment({ standalone: true, permission: "default" });
    mount("/settings");
    await userEvent.click(await screen.findByRole("button", { name: "Enable notifications" }));
    await screen.findByText("Notifications on");
    expect(requestPermission).toHaveBeenCalledTimes(1);
    expect(push.subscribeToPush).toHaveBeenCalledWith("dev_9", SEED_USER_ID);
    expect(screen.getByRole("button", { name: "Turn off" })).toBeInTheDocument();
  });

  it("shows the Enable button on /install in standalone mode", async () => {
    stubEnvironment({ standalone: true, permission: "default" });
    mount("/install");
    await userEvent.click(await screen.findByRole("button", { name: "Enable notifications" }));
    await screen.findByText("Notifications on");
    expect(requestPermission).toHaveBeenCalledTimes(1);
  });

  it("shows help when the permission is denied, before and after the click", async () => {
    stubEnvironment({ standalone: true, permission: "denied" });
    mount("/settings");
    expect(await screen.findByText("Notifications are blocked")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Enable notifications" })).toBeNull();
    cleanup();
    stubEnvironment({ standalone: true, permission: "default", result: "denied" });
    mount("/settings");
    await userEvent.click(await screen.findByRole("button", { name: "Enable notifications" }));
    expect(await screen.findByText("Notifications are blocked")).toBeInTheDocument();
    expect(push.subscribeToPush).not.toHaveBeenCalled();
  });

  it("has help text per platform without links", () => {
    expect(deniedHelp("ios")).toMatch(/Settings app/);
    expect(deniedHelp("android")).toMatch(/Android Settings/);
    expect(deniedHelp("desktop")).toMatch(/browser or system settings/);
  });

  it("turn off unsubscribes", async () => {
    stubEnvironment({ standalone: true, permission: "granted" });
    push.getSavedPushSubscription.mockResolvedValue({ deviceId: "dev_9", subscriptionId: "sub_1", userId: SEED_USER_ID });
    mount("/settings");
    await userEvent.click(await screen.findByRole("button", { name: "Turn off" }));
    await waitFor(() => expect(push.unsubscribeFromPush).toHaveBeenCalledTimes(1));
    expect(await screen.findByRole("button", { name: "Enable notifications" })).toBeInTheDocument();
  });

  it("sign out unsubscribes before the session ends", async () => {
    const queryClient = createQueryClient();
    await signOut(queryClient);
    expect(push.unsubscribeFromPush).toHaveBeenCalledTimes(1);
    expect(requests.some((r) => r.method === "POST" && r.url === "/api/auth/logout")).toBe(true);
  });

  const CATS = { agentFinished: false, agentInput: true, monoMessages: true };
  const known = (queryClient: ReturnType<typeof createQueryClient>, id: string, prefs: { enabled: boolean; categories: typeof CATS }) => queryClient.setQueryData(pushPrefsKey(id), prefs);

  it("Pause all with no known prefs changes nothing and says so", async () => {
    stubEnvironment({ standalone: false, permission: "default" });
    mount("/settings");
    await userEvent.click(await screen.findByRole("button", { name: "Pause all" }));
    await screen.findByText(/Nothing was paused/);
    expect(requests.filter((r) => r.method === "PUT")).toEqual([]);
  });

  it("Pause all turns off only known, enabled desktops (not an already off one) and keeps their categories; Resume all restores exactly those", async () => {
    stubEnvironment({ standalone: false, permission: "default" });
    const { queryClient } = mount("/settings");
    expect(desktopIds.length).toBeGreaterThan(1);
    const [a, b] = desktopIds as [string, string];
    known(queryClient, a, { enabled: true, categories: CATS });
    known(queryClient, b, { enabled: false, categories: CATS });
    await userEvent.click(await screen.findByRole("button", { name: "Pause all" }));
    await screen.findByText(/Notifications are paused for 1 desktop\./);
    let puts = requests.filter((r) => r.method === "PUT");
    expect(puts.map((p) => p.url)).toEqual([`/api/push/prefs/${a}`]);
    expect(puts[0]?.body).toEqual({ enabled: false, categories: CATS });
    expect(JSON.parse(window.localStorage.getItem("monoptah.pushPaused") ?? "{}")).toEqual({ [a]: CATS });

    requests = [];
    await userEvent.click(screen.getByRole("button", { name: "Resume all" }));
    await screen.findByText(/Notifications are back on for 1 desktop\./);
    puts = requests.filter((r) => r.method === "PUT");
    expect(puts.map((p) => p.url)).toEqual([`/api/push/prefs/${a}`]);
    expect(puts[0]?.body).toEqual({ enabled: true, categories: CATS });
    expect(window.localStorage.getItem("monoptah.pushPaused")).toBeNull();
  });

  it("Resume all is disabled when Pause all turned nothing off, so it never turns a desktop on by itself", async () => {
    stubEnvironment({ standalone: false, permission: "default" });
    mount("/settings");
    expect(await screen.findByRole("button", { name: "Resume all" })).toBeDisabled();
    expect(requests.filter((r) => r.method === "PUT")).toEqual([]);
  });

  it("a Resume all after a reload still restores what Pause all recorded", async () => {
    stubEnvironment({ standalone: false, permission: "default" });
    window.localStorage.setItem("monoptah.pushPaused", JSON.stringify({ [desktopIds[0] as string]: CATS }));
    mount("/settings");
    await userEvent.click(await screen.findByRole("button", { name: "Resume all" }));
    await screen.findByText(/Notifications are back on for 1 desktop\./);
    expect(requests.filter((r) => r.method === "PUT").map((p) => p.body)).toEqual([{ enabled: true, categories: CATS }]);
  });

  it("does not subscribe again on load for a subscription of another account", async () => {
    stubEnvironment({ standalone: true, permission: "granted" });
    push.getSavedPushSubscription.mockResolvedValue({ deviceId: "dev_9", subscriptionId: "sub_1", userId: "someone-else" });
    mount("/settings");
    expect(await screen.findByRole("button", { name: "Enable notifications" })).toBeInTheDocument();
    expect(push.subscribeToPush).not.toHaveBeenCalled();
  });

  it("re-sends the subscription of the signed-in user once, only while the browser still holds it", async () => {
    stubEnvironment({ standalone: true, permission: "granted" });
    push.getSavedPushSubscription.mockResolvedValue({ deviceId: "dev_9", subscriptionId: "sub_1", userId: SEED_USER_ID });
    push.hasBrowserSubscription.mockResolvedValue(false);
    mount("/settings");
    await screen.findByText("Notifications on");
    await waitFor(() => expect(push.hasBrowserSubscription).toHaveBeenCalled());
    expect(push.subscribeToPush).not.toHaveBeenCalled();
  });

  it("re-sends the subscription of the signed-in user when the browser still holds it", async () => {
    stubEnvironment({ standalone: true, permission: "granted" });
    push.getSavedPushSubscription.mockResolvedValue({ deviceId: "dev_9", subscriptionId: "sub_1", userId: SEED_USER_ID });
    push.hasBrowserSubscription.mockResolvedValue(true);
    mount("/settings");
    await waitFor(() => expect(push.subscribeToPush).toHaveBeenCalledWith("dev_9", SEED_USER_ID));
  });

  it("shows a sign-in link instead of the Enable button when nobody is signed in on /install", async () => {
    stubEnvironment({ standalone: true, permission: "default" });
    restore?.();
    restore = setApiTransport(createMockServer({ signedIn: false }).transport);
    mount("/install");
    expect(await screen.findByRole("link", { name: /Sign in to turn on notifications/ })).toHaveAttribute("href", "/login");
    expect(screen.queryByRole("button", { name: "Enable notifications" })).toBeNull();
    expect(requestPermission).not.toHaveBeenCalled();
  });

  it("the per-desktop categories still save with PUT", async () => {
    stubEnvironment({ standalone: true, permission: "default" });
    mount("/d/desktop-dev-1/settings");
    await userEvent.click(await screen.findByRole("switch", { name: "Agent finished" }));
    await userEvent.click(screen.getByRole("button", { name: "Save these settings" }));
    await waitFor(() => expect(requests.some((r) => r.method === "PUT" && r.url === "/api/push/prefs/desktop-dev-1")).toBe(true));
    const put = requests.find((r) => r.method === "PUT");
    expect(put?.body).toEqual({ enabled: true, categories: { agentFinished: false, agentInput: true, monoMessages: true } });
  });
});
