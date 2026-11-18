// U-09 (P1): the Mono chat, its menu, the Mono home list and the "Monos only" preference, against the real link
// wired to the mock desktop (sessions/dev/mockLink.ts).
import { QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { FIXTURE_HANDLERS, type MockDesktop } from "@monoptah/mock-desktop";
import { IDBFactory } from "fake-indexeddb";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { MemoryPersistAdapter, registerPersistAdapter, type PersistAdapter } from "@/data";
import { resetLinkConnections } from "@/link";
import { clearLinkStores } from "@/link/store.ts";
import { resetSessionActivity } from "@/features/sessions/activityStore";
import { DESKTOP_ID, connectMockDesktop, seedWorkspace, type MockLink, type MockLinkOptions } from "@/features/sessions/dev/mockLink";
import { resetMonoSeen } from "@/features/sessions/monoSeen";
import { stubTranscriptLayout } from "@/features/transcript/layoutStub";
import { createQueryClient } from "@/lib/queryClient";
import { createAppRouter } from "@/router";
import { clearHooks } from "@/ui/hooks";
import { clearSlots } from "@/ui/slots";
import { WsPSettingsSections } from "@/features/themes/SettingsSections";
import { REACTIONS_DB, REACTIONS_STORE, REACTION_EMOJIS, loadReactions, resetReactions } from "./reactions";
import { MONOS_ONLY_KEY, reloadMonoPrefs, useMonoPrefs } from "./monoPrefs";
import { MONO_ACCENTS_KEY, reloadMonoAccents, setMonoAccent } from "./monoAccent";

beforeAll(() => {
  window.matchMedia = ((query: string) => ({ matches: false, media: query, onchange: null, addEventListener() {}, removeEventListener() {}, addListener() {}, removeListener() {}, dispatchEvent: () => false })) as never;
  stubTranscriptLayout();
  Element.prototype.hasPointerCapture ??= () => false;
  Element.prototype.setPointerCapture ??= () => {};
  Element.prototype.releasePointerCapture ??= () => {};
  Element.prototype.scrollIntoView ??= () => {};
});

const original = { ...FIXTURE_HANDLERS };
let previous: PersistAdapter;
let link: MockLink;
beforeEach(() => {
  globalThis.indexedDB = new IDBFactory();
  resetReactions();
  localStorage.clear();
  reloadMonoPrefs();
  reloadMonoAccents();
  resetMonoSeen();
  previous = registerPersistAdapter(new MemoryPersistAdapter());
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

export const MONO = "mono-example";

/** A Mono's conversation is the session `mono:<id>` of the mock desktop. */
function seedMonoChat(desktop: MockDesktop) {
  const base = desktop.world.sessions.get("sess-local-1")!;
  desktop.world.sessions.set(`mono:${MONO}`, { ...base, id: `mono:${MONO}`, turns: structuredClone(base.turns), approvals: [], script: [] } as never);
}

function mount(path: string, options: MockLinkOptions = {}) {
  link = connectMockDesktop({
    ...options,
    seed: (desktop) => {
      seedWorkspace(desktop);
      seedMonoChat(desktop);
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

const field = () => screen.findByRole("textbox", { name: "Message" });
const noReset = () => {
  expect(screen.queryByText(/Reset conversation/i)).toBeNull();
  expect(screen.queryByText(/Delete Mono/i)).toBeNull();
};

describe("Mono chat (F-69)", () => {
  it("renders the transcript of mono:<id> under the Mono's name and sends from the composer", async () => {
    const user = userEvent.setup();
    mount(`/monos/${MONO}`);
    expect(await screen.findByRole("heading", { level: 1, name: "Example Mono" })).toBeInTheDocument();
    expect(await screen.findByText("Done. This is canned example output.")).toBeInTheDocument();
    await user.type(await field(), "Hello Mono");
    await user.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(JSON.stringify(link.desktop.world.sessions.get(`mono:${MONO}`)!.turns)).toContain("Hello Mono"));
  });

  it("shows the empty state of a Mono with no messages", async () => {
    mount(`/monos/${MONO}`, { seed: (desktop) => void (desktop.world.sessions.get(`mono:${MONO}`)!.turns = []) });
    expect(await screen.findByTestId("mono-empty")).toHaveTextContent("Example Mono");
  });

  it("a read-only device cannot send", async () => {
    mount(`/monos/${MONO}`, { permission: "read" });
    expect(await field()).toBeDisabled();
    expect(screen.getByRole("button", { name: "Send" })).toBeDisabled();
    expect(screen.getByTestId("read-only-chip")).toBeInTheDocument();
  });

  it("draws the chat in the accent picked for this Mono, kept on this browser only", async () => {
    setMonoAccent(DESKTOP_ID, MONO, "violet");
    expect(JSON.parse(localStorage.getItem(MONO_ACCENTS_KEY) ?? "{}")).toEqual({ [`${DESKTOP_ID}:${MONO}`]: "violet" });
    mount(`/monos/${MONO}`);
    const screenRoot = await screen.findByTestId("route-stub");
    expect(screenRoot.style.getPropertyValue("--accent-color")).toMatch(/^#/);
  });
});

describe("Mono menu (F-70)", () => {
  it("offers Details only: reset and delete are never shown, and Settings is not the action", async () => {
    const user = userEvent.setup();
    mount(`/monos/${MONO}`);
    await user.click(await screen.findByRole("button", { name: "Mono actions" }));
    const menu = await screen.findByRole("menu");
    expect(within(menu).getByRole("menuitem", { name: "View details" })).toHaveAttribute("href", `/d/${DESKTOP_ID}/monos/${MONO}/profile`);
    expect(within(menu).getAllByRole("menuitem")).toHaveLength(1);
    noReset();
    expect(screen.queryByText("Settings", { selector: "a" })).toBeNull();
  });

  it("shows a Settings action instead of the menu when monoTools is off", async () => {
    const handler = FIXTURE_HANDLERS["workspace.list"] as (p: never, c: never) => Record<string, unknown>;
    (FIXTURE_HANDLERS as Record<string, unknown>)["workspace.list"] = ((p: never, c: never) => ({ ...handler(p, c), monoTools: false })) as never;
    mount(`/monos/${MONO}`);
    const links = await screen.findAllByRole("link", { name: "Settings" });
    expect(links.some((link) => link.getAttribute("href") === `/d/${DESKTOP_ID}/settings` && link.textContent === "Settings")).toBe(true);
    expect(screen.queryByRole("button", { name: "Mono actions" })).toBeNull();
  });
});

describe("Mono home (F-13)", () => {
  it("lists the Monos newest first with a context menu that has no reset or delete", async () => {
    mount("/monos");
    const rows = await screen.findAllByTestId("mono-row");
    expect(rows.map((row) => row.textContent)).toEqual([expect.stringContaining("Example Mono"), expect.stringContaining("Research Mono")]);
    expect(rows[0]).toHaveAttribute("data-unread", "true");
    fireEvent.contextMenu(rows[0]!);
    const menu = await screen.findByRole("menu");
    expect(within(menu).getAllByRole("menuitem").map((item) => item.textContent)).toEqual(["Mark as read", "View details", "Hide"]);
    noReset();
  });

  it("marks an idle Mono unread, then read again", async () => {
    const user = userEvent.setup();
    mount("/monos", { seed: (desktop) => void ((desktop.world.monos() as unknown as Array<Record<string, unknown>>)[1]!.status = "idle") });
    const research = (await screen.findAllByTestId("mono-row")).find((row) => row.textContent?.includes("Research Mono"))!;
    expect(research).not.toHaveAttribute("data-unread");
    fireEvent.contextMenu(research);
    await user.click(await screen.findByRole("menuitem", { name: "Mark as unread" }));
    await waitFor(() => expect(research).toHaveAttribute("data-unread", "true"));
    fireEvent.contextMenu(research);
    await user.click(await screen.findByRole("menuitem", { name: "Mark as read" }));
    await waitFor(() => expect(research).not.toHaveAttribute("data-unread"));
  });

  it("hides a Mono and restores it, device-locally", async () => {
    const user = userEvent.setup();
    mount("/monos");
    const research = (await screen.findAllByTestId("mono-row")).find((row) => row.textContent?.includes("Research Mono"))!;
    fireEvent.contextMenu(research);
    await user.click(await screen.findByRole("menuitem", { name: "Hide" }));
    await waitFor(() => expect(screen.queryByText("Research Mono")).toBeNull());
    expect(JSON.parse(localStorage.getItem("monoptah.hiddenMonos") ?? "{}")).toEqual({ [DESKTOP_ID]: ["mono-two"] });
    await user.click(screen.getByRole("button", { name: /Hidden \(1\)/ }));
    await user.click(screen.getByRole("button", { name: "Show Research Mono" }));
    expect(await screen.findByText("Research Mono")).toBeInTheDocument();
  });
});

describe("Monos only setting (F-13)", () => {
  it("is a switch in the WS-P settings group, stored as monoptah.monosOnly", async () => {
    const user = userEvent.setup();
    render(<WsPSettingsSections />);
    const toggle = screen.getByRole("switch", { name: "Monos only" });
    expect(toggle).toHaveAttribute("aria-checked", "false");
    await user.click(toggle);
    expect(toggle).toHaveAttribute("aria-checked", "true");
    expect(localStorage.getItem(MONOS_ONLY_KEY)).toBe("1");
    expect(useMonoPrefs.getState().monosOnly).toBe(true);
  });
});

/** What this browser stored for `desktopId` in the reactions database. */
function storedReactions(desktopId: string): Promise<unknown> {
  return new Promise((resolve, reject) => {
    const open = indexedDB.open(REACTIONS_DB, 1);
    open.onsuccess = () => {
      const request = open.result.transaction(REACTIONS_STORE, "readonly").objectStore(REACTIONS_STORE).get(desktopId);
      request.onsuccess = () => resolve(request.result);
      request.onerror = () => reject(request.error);
    };
    open.onerror = () => reject(open.error);
  });
}

describe("message actions (F-71)", () => {
  const reply = () => screen.findByText("Done. This is canned example output.");

  it("offers 12 reactions on a Mono message and keeps the pick in IndexedDB, not on the desktop", async () => {
    const user = userEvent.setup();
    mount(`/monos/${MONO}`);
    fireEvent.contextMenu(await reply());
    const menu = await screen.findByRole("menu");
    const group = within(menu).getByRole("group", { name: "Reactions" });
    expect(within(group).getAllByRole("menuitem")).toHaveLength(12);
    expect(REACTION_EMOJIS).toHaveLength(12);
    await user.click(within(group).getByRole("menuitem", { name: "React \u{1F525}" }));
    const badge = await screen.findByTestId("message-reaction");
    expect(badge).toHaveTextContent("\u{1F525}");
    expect(badge.closest("[data-chat-message-role]")).not.toBeNull();
    await waitFor(async () => expect(JSON.stringify(await storedReactions(DESKTOP_ID))).toContain("\u{1F525}"));
    // Never sent to the desktop: the conversation holds no reaction.
    expect(JSON.stringify(link.desktop.world.sessions.get(`mono:${MONO}`)!.turns)).not.toContain("\u{1F525}");
    // A reload of the page reads it back.
    resetReactions();
    loadReactions(DESKTOP_ID);
    await waitFor(() => expect(screen.getByTestId("message-reaction")).toHaveTextContent("\u{1F525}"));
  });

  it("replies with the message quoted and marks the Mono unread", async () => {
    const user = userEvent.setup();
    const { router } = mount(`/monos/${MONO}`);
    fireEvent.contextMenu(await reply());
    await user.click(await screen.findByRole("menuitem", { name: "Reply" }));
    expect(await screen.findByTestId("reply-bar")).toHaveTextContent("Replying to: Done. This is canned example output.");
    await user.type(await field(), "Thanks");
    await user.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(JSON.stringify(link.desktop.world.sessions.get(`mono:${MONO}`)!.turns)).toContain("> Done. This is canned example output.\\n\\nThanks"));
    expect(screen.queryByTestId("reply-bar")).toBeNull();
    fireEvent.contextMenu(await reply());
    await user.click(await screen.findByRole("menuitem", { name: "Mark unread" }));
    await waitFor(() => expect(router.state.location.pathname).toBe(`/d/${DESKTOP_ID}`));
  });

  it("copies the message text", async () => {
    const user = userEvent.setup();
    const writeText = vi.fn(() => Promise.resolve());
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
    mount(`/monos/${MONO}`);
    fireEvent.contextMenu(await reply());
    await user.click(await screen.findByRole("menuitem", { name: "Copy" }));
    expect(writeText).toHaveBeenCalledWith("Done. This is canned example output.");
  });

  it("gives a read-only device Mark unread and Copy only: no reactions, no reply", async () => {
    mount(`/monos/${MONO}`, { permission: "read" });
    fireEvent.contextMenu(await reply());
    const menu = await screen.findByRole("menu");
    expect(within(menu).queryByRole("group", { name: "Reactions" })).toBeNull();
    expect(within(menu).getAllByRole("menuitem").map((item) => item.textContent)).toEqual(["Mark unread", "Copy"]);
  });
});
