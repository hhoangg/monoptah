// W-07: the session route against the real link wired to the mock desktop (dev/mockLink.ts).
import { QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { MemoryPersistAdapter, registerPersistAdapter, type PersistAdapter } from "@/data";
import { resetLinkConnections } from "@/link";
import { clearLinkStores } from "@/link/store.ts";
import { scrollTops, stubTranscriptLayout } from "@/features/transcript/layoutStub";
import { createQueryClient } from "@/lib/queryClient";
import { createAppRouter } from "@/router";
import { clearHooks } from "@/ui/hooks";
import { clearSlots, registerSlot } from "@/ui/slots";
import { resetSessionActivity } from "./activityStore";
import { LONG_SESSION_ID, seedLongSession } from "./dev/longSession";
import { DESKTOP_ID, connectMockDesktop, seedWorkspace, type MockLink, type MockLinkOptions } from "./dev/mockLink";
import { lastPlaceKey } from "./lastPlace";

beforeAll(() => {
  window.matchMedia = ((query: string) => ({ matches: false, media: query, onchange: null, addEventListener() {}, removeEventListener() {}, addListener() {}, removeListener() {}, dispatchEvent: () => false })) as never;
  stubTranscriptLayout();
});

let previous: PersistAdapter;
let link: MockLink;
beforeEach(() => {
  localStorage.clear();
  previous = registerPersistAdapter(new MemoryPersistAdapter());
});
afterEach(() => {
  resetLinkConnections();
  clearLinkStores();
  registerPersistAdapter(previous);
  resetSessionActivity();
  clearSlots();
  clearHooks();
  localStorage.clear();
});

const SESSION = "sess-local-1";

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

describe("session screen (W-07)", () => {
  it("shows the title, the conversation and the composer region with its slot host", async () => {
    registerSlot("composer-top", (props) => <div data-testid="composer-top-content">top of {String(props.sessionId)}</div>);
    registerSlot("session-header-chip", () => <span data-testid="chip">cloud chip</span>);
    mount(`/s/${SESSION}`);
    expect(await screen.findByText("Done. This is canned example output.")).toBeInTheDocument();
    expect(screen.getByRole("heading", { level: 1 }).textContent).not.toBe("");
    const region = screen.getByTestId("composer-region");
    expect(within(region).getByTestId("composer-top-content")).toHaveTextContent(`top of ${SESSION}`);
    expect(await screen.findByTestId("chip")).toBeInTheDocument();
    expect(screen.getByText("Please look at the example task.")).toBeInTheDocument();
  });

  it("goes back to the project of the session", async () => {
    mount(`/s/${SESSION}`);
    const back = await screen.findByRole("link", { name: "Back" });
    await waitFor(() => expect(back.getAttribute("href")).toMatch(new RegExp(`^/d/${DESKTOP_ID}/p/`)));
  });

  it("remembers the place for `/` (F-14)", async () => {
    mount(`/s/${SESSION}`);
    await screen.findByText("Done. This is canned example output.");
    await waitFor(() => expect(localStorage.getItem(lastPlaceKey(DESKTOP_ID))).toBe(`/d/${DESKTOP_ID}/s/${SESSION}`));
  });

  it("renders every turn item type the desktop sends, and an error notice opens its dialog", async () => {
    const user = userEvent.setup();
    mount(`/s/${SESSION}`, {
      seed: (desktop) => {
        const record = desktop.world.sessions.get(SESSION)!;
        record.turns.push({
          id: "t3",
          footer: true,
          copyText: "Final answer",
          completedAt: Date.now(),
          items: [
            { type: "header", id: "h", live: false, durationMs: 65_000, model: "Example", entries: [{ type: "prose", id: "e", text: "Thinking it through." }] },
            { type: "work", id: "w", live: false, phases: [{ id: "p", kind: "run", title: "Ran the tests", steps: [{ id: "s", label: "npm test" }] }] },
            { type: "text", id: "x", text: "Here is `code`:\n\n```ts\nconst a = 1;\n```\n" },
            { type: "notice", id: "n", text: "rate limited", error: true },
            { type: "sessions", id: "ss", sessions: [{ id: "sess-seed-0", projectId: "gone-project", project: "Gone", title: "Launched one", harness: "claude", state: "done" }] },
            { type: "choices", id: "c", options: ["Ship it", "Wait"] },
            { type: "document", id: "d", documentId: "doc1", title: "Plan", kind: "md" },
            { type: "hologram", id: "z", text: "unknown" },
          ],
        } as never);
      },
    });
    expect(await screen.findByText("Worked for 1m 5s", { exact: false })).toBeInTheDocument();
    expect(screen.getByText("Ran the tests")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Copy response" })).toBeInTheDocument();
    expect(screen.getByTestId("launched-session-unavailable")).toHaveTextContent("Not on this device's projects");
    expect(screen.getByRole("button", { name: "Ship it" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Open document Plan" })).toBeInTheDocument();
    expect(screen.queryByText("unknown")).toBeNull();
    await user.click(screen.getByRole("button", { name: "Couldn't continue. View details" }));
    expect(await screen.findByTestId("session-error-dialog")).toHaveTextContent("rate limited");
  });

  it("opens a file named in a reply on the files route of the session's project", async () => {
    const user = userEvent.setup();
    const { router } = mount(`/s/${SESSION}`, {
      seed: (desktop) => {
        desktop.world.sessions.get(SESSION)!.turns.push({ id: "t3", items: [{ type: "text", id: "x", text: "I changed `src/app/main.ts` today." }] } as never);
      },
    });
    await user.click(await screen.findByRole("button", { name: "src/app/main.ts" }));
    await waitFor(() => expect(router.state.location.pathname).toMatch(new RegExp(`^/d/${DESKTOP_ID}/p/[^/]+/files/src/app/main.ts$`)));
  });

  it("opens a document card in a dialog, and never renders a visual document", async () => {
    const user = userEvent.setup();
    mount(`/s/${SESSION}`, {
      seed: (desktop) => {
        desktop.world.sessions.get(SESSION)!.turns.push({
          id: "t3",
          items: [
            { type: "document", id: "d1", documentId: "doc1", title: "Plan", kind: "md" },
            { type: "document", id: "d2", documentId: "doc2", title: "Chart", kind: "visual" },
          ],
        } as never);
      },
    });
    await user.click(await screen.findByRole("button", { name: "Open document Plan" }));
    const dialog = await screen.findByTestId("document-dialog");
    expect(within(dialog).getByText("Plan")).toBeInTheDocument();
    // The mock desktop's document carries no text: the dialog says to open it on the desktop.
    expect(await within(dialog).findByText("Open this document on your desktop to read it.")).toBeInTheDocument();
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByTestId("document-dialog")).toBeNull());
    await user.click(screen.getByRole("button", { name: "Open visual Chart" }));
    expect(await screen.findByText("This is a visual document. Open it on your desktop to see it.")).toBeInTheDocument();
  });

  it("sends a suggested reply through the data layer", async () => {
    const user = userEvent.setup();
    mount(`/s/${SESSION}`, {
      seed: (desktop) => {
        desktop.world.sessions.get(SESSION)!.turns.push({ id: "t3", items: [{ type: "choices", id: "c", options: ["Ship it"] }] } as never);
      },
    });
    await user.click(await screen.findByRole("button", { name: "Ship it" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "Ship it" })).toHaveAttribute("aria-pressed", "true"));
    await waitFor(() => expect(link.desktop.world.sessions.get(SESSION)!.turns.some((t) => JSON.stringify(t).includes("Ship it") && t.id !== "t3")).toBe(true));
  });

  it("disables suggested replies on a read-only device", async () => {
    mount(`/s/${SESSION}`, {
      permission: "read",
      seed: (desktop) => {
        desktop.world.sessions.get(SESSION)!.turns.push({ id: "t3", items: [{ type: "choices", id: "c", options: ["Ship it"] }] } as never);
      },
    });
    expect(await screen.findByRole("button", { name: "Ship it" })).toBeDisabled();
    expect(screen.getByTestId("read-only-chip")).toBeInTheDocument();
  });
});

describe("older turns over the link (F-32)", () => {
  it("pages earlier turns in when the reader reaches the top and keeps the place", async () => {
    mount(`/s/${LONG_SESSION_ID}`, { seed: (desktop) => seedLongSession(desktop, { total: 60, tail: 12 }) });
    // Opens on the tail: the last of 60 turns is there, the first is not.
    expect(await screen.findByText("Next: part 59", { exact: false })).toBeInTheDocument();
    expect(screen.queryByText("Next: part 10", { exact: false })).toBeNull();
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 40));
    });
    const scroller = screen.getByTestId("transcript-scroller");
    scrollTops.set(scroller, 200);
    fireEvent.scroll(scroller);
    // 12 turns are held (6 exchanges); a page of 20 brings 10 more exchanges above them.
    await waitFor(() => expect(scroller.scrollTop).toBeGreaterThan(1000), { timeout: 4000 });
    // The turns just above the reader are in the window, those far above are not rendered.
    expect(await screen.findByText("Next: part 53", { exact: false })).toBeInTheDocument();
    expect(screen.queryByText("Next: part 44", { exact: false })).toBeNull();
    expect(screen.queryByRole("alert")).toBeNull();
  });
});
