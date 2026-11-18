// WS-Q: the wake consent over the real link wired to the mock desktop. The desktop answers a send, a resume and a
// resume at reset to a stopped instance with `wake_required` unless the request carries `wake: true`; the web
// sends it only after the user confirmed the cost notice (X-11, X-15, U-16), once per confirmation, and the composer
// keeps the draft until the send is accepted.
import { QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { FIXTURE_HANDLERS, FixtureError, type MockDesktop } from "@monoptah/mock-desktop";
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
import { cloudSendGuard, cloudSessionCapabilities } from "./guard";
import { clearLivePlacements } from "./liveState";
import { ArchiveDialogCloud, CardBadgeCloud, ComposerTopCloud, HeaderChipCloud, NavItemsCloud } from "./slotComponents";

beforeAll(() => {
  window.matchMedia = ((query: string) => ({
    matches: query.includes("min-width: 640px") && query.includes("pointer: fine"),
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
  registerSlot("archive-dialog", ArchiveDialogCloud);
  registerSlot("desktop-nav-items", NavItemsCloud);
  registerSendGuard(cloudSendGuard);
  registerSessionCapabilities(cloudSessionCapabilities);
});
afterEach(() => {
  Object.assign(FIXTURE_HANDLERS, original);
  resetLinkConnections();
  clearLinkStores();
  registerPersistAdapter(previous);
  resetSessionActivity();
  clearSlots();
  clearHooks();
  clearLivePlacements();
  localStorage.clear();
});

function mount(path: string, options: MockLinkOptions = {}) {
  connectMockDesktop({
    ...options,
    seed: (desktop) => {
      seedWorkspace(desktop);
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

const field = () => screen.findByRole("textbox", { name: "Message" });

type Recorded = "conversation.send" | "session.resumeUsage" | "session.resumeUsageAtReset";

/** Records the params of every call to a fixture handler, runs `before` (it may throw), then the real handler. */
function record(method: Recorded, before?: (params: Record<string, unknown>) => void) {
  const calls: Array<Record<string, unknown>> = [];
  const real = FIXTURE_HANDLERS[method] as (params: Record<string, unknown>, context: unknown) => unknown;
  (FIXTURE_HANDLERS as Record<string, unknown>)[method] = (params: Record<string, unknown>, context: unknown) => {
    calls.push(params);
    before?.(params);
    return real(params, context);
  };
  return calls;
}

const refuseWithoutWake = (params: Record<string, unknown>) => {
  if (params["wake"] !== true) throw new FixtureError("Sending wakes the stopped cloud instance", "wake_required");
};

describe("a send after the cost notice", () => {
  it("sends wake: true exactly once, only after the user confirms, and clears the draft once it is accepted", async () => {
    const user = userEvent.setup();
    mount("/s/sess-sandbox-stopped");
    const calls = record("conversation.send");
    await user.type(await field(), "Wake up and run{Enter}");
    const dialog = await screen.findByRole("dialog");
    expect(calls).toHaveLength(0);
    await user.click(within(dialog).getByRole("button", { name: "Send" }));
    expect(await screen.findByText(/Waking the instance\. Your message is queued/)).toBeInTheDocument();
    expect(calls).toHaveLength(1);
    expect(calls[0]).toMatchObject({ conversationId: "sess-sandbox-stopped", text: "Wake up and run", wake: true });
    await waitFor(() => expect(screen.getByRole("textbox", { name: "Message" })).toHaveValue(""));
  });

  it("cancelling the notice sends nothing and keeps the draft", async () => {
    const user = userEvent.setup();
    mount("/s/sess-sandbox-stopped");
    const calls = record("conversation.send");
    await user.type(await field(), "Keep me{Enter}");
    await user.click(within(await screen.findByRole("dialog")).getByRole("button", { name: /Cancel/ }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(calls).toHaveLength(0);
    expect(screen.getByRole("textbox", { name: "Message" })).toHaveValue("Keep me");
  });

  it("a send the guard allows carries no wake", async () => {
    const user = userEvent.setup();
    mount("/s/sess-codespace-1");
    const calls = record("conversation.send");
    await user.type(await field(), "Plain message{Enter}");
    await waitFor(() => expect(calls).toHaveLength(1));
    expect(calls[0]).not.toHaveProperty("wake");
    expect(screen.queryByRole("dialog")).toBeNull();
  });
});

describe("wake_required although the web thought the instance was available", () => {
  it("shows the cost notice (not a generic error), keeps the draft, and resends with wake: true once after the confirmation", async () => {
    const user = userEvent.setup();
    mount("/s/sess-codespace-1");
    const calls = record("conversation.send", refuseWithoutWake);
    await user.type(await field(), "Stale cache message{Enter}");
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent("billable");
    expect(calls).toHaveLength(1);
    expect(calls[0]).not.toHaveProperty("wake");
    expect(screen.getByRole("textbox", { name: "Message", hidden: true })).toHaveValue("Stale cache message");
    expect(screen.queryByTestId("not-sent")).toBeNull();
    await user.click(within(dialog).getByRole("button", { name: "Send" }));
    await waitFor(() => expect(calls).toHaveLength(2));
    expect(calls[1]).toMatchObject({ text: "Stale cache message", wake: true });
    await waitFor(() => expect(screen.getByRole("textbox", { name: "Message" })).toHaveValue(""));
    expect(calls).toHaveLength(2);
  });

  it("cancelling that notice never resends, keeps the draft and leaves no Not sent turn", async () => {
    const user = userEvent.setup();
    mount("/s/sess-codespace-1");
    const calls = record("conversation.send", refuseWithoutWake);
    await user.type(await field(), "Not now{Enter}");
    await user.click(within(await screen.findByRole("dialog")).getByRole("button", { name: /Cancel/ }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(calls).toHaveLength(1);
    expect(screen.getByRole("textbox", { name: "Message" })).toHaveValue("Not now");
    expect(screen.queryByTestId("not-sent")).toBeNull();
  });
});

describe("usage limit resume on a stopped instance", () => {
  const limit = { token: "t1", resetsAt: null, resumeAtReset: false, canResume: true, canAutoResume: true, resetLabel: "Resets later.", limitation: "Limit reached." };
  const seedOn = (id: string) => (desktop: MockDesktop) => {
    desktop.world.sessions.get(id)!.usageLimit = limit;
  };

  it("Resume asks for the wake cost first and sends wake: true once after the confirmation", async () => {
    const user = userEvent.setup();
    mount("/s/sess-sandbox-stopped", { seed: seedOn("sess-sandbox-stopped") });
    const calls = record("session.resumeUsage");
    await user.click(await screen.findByRole("button", { name: /^Resume$/ }));
    const dialog = await screen.findByRole("dialog");
    expect(calls).toHaveLength(0);
    await user.click(within(dialog).getByRole("button", { name: "Send" }));
    await waitFor(() => expect(calls).toHaveLength(1));
    expect(calls[0]).toMatchObject({ conversationId: "sess-sandbox-stopped", token: "t1", wake: true });
  });

  it("Resume at reset sends nothing when the notice is cancelled and carries wake: true after the confirmation", async () => {
    const user = userEvent.setup();
    mount("/s/sess-sandbox-stopped", { seed: seedOn("sess-sandbox-stopped") });
    const calls = record("session.resumeUsageAtReset");
    await user.click(await screen.findByRole("switch", { name: /Resume at reset/ }));
    await user.click(within(await screen.findByRole("dialog")).getByRole("button", { name: /Cancel/ }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(calls).toHaveLength(0);
    await user.click(screen.getByRole("switch", { name: /Resume at reset/ }));
    await user.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Send" }));
    await waitFor(() => expect(calls).toHaveLength(1));
    expect(calls[0]).toMatchObject({ token: "t1", enabled: true, wake: true });
  });

  it("a wake_required answer without a prior confirmation shows the notice and resends with wake: true once", async () => {
    const user = userEvent.setup();
    mount("/s/sess-codespace-1", { seed: seedOn("sess-codespace-1") });
    const calls = record("session.resumeUsage", refuseWithoutWake);
    await user.click(await screen.findByRole("button", { name: /^Resume$/ }));
    const dialog = await screen.findByRole("dialog");
    expect(calls).toHaveLength(1);
    expect(calls[0]).not.toHaveProperty("wake");
    await user.click(within(dialog).getByRole("button", { name: "Send" }));
    await waitFor(() => expect(calls).toHaveLength(2));
    expect(calls[1]).toMatchObject({ wake: true });
  });
});
