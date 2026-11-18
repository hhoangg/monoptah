// U-18 over the real link wired to the mock desktop: the target picker on the New session page, with the
// cloud slots and the send-guard registered as main.tsx does through `features/cloud/register.ts`.
import { QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { FIXTURE_HANDLERS, FixtureError } from "@monoptah/mock-desktop";
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
import { cloudSendGuard, cloudSessionCapabilities } from "../session/guard";
import { NewSessionTargetCloud } from "../session/slotComponents";
import { buildTarget, parseTarget } from "./target";

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
  registerSlot("new-session-target", NewSessionTargetCloud);
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
  localStorage.clear();
});

const NEW = "/p/proj-alpha/new";

function mount(options: MockLinkOptions = {}, remoteCloudStart = true) {
  connectMockDesktop({
    ...options,
    seed: (desktop) => {
      seedWorkspace(desktop);
      desktop.flags = { remoteCloudStart, remoteGitPush: true };
      options.seed?.(desktop);
    },
  });
  const queryClient = createQueryClient();
  const router = createAppRouter({ history: createMemoryHistory({ initialEntries: [`/d/${DESKTOP_ID}${NEW}`] }), queryClient });
  render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return { router };
}

const field = () => screen.findByRole("textbox", { name: "Message" });
const spyStart = () => {
  const spy = vi.fn();
  FIXTURE_HANDLERS["session.start"] = (params, context) => {
    spy(params);
    return (original["session.start"] as (p: unknown, c: unknown) => never)(params, context);
  };
  return spy;
};
const done = async (user: ReturnType<typeof userEvent.setup>, dialog: HTMLElement) => {
  await user.click(within(dialog).getByRole("button", { name: "Done" }));
  await waitFor(() => expect(screen.queryByRole("dialog", { name: "Where it runs" })).toBeNull());
};
const openPicker = async (user: ReturnType<typeof userEvent.setup>) => {
  await user.click(await screen.findByTestId("target-chip"));
  return screen.findByRole("dialog", { name: "Where it runs" });
};

describe("target helpers", () => {
  it("buildTarget only produces keys of SessionStartTargetSchema and the result strict-parses", () => {
    const targets = [
      buildTarget({ kind: "sandbox" }),
      buildTarget({ kind: "sandbox", size: "large", branch: "main" }),
      buildTarget({ kind: "codespace", machine: "m1", size: "ignored" }),
      buildTarget({ kind: "codespace", instance: "existing-one", machine: "dropped for an existing instance" }),
    ];
    for (const target of targets) expect(SessionStartTargetSchema.safeParse(target).success).toBe(true);
    expect(targets[1]).toEqual({ kind: "sandbox", instance: "new", size: "large", branch: "main" });
    expect(targets[2]).toEqual({ kind: "codespace", instance: "new", machine: "m1" });
    expect(targets[3]).toEqual({ kind: "codespace", instance: "existing-one" });
  });

  it("parseTarget treats anything that is not a valid target as local", () => {
    expect(parseTarget(null)).toBeNull();
    expect(parseTarget({ kind: "sandbox", image: "x" })).toBeNull();
    expect(parseTarget({ kind: "sandbox" })).toEqual({ kind: "sandbox" });
  });
});

describe("new-session-target", () => {
  it("is hidden when the desktop does not advertise cloud or cloudStart", async () => {
    mount({ capabilities: ["cloud"] });
    await field();
    expect(screen.queryByTestId("target-chip")).toBeNull();
  });

  it("is hidden without the cloud capability", async () => {
    mount({ capabilities: ["cloudStart"] });
    await field();
    expect(screen.queryByTestId("target-chip")).toBeNull();
  });

  it("shows Local by default and offers only the ready backends from startOptions", async () => {
    const user = userEvent.setup();
    FIXTURE_HANDLERS["cloud.startOptions"] = () => ({
      backends: [
        { kind: "codespace", ready: false },
        { kind: "sandbox", ready: true },
      ],
      branches: ["main"],
      sizes: [{ id: "small", name: "Small" }],
    });
    mount();
    expect(await screen.findByTestId("target-chip")).toHaveTextContent("Local");
    const dialog = await openPicker(user);
    const group = await within(dialog).findByRole("radiogroup", { name: "Where it runs" });
    const labels = within(group).getAllByRole("radio").map((radio) => radio.textContent);
    expect(labels).toEqual(["Local", "Docker sandbox"]);
  });

  it("offers the branch, size, machine and instance options from the desktop and nothing else", async () => {
    const user = userEvent.setup();
    mount();
    const dialog = await openPicker(user);
    await user.click(await within(dialog).findByRole("radio", { name: "Docker sandbox" }));
    expect(within(dialog).getByText("Cloud sessions run Claude.")).toBeInTheDocument();
    const instance = within(dialog).getByLabelText("Instance");
    expect(within(instance).getByRole("option", { name: "New instance" })).toBeInTheDocument();
    expect(within(instance).getAllByRole("option").length).toBeGreaterThan(1);
    expect(within(dialog).getByLabelText("Branch")).toHaveTextContent("example-branch");
    expect(within(dialog).getByLabelText("Size")).toHaveTextContent("Large");
    expect(within(dialog).queryByLabelText("Machine")).toBeNull();
    await user.click(within(dialog).getByRole("radio", { name: "GitHub codespace" }));
    expect(within(dialog).getByLabelText("Machine")).toHaveTextContent("Example 2-core");
    expect(within(dialog).queryByLabelText("Size")).toBeNull();
    // No control for anything outside the schema.
    for (const forbidden of [/kit/i, /template/i, /image/i, /cpu/i, /memory/i, /ttl/i, /time to live/i]) {
      expect(within(dialog).queryByLabelText(forbidden)).toBeNull();
      expect(within(dialog).queryByText(forbidden)).toBeNull();
    }
  });

  it("a start with a cloud target asks to confirm the cost and sends a target that strict-parses", async () => {
    const user = userEvent.setup();
    const spy = spyStart();
    mount();
    const dialog = await openPicker(user);
    await user.click(await within(dialog).findByRole("radio", { name: "Docker sandbox" }));
    await user.selectOptions(within(dialog).getByLabelText("Branch"), "example-branch");
    await user.selectOptions(within(dialog).getByLabelText("Size"), "large");
    await done(user, dialog);
    expect(await screen.findByTestId("target-chip")).toHaveTextContent("Docker sandbox");
    await user.type(await field(), "Run it in the cloud{Enter}");
    const confirm = await screen.findByRole("dialog");
    expect(confirm).toHaveTextContent(/billable/i);
    expect(spy).not.toHaveBeenCalled();
    await user.click(within(confirm).getAllByRole("button").at(-1)!);
    await waitFor(() => expect(spy).toHaveBeenCalledTimes(1));
    const params = spy.mock.calls[0]![0] as { target: unknown };
    expect(params.target).toEqual({ kind: "sandbox", instance: "new", branch: "example-branch", size: "large" });
    expect(SessionStartTargetSchema.safeParse(params.target).success).toBe(true);
  });

  it("picks an existing instance of the kind", async () => {
    const user = userEvent.setup();
    const spy = spyStart();
    mount();
    const dialog = await openPicker(user);
    await user.click(await within(dialog).findByRole("radio", { name: "Docker sandbox" }));
    const instance = within(dialog).getByLabelText("Instance");
    await waitFor(() => expect(within(instance).getAllByRole("option").length).toBeGreaterThan(1));
    const existing = within(instance).getAllByRole("option").find((option) => option.textContent !== "New instance")!;
    await user.selectOptions(instance, existing);
    await done(user, dialog);
    await user.type(await field(), "Use the existing one{Enter}");
    const confirm = await screen.findByRole("dialog");
    await user.click(within(confirm).getAllByRole("button").at(-1)!);
    await waitFor(() => expect(spy).toHaveBeenCalledTimes(1));
    const target = (spy.mock.calls[0]![0] as { target: { kind: string; instance: string } }).target;
    expect(target.kind).toBe("sandbox");
    expect(target.instance).not.toBe("new");
    expect(SessionStartTargetSchema.safeParse(target).success).toBe(true);
  });

  it("a local start sends no target and no cost confirm", async () => {
    const user = userEvent.setup();
    const spy = spyStart();
    mount();
    await user.type(await field(), "Stay local{Enter}");
    await waitFor(() => expect(spy).toHaveBeenCalledTimes(1));
    expect(spy.mock.calls[0]![0]).not.toHaveProperty("target");
  });

  it("explains that cloud start is off when the desktop answers disabled", async () => {
    const user = userEvent.setup();
    FIXTURE_HANDLERS["cloud.startOptions"] = () => {
      throw new FixtureError("This action is not available from the web", "disabled");
    };
    mount();
    const dialog = await openPicker(user);
    expect(await within(dialog).findByText(/Remote cloud start is turned off/)).toBeInTheDocument();
    expect(within(dialog).queryByRole("radiogroup")).toBeNull();
  });

  it("a read-only device sees the chip disabled", async () => {
    mount({ permission: "read" });
    expect(await screen.findByTestId("target-chip")).toBeDisabled();
  });
});
