// The `/` menu of a session composer and of the new-session page over the real link wired to the mock desktop
// (dev/mockLink.ts), as the composer integration test does.
import { QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { resetCollapsedToolCalls } from "@/features/transcript/toolCallsSetting";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { FIXTURE_HANDLERS } from "@monoptah/mock-desktop";
import { KNOWN_CAPABILITIES } from "@monoptah/protocol";
import { MemoryPersistAdapter, registerPersistAdapter, type PersistAdapter } from "@/data";
import { stubTranscriptLayout } from "@/features/transcript/layoutStub";
import { resetLinkConnections } from "@/link";
import { clearLinkStores } from "@/link/store.ts";
import { createQueryClient } from "@/lib/queryClient";
import { createAppRouter } from "@/router";
import { clearHooks } from "@/ui/hooks";
import { clearSlots } from "@/ui/slots";
const { toastMock } = vi.hoisted(() => ({ toastMock: vi.fn() }));
vi.mock("@/ui/sonner", () => ({ toast: toastMock }));
import { resetChatDrafts } from "@/features/composer/chatDraft";
import { resetSessionActivity } from "./activityStore";
import { resetNewSessionDrafts } from "./newSessionDraft";
import { DESKTOP_ID, connectMockDesktop, seedWorkspace, type MockLink, type MockLinkOptions } from "./dev/mockLink";

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
  let n = 0;
  URL.createObjectURL = vi.fn(() => `blob:fake-${n++}`);
  URL.revokeObjectURL = vi.fn();
});

const original = { ...FIXTURE_HANDLERS };
let previous: PersistAdapter;
let link: MockLink;
beforeEach(() => {
  resetCollapsedToolCalls();
  localStorage.clear();
  previous = registerPersistAdapter(new MemoryPersistAdapter());
});
afterEach(() => {
  Object.assign(FIXTURE_HANDLERS, original);
  resetLinkConnections();
  clearLinkStores();
  registerPersistAdapter(previous);
  resetSessionActivity();
  resetChatDrafts();
  resetNewSessionDrafts();
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

const field = () => screen.findByRole("textbox", { name: "Message" }) as Promise<HTMLTextAreaElement>;
const menu = () => screen.findByRole("listbox", { name: "Skills and commands" });
const ids = (list: HTMLElement) => within(list).getAllByRole("option").map((option) => option.getAttribute("data-command"));

describe("the / menu of a session composer", () => {
  it("opens on a slash with the commands in the desktop's order and the skills after them", async () => {
    const user = userEvent.setup();
    mount(`/s/${SESSION}`);
    await user.type(await field(), "/");
    const list = await menu();
    await waitFor(() => expect(ids(list)).toEqual(["draft", "orchestrator", "operator", "plan", "btw", "compact", "add-to-folder", "lint", "review"]));
    expect(screen.getByTestId("slash-menu")).toHaveAttribute("data-layout", "popover");
  });

  it("filters while typing and closes when nothing is left to type", async () => {
    const user = userEvent.setup();
    mount(`/s/${SESSION}`);
    await user.type(await field(), "/pla");
    const list = await menu();
    await waitFor(() => expect(ids(list)).toEqual(["plan"]));
    await user.type(await field(), " ");
    await waitFor(() => expect(screen.queryByRole("listbox")).toBeNull());
  });

  it("Enter chooses Plan mode on the desktop, shows its chip and takes the word out of the text", async () => {
    const user = userEvent.setup();
    mount(`/s/${SESSION}`);
    await user.type(await field(), "Do this /pl");
    await waitFor(async () => expect(ids(await menu())).toEqual(["plan"]));
    await user.keyboard("{Enter}");
    await waitFor(() => expect(link.desktop.world.slashState(SESSION).mode).toBe("plan"));
    expect(await screen.findByTestId("slash-chip-plan")).toBeInTheDocument();
    expect((await field()).value).toBe("Do this ");
    expect(link.desktop.world.commandRuns).toEqual([{ conversationId: SESSION, command: "plan", enabled: true }]);
    // The chip turns it off again on the desktop.
    await user.click(screen.getByTestId("slash-chip-plan"));
    await waitFor(() => expect(link.desktop.world.slashState(SESSION).mode).toBeNull());
    await waitFor(() => expect(screen.queryByTestId("slash-chip-plan")).toBeNull());
  });

  it("arrow keys move the highlight, Tab chooses and Escape closes without losing the text", async () => {
    const user = userEvent.setup();
    mount(`/s/${SESSION}`);
    await user.type(await field(), "/");
    const list = await menu();
    await waitFor(() => expect(ids(list).length).toBeGreaterThan(3));
    const selected = () => within(list).getByRole("option", { selected: true }).getAttribute("data-command");
    expect(selected()).toBe("draft");
    await user.keyboard("{ArrowDown}{ArrowDown}");
    expect(selected()).toBe("operator");
    await user.keyboard("{ArrowUp}");
    expect(selected()).toBe("orchestrator");
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("listbox")).toBeNull());
    expect((await field()).value).toBe("/");
    // Typing on changes the word, which opens it again.
    await user.type(await field(), "li");
    await user.keyboard("{Tab}");
    expect((await field()).value).toBe("/lint ");
  });

  it("a skill puts its name in the message, so it goes out as ordinary text", async () => {
    const user = userEvent.setup();
    mount(`/s/${SESSION}`);
    await user.type(await field(), "/rev");
    await user.keyboard("{Enter}");
    expect((await field()).value).toBe("/review ");
    await user.type(await field(), "the parser{Enter}");
    await waitFor(() => expect(JSON.stringify(link.desktop.world.sessions.get(SESSION)!.turns)).toContain("/review the parser"));
  });

  it("Draft saves the next message on the desktop without a turn", async () => {
    const user = userEvent.setup();
    mount(`/s/${SESSION}`);
    await user.type(await field(), "/dra");
    await user.keyboard("{Enter}");
    expect(await screen.findByTestId("slash-chip-draft")).toBeInTheDocument();
    await user.type(await field(), "Remember this");
    await user.click(screen.getByRole("button", { name: "Save draft" }));
    await waitFor(() => expect(link.desktop.world.slashState(SESSION).draft).toBe("Remember this"));
    await waitFor(() => expect(screen.queryByTestId("slash-chip-draft")).toBeNull());
    expect(JSON.stringify(link.desktop.world.sessions.get(SESSION)!.turns)).not.toContain("Remember this");
  });

  it("a side question tells the web user it opened on the desktop", async () => {
    const user = userEvent.setup();
    mount(`/s/${SESSION}`);
    await user.type(await field(), "/side");
    await waitFor(async () => expect(ids(await menu())).toEqual(["btw"]));
    await user.keyboard("{Enter}");
    await user.type(await field(), "Why this approach?");
    await user.click(screen.getByRole("button", { name: "Ask" }));
    await waitFor(() => expect(link.desktop.world.slashState(SESSION).sideQuestions).toEqual(["Why this approach?"]));
    await waitFor(() => expect(toastMock).toHaveBeenCalledWith(expect.stringMatching(/side question opened on the desktop/i)));
  });

  it("follows the desktop when its mode changes behind the page", async () => {
    const user = userEvent.setup();
    mount(`/s/${SESSION}`);
    await user.type(await field(), "/pla");
    await waitFor(async () => expect(ids(await menu())).toEqual(["plan"]));
    await user.keyboard("{Enter}");
    expect(await screen.findByTestId("slash-chip-plan")).toBeInTheDocument();
    link.desktop.world.slashState(SESSION).mode = "orchestrator";
    // Opening the menu reads the desktop again.
    await user.type(await field(), "/");
    expect(await screen.findByTestId("slash-chip-orchestrator")).toBeInTheDocument();
    expect(screen.queryByTestId("slash-chip-plan")).toBeNull();
  });

  it("a command the desktop says is not available shows why and does not run", async () => {
    const user = userEvent.setup();
    mount(`/s/${SESSION}`, { seed: (desktop) => void (desktop.world.sessions.get(SESSION)!.busy = true) });
    await user.type(await field(), "/comp");
    const list = await menu();
    await waitFor(() => expect(ids(list)).toEqual(["compact"]));
    expect(within(list).getByRole("option")).toHaveAttribute("aria-disabled", "true");
    expect(list).toHaveTextContent("Wait for the current turn");
    await user.keyboard("{Enter}");
    expect(link.desktop.world.commandRuns).toEqual([]);
    expect(await screen.findByRole("alert")).toHaveTextContent("Wait for the current turn");
  });

  it("Compact context runs at once and Add to folder opens the folder dialog", async () => {
    const user = userEvent.setup();
    mount(`/s/${SESSION}`);
    await user.type(await field(), "/comp");
    await waitFor(async () => expect(ids(await menu())).toEqual(["compact"]));
    await user.keyboard("{Enter}");
    await waitFor(() => expect(link.desktop.world.slashState(SESSION).compactions).toBe(1));
    expect((await field()).value).toBe("");
    await user.type(await field(), "/add");
    await waitFor(async () => expect(ids(await menu())).toEqual(["add-to-folder"]));
    await user.keyboard("{Enter}");
    expect(await screen.findByRole("heading", { name: "Add to folder" })).toBeInTheDocument();
  });

  it("shows no menu on a desktop that does not advertise slashCommands", async () => {
    const user = userEvent.setup();
    mount(`/s/${SESSION}`, { capabilities: KNOWN_CAPABILITIES.filter((name) => name !== "slashCommands" && name !== "terminal" && name !== "fileTransfer") });
    await user.type(await field(), "/");
    expect(screen.queryByRole("listbox")).toBeNull();
    expect(link.desktop.world.commandRuns).toEqual([]);
  });

  it("a read-only device has no menu", async () => {
    mount(`/s/${SESSION}`, { permission: "read" });
    expect(await field()).toBeDisabled();
    expect(screen.queryByRole("listbox")).toBeNull();
  });
});

describe("the / menu of the new-session page", () => {
  it("offers Plan and Draft, turns Operator and the session commands off with a reason, and starts with the prefix", async () => {
    const user = userEvent.setup();
    mount("/p/proj-alpha/new");
    await user.type(await field(), "/");
    const list = await menu();
    await waitFor(() => expect(ids(list).length).toBeGreaterThan(3));
    expect(within(list).getByRole("option", { name: /Operator/ })).toHaveAttribute("aria-disabled", "true");
    expect(within(list).getByRole("option", { name: /Compact context/ })).toHaveAttribute("aria-disabled", "true");
    expect(within(list).getByRole("option", { name: /Plan mode/ })).not.toHaveAttribute("aria-disabled");
    await user.keyboard("{Escape}");
    await user.clear(await field());
    await user.type(await field(), "/plan");
    await user.keyboard("{Enter}");
    expect(await screen.findByTestId("slash-chip-plan")).toBeInTheDocument();
    expect(link.desktop.world.commandRuns).toEqual([]);
    await user.type(await field(), "Build the thing{Enter}");
    await waitFor(() => expect(JSON.stringify([...link.desktop.world.sessions.values()].map((s) => s.turns))).toContain("/plan Build the thing"));
  });
});
