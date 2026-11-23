// U-16 over the real link wired to the mock desktop: the session screens with the cloud slots registered.
import { QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { FIXTURE_HANDLERS, FixtureError, type MockDesktop } from "@monoptah/mock-desktop";
import type { CloudField } from "@monoptah/protocol";
import { MemoryPersistAdapter, registerPersistAdapter, type PersistAdapter } from "@/data";
import { DESKTOP_ID, connectMockDesktop, seedWorkspace, type MockLink, type MockLinkOptions } from "@/features/sessions/dev/mockLink";
import { resetSessionActivity } from "@/features/sessions/activityStore";
import { stubTranscriptLayout } from "@/features/transcript/layoutStub";
import { resetCollapsedToolCalls } from "@/features/transcript/toolCallsSetting";
import { resetLinkConnections } from "@/link";
import { clearLinkStores } from "@/link/store.ts";
import { createQueryClient } from "@/lib/queryClient";
import { createAppRouter } from "@/router";
import { clearHooks, registerSendGuard, registerSessionCapabilities } from "@/ui/hooks";
import { DesktopSwitcher } from "@/features/desktops/DesktopSwitcher";
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
let link: MockLink;
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
  link = connectMockDesktop({
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
  return { router, queryClient };
}

const withCloud = (id: string, patch: Partial<CloudField>) => (desktop: MockDesktop) => {
  const record = desktop.world.sessions.get(id)!;
  record.cloud = { ...record.cloud!, ...patch };
};

const top = () => screen.findByTestId("cloud-composer-top");
const field = () => screen.findByRole("textbox", { name: "Message" });

describe("badge, chip and capabilities", () => {
  it("the session list badges a non-local session with its kind and state as text, and nothing for a local one", async () => {
    mount("/p/proj-beta");
    const card = (await screen.findByText("Stopped sandbox work")).closest("div")!.parentElement!;
    expect(await within(card).findByText("Docker sandbox")).toBeInTheDocument();
    expect(within(card).getByText("Stopped")).toBeInTheDocument();
    const remote = screen.getByText("Work on a remote machine").closest("div")!.parentElement!;
    expect(within(remote).getByText("Remote machine")).toBeInTheDocument();
    expect(within(remote).getByText("Available")).toBeInTheDocument();
  });

  it("a local session shows no chip in the list", async () => {
    mount("/p/proj-alpha");
    const local = (await screen.findByText("Fix the example parser")).closest("div")!.parentElement!;
    expect(within(local).queryByTestId("cloud-chip")).toBeNull();
  });

  it("the header chip follows the live state from the watch", async () => {
    mount("/s/sess-codespace-starting");
    const chip = await screen.findByTestId("cloud-chip");
    expect(chip).toHaveTextContent("GitHub codespace");
    expect(chip).toHaveTextContent("Starting");
    for (let i = 0; i < 9; i += 1) await act(async () => void link.desktop.advance("sess-codespace-starting"));
    await waitFor(() => expect(screen.getByTestId("cloud-chip")).toHaveTextContent("Available"));
  });

  it("session-capabilities hides Delete for a cloud session and keeps it for a local one", () => {
    const context = (kind?: string) => ({ target: kind ? { kind } : null, desktopCapabilities: [] });
    for (const capability of ["delete", "files", "changes", "git"] as const) {
      expect(cloudSessionCapabilities(capability, context("sandbox"))).toBe(false);
      expect(cloudSessionCapabilities(capability, context("remote"))).toBe(false);
      expect(cloudSessionCapabilities(capability, context("local"))).toBe(true);
      expect(cloudSessionCapabilities(capability, context())).toBe(true);
    }
  });
});

describe("composer-top states", () => {
  it("starting: the 9 steps with a status in text and the current step announced", async () => {
    mount("/s/sess-codespace-starting");
    const region = await top();
    const steps = within(await within(region).findByRole("list", { name: "Startup steps" })).getAllByRole("listitem");
    expect(steps).toHaveLength(9);
    expect(steps[0]).toHaveTextContent("Done");
    expect(steps[1]).toHaveTextContent("Running");
    expect(steps[2]).toHaveTextContent("Pending");
    expect(within(region).getByText(/Step 2 of 9: Provision resources, running/)).toHaveAttribute("aria-live", "polite");
  });

  it("failed start with kept: the sanitized error as plain text, Retry behind a cost notice, and Stop with the warning", async () => {
    const user = userEvent.setup();
    const retried = vi.fn();
    const stopped = vi.fn();
    FIXTURE_HANDLERS["cloud.retryStart"] = (params) => {
      retried(params);
      return { accepted: true };
    };
    FIXTURE_HANDLERS["cloud.stop"] = (params) => {
      stopped(params);
      return { stopped: true, warning: "Interrupted 1 busy session(s)" };
    };
    mount("/s/sess-sandbox-stopped", {
      seed: withCloud("sess-sandbox-stopped", {
        state: "unavailable",
        kept: true,
        startup: {
          steps: [
            { id: "create", status: "done" },
            { id: "provision", status: "failed" },
          ],
          current: "provision",
          error: "<b>Provisioning failed</b> <img src=x onerror=alert(1)>",
        },
      }),
    });
    const region = await top();
    const alert = await within(region).findByRole("alert");
    expect(alert).toHaveTextContent("<b>Provisioning failed</b>");
    expect(region.querySelector("b, img")).toBeNull();
    expect(within(region).getByText(/still bills/)).toBeInTheDocument();
    const steps = within(within(region).getByRole("list", { name: "Startup steps" })).getAllByRole("listitem");
    expect(steps[1]).toHaveTextContent("Failed");

    await user.click(within(region).getByRole("button", { name: "Retry" }));
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent("billable");
    expect(retried).not.toHaveBeenCalled();
    await user.click(within(dialog).getByRole("button", { name: "Retry start" }));
    await waitFor(() => expect(retried).toHaveBeenCalledWith({ sessionId: "sess-sandbox-stopped" }));

    // BILL I-5: the Stop confirm names the sessions that share the instance before anything is sent.
    await user.click(await within(region).findByRole("button", { name: "Stop instance" }));
    const stopDialog = await screen.findByRole("dialog");
    await waitFor(() => expect(stopDialog).toHaveTextContent(/"Stopped sandbox work"/));
    expect(stopDialog).toHaveTextContent("Stop Docker sandbox?");
    expect(stopped).not.toHaveBeenCalled();
    await user.click(within(stopDialog).getByRole("button", { name: "Stop instance" }));
    await waitFor(() => expect(stopped).toHaveBeenCalledWith({ kind: "sandbox", instance: "fake-sandbox-one" }));
    expect(await screen.findByText("Interrupted 1 busy session(s)")).toBeInTheDocument();
  });

  it("falls back to the generic Stop confirm when the instance list cannot be read", async () => {
    const user = userEvent.setup();
    const stopped = vi.fn();
    FIXTURE_HANDLERS["cloud.instances"] = () => {
      throw new FixtureError("no list", "busy");
    };
    FIXTURE_HANDLERS["cloud.stop"] = (params) => {
      stopped(params);
      return { stopped: true };
    };
    mount("/s/sess-sandbox-stopped", { seed: withCloud("sess-sandbox-stopped", { state: "unavailable", kept: true }) });
    const region = await top();
    await user.click(await within(region).findByRole("button", { name: "Stop instance" }));
    const dialog = await screen.findByRole("dialog");
    await waitFor(() => expect(dialog).toHaveTextContent("The desktop tells you what was interrupted"));
    await user.click(within(dialog).getByRole("button", { name: "Stop instance" }));
    await waitFor(() => expect(stopped).toHaveBeenCalledWith({ sessionId: "sess-sandbox-stopped" }));
  });

  it("stopped: says that sending wakes the instance and that it is billable", async () => {
    mount("/s/sess-sandbox-stopped");
    expect(await screen.findByText("This instance is stopped. Sending wakes it (billable).")).toBeInTheDocument();
  });

  it("auto-stop: announces the time and reason as plain text and Keep running calls cloud.keepRunning", async () => {
    const user = userEvent.setup();
    const kept = vi.fn();
    FIXTURE_HANDLERS["cloud.keepRunning"] = (params, context) => {
      kept(params);
      return (original["cloud.keepRunning"] as never as (p: unknown, c: unknown) => never)(params, context);
    };
    mount("/s/sess-codespace-1", { seed: withCloud("sess-codespace-1", { autoStop: { at: Date.now() + 600_000, reason: "Idle <i>timeout</i>" } }) });
    const region = await top();
    expect(region).toHaveTextContent("Reason: Idle <i>timeout</i>");
    expect(region.querySelector("i")).toBeNull();
    await user.click(within(region).getByRole("button", { name: "Keep running" }));
    await waitFor(() => expect(kept).toHaveBeenCalledWith({ sessionId: "sess-codespace-1" }));
  });

  it("move: shows the 7 move steps", async () => {
    mount("/s/sess-codespace-1", { seed: withCloud("sess-codespace-1", { move: { step: "upload", status: "running" } }) });
    const region = await top();
    const steps = within(within(region).getByRole("list", { name: "Move steps" })).getAllByRole("listitem");
    expect(steps).toHaveLength(7);
    expect(steps[2]).toHaveTextContent("Done");
    expect(steps[3]).toHaveTextContent("Running");
    expect(steps[4]).toHaveTextContent("Pending");
  });

  it("unknown: Not connected", async () => {
    mount("/s/sess-codespace-1", { seed: withCloud("sess-codespace-1", { state: "unknown" }) });
    expect(await screen.findByText(/Not connected\. The desktop cannot reach/)).toBeInTheDocument();
  });

  it("an available instance shows nothing above the composer", async () => {
    mount("/s/sess-codespace-1");
    await field();
    expect(screen.queryByTestId("cloud-composer-top")).toBeNull();
  });

  it("a read-only device sees the controls disabled with the explanation", async () => {
    mount("/s/sess-codespace-1", { permission: "read", seed: withCloud("sess-codespace-1", { autoStop: { at: Date.now() + 600_000, reason: "Idle" } }) });
    const region = await top();
    expect(within(region).getByRole("button", { name: "Keep running" })).toBeDisabled();
    expect(region).toHaveTextContent("read-only access");
  });

  it("without the cloud capability nothing is shown", async () => {
    mount("/s/sess-local-1", { capabilities: ["approvals"] });
    await field();
    expect(screen.queryByTestId("cloud-composer-top")).toBeNull();
    expect(screen.queryByRole("link", { name: "Cloud" })).toBeNull();
  });
});

describe("archive dialog", () => {
  const openArchive = async (user: ReturnType<typeof userEvent.setup>) => {
    await user.click(await screen.findByRole("button", { name: "Session actions" }));
    await user.click(screen.getByRole("menuitem", { name: "Archive" }));
    return screen.findByRole("dialog");
  };

  it("a local session keeps the plain confirmation and sends no cloud choice", async () => {
    const user = userEvent.setup();
    const archived = vi.fn();
    FIXTURE_HANDLERS["session.archive"] = (params, context) => {
      archived(params);
      return (original["session.archive"] as never as (p: unknown, c: unknown) => never)(params, context);
    };
    mount("/s/sess-local-1");
    const dialog = await openArchive(user);
    expect(dialog).toHaveTextContent('"Fix the example parser" moves out of the list. It stays on your desktop.');
    expect(within(dialog).queryByRole("radio")).toBeNull();
    await user.click(within(dialog).getByRole("button", { name: "Archive" }));
    await waitFor(() => expect(archived).toHaveBeenCalledWith({ conversationId: "sess-local-1" }));
  });

  it.each([
    ["Keep instance running", "keep"],
    ["Stop instance", "stop"],
  ])("a cloud session: %s sends cloud=%s with a cost notice and no delete choice", async (label, value) => {
    const user = userEvent.setup();
    const archived = vi.fn();
    FIXTURE_HANDLERS["session.archive"] = (params, context) => {
      archived(params);
      return (original["session.archive"] as never as (p: unknown, c: unknown) => never)(params, context);
    };
    mount("/s/sess-codespace-1");
    const dialog = await openArchive(user);
    expect(dialog).toHaveTextContent("may wake the instance");
    expect(within(dialog).queryByText(/delete/i)).toBeNull();
    await user.click(within(dialog).getByRole("radio", { name: new RegExp(label) }));
    await user.click(within(dialog).getByRole("button", { name: "Archive" }));
    await waitFor(() => expect(archived).toHaveBeenCalledWith({ conversationId: "sess-codespace-1", cloud: value }));
  });

  it("a cloud session on a read-only device cannot archive", async () => {
    const user = userEvent.setup();
    mount("/s/sess-codespace-1", { permission: "read" });
    // The header menu hides Archive for read-only devices; the dialog itself is disabled as well.
    await user.click(await screen.findByRole("button", { name: "Session actions" }));
    expect(screen.queryByRole("menuitem", { name: "Archive" })).toBeNull();
  });
});

describe("composer-top: more states (SPEC M-4, M-5, X-10, BILL M-1, M-9)", () => {
  it("stopping: says the instance is stopping and that a send wakes it again", async () => {
    mount("/s/sess-codespace-1", { seed: withCloud("sess-codespace-1", { state: "stopping" }) });
    const region = await top();
    expect(region).toHaveTextContent("This instance is stopping.");
    expect(region).toHaveTextContent("billable");
  });

  it("unavailable without kept: says so, offers neither Retry nor Stop", async () => {
    mount("/s/sess-codespace-1", { seed: withCloud("sess-codespace-1", { state: "unavailable" }) });
    const region = await top();
    expect(region).toHaveTextContent("This instance is unavailable right now.");
    expect(within(region).queryByRole("button", { name: "Stop instance" })).toBeNull();
    expect(within(region).queryByRole("button", { name: "Retry" })).toBeNull();
  });

  it("failed start without kept: Retry behind a cost notice, and no Stop", async () => {
    mount("/s/sess-sandbox-stopped", {
      seed: withCloud("sess-sandbox-stopped", {
        state: "unavailable",
        startup: { steps: [{ id: "create", status: "failed" }], current: "create", error: "Could not create it" },
      }),
    });
    const region = await top();
    expect(await within(region).findByRole("alert")).toHaveTextContent("Could not create it");
    expect(within(region).getByRole("button", { name: "Retry" })).toBeEnabled();
    expect(within(region).queryByRole("button", { name: "Stop instance" })).toBeNull();
    expect(region).not.toHaveTextContent("still bills");
  });

  it("untrusted desktop text is capped and loses hidden characters (startup error, auto-stop reason)", async () => {
    mount("/s/sess-codespace-1", {
      seed: withCloud("sess-codespace-1", {
        state: "unavailable",
        startup: { steps: [{ id: "create", status: "failed" }], current: "create", error: `Failed ‮ gnp.exe ${"x".repeat(900)}` },
        autoStop: { at: Date.now() + 600_000, reason: `⁦Idle⁩ ${"y".repeat(900)}` },
      }),
    });
    const region = await top();
    const alert = await within(region).findByRole("alert");
    expect(alert.textContent).not.toMatch(/[‮⁦⁩]/);
    expect(alert.textContent!.length).toBeLessThanOrEqual(200);
    expect(region.textContent).not.toMatch(/[‮⁦⁩]/);
    expect(region.textContent).not.toContain("y".repeat(300));
  });

  it("the startup list shows durations of finished steps and highlights the current step", async () => {
    const at = Date.now();
    mount("/s/sess-codespace-starting", {
      seed: withCloud("sess-codespace-starting", {
        state: "starting",
        startup: {
          steps: [
            { id: "create", status: "done", startedAt: at - 20_000, finishedAt: at - 12_000 },
            { id: "provision", status: "running", startedAt: at - 12_000 },
          ],
          current: "provision",
        },
      }),
    });
    const region = await top();
    const steps = within(await within(region).findByRole("list", { name: "Startup steps" })).getAllByRole("listitem");
    expect(steps[0]).toHaveTextContent("Done - 8 s");
    expect(steps[1]).toHaveAttribute("aria-current", "step");
    expect(steps[1]).toHaveTextContent(/Running - \d+ s/);
    expect(steps[0]).not.toHaveAttribute("aria-current");
  });

  it("Keep running carries a billing note", async () => {
    mount("/s/sess-codespace-1", { seed: withCloud("sess-codespace-1", { autoStop: { at: Date.now() + 600_000, reason: "Idle" } }) });
    expect(await top()).toHaveTextContent("Keeping it running keeps billing until it stops.");
  });

  it("the header chip announces a state change in a live region", async () => {
    mount("/s/sess-codespace-starting");
    const announce = async (text: string) => await waitFor(() => expect(screen.getAllByRole("status").some((node) => node.textContent === text && node.className.includes("sr-only"))).toBe(true));
    await screen.findByTestId("cloud-chip");
    // Not on mount: the region is empty until the state changes.
    expect(screen.getAllByRole("status").some((node) => node.textContent === "GitHub codespace: Starting")).toBe(false);
    for (let i = 0; i < 9; i += 1) await act(async () => void link.desktop.advance("sess-codespace-starting"));
    await announce("GitHub codespace: Available");
  });

  it("the waking note appears only after this device sent, not when another device resumes the instance", async () => {
    mount("/s/sess-sandbox-stopped");
    await screen.findByText("This instance is stopped. Sending wakes it (billable).");
    await act(async () => void link.desktop.world.resume("sess-sandbox-stopped"));
    expect(await screen.findByRole("list", { name: "Startup steps" })).toBeInTheDocument();
    expect(screen.queryByText(/Waking the instance/)).toBeNull();
  });
});

describe("send guard with the live conversation state (BILL I-1, SPEC I-1)", () => {
  it("asks before sending when the conversation says stopped although the catalog card says available", async () => {
    const user = userEvent.setup();
    mount("/s/sess-codespace-1", { seed: withCloud("sess-codespace-1", { state: "stopped" }) });
    await screen.findByText("This instance is stopped. Sending wakes it (billable).");
    await user.type(await field(), "hello{Enter}");
    expect(await screen.findByRole("dialog")).toHaveTextContent("billable");
  });

  it("asks before sending to an instance that is not connected", async () => {
    const user = userEvent.setup();
    mount("/s/sess-codespace-1", { seed: withCloud("sess-codespace-1", { state: "unknown" }) });
    await screen.findByText(/Not connected\. The desktop cannot reach/);
    await user.type(await field(), "hello{Enter}");
    expect(await screen.findByRole("dialog")).toHaveTextContent("cannot reach this instance");
  });

  it("asks before sending to a stopping instance", async () => {
    const user = userEvent.setup();
    mount("/s/sess-codespace-1", { seed: withCloud("sess-codespace-1", { state: "stopping" }) });
    await screen.findByText(/This instance is stopping\./);
    await user.type(await field(), "hello{Enter}");
    expect(await screen.findByRole("dialog")).toHaveTextContent("stopping");
  });

  it("refuses a send to an unavailable instance", async () => {
    const user = userEvent.setup();
    const sent = vi.fn();
    FIXTURE_HANDLERS["conversation.send"] = (params) => {
      sent(params);
      return { sessionId: "sess-codespace-1" };
    };
    mount("/s/sess-codespace-1", { seed: withCloud("sess-codespace-1", { state: "unavailable" }) });
    await screen.findByText(/This instance is unavailable right now\./);
    await user.type(await field(), "hello{Enter}");
    expect(await screen.findByText("This instance is unavailable right now. Check it on your desktop, then send again.")).toBeInTheDocument();
    expect(sent).not.toHaveBeenCalled();
  });
});

describe("Stop dialog stays the instance dialog (round 2, item 5)", () => {
  it("keeps naming the sharing sessions when a refetch drops the instance row", async () => {
    const user = userEvent.setup();
    const { queryClient } = mount("/s/sess-sandbox-stopped", { seed: withCloud("sess-sandbox-stopped", { state: "unavailable", kept: true }) });
    const region = await top();
    await user.click(await within(region).findByRole("button", { name: "Stop instance" }));
    const dialog = await screen.findByRole("dialog");
    await waitFor(() => expect(dialog).toHaveTextContent(/"Stopped sandbox work"/));
    FIXTURE_HANDLERS["cloud.instances"] = (() => ({ instances: [] })) as never;
    await act(async () => void (await queryClient.invalidateQueries()));
    expect(screen.getByRole("dialog")).toHaveTextContent(/"Stopped sandbox work"/);
  });
});

describe("archive of a remote session (SPEC M-1)", () => {
  it("gets the plain confirmation and sends no cloud choice", async () => {
    const user = userEvent.setup();
    const archived = vi.fn();
    FIXTURE_HANDLERS["session.archive"] = (params, context) => {
      archived(params);
      return (original["session.archive"] as never as (p: unknown, c: unknown) => never)(params, context);
    };
    mount("/s/sess-remote-1");
    await user.click(await screen.findByRole("button", { name: "Session actions" }));
    await user.click(screen.getByRole("menuitem", { name: "Archive" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).queryByRole("radio")).toBeNull();
    expect(dialog).not.toHaveTextContent("billing");
    await user.click(within(dialog).getByRole("button", { name: "Archive" }));
    await waitFor(() => expect(archived).toHaveBeenCalledWith({ conversationId: "sess-remote-1" }));
  });
});

describe("navigation", () => {
  it("adds a Cloud entry to the desktop nav when cloud is advertised, and not otherwise", async () => {
    const user = userEvent.setup();
    registerSlot("desktop-switcher", DesktopSwitcher);
    mount("/p/proj-alpha");
    await user.click(await screen.findByRole("button", { name: "Open workspace list" }));
    const entry = await screen.findByRole("link", { name: "Cloud" });
    expect(entry).toHaveAttribute("href", `/d/${DESKTOP_ID}/cloud`);
  });

  it("closes the drawer when the Cloud entry is tapped", async () => {
    const user = userEvent.setup();
    registerSlot("desktop-switcher", DesktopSwitcher);
    const { router } = mount("/p/proj-alpha");
    await user.click(await screen.findByRole("button", { name: "Open workspace list" }));
    await user.click(await screen.findByRole("link", { name: "Cloud" }));
    await waitFor(() => expect(router.state.location.pathname).toBe(`/d/${DESKTOP_ID}/cloud`));
    await waitFor(() => expect(screen.getByRole("button", { name: /^Open workspace list/, hidden: true })).toHaveAttribute("aria-expanded", "false"));
  });

  it("has no Cloud entry when the desktop does not advertise cloud", async () => {
    const user = userEvent.setup();
    registerSlot("desktop-switcher", DesktopSwitcher);
    mount("/p/proj-alpha", { capabilities: ["approvals"] });
    await user.click(await screen.findByRole("button", { name: "Open workspace list" }));
    await screen.findByRole("link", { name: "Desktop settings" });
    expect(screen.queryByRole("link", { name: "Cloud" })).toBeNull();
  });
});

describe("send to a stopped instance", () => {
  it("asks for confirmation, then shows waking and queued, and the reply arrives through the watch", async () => {
    const user = userEvent.setup();
    mount("/s/sess-sandbox-stopped");
    await screen.findByText("This instance is stopped. Sending wakes it (billable).");
    await user.type(await field(), "Wake up and run{Enter}");
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent("billable");
    expect(JSON.stringify(link.desktop.world.sessions.get("sess-sandbox-stopped")!.turns)).not.toContain("Wake up and run");
    await user.click(within(dialog).getByRole("button", { name: "Send" }));
    expect(await screen.findByText(/Waking the instance\. Your message is queued/)).toBeInTheDocument();
    expect(await screen.findByRole("list", { name: "Startup steps" })).toBeInTheDocument();

    // A send while it starts is refused with the starting explanation.
    await user.type(await field(), "Too soon{Enter}");
    expect(await screen.findByText("The instance is still starting. Wait for the steps to finish.")).toBeInTheDocument();

    for (let i = 0; i < 9; i += 1) await act(async () => void link.desktop.advance("sess-sandbox-stopped"));
    expect(await screen.findByText("Canned reply from the mock desktop.", {}, { timeout: 3000 })).toBeInTheDocument();
    await waitFor(() => expect(screen.queryByTestId("cloud-composer-top")).toBeNull());
  });
});
