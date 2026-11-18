// U-08 over the mock desktop: the approval card in the composer-top slot of `sess-local-approval`.
import { QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { FIXTURE_HANDLERS, type MockDesktop } from "@monoptah/mock-desktop";
import { MemoryPersistAdapter, registerPersistAdapter, type PersistAdapter } from "@/data";
import { stubTranscriptLayout } from "@/features/transcript/layoutStub";
import { DESKTOP_ID, connectMockDesktop, seedWorkspace, type MockLinkOptions } from "@/features/sessions/dev/mockLink";
import { resetSessionActivity } from "@/features/sessions/activityStore";
import { resetLinkConnections } from "@/link";
import { clearLinkStores } from "@/link/store.ts";
import { createQueryClient } from "@/lib/queryClient";
import { createAppRouter } from "@/router";
import { clearHooks } from "@/ui/hooks";
import { clearSlots, registerSlot } from "@/ui/slots";
import { ApprovalCard } from "./index";

beforeAll(() => {
  window.matchMedia = ((query: string) => ({ matches: false, media: query, onchange: null, addEventListener() {}, removeEventListener() {}, addListener() {}, removeListener() {}, dispatchEvent: () => false })) as never;
  stubTranscriptLayout();
  window.scrollTo = vi.fn() as never;
});

const original = { ...FIXTURE_HANDLERS };
let previous: PersistAdapter;
beforeEach(() => {
  localStorage.clear();
  previous = registerPersistAdapter(new MemoryPersistAdapter());
  vi.stubEnv("VITE_WEB_APPROVALS", "on");
  registerSlot("composer-top", (props) => <ApprovalCard sessionId={String(props.sessionId)} />);
});
afterEach(() => {
  vi.unstubAllEnvs();
  Object.assign(FIXTURE_HANDLERS, original);
  resetLinkConnections();
  clearLinkStores();
  registerPersistAdapter(previous);
  resetSessionActivity();
  clearSlots();
  clearHooks();
  localStorage.clear();
});

const SESSION = "sess-local-approval";

function mount(options: MockLinkOptions = {}) {
  const link = connectMockDesktop({
    ...options,
    seed: (desktop) => {
      seedWorkspace(desktop);
      options.seed?.(desktop);
    },
  });
  const queryClient = createQueryClient();
  const router = createAppRouter({ history: createMemoryHistory({ initialEntries: [`/d/${DESKTOP_ID}/s/${SESSION}`] }), queryClient });
  render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return link;
}

/** Records every `session.answerApproval` the desktop receives. */
function spyAnswers() {
  const calls: unknown[] = [];
  const handler = original["session.answerApproval"] as (params: unknown, context: unknown) => unknown;
  (FIXTURE_HANDLERS as Record<string, unknown>)["session.answerApproval"] = (params: unknown, context: unknown) => {
    calls.push(params);
    return handler(params, context);
  };
  return calls;
}

describe("approval card over the mock desktop (U-08)", () => {
  it("answers once and clears through the next watch frame", async () => {
    const user = userEvent.setup();
    const calls = spyAnswers();
    mount();
    const region = await screen.findByRole("region", { name: "Needs your approval" });
    expect(region).toHaveTextContent("Run example command");
    expect(region).toHaveTextContent("example-cli --check (fake detail)");
    await waitFor(() => expect(screen.getByRole("button", { name: "Allow once" })).toBeEnabled(), { timeout: 3000 });
    await user.dblClick(screen.getByRole("button", { name: "Allow once" }));
    await waitFor(() => expect(screen.queryByRole("region", { name: "Needs your approval" })).toBeNull());
    expect(calls).toEqual([{ conversationId: SESSION, requestId: "41", decision: "allow" }]);
  });

  it("sends deny", async () => {
    const user = userEvent.setup();
    const calls = spyAnswers();
    mount();
    const deny = await screen.findByRole("button", { name: "Deny" });
    await waitFor(() => expect(deny).toBeEnabled(), { timeout: 3000 });
    await user.click(deny);
    await waitFor(() => expect(calls).toHaveLength(1));
    expect(calls[0]).toEqual({ conversationId: SESSION, requestId: "41", decision: "deny" });
  });

  it("renders hostile provider text as inert text", async () => {
    const seed = (desktop: MockDesktop) => {
      desktop.world.session(SESSION).approvals = [{ requestId: "41", label: "<img src=x onerror=alert(1)>", detail: "[x](javascript:alert(1))\n<script>alert(1)</script>" }];
    };
    mount({ seed });
    const region = await screen.findByRole("region", { name: "Needs your approval" });
    expect(region.querySelector("img, script, a, [href]")).toBeNull();
    expect(region).toHaveTextContent("<img src=x onerror=alert(1)>");
    expect(region).toHaveTextContent("[x](javascript:alert(1))");
  });

  it("is disabled with an explanation on a read-only device and sends nothing", async () => {
    const user = userEvent.setup();
    const calls = spyAnswers();
    mount({ permission: "read" });
    const allow = await screen.findByRole("button", { name: "Allow once" });
    expect(allow).toBeDisabled();
    expect(screen.getByRole("button", { name: "Deny" })).toBeDisabled();
    await new Promise((resolve) => setTimeout(resolve, 900));
    expect(allow).toBeDisabled();
    expect(screen.getByText("Approvals need a device with control access.")).toBeInTheDocument();
    await user.click(allow);
    expect(calls).toHaveLength(0);
  });

  it("renders nothing while the kill switch is set", async () => {
    vi.stubEnv("VITE_WEB_APPROVALS", "off");
    mount();
    await screen.findByText("Run the example checks");
    expect(screen.queryByRole("region", { name: "Needs your approval" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Allow once" })).toBeNull();
  });

  it("renders nothing when the desktop does not advertise approvals", async () => {
    mount({ capabilities: ["cloud"] });
    await screen.findByText("Run the example checks");
    expect(screen.queryByRole("region", { name: "Needs your approval" })).toBeNull();
  });
});
