// The touch gestures of a session row in a project's list (long press opens the menu, a right-to-left swipe reveals
// Archive and Delete), the vertical menu icon, and the icon-only New session button, against the real link wired to
// the mock desktop. A touch layout is a `(pointer: coarse)` media query; the mouse layout keeps the button.
import { QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { registerPersistAdapter, MemoryPersistAdapter, type PersistAdapter } from "@/data";
import { getLinkConnection, resetLinkConnections } from "@/link";
import { clearLinkStores } from "@/link/store.ts";
import { createQueryClient } from "@/lib/queryClient";
import { createAppRouter } from "@/router";
import { clearHooks, registerSessionCapabilities } from "@/ui/hooks";
import { clearSlots } from "@/ui/slots";
import { resetSessionActivity } from "./activityStore";
import { DESKTOP_ID, connectMockDesktop, seedWorkspace, type MockLinkOptions } from "./dev/mockLink";
import { closeOpenSwipeRow } from "./useSwipeReveal";

let coarse = false;
let reduced = false;
function stubMatchMedia() {
  window.matchMedia = ((query: string) => ({
    matches: query.includes("pointer: coarse") ? coarse : query.includes("prefers-reduced-motion") ? reduced : false,
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
  coarse = false;
  reduced = false;
  stubMatchMedia();
  localStorage.clear();
  previous = registerPersistAdapter(new MemoryPersistAdapter());
});
afterEach(() => {
  vi.useRealTimers();
  closeOpenSwipeRow();
  resetLinkConnections();
  clearLinkStores();
  registerPersistAdapter(previous);
  resetSessionActivity();
  clearSlots();
  clearHooks();
  localStorage.clear();
});

type Request = (method: string, params: Record<string, unknown>, ...rest: unknown[]) => unknown;

function spyRequests() {
  const connection = getLinkConnection(DESKTOP_ID);
  const calls: Array<{ method: string; params: Record<string, unknown> }> = [];
  const original = connection.request.bind(connection) as unknown as Request;
  connection.request = ((method: string, params: Record<string, unknown>, ...rest: unknown[]) => {
    calls.push({ method, params });
    return original(method, params, ...rest);
  }) as typeof connection.request;
  return calls;
}

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
  return { router };
}

const card = (id: string) => screen.getAllByTestId("session-card").find((element) => element.dataset.sessionId === id)!;
/** The element a finger lands on: the card's session link. */
const surface = (id: string) => within(card(id)).getByRole("link", { name: /./ });
const wait = (ms: number) => act(() => new Promise<void>((resolve) => setTimeout(resolve, ms)));

const finger = (x: number, y: number) => ({ pointerType: "touch", pointerId: 1, isPrimary: true, button: 0, clientX: x, clientY: y });

/** A slow drag from (x0, y) to (x1, y1) in steps, so the release is not read as a flick. */
async function drag(element: Element, from: [number, number], to: [number, number], { release = true, stepMs = 60 } = {}) {
  fireEvent.pointerDown(element, finger(...from));
  const steps = 4;
  for (let i = 1; i <= steps; i++) {
    await wait(stepMs);
    fireEvent.pointerMove(element, finger(from[0] + ((to[0] - from[0]) * i) / steps, from[1] + ((to[1] - from[1]) * i) / steps));
  }
  if (release) fireEvent.pointerUp(element, finger(...to));
}

describe("the menu icon and the New session button", () => {
  it("draws the row and folder menu buttons with vertical dots", async () => {
    mount("/p/proj-alpha");
    await screen.findAllByTestId("session-card");
    const row = within(card("sess-local-1")).getByRole("button", { name: /Actions for/ });
    expect(row.querySelector("[data-icon]")).toHaveAttribute("data-icon", "moreVertical");
    const folder = (await screen.findAllByRole("button", { name: /Actions for folder/ }))[0]!;
    expect(folder.querySelector("[data-icon]")).toHaveAttribute("data-icon", "moreVertical");
  });

  it("shows New session as an icon with the accessible name, no label text, and a 44 px touch size", async () => {
    mount("/p/proj-alpha");
    const link = await screen.findByRole("link", { name: "New session" });
    expect(link).toHaveAttribute("href", `/d/${DESKTOP_ID}/p/proj-alpha/new`);
    expect(link.textContent).toBe("");
    expect(link.querySelector("[data-icon='compose']")).not.toBeNull();
    // `size-11` is 44 px on touch; the fine pointer rule (36 px) is the design system's.
    expect(link.className).toContain("size-11");
  });

  it("keeps the icon-only button when the device is read-only: disabled, still named", async () => {
    mount("/p/proj-alpha", { permission: "read" });
    const button = await screen.findByRole("button", { name: "New session" });
    expect(button).toBeDisabled();
    expect(button.textContent).toBe("");
  });
});

describe("long press on a session row (touch layout)", () => {
  beforeEach(() => {
    coarse = true;
    stubMatchMedia();
  });

  it("hides the row button and shows no text-selection or callout affordance", async () => {
    mount("/p/proj-alpha");
    await screen.findAllByTestId("session-card");
    // Not drawn, but still there for VoiceOver and a keyboard: a visually hidden button that opens the same menu.
    const hidden = within(card("sess-local-1")).getByRole("button", { name: /Actions for/ });
    expect(hidden.className).toContain("sr-only");
    // The surface the finger lands on blocks iOS selection and the link callout.
    const surfaceElement = card("sess-local-1").querySelector<HTMLElement>(".select-none");
    expect(surfaceElement).not.toBeNull();
    expect(surfaceElement!.className).toContain("[-webkit-touch-callout:none]");
    expect(surfaceElement!.className).toContain("touch-pan-y");
  });

  it("opens the same menu as the button after 500 ms, and does not open the session on the release", async () => {
    const vibrate = vi.fn();
    Object.defineProperty(navigator, "vibrate", { value: vibrate, configurable: true, writable: true });
    const { router } = mount("/p/proj-alpha");
    await screen.findAllByTestId("session-card");
    const link = surface("sess-local-1");
    fireEvent.pointerDown(link, finger(60, 40));
    await wait(300);
    expect(screen.queryByRole("menu")).toBeNull();
    await wait(300);
    const menu = await screen.findByRole("menu");
    expect(vibrate).toHaveBeenCalled();
    expect(within(menu).getAllByRole("menuitem").map((item) => item.textContent)).toEqual(expect.arrayContaining(["Open", "Rename", "Archive", "Delete"]));
    fireEvent.pointerUp(link, finger(60, 40));
    fireEvent.click(link);
    expect(router.state.location.pathname).toBe(`/d/${DESKTOP_ID}/p/proj-alpha`);
    Reflect.deleteProperty(navigator, "vibrate");
  });

  it("the visually hidden button opens the same menu from the keyboard", async () => {
    const user = userEvent.setup();
    mount("/p/proj-alpha");
    await screen.findAllByTestId("session-card");
    within(card("sess-local-1")).getByRole("button", { name: /Actions for/ }).focus();
    await user.keyboard("{Enter}");
    const menu = await screen.findByRole("menu");
    for (const name of ["Open", "Rename", "Archive", "Delete"]) expect(within(menu).getByRole("menuitem", { name })).toBeInTheDocument();
  });

  it("does not open from a short tap, which opens the session", async () => {
    const user = userEvent.setup();
    const { router } = mount("/p/proj-alpha");
    await screen.findAllByTestId("session-card");
    await user.click(surface("sess-local-1"));
    expect(screen.queryByRole("menu")).toBeNull();
    await waitFor(() => expect(router.state.location.pathname).toBe(`/d/${DESKTOP_ID}/s/sess-local-1`));
  });

  it("is cancelled when the finger moves (a scroll)", async () => {
    mount("/p/proj-alpha");
    await screen.findAllByTestId("session-card");
    const link = surface("sess-local-1");
    fireEvent.pointerDown(link, finger(60, 40));
    fireEvent.pointerMove(link, finger(60, 90));
    await wait(650);
    expect(screen.queryByRole("menu")).toBeNull();
  });

  it("the menu's Delete asks for confirmation and deletes over the link", async () => {
    const user = userEvent.setup();
    mount("/p/proj-alpha");
    await screen.findAllByTestId("session-card");
    const calls = spyRequests();
    fireEvent.pointerDown(surface("sess-local-1"), finger(60, 40));
    await wait(600);
    await user.click(await screen.findByRole("menuitem", { name: "Delete" }));
    const dialog = await screen.findByRole("dialog", { name: "Delete this session?" });
    await user.click(within(dialog).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(calls.find((call) => call.method === "session.delete")?.params).toEqual({ conversationId: "sess-local-1" }));
  });

  it("a read-only device gets the menu without Rename, Archive and Delete", async () => {
    mount("/p/proj-alpha", { permission: "read" });
    await screen.findAllByTestId("session-card");
    fireEvent.pointerDown(surface("sess-local-1"), finger(60, 40));
    await wait(600);
    const menu = await screen.findByRole("menu");
    const names = within(menu).getAllByRole("menuitem").map((item) => item.textContent);
    expect(names).toContain("Open");
    for (const name of ["Rename", "Archive", "Delete"]) expect(names).not.toContain(name);
  });

  it("a mouse layout keeps the button and has no long press", async () => {
    coarse = false;
    stubMatchMedia();
    mount("/p/proj-alpha");
    await screen.findAllByTestId("session-card");
    expect(within(card("sess-local-1")).getByRole("button", { name: /Actions for/ })).toBeInTheDocument();
    fireEvent.pointerDown(surface("sess-local-1"), { ...finger(60, 40), pointerType: "mouse" });
    await wait(650);
    expect(screen.queryByRole("menu")).toBeNull();
  });
});

describe("swipe actions on a session row (touch layout)", () => {
  beforeEach(() => {
    coarse = true;
    stubMatchMedia();
  });

  const actionsOf = (id: string) => card(id).querySelector<HTMLElement>("[data-swipe-actions]")!;

  it("a partial swipe from right to left snaps open and shows Archive and Delete", async () => {
    mount("/p/proj-alpha");
    await screen.findAllByTestId("session-card");
    const link = surface("sess-local-1");
    await drag(link, [300, 40], [190, 42]);
    expect(card("sess-local-1")).toHaveAttribute("data-swipe-open", "true");
    // Snapped to the full width of the buttons, not left where the finger stopped.
    expect(link.parentElement!.style.transform).toBe("translate3d(-168px, 0px, 0px)");
    expect(within(actionsOf("sess-local-1")).getByRole("button", { name: /^Archive / })).toBeInTheDocument();
    expect(within(actionsOf("sess-local-1")).getByRole("button", { name: /^Delete / })).toBeInTheDocument();
  });

  it("a short slow swipe snaps shut; a quick flick opens", async () => {
    mount("/p/proj-alpha");
    await screen.findAllByTestId("session-card");
    const link = surface("sess-local-1");
    await drag(link, [300, 40], [260, 40]);
    expect(card("sess-local-1")).not.toHaveAttribute("data-swipe-open");
    expect(link.parentElement!.style.transform).toBe("translate3d(0px, 0px, 0px)");
    await drag(link, [300, 40], [250, 40], { stepMs: 1 });
    expect(card("sess-local-1")).toHaveAttribute("data-swipe-open", "true");
  });

  it("does not open the session on the release of a swipe, nor on a tap of the open row (which only closes it)", async () => {
    const { router } = mount("/p/proj-alpha");
    await screen.findAllByTestId("session-card");
    const link = surface("sess-local-1");
    await drag(link, [300, 40], [190, 40]);
    fireEvent.click(link);
    expect(router.state.location.pathname).toBe(`/d/${DESKTOP_ID}/p/proj-alpha`);
    expect(card("sess-local-1")).toHaveAttribute("data-swipe-open", "true");
    fireEvent.pointerDown(link, finger(100, 40));
    fireEvent.pointerUp(link, finger(100, 40));
    fireEvent.click(link);
    expect(router.state.location.pathname).toBe(`/d/${DESKTOP_ID}/p/proj-alpha`);
    expect(card("sess-local-1")).not.toHaveAttribute("data-swipe-open");
  });

  it("tapping another row while one is open only closes the open row", async () => {
    const { router } = mount("/p/proj-alpha");
    await screen.findAllByTestId("session-card");
    await drag(surface("sess-local-1"), [300, 40], [190, 40]);
    const other = surface("sess-codespace-1");
    fireEvent.pointerDown(other, finger(100, 40));
    fireEvent.pointerUp(other, finger(100, 40));
    fireEvent.click(other);
    expect(card("sess-local-1")).not.toHaveAttribute("data-swipe-open");
    expect(router.state.location.pathname).toBe(`/d/${DESKTOP_ID}/p/proj-alpha`);
    // The next tap navigates as usual.
    fireEvent.click(other);
    await waitFor(() => expect(router.state.location.pathname).toBe(`/d/${DESKTOP_ID}/s/sess-codespace-1`));
  });

  it("swiping back closes, and a press outside the row closes", async () => {
    mount("/p/proj-alpha");
    await screen.findAllByTestId("session-card");
    const link = surface("sess-local-1");
    await drag(link, [300, 40], [190, 40]);
    await drag(link, [190, 40], [300, 40]);
    expect(card("sess-local-1")).not.toHaveAttribute("data-swipe-open");
    await drag(link, [300, 40], [190, 40]);
    expect(card("sess-local-1")).toHaveAttribute("data-swipe-open", "true");
    fireEvent.pointerDown(document.body, finger(10, 10));
    expect(card("sess-local-1")).not.toHaveAttribute("data-swipe-open");
  });

  it("only one row is open at a time", async () => {
    mount("/p/proj-alpha");
    await screen.findAllByTestId("session-card");
    await drag(surface("sess-local-1"), [300, 40], [190, 40]);
    expect(card("sess-local-1")).toHaveAttribute("data-swipe-open", "true");
    await drag(surface("sess-codespace-1"), [300, 40], [190, 40]);
    expect(card("sess-codespace-1")).toHaveAttribute("data-swipe-open", "true");
    expect(card("sess-local-1")).not.toHaveAttribute("data-swipe-open");
  });

  it("a mostly vertical drag is left to the browser to scroll: the row does not move or open", async () => {
    mount("/p/proj-alpha");
    await screen.findAllByTestId("session-card");
    const link = surface("sess-local-1");
    await drag(link, [300, 40], [270, 200]);
    expect(card("sess-local-1")).not.toHaveAttribute("data-swipe-open");
    expect(link.parentElement!.style.transform).toBe("translate3d(0px, 0px, 0px)");
    // The browser cancels the pointer when it starts to scroll: nothing is left half done.
    fireEvent.pointerCancel(link, finger(270, 200));
    expect(link.parentElement!.style.transform).toBe("translate3d(0px, 0px, 0px)");
  });

  it("Archive asks first, then archives over the link", async () => {
    const user = userEvent.setup();
    mount("/p/proj-alpha");
    await screen.findAllByTestId("session-card");
    const calls = spyRequests();
    await drag(surface("sess-local-1"), [300, 40], [190, 40]);
    await user.click(within(actionsOf("sess-local-1")).getByRole("button", { name: /^Archive / }));
    const dialog = await screen.findByRole("dialog", { name: "Archive this session?" });
    expect(calls.some((call) => call.method === "session.archive")).toBe(false);
    await user.click(within(dialog).getByRole("button", { name: "Archive" }));
    await waitFor(() => expect(calls.find((call) => call.method === "session.archive")?.params).toEqual({ conversationId: "sess-local-1" }));
  });

  it("Delete opens the existing confirmation, deletes only after it, and Cancel keeps the session", async () => {
    const user = userEvent.setup();
    mount("/p/proj-alpha");
    await screen.findAllByTestId("session-card");
    const calls = spyRequests();
    await drag(surface("sess-local-1"), [300, 40], [190, 40]);
    await user.click(within(actionsOf("sess-local-1")).getByRole("button", { name: /^Delete / }));
    let dialog = await screen.findByRole("dialog", { name: "Delete this session?" });
    expect(calls.some((call) => call.method === "session.delete")).toBe(false);
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(calls.some((call) => call.method === "session.delete")).toBe(false);

    await drag(surface("sess-local-1"), [300, 40], [190, 40]);
    await user.click(within(actionsOf("sess-local-1")).getByRole("button", { name: /^Delete / }));
    dialog = await screen.findByRole("dialog", { name: "Delete this session?" });
    await user.click(within(dialog).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(calls.find((call) => call.method === "session.delete")?.params).toEqual({ conversationId: "sess-local-1" }));
  });

  it("a read-only device has no swipe actions", async () => {
    mount("/p/proj-alpha", { permission: "read" });
    await screen.findAllByTestId("session-card");
    expect(card("sess-local-1").querySelector("[data-swipe-actions]")).toBeNull();
    await drag(surface("sess-local-1"), [300, 40], [190, 40]);
    expect(card("sess-local-1")).not.toHaveAttribute("data-swipe-open");
  });

  it("hides Delete, and keeps Archive, where the capability predicate removes delete", async () => {
    registerSessionCapabilities((capability) => capability !== "delete");
    mount("/p/proj-alpha");
    await screen.findAllByTestId("session-card");
    await drag(surface("sess-local-1"), [300, 40], [190, 40]);
    expect(within(actionsOf("sess-local-1")).getByRole("button", { name: /^Archive / })).toBeInTheDocument();
    expect(within(actionsOf("sess-local-1")).queryByRole("button", { name: /^Delete / })).toBeNull();
  });

  it("the buttons behind a shut row are not reachable (inert, invisible)", async () => {
    mount("/p/proj-alpha");
    await screen.findAllByTestId("session-card");
    const actions = actionsOf("sess-local-1");
    expect(actions).toHaveAttribute("inert");
    expect(actions.className).toContain("invisible");
  });

  it("folder headers are not swipeable and keep their menu button", async () => {
    mount("/p/proj-alpha");
    const header = (await screen.findAllByTestId("folder-header"))[0]!;
    expect(header.querySelector("[data-swipe-actions]")).toBeNull();
    expect(within(header).getByRole("button", { name: /Actions for folder/ })).toBeInTheDocument();
  });

  it("snaps without a transition for people who prefer reduced motion", async () => {
    mount("/p/proj-alpha");
    await screen.findAllByTestId("session-card");
    expect(surface("sess-local-1").parentElement!.className).toContain("motion-reduce:transition-none");
  });
});
