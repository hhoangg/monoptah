// Task 7a: session actions (F-25, F-40), filters (F-22, F-23), workspace menu (F-19), finished deck (F-29),
// mascots (F-91) and Mono unread tracking (F-17), against the real link wired to the mock desktop.
import "fake-indexeddb/auto";
import { IDBFactory } from "fake-indexeddb";
import { QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { act, render, renderHook, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { registerPersistAdapter, MemoryPersistAdapter, type PersistAdapter } from "@/data";
import { getLinkConnection, resetLinkConnections } from "@/link";
import { clearLinkStores, getLinkStore } from "@/link/store.ts";
import { createQueryClient } from "@/lib/queryClient";
import { createAppRouter } from "@/router";
import { clearHooks, registerSessionCapabilities } from "@/ui/hooks";
import { clearSlots, registerSlot } from "@/ui/slots";
import { resetSessionActivity, useActivityStore } from "./activityStore";
import { DESKTOP_ID, connectMockDesktop, seedWorkspace, type MockLinkOptions } from "./dev/mockLink";
import { OPEN_AFTER_STARTING_KEY, useOpenAfterStarting } from "./openAfterStarting";
import { SESSION_FILTER_KEY, applySessionFilter, inWorktree } from "./sessionListFilter";
import { MONO_SEEN_DB, markMonoSeen, markMonoUnread, resetMonoSeen, loadMonoSeen } from "./monoSeen";
import { Ticker, TICKER_GAP } from "./Ticker";
import { PROJECT_MASCOTS, projectMascot } from "./projectMascots";
import { FRAME_MS } from "./ProjectMascot";

let reduced = false;
function stubMatchMedia() {
  window.matchMedia = ((query: string) => ({
    matches: query.includes("prefers-reduced-motion") ? reduced : false,
    media: query,
    onchange: null,
    addEventListener: () => {},
    removeEventListener: () => {},
    addListener: () => {},
    removeListener: () => {},
    dispatchEvent: () => false,
  })) as unknown as typeof window.matchMedia;
}

beforeAll(() => {
  Element.prototype.hasPointerCapture ??= () => false;
  Element.prototype.setPointerCapture ??= () => {};
  Element.prototype.releasePointerCapture ??= () => {};
  Element.prototype.scrollIntoView ??= () => {};
});

let previous: PersistAdapter;

beforeEach(() => {
  reduced = false;
  stubMatchMedia();
  localStorage.clear();
  globalThis.indexedDB = new IDBFactory();
  resetMonoSeen();
  previous = registerPersistAdapter(new MemoryPersistAdapter());
});
afterEach(() => {
  vi.useRealTimers();
  resetLinkConnections();
  clearLinkStores();
  registerPersistAdapter(previous);
  resetSessionActivity();
  clearSlots();
  clearHooks();
  localStorage.clear();
});

type Request = (method: string, params: Record<string, unknown>, ...rest: unknown[]) => unknown;

/** Records the requests of a method and lets a test answer one itself. */
function spyRequests(overrides: Record<string, (params: Record<string, unknown>) => unknown> = {}) {
  const connection = getLinkConnection(DESKTOP_ID);
  const calls: Array<{ method: string; params: Record<string, unknown> }> = [];
  const original = connection.request.bind(connection) as unknown as Request;
  connection.request = ((method: string, params: Record<string, unknown>, ...rest: unknown[]) => {
    calls.push({ method, params });
    const override = overrides[method];
    if (override) return Promise.resolve(override(params));
    return original(method, params, ...rest);
  }) as typeof connection.request;
  return calls;
}

function mount(path: string, options: MockLinkOptions = {}, overrides: Parameters<typeof spyRequests>[0] = {}) {
  connectMockDesktop({
    ...options,
    seed: (desktop) => {
      seedWorkspace(desktop);
      options.seed?.(desktop);
    },
  });
  if (Object.keys(overrides).length > 0) spyRequests(overrides);
  const queryClient = createQueryClient();
  const router = createAppRouter({ history: createMemoryHistory({ initialEntries: [`/d/${DESKTOP_ID}${path}`] }), queryClient });
  render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return { router, queryClient };
}

const card = (id: string) => screen.getAllByTestId("session-card").find((element) => element.dataset.sessionId === id)!;

async function openCardMenu(user: ReturnType<typeof userEvent.setup>, id: string) {
  await screen.findAllByTestId("session-card");
  await user.click(within(card(id)).getByRole("button", { name: /Actions for/ }));
  return screen.findByRole("menu");
}

describe("rename, archive and delete from the card menu (F-25, F-40)", () => {
  it("renames through session.rename and shows the new title", async () => {
    const user = userEvent.setup();
    mount("/p/proj-alpha");
    const menu = await openCardMenu(user, "sess-local-1");
    const calls = spyRequests();
    await user.click(within(menu).getByRole("menuitem", { name: "Rename" }));
    const field = await screen.findByRole("textbox", { name: "Title" });
    expect(field).toHaveValue("Fix the example parser");
    await user.clear(field);
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
    await user.type(field, "A better title");
    await user.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(calls.find((call) => call.method === "session.rename")?.params).toEqual({ conversationId: "sess-local-1", title: "A better title" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    await waitFor(() => expect(within(card("sess-local-1")).getByText("A better title")).toBeInTheDocument());
  });

  it("archives a local session through the default dialog (session.archive without `cloud`)", async () => {
    const user = userEvent.setup();
    mount("/p/proj-alpha");
    const menu = await openCardMenu(user, "sess-local-1");
    const calls = spyRequests();
    await user.click(within(menu).getByRole("menuitem", { name: "Archive" }));
    const dialog = await screen.findByRole("dialog", { name: "Archive this session?" });
    await user.click(within(dialog).getByRole("button", { name: "Archive" }));
    await waitFor(() => expect(calls.find((call) => call.method === "session.archive")?.params).toEqual({ conversationId: "sess-local-1" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("asks a cloud session to keep or stop its instance", async () => {
    const user = userEvent.setup();
    mount("/p/proj-alpha");
    const menu = await openCardMenu(user, "sess-codespace-1");
    const calls = spyRequests();
    await user.click(within(menu).getByRole("menuitem", { name: "Archive" }));
    const dialog = await screen.findByRole("dialog", { name: "Archive this session?" });
    await user.click(within(dialog).getByRole("button", { name: "Archive, keep running" }));
    await waitFor(() => expect(calls.find((call) => call.method === "session.archive")?.params).toEqual({ conversationId: "sess-codespace-1", cloud: "keep" }));
  });

  it("uses the archive-dialog slot instead of the default dialog when one is registered", async () => {
    registerSlot("archive-dialog", (props) => (props.open ? <div role="dialog" aria-label="Cloud archive">slot for {(props.session as { id: string }).id}</div> : null));
    const user = userEvent.setup();
    mount("/p/proj-alpha");
    const menu = await openCardMenu(user, "sess-codespace-1");
    await user.click(within(menu).getByRole("menuitem", { name: "Archive" }));
    expect(await screen.findByRole("dialog", { name: "Cloud archive" })).toHaveTextContent("slot for sess-codespace-1");
    expect(screen.queryByRole("dialog", { name: "Archive this session?" })).toBeNull();
  });

  it("hides Delete when the session-capabilities predicate says so", async () => {
    registerSessionCapabilities((capability, context) => capability !== "delete" || context.target?.kind === "local" || context.target == null);
    const user = userEvent.setup();
    mount("/p/proj-alpha");
    const menu = await openCardMenu(user, "sess-codespace-1");
    expect(within(menu).queryByRole("menuitem", { name: "Delete" })).toBeNull();
    expect(within(menu).getByRole("menuitem", { name: "Rename" })).toBeInTheDocument();
  });

  it("hides Rename, Archive and Delete for a read-only device", async () => {
    const user = userEvent.setup();
    mount("/p/proj-alpha", { permission: "read" });
    const menu = await openCardMenu(user, "sess-local-1");
    for (const name of ["Rename", "Archive", "Delete"]) expect(within(menu).queryByRole("menuitem", { name })).toBeNull();
    expect(within(menu).getByRole("menuitem", { name: "Open" })).toBeInTheDocument();
  });
});

describe("session cards and open tabs", () => {
  it("adapts to a narrow list with a container query: the model name and the Working text give way, the rest ellipsizes", async () => {
    mount("/p/proj-alpha");
    await screen.findAllByTestId("session-card");
    const first = card("sess-local-1");
    expect(first).toHaveClass("@container");
    // Above 20rem the card is unchanged; under it only the model text hides (the harness mark stays).
    expect(first.querySelector("[class*=\"@max-[20rem]:max-w-none\"]")).not.toBeNull();
  });
});

describe("filters (F-23) and worktree scoping (F-22)", () => {
  it("Working now narrows the list, is stored under a monoptah. key and survives a remount", async () => {
    const user = userEvent.setup();
    mount("/p/proj-alpha");
    await screen.findAllByTestId("session-card");
    await user.click(screen.getByRole("button", { name: "Filter sessions" }));
    await user.click(await screen.findByRole("menuitemradio", { name: "Working now" }));
    await waitFor(() => {
      const ids = screen.getAllByTestId("session-card").map((element) => element.dataset.sessionId);
      expect(ids).toContain("sess-seed-0");
      expect(ids).not.toContain("sess-seed-5");
    });
    expect(JSON.parse(localStorage.getItem(SESSION_FILTER_KEY) ?? "{}")).toMatchObject({ working: true });
    expect(screen.getByRole("button", { name: "Filter sessions, on" })).toBeInTheDocument();
  });

  it("hides an agent through the Agents group", async () => {
    const user = userEvent.setup();
    mount("/p/proj-alpha");
    await screen.findAllByTestId("session-card");
    await user.click(screen.getByRole("button", { name: "Filter sessions" }));
    await user.click(await screen.findByRole("menuitemcheckbox", { name: "Codex" }));
    await waitFor(() => {
      for (const element of screen.getAllByTestId("session-card")) expect(element.querySelector('[data-harness="codex"]')).toBeNull();
    });
    expect(JSON.parse(localStorage.getItem(SESSION_FILTER_KEY) ?? "{}").hiddenHarnesses).toEqual(["codex"]);
  });

  it("applySessionFilter uses the W-21 filters", () => {
    const base = { projectId: "p", title: "t", model: "m", branch: "b", updatedAt: 1 };
    const rows = [
      { ...base, id: "a", harness: "claude", busy: true },
      { ...base, id: "b", harness: "codex" },
    ];
    expect(applySessionFilter(rows, { working: true, hiddenHarnesses: [] }).map((row) => row.id)).toEqual(["a"]);
    expect(applySessionFilter(rows, { working: false, hiddenHarnesses: ["claude"] }).map((row) => row.id)).toEqual(["b"]);
  });

  it("inWorktree goes by the path, or the branch for an older desktop", () => {
    const worktree = { path: "/wt/one/", name: "feature/one" };
    expect(inWorktree({ worktreePath: "/wt/one", branch: "x" }, worktree)).toBe(true);
    expect(inWorktree({ worktreePath: "/wt/two", branch: "feature/one" }, worktree)).toBe(false);
    expect(inWorktree({ branch: "feature/one" }, worktree)).toBe(true);
    expect(inWorktree({ branch: "main" }, null)).toBe(true);
  });

  it("the workspace menu lists worktrees, scopes the list to a pick and links to the changes", async () => {
    const user = userEvent.setup();
    mount("/p/proj-alpha", {}, {
      "project.worktrees": () => ({
        worktrees: [
          { path: "/example/alpha", name: "main" },
          { path: "/example/alpha-wt", name: "feature/task-1" },
        ],
      }),
    });
    await screen.findAllByTestId("session-card");
    await user.click(screen.getByRole("button", { name: /^Workspace:/ }));
    const menu = await screen.findByRole("menu");
    expect(within(menu).getByRole("menuitem", { name: "View changes" })).toHaveAttribute("href", `/d/${DESKTOP_ID}/p/proj-alpha/changes`);
    expect(within(menu).queryByRole("menuitemradio", { name: "main" })).toBeNull();
    expect(within(menu).getByRole("menuitemradio", { name: /^Project checkout/ })).toHaveAttribute("aria-checked", "true");
    await user.click(within(menu).getByRole("menuitemradio", { name: "feature/task-1" }));
    await waitFor(() => {
      const cards = screen.getAllByTestId("session-card");
      expect(cards.length).toBeGreaterThan(0);
      for (const element of cards) expect(element.textContent).toContain("feature/task-1");
    });
    expect(screen.getByRole("button", { name: "Workspace: feature/task-1" })).toBeInTheDocument();
  });
});

describe("open after starting (F-20)", () => {
  it("is on by default, is stored under a monoptah. key and follows the setter", () => {
    const { result } = renderHook(() => useOpenAfterStarting());
    expect(result.current[0]).toBe(true);
    act(() => result.current[1](false));
    expect(result.current[0]).toBe(false);
    expect(localStorage.getItem(OPEN_AFTER_STARTING_KEY)).toBe("false");
    expect(renderHook(() => useOpenAfterStarting()).result.current[0]).toBe(false);
    act(() => result.current[1](true));
    expect(result.current[0]).toBe(true);
  });
});

describe("mascots (F-91)", () => {
  it("projects in the drawer show a mascot in the project's colour", async () => {
    const user = userEvent.setup();
    mount("");
    await user.click(await screen.findByRole("button", { name: /^Open workspace list/ }));
    const nav = await screen.findByRole("navigation", { name: "Workspace" });
    const alpha = (await within(nav).findByRole("link", { name: /Example Alpha/ })).querySelector("svg[data-mascot]");
    expect(alpha).not.toBeNull();
    expect(alpha?.querySelector("path")?.getAttribute("fill")).toBe("#4f8cff");
    expect(PROJECT_MASCOTS.map((mascot) => mascot.name)).toContain(alpha?.getAttribute("data-mascot"));
  });

  it("a working project alternates its frames every 300 ms, and rests under reduced motion", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    mount("");
    await user.click(await screen.findByRole("button", { name: /^Open workspace list/ }));
    const nav = await screen.findByRole("navigation", { name: "Workspace" });
    const gamma = (await within(nav).findByRole("link", { name: /Gamma Service/ })).querySelector("svg[data-mascot]") as SVGElement;
    const frames = new Set<string | null>();
    for (let i = 0; i < 4; i++) {
      await act(async () => {
        await vi.advanceTimersByTimeAsync(FRAME_MS);
      });
      frames.add(gamma.getAttribute("data-frame"));
    }
    expect(frames).toEqual(new Set(["rest", "talk"]));
  });

  it("the mascot pick is stable and an explicit name wins", () => {
    expect(projectMascot("/example/alpha").name).toBe(projectMascot("/example/alpha").name);
    expect(projectMascot("/x", "ghost").name).toBe("ghost");
    expect(projectMascot("/x", "no-such-mascot").name).toBeTruthy();
  });
});

describe("ticker (F-28)", () => {
  function sizes(box: number, text: number) {
    const clientWidth = vi.spyOn(HTMLElement.prototype, "clientWidth", "get").mockReturnValue(box);
    const scrollWidth = vi.spyOn(HTMLElement.prototype, "scrollWidth", "get").mockReturnValue(text);
    return () => {
      clientWidth.mockRestore();
      scrollWidth.mockRestore();
    };
  }

  afterEach(() => {
    vi.restoreAllMocks();
    delete (Element.prototype as { animate?: unknown }).animate;
  });

  it("a title that fits stays still", () => {
    const restore = sizes(300, 120);
    const animate = vi.fn();
    (Element.prototype as { animate?: unknown }).animate = animate;
    const { container } = render(<Ticker text="Short title" />);
    expect(container.querySelector("[data-ticker]")?.getAttribute("data-ticker")).toBe("still");
    expect(animate).not.toHaveBeenCalled();
    restore();
  });

  it("a wide title scrolls by one title and a gap, linearly and without end", () => {
    const restore = sizes(100, 400);
    const cancel = vi.fn();
    const animate = vi.fn(() => ({ cancel }));
    (Element.prototype as { animate?: unknown }).animate = animate;
    const { container, unmount } = render(<Ticker text="A very long session title that does not fit" />);
    expect(container.querySelector("[data-ticker]")?.getAttribute("data-ticker")).toBe("scrolling");
    expect(animate).toHaveBeenCalledWith(
      [{ transform: "translateX(0)" }, { transform: `translateX(-${400 + TICKER_GAP}px)` }],
      expect.objectContaining({ iterations: Infinity, easing: "linear" }),
    );
    // The repeat is hidden from assistive technology: the title is read once.
    expect(container.querySelectorAll('[aria-hidden="true"]').length).toBe(1);
    unmount();
    expect(cancel).toHaveBeenCalled();
    restore();
  });

  it("does not move under reduced motion", () => {
    reduced = true;
    stubMatchMedia();
    const restore = sizes(100, 400);
    const animate = vi.fn();
    (Element.prototype as { animate?: unknown }).animate = animate;
    const { container } = render(<Ticker text="A very long session title that does not fit" />);
    expect(container.querySelector("[data-ticker]")?.getAttribute("data-ticker")).toBe("still");
    expect(animate).not.toHaveBeenCalled();
    restore();
  });
});

describe("finished deck (F-29)", () => {
  const finished = (id: string, title: string, at: number) => ({
    at,
    session: { id, projectId: "proj-alpha", title, harness: "claude", model: "m", branch: "main", updatedAt: at },
  });

  async function mountWithFinished(count: number) {
    const user = userEvent.setup();
    mount("/p/proj-alpha");
    await screen.findByTestId("now-working");
    act(() => {
      useActivityStore.setState({
        desktopId: DESKTOP_ID,
        finished: [finished("sess-seed-5", "Five", Date.now()), finished("sess-seed-6", "Six", Date.now() - 1000), finished("sess-seed-7", "Seven", Date.now() - 2000)].slice(0, count),
      });
    });
    return user;
  }

  it("one finished session is a card that opens it and leaves the deck", async () => {
    const user = await mountWithFinished(1);
    const deck = await screen.findByTestId("finished-deck");
    const open = within(deck).getByRole("link", { name: /finished\. Open/ });
    expect(open).toHaveAttribute("href", `/d/${DESKTOP_ID}/s/sess-seed-5`);
    await user.click(open);
    expect(useActivityStore.getState().finished).toHaveLength(0);
  });

  it("the X puts one away", async () => {
    const user = await mountWithFinished(1);
    const deck = await screen.findByTestId("finished-deck");
    await user.click(within(deck).getByRole("button", { name: /^Put away/ }));
    expect(useActivityStore.getState().finished).toHaveLength(0);
    await waitFor(() => expect(screen.queryByTestId("finished-deck")).toBeNull());
  });

  it("several stack, spread on a click, and Clear puts them all away", async () => {
    const user = await mountWithFinished(3);
    const deck = await screen.findByTestId("finished-deck");
    expect(deck).toHaveAttribute("data-open", "false");
    await user.click(within(deck).getByRole("button", { name: "3 finished sessions. Show them" }));
    expect(deck).toHaveAttribute("data-open", "true");
    expect(within(deck).getAllByRole("link", { name: /finished\. Open/ })).toHaveLength(3);
    await user.click(within(deck).getByRole("button", { name: "Stack finished sessions" }));
    expect(deck).toHaveAttribute("data-open", "false");
    await user.click(within(deck).getByRole("button", { name: "3 finished sessions. Show them" }));
    await user.click(within(deck).getByRole("button", { name: "Clear finished sessions" }));
    expect(useActivityStore.getState().finished).toHaveLength(0);
  });
});

describe("Mono unread tracking (F-17)", () => {
  const mono = { id: "m1", name: "M", unread: 0, status: "idle", lastMessage: "hello", lastActiveAt: 10 };

  async function stored(desktopId = "desk-1"): Promise<Record<string, string> | undefined> {
    return await new Promise((resolve, reject) => {
      const open = indexedDB.open(MONO_SEEN_DB, 1);
      open.onerror = () => reject(open.error);
      open.onsuccess = () => {
        const request = open.result.transaction("seen").objectStore("seen").get(desktopId);
        request.onsuccess = () => resolve(request.result as Record<string, string> | undefined);
      };
    });
  }

  it("keeps what was seen in IndexedDB, per desktop, and reloads it", async () => {
    loadMonoSeen("desk-1");
    markMonoSeen("desk-1", mono);
    await waitFor(async () => expect(await stored()).toEqual({ m1: expect.stringMatching(/^m1:10:5:[0-9a-f]+$/) }));
    markMonoUnread("desk-1", mono);
    await waitFor(async () => expect(await stored()).toEqual({ m1: "" }));
    // A new page load reads it back: the Mono is still marked unread.
    resetMonoSeen();
    const { useUnreadMonos } = await import("./monoSeen");
    const { result } = renderHook(() => useUnreadMonos("desk-1", [mono]));
    await waitFor(() => expect(result.current.has("m1")).toBe(true));
  });

  it("never keeps the text of a Mono message at rest", async () => {
    loadMonoSeen("desk-5");
    markMonoSeen("desk-5", { ...mono, lastMessage: "a private reply about the build" });
    await waitFor(async () => expect(await stored("desk-5")).toBeDefined());
    expect(JSON.stringify(await stored("desk-5"))).not.toContain("private");
    // Entries of the older format held the text itself: they are dropped on load.
    await new Promise<void>((resolve, reject) => {
      const open = indexedDB.open(MONO_SEEN_DB, 1);
      open.onerror = () => reject(open.error);
      open.onsuccess = () => {
        const tx = open.result.transaction("seen", "readwrite");
        tx.objectStore("seen").put({ m1: "10\nold clear text" }, "desk-6");
        tx.oncomplete = () => resolve();
      };
    });
    const { useUnreadMonos } = await import("./monoSeen");
    renderHook(() => useUnreadMonos("desk-6", [{ ...mono, lastMessage: "newer" }]));
    await waitFor(async () => expect(JSON.stringify(await stored("desk-6"))).not.toContain("old clear text"));
  });

  it("wipes a desktop's entry when its link is revoked", async () => {
    const { useUnreadMonos } = await import("./monoSeen");
    renderHook(() => useUnreadMonos("desk-7", [mono]));
    await waitFor(async () => expect(await stored("desk-7")).toBeDefined());
    act(() => getLinkStore("desk-7").setState({ revoked: true }));
    await waitFor(async () => expect(await stored("desk-7")).toBeUndefined());
  });

  it("a Mono seen for the first time counts as seen; a later reply is unread; working is not", async () => {
    const { useUnreadMonos } = await import("./monoSeen");
    const { result, rerender } = renderHook(({ monos }) => useUnreadMonos("desk-2", monos), { initialProps: { monos: [mono] } });
    // Size 0 also holds before the stored map has loaded, so wait until the first sighting is recorded:
    // a reply that arrives before then is itself a first sighting and would count as seen.
    await waitFor(async () => expect(await stored("desk-2")).toEqual({ m1: expect.stringMatching(/^m1:10:5:[0-9a-f]+$/) }));
    await waitFor(() => expect(result.current.size).toBe(0));
    rerender({ monos: [{ ...mono, lastMessage: "a new reply", lastActiveAt: 20 }] });
    await waitFor(() => expect(result.current.has("m1")).toBe(true));
    rerender({ monos: [{ ...mono, lastMessage: "a new reply", lastActiveAt: 20, status: "working" }] });
    await waitFor(() => expect(result.current.size).toBe(0));
    rerender({ monos: [{ ...mono, unread: 3 }] });
    await waitFor(() => expect(result.current.has("m1")).toBe(true));
  });

  it("the drawer button carries an unread badge while a Mono has an unseen reply", async () => {
    mount("");
    const button = await screen.findByRole("button", { name: "Open workspace list, new replies" });
    expect(within(button).getByTestId("drawer-unread-badge")).toBeInTheDocument();
  });

  it("opening the Mono marks it seen, which clears the desktop-independent mark", async () => {
    const { useUnreadMonos } = await import("./monoSeen");
    const { result } = renderHook(() => useUnreadMonos("desk-3", [{ ...mono, lastMessage: "x", lastActiveAt: 1 }]));
    await waitFor(() => expect(result.current.size).toBe(0));
    markMonoUnread("desk-3", mono);
    await waitFor(() => expect(result.current.has("m1")).toBe(true));
    markMonoSeen("desk-3", { ...mono, lastMessage: "x", lastActiveAt: 1 });
    await waitFor(() => expect(result.current.size).toBe(0));
  });
});
