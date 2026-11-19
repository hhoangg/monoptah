// The saved copy shows at once while the link connects or reconnects, and the only connection UI is a small pill
// (no blocking state). Mounts the REAL routes: the session list, the transcript and the Mono chat, first against a
// connected mock desktop (which fills the persisted cache), then again with a link that never becomes ready.
import { QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { MemoryPersistAdapter, registerPersistAdapter, type PersistAdapter } from "@/data";
import { resetLinkConnections } from "@/link";
import { clearLinkStores, getLinkStore } from "@/link/store.ts";
import { resetSessionActivity } from "@/features/sessions/activityStore";
import { DESKTOP_ID, connectMockDesktop, seedWorkspace } from "@/features/sessions/dev/mockLink";
import { stubTranscriptLayout } from "@/features/transcript/layoutStub";
import { createQueryClient } from "@/lib/queryClient";
import { createAppRouter } from "@/router";
import { clearHooks } from "@/ui/hooks";
// The slots the app registers at startup (the switcher row with the banner, the blocking layers) stay registered.
import "@/features/desktops/register";
import "@/features/monos/register";

class RecordingAdapter extends MemoryPersistAdapter {
  readonly keys = new Set<string>();
  override async set(key: string, value: unknown): Promise<void> {
    this.keys.add(key);
    await super.set(key, value);
  }
}

beforeAll(() => {
  window.matchMedia = ((query: string) => ({ matches: false, media: query, onchange: null, addEventListener() {}, removeEventListener() {}, addListener() {}, removeListener() {}, dispatchEvent: () => false })) as never;
  stubTranscriptLayout();
  Element.prototype.scrollIntoView ??= () => {};
});

let previous: PersistAdapter;
let adapter: RecordingAdapter;
beforeEach(() => {
  localStorage.clear();
  adapter = new RecordingAdapter();
  previous = registerPersistAdapter(adapter);
});
afterEach(() => {
  cleanup();
  resetLinkConnections();
  clearLinkStores();
  registerPersistAdapter(previous);
  resetSessionActivity();
  clearHooks();
  localStorage.clear();
});

const MONO = "mono-example";

function mountRoute(path: string, holdRelay: boolean) {
  connectMockDesktop({
    holdRelay,
    seed: (desktop) => {
      seedWorkspace(desktop);
      const base = desktop.world.sessions.get("sess-local-1")!;
      desktop.world.sessions.set(`mono:${MONO}`, { ...base, id: `mono:${MONO}`, turns: structuredClone(base.turns), approvals: [], script: [] } as never);
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

const SCREENS = [
  { name: "session list", path: "/p/proj-alpha", saved: "Fix the flaky retry test", persisted: "catalog:" },
  { name: "transcript", path: "/s/sess-local-1", saved: "Done. This is canned example output.", persisted: "turns:" },
  { name: "Mono chat", path: `/monos/${MONO}`, saved: "Done. This is canned example output.", persisted: "turns:" },
] as const;

describe.each(SCREENS)("$name with a saved copy and a link that is not ready", ({ path, saved, persisted }) => {
  async function coldStart() {
    // Warm run: a connected desktop fills the persisted cache.
    mountRoute(path, false);
    expect((await screen.findAllByText(saved, { exact: false })).length).toBeGreaterThan(0);
    await waitFor(() => expect(screen.getByTestId("connection-banner")).toBeEmptyDOMElement());
    cleanup(); // Stopping a conversation writes its turns.
    await waitFor(() => expect([...adapter.keys].some((key) => key.startsWith(persisted))).toBe(true));
    resetLinkConnections();
    clearLinkStores();
    // Cold run: the page was killed and reopened; the relay has not answered.
    mountRoute(path, true);
    await waitFor(() => expect(["connecting", "handshaking"]).toContain(getLinkStore(DESKTOP_ID).getState().status));
  }

  it.each(["connecting", "reconnecting"] as const)("shows the saved content at once and only a small %s pill", async (status) => {
    await coldStart();
    getLinkStore(DESKTOP_ID).setState({ status });
    expect((await screen.findAllByText(saved, { exact: false })).length).toBeGreaterThan(0);
    const banner = screen.getByTestId("connection-banner");
    await waitFor(() => expect(banner).toHaveTextContent(status === "connecting" ? "Connecting..." : "Reconnecting..."));
    // No blocking state: no modal layer, no full-screen connecting copy.
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(screen.queryByText(/^Connecting\.? ?$/)).toBeNull();
    expect(screen.queryByText("Requesting access...")).toBeNull();
    expect(screen.queryByText("Confirm this device on your desktop")).toBeNull();
  });
});

describe("a screen with a composer keeps sending blocked, but writing stays open", () => {
  it("the transcript composer shows the short placeholder while connecting and Send waits", async () => {
    mountRoute("/s/sess-local-1", false);
    await screen.findAllByText("Done. This is canned example output.", { exact: false });
    cleanup();
    await waitFor(() => expect([...adapter.keys].some((key) => key.startsWith("turns:"))).toBe(true));
    resetLinkConnections();
    clearLinkStores();
    mountRoute("/s/sess-local-1", true);
    const field = await screen.findByRole("textbox", { name: "Message" });
    expect(field).toHaveAttribute("placeholder", "Reconnecting to desktop...");
    expect(screen.getByRole("button", { name: "Send" })).toBeDisabled();
  });
});
