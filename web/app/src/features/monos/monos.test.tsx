// U-09 (P1): the Mono chat, its menu, the Mono home list and the "Monos only" preference, against the real link
// wired to the mock desktop (sessions/dev/mockLink.ts).
import { QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
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
import { forgetDesktopLocally } from "@/features/settings/desktopLink";
import "./register";
import { MonosOnlySection } from "./MonosOnlySection";
import { REACTIONS_DB, REACTIONS_STORE, REACTION_EMOJIS, loadReactions, resetReactions, setReaction } from "./reactions";
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
  it("is offered in the settings page, now that the workspace home reads it", () => {
    render(<WsPSettingsSections />);
    expect(screen.getByRole("switch", { name: "Monos only" })).toBeInTheDocument();
  });

  it("is a switch stored as monoptah.monosOnly", async () => {
    const user = userEvent.setup();
    render(<MonosOnlySection />);
    const toggle = screen.getByRole("switch", { name: "Monos only" });
    expect(toggle).toHaveAttribute("aria-checked", "false");
    await user.click(toggle);
    expect(toggle).toHaveAttribute("aria-checked", "true");
    expect(localStorage.getItem(MONOS_ONLY_KEY)).toBe("1");
    expect(useMonoPrefs.getState().monosOnly).toBe(true);
  });
});

describe("Monos only home (F-13)", () => {
  it("shows the project and session home while the preference is off", async () => {
    mount("");
    expect(await screen.findByRole("region", { name: "Projects" })).toBeInTheDocument();
    expect(screen.queryByTestId("mono-home")).toBeNull();
  });

  it("shows the Mono list instead of projects when it is on, and follows the switch in settings", async () => {
    localStorage.setItem(MONOS_ONLY_KEY, "1");
    reloadMonoPrefs();
    mount("");
    expect(await screen.findByTestId("mono-home")).toBeInTheDocument();
    expect(await screen.findAllByTestId("mono-row")).not.toHaveLength(0);
    expect(screen.queryByRole("region", { name: "Projects" })).toBeNull();
    expect(screen.queryByRole("region", { name: "Recent sessions" })).toBeNull();
    act(() => useMonoPrefs.getState().setMonosOnly(false));
    expect(await screen.findByRole("region", { name: "Projects" })).toBeInTheDocument();
  });
});

describe("forgetting a desktop (F-71)", () => {
  it("wipes its stored reactions", async () => {
    loadReactions(DESKTOP_ID);
    setReaction(DESKTOP_ID, MONO, "t1:0", "\u{1F525}");
    await waitFor(async () => expect(JSON.stringify(await storedReactions(DESKTOP_ID))).toContain("\u{1F525}"));
    await forgetDesktopLocally(DESKTOP_ID);
    await waitFor(async () => expect(await storedReactions(DESKTOP_ID)).toBeUndefined());
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
    // Opening the chat had marked it seen; now the Mono list shows it unread again.
    await router.navigate({ to: "/d/$desktopId/monos", params: { desktopId: DESKTOP_ID } });
    const row = (await screen.findAllByTestId("mono-row")).find((candidate) => candidate.textContent?.includes("Example Mono"))!;
    expect(row).toHaveAttribute("data-unread", "true");
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

describe("message actions on touch, from the keyboard and with the native menu (F-71)", () => {
  const reply = () => screen.findByText("Done. This is canned example output.");
  const messageOf = async () => (await reply()).closest<HTMLElement>("[data-chat-message-role]")!;

  it("opens from a long press on touch, where iOS fires no contextmenu", async () => {
    mount(`/monos/${MONO}`);
    const message = await messageOf();
    fireEvent.pointerDown(message, { pointerType: "touch", button: 0 });
    // Radix opens the menu from a 700 ms pointerdown timer.
    expect(await screen.findByRole("menu", undefined, { timeout: 2000 })).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: "Copy" })).toBeInTheDocument();
  });

  it("does not open from a long press outside a message", async () => {
    mount(`/monos/${MONO}`);
    const message = await messageOf();
    fireEvent.pointerDown(message.closest("[data-turn-id]")!, { pointerType: "touch", button: 0 });
    await new Promise((resolve) => setTimeout(resolve, 900));
    expect(screen.queryByRole("menu")).toBeNull();
  });

  it("opens from the keyboard through the Message actions button", async () => {
    const user = userEvent.setup();
    mount(`/monos/${MONO}`);
    const button = await within(await messageOf()).findByRole("button", { name: "Message actions" });
    button.focus();
    await user.keyboard("{Enter}");
    expect(await screen.findByRole("menu")).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: "Reply" })).toBeInTheDocument();
    // The button is not part of what Copy copies.
    const writeText = vi.fn(() => Promise.resolve());
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
    await user.click(screen.getByRole("menuitem", { name: "Copy" }));
    expect(writeText).toHaveBeenCalledWith("Done. This is canned example output.");
  });

  it("keeps the hidden Message actions button out of the layout (only paint classes outside focus-visible)", async () => {
    mount(`/monos/${MONO}`);
    const button = await within(await messageOf()).findByRole("button", { name: "Message actions" });
    // Outside `focus-visible:` the button is exactly sr-only (a 1px clipped box) plus paint and flex alignment. A margin,
    // padding, size, inset, translate or min-size there pushed its box (24px with padding) past the right edge on
    // phones and made the whole transcript scroll sideways. A new resting class has to be added here on purpose.
    const resting = button.className.split(/\s+/).filter((token) => token && !token.includes("focus-visible:"));
    expect(resting.sort()).toEqual(["bg-fill", "gap-1.5", "items-center", "rounded-pill", "sr-only", "text-muted-text", "text-small"].sort());
  });

  it("opens from Shift+F10 and the Menu key on a focused part of a message", async () => {
    mount(`/monos/${MONO}`);
    const button = await within(await messageOf()).findByRole("button", { name: "Message actions" });
    fireEvent.keyDown(button, { key: "F10", shiftKey: true });
    expect(await screen.findByRole("menu")).toBeInTheDocument();
    fireEvent.keyDown(screen.getByRole("menu"), { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("menu")).toBeNull());
    fireEvent.keyDown(button, { key: "ContextMenu" });
    expect(await screen.findByRole("menu")).toBeInTheDocument();
  });

  it("keeps the browser's menu when text is selected in the message", async () => {
    mount(`/monos/${MONO}`);
    const message = await messageOf();
    window.getSelection()?.selectAllChildren(message);
    const notPrevented = fireEvent.contextMenu(await reply());
    expect(notPrevented).toBe(true);
    expect(screen.queryByRole("menu")).toBeNull();
    window.getSelection()?.removeAllRanges();
  });

  it("keeps the browser's menu on a link inside a message", async () => {
    mount(`/monos/${MONO}`);
    const message = await messageOf();
    const link = document.createElement("a");
    link.href = "https://example.com/";
    link.textContent = "a link";
    message.append(link);
    expect(fireEvent.contextMenu(link)).toBe(true);
    expect(screen.queryByRole("menu")).toBeNull();
  });
});

describe("Mono chat cards", () => {
  const prompt = {
    requestId: 7,
    autoResolveAt: Date.now() + 120_000,
    questions: [{ id: "q1", prompt: "Which branch?", multiSelect: false, allowCustom: true, options: [{ id: "main", label: "main" }, { id: "dev", label: "dev" }] }],
  };
  const usage = { token: "tok", resetsAt: null, resumeAtReset: false, canResume: true, canAutoResume: false, resetLabel: "in 2 hours", limitation: "Usage limit reached" };

  function withFrame(extra: (first: boolean) => Record<string, unknown>) {
    return (desktop: MockDesktop) => {
      const next = desktop.world.nextFrame.bind(desktop.world);
      desktop.world.nextFrame = (id, cursor) => {
        const first = cursor.first;
        const frame = next(id, cursor);
        return frame && id === `mono:${MONO}` ? ({ ...frame, ...extra(first) } as typeof frame) : frame;
      };
    };
  }

  it("shows the pending question above the composer", async () => {
    mount(`/monos/${MONO}`, { seed: withFrame((first) => (first ? { question: prompt } : {})) });
    const card = await screen.findByTestId("question-card");
    expect(card).toHaveTextContent("Which branch?");
    expect(within(screen.getByTestId("composer-region")).getByTestId("question-card")).toBe(card);
  });

  it("shows the usage limit card above the composer", async () => {
    mount(`/monos/${MONO}`, { seed: withFrame(() => ({ usageLimit: usage })) });
    expect(await screen.findByTestId("usage-limit-card")).toBeInTheDocument();
  });

  it("opens a document card on the Mono's document page, not a dialog", async () => {
    const user = userEvent.setup();
    const { router } = mount(`/monos/${MONO}`, {
      seed: (desktop) => {
        desktop.world.sessions.get(`mono:${MONO}`)!.turns.push({ id: "t-doc", stampAt: Date.now(), items: [{ type: "document", id: "dd1", documentId: "doc-0", title: "Plan", kind: "md" }] } as never);
      },
    });
    await user.click(await screen.findByRole("button", { name: "Open document Plan" }));
    await waitFor(() => expect(router.state.location.pathname).toBe(`/d/${DESKTOP_ID}/monos/${MONO}/profile/file/doc-0`));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("quotes a reply on one line, cut at 280 characters", async () => {
    const user = userEvent.setup();
    const long = `Line  one   ${"x".repeat(400)}`;
    mount(`/monos/${MONO}`, {
      seed: (desktop) => {
        const turn = desktop.world.sessions.get(`mono:${MONO}`)!.turns.find((candidate) => candidate.items.some((item) => item.type === "text"))!;
        (turn.items.find((item) => item.type === "text") as unknown as { text: string }).text = long;
      },
    });
    fireEvent.contextMenu(await screen.findByText(/Line one/));
    await user.click(await screen.findByRole("menuitem", { name: "Reply" }));
    await user.type(await field(), "ok");
    await user.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(JSON.stringify(link.desktop.world.sessions.get(`mono:${MONO}`)!.turns)).toContain("\\n\\nok"));
    const sent = JSON.stringify(link.desktop.world.sessions.get(`mono:${MONO}`)!.turns);
    const quote = /> (Line one x+)…\\n\\nok/.exec(sent);
    expect(quote).not.toBeNull();
    // 279 characters and an ellipsis.
    expect(quote![1]!).toHaveLength(279);
  });

  it("offers a profile action in the header when the desktop allows Mono tools", async () => {
    mount(`/monos/${MONO}`);
    expect(await screen.findByRole("link", { name: "Mono profile" })).toHaveAttribute("href", `/d/${DESKTOP_ID}/monos/${MONO}/profile`);
  });
});

describe("Mono home status", () => {
  it("shows Needs you in the caution colour, and only a working Mono as working", async () => {
    mount("/monos", {
      seed: (desktop) => {
        const monos = desktop.world.monos() as unknown as Array<Record<string, unknown>>;
        monos[0]!.status = "needs-you";
        monos[1]!.status = "working";
      },
    });
    const rows = await screen.findAllByTestId("mono-row");
    const needs = rows.find((row) => row.textContent?.includes("Example Mono"))!;
    const working = rows.find((row) => row.textContent?.includes("Research Mono"))!;
    expect(within(needs).getByText("Needs you")).toHaveClass("text-caution");
    expect(needs).not.toHaveTextContent("Working");
    expect(working).toHaveTextContent("Working...");
  });

  it("keeps the Hidden line a 44px target", async () => {
    mount("/monos");
    const research = (await screen.findAllByTestId("mono-row")).find((row) => row.textContent?.includes("Research Mono"))!;
    fireEvent.contextMenu(research);
    await userEvent.setup().click(await screen.findByRole("menuitem", { name: "Hide" }));
    expect(await screen.findByRole("button", { name: /Hidden \(1\)/ })).toHaveClass("min-h-11");
  });
});

describe("reactions store (F-71)", () => {
  const seedStored = (desktopId: string, value: unknown) =>
    new Promise<void>((resolve, reject) => {
      const open = indexedDB.open(REACTIONS_DB, 1);
      open.onupgradeneeded = () => open.result.createObjectStore(REACTIONS_STORE);
      open.onsuccess = () => {
        const tx = open.result.transaction(REACTIONS_STORE, "readwrite");
        tx.objectStore(REACTIONS_STORE).put(value, desktopId);
        tx.oncomplete = () => {
          open.result.close();
          resolve();
        };
        tx.onerror = () => reject(tx.error);
      };
      open.onerror = () => reject(open.error);
    });

  it("lives in a database under the monoptah. prefix", () => {
    expect(REACTIONS_DB).toBe("monoptah.monoReactions");
  });

  it("does not overwrite the stored reactions when one is set before they were read", async () => {
    await seedStored("desk-x", { [MONO]: { "t1#0": "\u{1F44D}" } });
    setReaction("desk-x", MONO, "t2#0", "\u{1F525}");
    await waitFor(async () => {
      const stored = JSON.stringify(await storedReactions("desk-x"));
      expect(stored).toContain("\u{1F525}");
      expect(stored).toContain("\u{1F44D}");
    });
  });
});
