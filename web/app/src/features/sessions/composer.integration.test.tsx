// W-08, W-10: the composer, the question card, the header menu and the new-session page over the real link wired
// to the mock desktop (dev/mockLink.ts), as the data layer's integration test does.
import { QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { resetCollapsedToolCalls } from "@/features/transcript/toolCallsSetting";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { FIXTURE_HANDLERS, FixtureError, type MockDesktop } from "@monoptah/mock-desktop";
import { MemoryPersistAdapter, registerPersistAdapter, type PersistAdapter } from "@/data";
import { stubTranscriptLayout } from "@/features/transcript/layoutStub";
import { resetLinkConnections } from "@/link";
import { clearLinkStores } from "@/link/store.ts";
import { createQueryClient } from "@/lib/queryClient";
import { createAppRouter } from "@/router";
import { clearHooks, registerSendGuard, registerSessionCapabilities } from "@/ui/hooks";
import { clearSlots, registerSlot } from "@/ui/slots";
import { putBackground } from "@/features/backgrounds/store";
import { IDBFactory } from "fake-indexeddb";
import { MAX_FILE_BYTES } from "@/features/composer/attachments";
import { resetSessionActivity } from "./activityStore";
import { DESKTOP_ID, connectMockDesktop, seedWorkspace, type MockLink, type MockLinkOptions } from "./dev/mockLink";
import { OPEN_AFTER_STARTING_KEY } from "./openAfterStarting";

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

type Handlers = typeof FIXTURE_HANDLERS;
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

const field = () => screen.findByRole("textbox", { name: "Message" });
const sentTexts = (desktop: MockDesktop, id = SESSION) => JSON.stringify(desktop.world.sessions.get(id)!.turns);

describe("composer in the session screen (W-08)", () => {
  it("sends through conversation.send and shows the reply from the watch", async () => {
    const user = userEvent.setup();
    mount(`/s/${SESSION}`);
    await user.type(await field(), "Please add a test{Enter}");
    await waitFor(() => expect(sentTexts(link.desktop)).toContain("Please add a test"));
    expect(await screen.findByText("Please add a test")).toBeInTheDocument();
    expect((await field()) as HTMLTextAreaElement).toHaveValue("");
  });

  it("send-guard: a refusal shows the reason and sends nothing; a confirm asks first", async () => {
    const user = userEvent.setup();
    const answers = [{ kind: "refuse" as const, reason: "The instance is starting" }, { kind: "confirm" as const, notice: "This wakes a cloud instance and costs money." }, { kind: "confirm" as const, notice: "Again" }];
    registerSendGuard(() => Promise.resolve(answers.shift()!));
    mount(`/s/${SESSION}`);
    await user.type(await field(), "first{Enter}");
    expect(await screen.findByText("The instance is starting")).toBeInTheDocument();
    expect(sentTexts(link.desktop)).not.toContain("first");

    await user.click(screen.getByRole("button", { name: "Send" }));
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent("costs money");
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(sentTexts(link.desktop)).not.toContain("first");
    expect(((await field()) as HTMLTextAreaElement).value).toBe("first");

    await user.click(screen.getByRole("button", { name: "Send" }));
    await user.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Send" }));
    await waitFor(() => expect(sentTexts(link.desktop)).toContain("first"));
  });

  it("Stop calls session.cancel while a turn runs", async () => {
    const user = userEvent.setup();
    mount(`/s/${SESSION}`, { seed: (desktop) => void (desktop.world.sessions.get(SESSION)!.busy = true) });
    await user.click(await screen.findByRole("button", { name: "Stop" }));
    await waitFor(() => expect(link.desktop.world.sessions.get(SESSION)!.busy).toBe(false));
  });

  it("a read-only device cannot send and sees why", async () => {
    mount(`/s/${SESSION}`, { permission: "read" });
    expect(await field()).toBeDisabled();
    expect(screen.getByRole("button", { name: "Send" })).toBeDisabled();
    expect(screen.getByText(/Read-only: this device can read sessions/)).toBeInTheDocument();
  });

  it("mounts the model controls in the toolbar", async () => {
    mount(`/s/${SESSION}`);
    expect(await screen.findByRole("button", { name: /^Model: / })).toBeInTheDocument();
  });

  it("an 8 MiB message crosses the relay in chunks and the desktop reassembles it", async () => {
    const user = userEvent.setup();
    const seen: number[] = [];
    FIXTURE_HANDLERS["conversation.send"] = (params, context) => {
      for (const attachment of params.attachments ?? []) seen.push(attachment.data.length);
      return (original["conversation.send"] as Handlers["conversation.send"])(params, context);
    };
    mount(`/s/${SESSION}`);
    await field();
    // Close to the largest the composer allows: the base64 of two files filling the message limit minus the envelope.
    const half = Math.floor(MAX_FILE_BYTES / 2) - 4096;
    const file = (name: string) => new File([new Uint8Array(half)], name, { type: "application/octet-stream" });
    await user.upload(screen.getByTestId("composer-file-input"), [file("one.bin"), file("two.bin")]);
    await screen.findByRole("list", { name: "Attachments" });
    await user.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(seen).toHaveLength(2), { timeout: 20000 });
    const total = seen[0]! + seen[1]!;
    expect(total).toBeGreaterThan(7.9 * 1024 * 1024);
    expect(total).toBeLessThanOrEqual(8 * 1024 * 1024);
    expect(screen.queryByText(/Not sent/)).toBeNull();
  }, 40000);

  it("refuses before sending a message whose request would pass the link's 8 MiB limit", async () => {
    const user = userEvent.setup();
    mount(`/s/${SESSION}`);
    await field();
    // One file of the per-file cap is exactly 8 MiB of base64; the envelope no longer fits.
    await user.upload(screen.getByTestId("composer-file-input"), [new File([new Uint8Array(MAX_FILE_BYTES)], "all.bin", { type: "application/octet-stream" })]);
    await screen.findByRole("list", { name: "Attachments" });
    await user.click(screen.getByRole("button", { name: "Send" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("too large to send");
    expect(screen.getByRole("list", { name: "Attachments" })).toBeInTheDocument();
    expect(screen.queryByText(/Not sent/)).toBeNull();
  }, 40000);
});

describe("refused sends (fix-up point 4)", () => {
  const sendOnce = (code: "starting" | "read_only") => {
    let first = true;
    FIXTURE_HANDLERS["conversation.send"] = (params, context) => {
      if (first) {
        first = false;
        throw new FixtureError("refused", code);
      }
      return (original["conversation.send"] as Handlers["conversation.send"])(params, context);
    };
  };

  it("shows the reason of the desktop code under the unsent message", async () => {
    const user = userEvent.setup();
    sendOnce("starting");
    mount(`/s/${SESSION}`);
    await user.type(await field(), "hello{Enter}");
    expect(await screen.findByTestId("not-sent-reason", undefined, { timeout: 15000 })).toHaveTextContent("still starting");
  }, 20000);

  it("Retry goes through the send-guard: a refusal shows its reason and sends nothing", async () => {
    const user = userEvent.setup();
    sendOnce("starting");
    const seen: string[] = [];
    registerSendGuard(() => {
      seen.push("guard");
      return Promise.resolve(seen.length === 1 ? { kind: "allow" as const } : { kind: "refuse" as const, reason: "Still starting, wait" });
    });
    mount(`/s/${SESSION}`);
    await user.type(await field(), "hello{Enter}");
    await user.click(await screen.findByRole("button", { name: "Not sent, retry" }));
    expect(await screen.findByTestId("send-refused")).toHaveTextContent("Still starting, wait");
    expect(seen).toHaveLength(2);
    expect(sentTexts(link.desktop)).not.toContain("hello");
  });

  it("Retry asks the send-guard to confirm, then sends", async () => {
    const user = userEvent.setup();
    sendOnce("starting");
    let calls = 0;
    registerSendGuard(() => Promise.resolve(++calls === 1 ? { kind: "allow" as const } : { kind: "confirm" as const, notice: "Wake the instance?" }));
    mount(`/s/${SESSION}`);
    await user.type(await field(), "hello{Enter}");
    await user.click(await screen.findByRole("button", { name: "Not sent, retry" }));
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent("Wake the instance?");
    await user.click(within(dialog).getByRole("button", { name: "Send" }));
    await waitFor(() => expect(sentTexts(link.desktop)).toContain("hello"));
  });
});

describe("question card (W-10)", () => {
  const prompt = {
    requestId: 4,
    autoResolveAt: Date.now() + 120_000,
    questions: [{ id: "q1", prompt: "Which branch?", multiSelect: false, allowCustom: true, options: [{ id: "main", label: "main" }, { id: "dev", label: "dev" }] }],
  };

  function withQuestion(desktop: MockDesktop) {
    const next = desktop.world.nextFrame.bind(desktop.world);
    desktop.world.nextFrame = (id, cursor) => {
      const first = cursor.first;
      const frame = next(id, cursor);
      return frame && first && id === SESSION ? { ...frame, question: prompt } : frame;
    };
  }

  it("shows the pending question above the composer, keeps it open on first touch and answers it", async () => {
    const user = userEvent.setup();
    const answered = vi.fn();
    const kept = vi.fn();
    FIXTURE_HANDLERS["session.answerQuestion"] = (params, context) => {
      answered(params);
      return (original["session.answerQuestion"] as Handlers["session.answerQuestion"])(params, context);
    };
    FIXTURE_HANDLERS["session.keepQuestionOpen"] = (params, context) => {
      kept(params);
      return (original["session.keepQuestionOpen"] as Handlers["session.keepQuestionOpen"])(params, context);
    };
    mount(`/s/${SESSION}`, { seed: withQuestion });
    const card = await screen.findByTestId("question-card");
    expect(within(screen.getByTestId("composer-region")).getByTestId("question-card")).toBe(card);
    expect(card).toHaveTextContent("Which branch?");
    await user.click(within(card).getByRole("radio", { name: /dev/ }));
    await waitFor(() => expect(kept).toHaveBeenCalledWith({ conversationId: SESSION, requestId: 4 }));
    await user.click(within(card).getByRole("button", { name: "Send" }));
    await waitFor(() => expect(answered).toHaveBeenCalledWith({ conversationId: SESSION, requestId: 4, reply: { kind: "answered", answers: { q1: ["dev"] } } }));
  });

  it("is disabled with an explanation on a read-only device", async () => {
    mount(`/s/${SESSION}`, { permission: "read", seed: withQuestion });
    const card = await screen.findByTestId("question-card");
    expect(card).toHaveTextContent("Answer on desktop or use a control device");
    expect(within(card).getAllByRole("radio")[0]).toBeDisabled();
  });
});

describe("session header menu (F-25, F-40)", () => {
  it("offers Rename, Archive and Delete, plus the slot items, and renames over the link", async () => {
    const user = userEvent.setup();
    registerSlot("session-menu-items", () => <div role="menuitem">Cloud extra</div>);
    mount(`/s/${SESSION}`);
    await user.click(await screen.findByRole("button", { name: "Session actions" }));
    expect(screen.getByRole("menuitem", { name: "Cloud extra" })).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: "Archive" })).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: "Delete" })).toBeInTheDocument();
    await user.click(screen.getByRole("menuitem", { name: "Rename" }));
    const input = await screen.findByLabelText("Title");
    await user.clear(input);
    await user.type(input, "Renamed from the header");
    await user.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(link.desktop.world.sessions.get(SESSION)!.title).toBe("Renamed from the header"));
  });

  it("toggles collapse tool calls from the menu and stores it", async () => {
    const user = userEvent.setup();
    mount(`/s/${SESSION}`);
    await user.click(await screen.findByRole("button", { name: "Session actions" }));
    expect(screen.getByRole("menuitemcheckbox", { name: "Collapse tool calls" })).toHaveAttribute("aria-checked", "true");
    await user.click(screen.getByRole("menuitemcheckbox", { name: "Collapse tool calls" }));
    expect(localStorage.getItem("monoptah.toolCalls.collapsed")).toBe("false");
    await user.click(await screen.findByRole("button", { name: "Session actions" }));
    expect(screen.getByRole("menuitemcheckbox", { name: "Collapse tool calls" })).toHaveAttribute("aria-checked", "false");
  });

  it("archives through the archive-dialog slot when one is registered", async () => {
    const user = userEvent.setup();
    registerSlot("archive-dialog", (props) => <div role="dialog" aria-label="Keep or stop">{String((props as { session: { id: string } }).session.id)}</div>);
    mount(`/s/${SESSION}`);
    await user.click(await screen.findByRole("button", { name: "Session actions" }));
    await user.click(screen.getByRole("menuitem", { name: "Archive" }));
    expect(await screen.findByRole("dialog", { name: "Keep or stop" })).toHaveTextContent(SESSION);
  });

  it("hides Delete when session-capabilities says so", async () => {
    const user = userEvent.setup();
    registerSessionCapabilities((capability) => capability !== "delete");
    mount(`/s/${SESSION}`);
    await user.click(await screen.findByRole("button", { name: "Session actions" }));
    expect(screen.getByRole("menuitem", { name: "Rename" })).toBeInTheDocument();
    expect(screen.queryByRole("menuitem", { name: "Delete" })).toBeNull();
  });

  it("hides the actions that change the session for a read-only device", async () => {
    const user = userEvent.setup();
    mount(`/s/${SESSION}`, { permission: "read" });
    await user.click(await screen.findByRole("button", { name: "Session actions" }));
    for (const name of ["Rename", "Archive", "Delete"]) expect(screen.queryByRole("menuitem", { name })).toBeNull();
    expect(screen.getByRole("menuitem", { name: "Copy Session ID" })).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: "Settings" })).toBeInTheDocument();
  });

  it("copies the session ID on a user click and links to the desktop's settings", async () => {
    const user = userEvent.setup();
    const { router } = mount(`/s/${SESSION}`);
    await user.click(await screen.findByRole("button", { name: "Session actions" }));
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    await user.click(screen.getByRole("menuitem", { name: "Copy Session ID" }));
    expect(writeText).toHaveBeenCalledWith(SESSION);
    await user.click(await screen.findByRole("button", { name: "Session actions" }));
    expect(screen.getByRole("menuitem", { name: "Settings" })).toHaveAttribute("href", `/d/${DESKTOP_ID}/settings`);
    await user.click(screen.getByRole("menuitem", { name: "Settings" }));
    await waitFor(() => expect(router.state.location.pathname).toBe(`/d/${DESKTOP_ID}/settings`));
  });
});

describe("new session page (F-30, F-31)", () => {
  const NEW = "/p/proj-alpha/new";
  const startSpy = () => {
    const spy = vi.fn();
    FIXTURE_HANDLERS["session.start"] = (params, context) => {
      spy(params);
      return (original["session.start"] as Handlers["session.start"])(params, context);
    };
    return spy;
  };

  it("shows the headline, starts a session with the model and opens it", async () => {
    const user = userEvent.setup();
    const spy = startSpy();
    const { router } = mount(NEW);
    expect(await screen.findByText("What should we work on?")).toBeInTheDocument();
    await user.type(await field(), "Fix the build{Enter}");
    await waitFor(() => expect(spy).toHaveBeenCalledTimes(1));
    const params = spy.mock.calls[0]![0] as Record<string, unknown>;
    expect(params).toMatchObject({ projectId: "proj-alpha", text: "Fix the build", harness: "claude", model: "example-model-1", runtimeMode: "supervised" });
    expect(params.runtimeMode).not.toBe("full-access");
    expect(params).not.toHaveProperty("target");
    await waitFor(() => expect(router.state.location.pathname).toMatch(new RegExp(`^/d/${DESKTOP_ID}/s/sess-new-`)));
  });

  it("shows the project's stored background behind the empty-session screen (F-90)", async () => {
    globalThis.indexedDB = new IDBFactory();
    await putBackground({ projectId: "proj-alpha", projectName: "Example Alpha", image: new Blob(["x"], { type: "image/png" }), opacity: 0.3, updatedAt: 1 });
    mount(NEW);
    const image = await screen.findByTestId("empty-session-background");
    expect(image).toHaveStyle({ opacity: "0.3" });
    expect(screen.getByTestId("new-session-screen")).toContainElement(image);
    expect(screen.getByTestId("new-session-screen")).toHaveClass("isolate");
  });

  it("goes back to the project when Open after starting is off", async () => {
    localStorage.setItem(OPEN_AFTER_STARTING_KEY, "false");
    const user = userEvent.setup();
    const { router } = mount(NEW);
    await user.type(await field(), "Quietly{Enter}");
    await waitFor(() => expect(router.state.location.pathname).toBe(`/d/${DESKTOP_ID}/p/proj-alpha`));
  });

  it("the new-session page has a control for Open after starting that is stored and used", async () => {
    const user = userEvent.setup();
    const { router } = mount(NEW);
    const toggle = await screen.findByRole("switch", { name: "Open after starting" });
    expect(toggle).toBeChecked();
    await user.click(toggle);
    expect(toggle).not.toBeChecked();
    expect(localStorage.getItem(OPEN_AFTER_STARTING_KEY)).toBe("false");
    await user.type(await field(), "From the toggle{Enter}");
    await waitFor(() => expect(router.state.location.pathname).toBe(`/d/${DESKTOP_ID}/p/proj-alpha`));
  });

  it("sends the value of the new-session-target slot as the target; null means local", async () => {
    const user = userEvent.setup();
    registerSlot("new-session-target", (props) => (
      <button type="button" onClick={() => props.onChange?.({ kind: "codespace", instance: "new" })}>
        Pick codespace
      </button>
    ));
    const spy = startSpy();
    mount(NEW, { seed: (desktop) => void (desktop.flags = { remoteCloudStart: true }) });
    await user.click(await screen.findByRole("button", { name: "Pick codespace" }));
    await user.type(await field(), "In the cloud{Enter}");
    await waitFor(() => expect(spy).toHaveBeenCalled());
    expect(spy.mock.calls[0]![0]).toMatchObject({ target: { kind: "codespace", instance: "new" } });
  });

  it("a send-guard refusal keeps the draft and starts nothing", async () => {
    const user = userEvent.setup();
    registerSendGuard(() => Promise.resolve({ kind: "refuse", reason: "Not connected" }));
    const spy = startSpy();
    mount(NEW);
    await user.type(await field(), "nope{Enter}");
    expect(await screen.findByText("Not connected")).toBeInTheDocument();
    expect(spy).not.toHaveBeenCalled();
    expect(((await field()) as HTMLTextAreaElement).value).toBe("nope");
  });

  it("sends a new worktree as workspaceMode", async () => {
    const user = userEvent.setup();
    const spy = startSpy();
    mount(NEW);
    await user.click(await screen.findByRole("button", { name: /^Workspace:/ }));
    await user.click(await screen.findByRole("menuitem", { name: "New worktree" }));
    await user.type(await field(), "isolated{Enter}");
    await waitFor(() => expect(spy).toHaveBeenCalled());
    expect(spy.mock.calls[0]![0]).toMatchObject({ workspaceMode: "worktree" });
  });

  it("is disabled for a read-only device", async () => {
    mount(NEW, { permission: "read" });
    expect(await field()).toBeDisabled();
    expect(screen.getByRole("button", { name: "Send" })).toBeDisabled();
  });
});

void act;
