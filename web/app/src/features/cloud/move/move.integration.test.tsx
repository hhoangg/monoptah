// U-19 over the real link wired to the mock desktop: the move actions in the session menus (header and list
// card), their cost notices and confirms, and the move progress in composer-top.
import { QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { FIXTURE_HANDLERS, FixtureError, type MockDesktop } from "@monoptah/mock-desktop";
import { SessionStartTargetSchema } from "@monoptah/protocol";
import { MemoryPersistAdapter, registerPersistAdapter, type PersistAdapter } from "@/data";
import { DESKTOP_ID, connectMockDesktop, seedWorkspace, type MockLinkOptions } from "@/features/sessions/dev/mockLink";
import { resetSessionActivity } from "@/features/sessions/activityStore";
import { stubTranscriptLayout } from "@/features/transcript/layoutStub";
import { resetCollapsedToolCalls } from "@/features/transcript/toolCallsSetting";
import { resetLinkConnections } from "@/link";
import { clearLinkStores } from "@/link/store.ts";
import { createQueryClient } from "@/lib/queryClient";
import { createAppRouter } from "@/router";
import { clearHooks, registerSendGuard, registerSessionCapabilities } from "@/ui/hooks";
import { clearSlots, registerSlot } from "@/ui/slots";
import { clearLiveCloudStates } from "../session/liveState";
import { cloudSendGuard, cloudSessionCapabilities } from "../session/guard";
import { CardBadgeCloud, ComposerTopCloud, HeaderChipCloud, MenuItemsCloud } from "../session/slotComponents";
import { resetMoveStore } from "./moveStore";

beforeAll(() => {
  window.matchMedia = ((query: string) => ({
    matches: query.includes("min-width: 640px"),
    media: query,
    onchange: null,
    addEventListener() {},
    removeEventListener() {},
    addListener() {},
    removeListener() {},
    dispatchEvent: () => false,
  })) as never;
  stubTranscriptLayout();
  window.scrollTo = vi.fn() as never;
  URL.createObjectURL = vi.fn(() => "blob:fake");
  URL.revokeObjectURL = vi.fn();
});

const original = { ...FIXTURE_HANDLERS };
let previous: PersistAdapter;
beforeEach(() => {
  resetCollapsedToolCalls();
  localStorage.clear();
  previous = registerPersistAdapter(new MemoryPersistAdapter());
  registerSlot("composer-top", ComposerTopCloud);
  registerSlot("session-card-badge", CardBadgeCloud);
  registerSlot("session-header-chip", HeaderChipCloud);
  registerSlot("session-menu-items", MenuItemsCloud);
  registerSendGuard(cloudSendGuard);
  registerSessionCapabilities(cloudSessionCapabilities);
});
afterEach(() => {
  Object.assign(FIXTURE_HANDLERS, original);
  resetLinkConnections();
  clearLinkStores();
  registerPersistAdapter(previous);
  resetSessionActivity();
  resetMoveStore();
  clearSlots();
  clearHooks();
  clearLiveCloudStates();
  localStorage.clear();
});

function mount(path: string, options: MockLinkOptions = {}) {
  connectMockDesktop({
    ...options,
    seed: (desktop) => {
      seedWorkspace(desktop);
      desktop.flags = { remoteCloudStart: true, remoteGitPush: true };
      options.seed?.(desktop);
    },
  });
  const queryClient = createQueryClient();
  const router = createAppRouter({ history: createMemoryHistory({ initialEntries: [`/d/${DESKTOP_ID}${path}`] }), queryClient });
  render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
}

const moving = (id: string) => (desktop: MockDesktop) => {
  const record = desktop.world.sessions.get(id)!;
  record.cloud = { kind: "codespace", instance: "fake-codespace-one", state: "available", move: { step: "push", status: "running" } };
};

const spyOn = (method: "cloud.moveToCloud" | "cloud.moveToLocal" | "cloud.cancelMove") => {
  const spy = vi.fn();
  (FIXTURE_HANDLERS as Record<string, unknown>)[method] = (params: unknown, context: unknown) => {
    spy(params);
    return (original[method] as (p: unknown, c: unknown) => never)(params, context);
  };
  return spy;
};

const openHeaderMenu = async (user: ReturnType<typeof userEvent.setup>) => {
  await user.click(await screen.findByRole("button", { name: "Session actions" }));
  return screen.findByRole("menu");
};

describe("move actions are gated", () => {
  it.each([
    ["without cloudMove", { capabilities: ["cloud", "cloudStart"] }],
    ["without cloud", { capabilities: ["cloudMove"] }],
    ["on a read-only device", { permission: "read" as const }],
  ])("a local session shows no Move to cloud %s", async (_name, options) => {
    const user = userEvent.setup();
    mount("/s/sess-local-1", options);
    const menu = await openHeaderMenu(user);
    expect(within(menu).getByRole("menuitem", { name: "Settings" })).toBeInTheDocument();
    expect(within(menu).queryByRole("menuitem", { name: /Move to cloud|Continue locally|Cancel move/ })).toBeNull();
  });

  it("a remote machine session has no move action", async () => {
    const user = userEvent.setup();
    mount("/s/sess-remote-1");
    const menu = await openHeaderMenu(user);
    expect(within(menu).queryByRole("menuitem", { name: /Move to cloud|Continue locally|Cancel move/ })).toBeNull();
  });
});

describe("Move to cloud", () => {
  it("shows the cost notice, defaults to the desktop target and sends only the session id", async () => {
    const user = userEvent.setup();
    const spy = spyOn("cloud.moveToCloud");
    mount("/s/sess-local-1");
    await openHeaderMenu(user);
    await user.click(screen.getByRole("menuitem", { name: "Move to cloud" }));
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent(/billable/i);
    expect(dialog).not.toHaveTextContent(/delete/i);
    await user.click(within(dialog).getByRole("button", { name: "Move and start billing" }));
    await waitFor(() => expect(spy).toHaveBeenCalledWith({ sessionId: "sess-local-1" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("an optional target comes from startOptions and strict-parses", async () => {
    const user = userEvent.setup();
    const spy = spyOn("cloud.moveToCloud");
    mount("/s/sess-local-1");
    await openHeaderMenu(user);
    await user.click(screen.getByRole("menuitem", { name: "Move to cloud" }));
    const dialog = await screen.findByRole("dialog");
    await user.click(await within(dialog).findByRole("radio", { name: "Docker sandbox" }));
    await user.selectOptions(within(dialog).getByLabelText("Size"), "small");
    await user.click(within(dialog).getByRole("button", { name: "Move and start billing" }));
    await waitFor(() => expect(spy).toHaveBeenCalledTimes(1));
    const params = spy.mock.calls[0]![0] as { sessionId: string; target: unknown };
    expect(params.sessionId).toBe("sess-local-1");
    expect(params.target).toEqual({ kind: "sandbox", instance: "new", size: "small" });
    expect(SessionStartTargetSchema.safeParse(params.target).success).toBe(true);
  });

  it("explains a desktop that answers disabled", async () => {
    const user = userEvent.setup();
    FIXTURE_HANDLERS["cloud.moveToCloud"] = () => {
      throw new FixtureError("This action is not available from the web", "disabled");
    };
    mount("/s/sess-local-1");
    await openHeaderMenu(user);
    await user.click(screen.getByRole("menuitem", { name: "Move to cloud" }));
    const dialog = await screen.findByRole("dialog");
    await user.click(within(dialog).getByRole("button", { name: "Move and start billing" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(/turned off on your desktop/);
  });

  it("works from the menu of a session card in the list", async () => {
    const user = userEvent.setup();
    const spy = spyOn("cloud.moveToCloud");
    mount("/p/proj-alpha");
    await user.click(await screen.findByRole("button", { name: "Actions for Fix the example parser" }));
    await user.click(await screen.findByRole("menuitem", { name: "Move to cloud" }));
    const dialog = await screen.findByRole("dialog");
    await user.click(within(dialog).getByRole("button", { name: "Move and start billing" }));
    await waitFor(() => expect(spy).toHaveBeenCalledWith({ sessionId: "sess-local-1" }));
  });
});

describe("Continue locally", () => {
  it("shows the cost notice and calls cloud.moveToLocal after the confirm", async () => {
    const user = userEvent.setup();
    const spy = spyOn("cloud.moveToLocal");
    mount("/s/sess-codespace-1");
    await openHeaderMenu(user);
    expect(screen.queryByRole("menuitem", { name: "Move to cloud" })).toBeNull();
    await user.click(screen.getByRole("menuitem", { name: "Continue locally" }));
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent(/may wake the cloud instance/);
    expect(dialog).toHaveTextContent(/billable/i);
    expect(spy).not.toHaveBeenCalled();
    await user.click(within(dialog).getByRole("button", { name: "Continue locally" }));
    await waitFor(() => expect(spy).toHaveBeenCalledWith({ sessionId: "sess-codespace-1" }));
  });
});

describe("Cancel move", () => {
  it("shows the 7 steps in composer-top and cancels after a confirm that says the workspace is kept", async () => {
    const user = userEvent.setup();
    const spy = spyOn("cloud.cancelMove");
    mount("/s/sess-codespace-1", { seed: moving("sess-codespace-1") });
    expect(await within(await screen.findByTestId("cloud-composer-top")).findByRole("list", { name: "Move steps" })).toBeInTheDocument();
    await openHeaderMenu(user);
    expect(screen.queryByRole("menuitem", { name: "Continue locally" })).toBeNull();
    await user.click(screen.getByRole("menuitem", { name: "Cancel move" }));
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent(/workspace are kept/);
    expect(dialog).not.toHaveTextContent(/delete (the|your|this)/i);
    expect(within(dialog).queryByRole("button", { name: /delete/i })).toBeNull();
    expect(spy).not.toHaveBeenCalled();
    await user.click(within(dialog).getByRole("button", { name: "Cancel move" }));
    await waitFor(() => expect(spy).toHaveBeenCalledWith({ sessionId: "sess-codespace-1" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });
});
