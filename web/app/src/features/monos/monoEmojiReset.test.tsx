// Mono emoji (display only) and Mono reset on the web, against the real link wired to the mock desktop.
import { QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { FIXTURE_HANDLERS, FixtureError, MONO_EMOJI_FIXTURE_HASH, type MockDesktop } from "@monoptah/mock-desktop";
import { KNOWN_CAPABILITIES } from "@monoptah/protocol";
import { IDBFactory } from "fake-indexeddb";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { MemoryPersistAdapter, registerPersistAdapter, type PersistAdapter } from "@/data";
import { resetLinkConnections } from "@/link";
import { clearLinkStores } from "@/link/store.ts";
import { resetSessionActivity } from "@/features/sessions/activityStore";
import { DESKTOP_ID, connectMockDesktop, seedWorkspace, type MockLinkOptions } from "@/features/sessions/dev/mockLink";
import { resetMonoSeen } from "@/features/sessions/monoSeen";
import { stubTranscriptLayout } from "@/features/transcript/layoutStub";
import { createQueryClient } from "@/lib/queryClient";
import { createAppRouter } from "@/router";
import { clearHooks } from "@/ui/hooks";
import { clearSlots } from "@/ui/slots";
import { reloadMonoAccents } from "./monoAccent";
import { reloadMonoPrefs } from "./monoPrefs";
import type * as DesktopsApi from "@/features/desktops/api";
import "./register";

const api = vi.hoisted(() => ({ requestMonoResetTicket: vi.fn() }));
vi.mock("@/features/desktops/api", async (importOriginal) => ({ ...(await importOriginal<typeof DesktopsApi>()), requestMonoResetTicket: api.requestMonoResetTicket }));
vi.mock("@/features/desktops/thisDevice", () => ({ useThisDevice: () => ({ deviceId: "device-1", fingerprint: "f" }) }));

beforeAll(() => {
  window.matchMedia = ((query: string) => ({ matches: false, media: query, onchange: null, addEventListener() {}, removeEventListener() {}, addListener() {}, removeListener() {}, dispatchEvent: () => false })) as never;
  stubTranscriptLayout();
  Element.prototype.hasPointerCapture ??= () => false;
  Element.prototype.setPointerCapture ??= () => {};
  Element.prototype.releasePointerCapture ??= () => {};
  Element.prototype.scrollIntoView ??= () => {};
});

const MONO = "mono-example";
const original = { ...FIXTURE_HANDLERS };
const handlers = FIXTURE_HANDLERS as unknown as Record<string, (p: never, c: never) => unknown>;
let calls: Array<{ method: string; params: Record<string, unknown> }>;
let previous: PersistAdapter;
const called = (method: string) => calls.filter((call) => call.method === method);

beforeEach(() => {
  globalThis.indexedDB = new IDBFactory();
  localStorage.clear();
  reloadMonoPrefs();
  reloadMonoAccents();
  resetMonoSeen();
  previous = registerPersistAdapter(new MemoryPersistAdapter());
  calls = [];
  api.requestMonoResetTicket.mockReset().mockResolvedValue("ticket-1");
  for (const method of ["mono.reset", "mono.profile", "mono.emoji", "workspace.list"]) {
    handlers[method] = (p: never, c: never) => {
      calls.push({ method, params: p as Record<string, unknown> });
      return (original as unknown as Record<string, (p: never, c: never) => unknown>)[method]!(p, c);
    };
  }
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

function turn(items: unknown[]) {
  return { id: "mono-turn-1", stampAt: Date.now() - 60_000, completedAt: Date.now() - 50_000, durationMs: 10_000, items };
}
const PICK = { emotion: "Happy", name: "wave", hash: MONO_EMOJI_FIXTURE_HASH, format: "png" };

function mount(path: string, options: MockLinkOptions & { items?: unknown[] } = {}) {
  connectMockDesktop({
    ...options,
    seed: (desktop: MockDesktop) => {
      seedWorkspace(desktop);
      if (options.items) desktop.world.sessions.get(`mono:${MONO}`)!.turns = [turn(options.items)] as never;
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
}

describe("Mono emoji in the chat", () => {
  const text = "Good morning !\n\nAll done.";
  const items = [{ type: "text", id: "a1", text, emoji: [{ at: 13, ...PICK }, { at: 15, ...PICK, large: true }] }];

  it("draws each picture where the desktop put it: 20 px inline, 56 px alone, with the name and emotion as title", async () => {
    mount(`/monos/${MONO}`, { items });
    const pictures = await screen.findAllByTestId("mono-emoji");
    expect(pictures).toHaveLength(2);
    expect(pictures[0]).toHaveAttribute("width", "20");
    expect(pictures[0]).toHaveAttribute("title", "wave · Happy");
    expect(pictures[0]).toHaveAttribute("alt", "Happy");
    expect(pictures[0]?.getAttribute("src")).toMatch(/^data:image\/png;base64,/);
    expect(pictures[1]).toHaveAttribute("width", "56");
    expect(pictures[1]).toHaveAttribute("data-large", "true");
    // The inline one flows with the sentence; the large one is a block, so it has a line to itself.
    expect(pictures[0]).toHaveClass("inline-block");
    expect(pictures[1]).toHaveClass("block");
    // The image is asked for once: the second picture has the same hash.
    expect(called("mono.emoji")).toHaveLength(1);
    expect(called("mono.emoji")[0]?.params).toEqual({ monoId: MONO, hash: MONO_EMOJI_FIXTURE_HASH });
  });

  it("leaves no raw token and no private-use marker in the text a user can select or copy", async () => {
    mount(`/monos/${MONO}`, { items });
    await screen.findAllByTestId("mono-emoji");
    const reply = document.querySelector("[data-chat-message-role='assistant']")!;
    // MessageActions portals its screen-reader "More" button into the message some time later: only the reply's own
    // text is compared, the markers are still checked on the whole message below.
    const own = reply.cloneNode(true) as HTMLElement;
    own.querySelectorAll("button").forEach((button) => button.remove());
    expect(own.textContent).toBe("Good morning !\nAll done.");
    expect(reply.textContent).not.toMatch(/[-]/);
    expect(document.body.textContent).not.toContain(":emo[");
  });

  it("keeps a picture out of a code span even when its offset lands inside one", async () => {
    mount(`/monos/${MONO}`, { items: [{ type: "text", id: "a1", text: "run `ls -la` now", emoji: [{ at: 7, ...PICK }] }] });
    await waitFor(() => expect(screen.getByText("run", { exact: false })).toBeInTheDocument());
    const code = await screen.findByText("ls -la", { selector: "code" });
    expect(code.textContent).toBe("ls -la");
    expect(document.body.textContent).not.toMatch(/[-]/);
  });

  it("draws nothing when the desktop does not have the picture", async () => {
    handlers["mono.emoji"] = () => {
      throw new FixtureError("No such emoji", "not_found");
    };
    mount(`/monos/${MONO}`, { items });
    await screen.findByText(/All done\./);
    await waitFor(() => expect(screen.queryByTestId("mono-emoji-pending")).toBeNull());
    expect(screen.queryByTestId("mono-emoji")).toBeNull();
    expect(document.body.textContent).not.toMatch(/[-]/);
  });

  it("draws nothing without the monoEmoji capability and never shows a raw token", async () => {
    mount(`/monos/${MONO}`, {
      capabilities: KNOWN_CAPABILITIES.filter((name) => name !== "monoEmoji" && name !== "terminal" && name !== "fileTransfer"),
      items: [{ type: "text", id: "a1", text: "Hello :emo[happy] there `:emo[kept]`", emoji: [{ at: 6, ...PICK }] }],
    });
    await screen.findByText(/Hello there/);
    expect(screen.queryByTestId("mono-emoji")).toBeNull();
    expect(called("mono.emoji")).toHaveLength(0);
    expect(document.body.textContent).not.toContain(":emo[happy]");
    expect(document.body.textContent).toContain(":emo[kept]");
  });

  it("strips a raw token from a reply that has no pictures", async () => {
    mount(`/monos/${MONO}`, { items: [{ type: "text", id: "a1", text: "All done :emo[proud]" }] });
    await screen.findByText("All done");
    expect(document.body.textContent).not.toContain(":emo[");
  });

  it("strips tokens from the last message of a Mono in the list", async () => {
    mount("/monos", { seed: (desktop) => void ((desktop.world.monos() as unknown as Array<Record<string, unknown>>)[0]!.lastMessage = "Done :emo[happy]") });
    const row = (await screen.findAllByTestId("mono-row"))[0]!;
    expect(row).toHaveTextContent("Done");
    expect(row.textContent).not.toContain(":emo[");
  });
});

describe("Reset conversation", () => {
  const open = async (user = userEvent.setup()) => {
    await user.click(await screen.findByRole("button", { name: "Reset conversation" }));
    return screen.findByRole("dialog");
  };

  it("is offered on the profile page and in the list menu, and shows the desktop app's wording", async () => {
    mount(`/monos/${MONO}/profile`);
    const dialog = await open();
    expect(within(dialog).getByText("Reset Example Mono's conversation?")).toBeInTheDocument();
    expect(dialog).toHaveTextContent("All messages in this Mono's conversation will be deleted and any active reply will be stopped. This can't be undone.");
    expect(dialog).toHaveTextContent("Its soul, memory and habits will be kept.");
    const option = await within(dialog).findByRole("checkbox", { name: "Also delete the Claude Code session files on this computer" });
    expect(option).toBeChecked();
    expect(option).toBeEnabled();
    expect(dialog).toHaveTextContent("They can't be recovered.");
    expect(within(dialog).getByRole("button", { name: "Cancel" })).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Reset conversation" })).toBeInTheDocument();
  });

  it("opens from the Mono list context menu", async () => {
    mount("/monos");
    fireEvent.contextMenu((await screen.findAllByTestId("mono-row"))[0]!);
    await userEvent.setup().click(await screen.findByRole("menuitem", { name: "Reset conversation" }));
    expect(await screen.findByRole("dialog")).toHaveTextContent("Its soul, memory and habits will be kept.");
  });

  it("is hidden from a read-only device and from a desktop without monoReset", async () => {
    mount(`/monos/${MONO}/profile`, { permission: "read" });
    await screen.findByText("Example Mono", { selector: "h2" });
    expect(screen.queryByRole("button", { name: "Reset conversation" })).toBeNull();
  });

  it("is hidden when the desktop does not advertise monoReset", async () => {
    mount(`/monos/${MONO}/profile`, { capabilities: KNOWN_CAPABILITIES.filter((name) => name !== "monoReset" && name !== "terminal" && name !== "fileTransfer") });
    await screen.findByText("Example Mono", { selector: "h2" });
    expect(screen.queryByRole("button", { name: "Reset conversation" })).toBeNull();
  });

  it("disables and unchecks the option for a provider that cannot delete its files, with its note", async () => {
    handlers["mono.profile"] = () => ({ id: MONO, name: "Example Mono", resetFiles: { label: "Codex", supported: false, note: "Codex keeps its own history." } });
    mount(`/monos/${MONO}/profile`);
    const dialog = await open();
    const option = await within(dialog).findByRole("checkbox", { name: "Also delete the Codex session files on this computer" });
    expect(option).toBeDisabled();
    expect(option).not.toBeChecked();
    expect(dialog).toHaveTextContent("Codex keeps its own history.");
    expect(dialog).not.toHaveTextContent("They can't be recovered.");
  });

  it("joins the note to the warning for a provider that can delete", async () => {
    handlers["mono.profile"] = () => ({ id: MONO, name: "Example Mono", resetFiles: { label: "Claude Code", supported: true, note: "Other chats stay." } });
    mount(`/monos/${MONO}/profile`);
    expect(await open()).toHaveTextContent("They can't be recovered. Other chats stay.");
  });

  it("mints a ticket for this Mono, then resets with the ticket and the option, and refreshes", async () => {
    const user = userEvent.setup();
    mount(`/monos/${MONO}/profile`);
    const dialog = await open(user);
    await within(dialog).findByRole("checkbox", { name: /Also delete/ });
    const catalogBefore = called("workspace.list").length;
    const profileBefore = called("mono.profile").length;
    await user.click(within(dialog).getByRole("button", { name: "Reset conversation" }));
    await waitFor(() => expect(called("mono.reset")).toHaveLength(1));
    expect(api.requestMonoResetTicket).toHaveBeenCalledWith(DESKTOP_ID, "device-1", MONO);
    expect(called("mono.reset")[0]?.params).toEqual({ monoId: MONO, ticket: "ticket-1", deleteFiles: true });
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    await waitFor(() => expect(called("workspace.list").length).toBeGreaterThan(catalogBefore));
    await waitFor(() => expect(called("mono.profile").length).toBeGreaterThan(profileBefore));
  });

  it("sends deleteFiles false when the option is unchecked, and each retry mints a new ticket", async () => {
    const user = userEvent.setup();
    let fail = true;
    handlers["mono.reset"] = (p: never) => {
      calls.push({ method: "mono.reset", params: p as Record<string, unknown> });
      if (fail) throw new FixtureError("Disk is busy");
      return {};
    };
    api.requestMonoResetTicket.mockResolvedValueOnce("ticket-1").mockResolvedValueOnce("ticket-2");
    mount(`/monos/${MONO}/profile`);
    const dialog = await open(user);
    await user.click(await within(dialog).findByRole("checkbox", { name: /Also delete/ }));
    await user.click(within(dialog).getByRole("button", { name: "Reset conversation" }));
    expect(await within(dialog).findByText("Could not reset the conversation. Disk is busy")).toBeInTheDocument();
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    fail = false;
    await user.click(within(dialog).getByRole("button", { name: "Reset conversation" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(called("mono.reset").map((call) => call.params)).toEqual([
      { monoId: MONO, ticket: "ticket-1", deleteFiles: false },
      { monoId: MONO, ticket: "ticket-2", deleteFiles: false },
    ]);
  });

  it("sends deleteFiles false when the desktop offers no option", async () => {
    handlers["mono.profile"] = () => ({ id: MONO, name: "Example Mono" });
    const user = userEvent.setup();
    mount(`/monos/${MONO}/profile`);
    const dialog = await open(user);
    await waitFor(() => expect(within(dialog).queryByRole("checkbox")).toBeNull());
    await user.click(within(dialog).getByRole("button", { name: "Reset conversation" }));
    await waitFor(() => expect(called("mono.reset")).toHaveLength(1));
    expect(called("mono.reset")[0]?.params).toEqual({ monoId: MONO, ticket: "ticket-1", deleteFiles: false });
  });

  it("reads the profile again on open, so a stale answer with the option is not used", async () => {
    let reads = 0;
    handlers["mono.profile"] = () => {
      reads += 1;
      return reads === 1 ? { id: MONO, name: "Example Mono", resetFiles: { label: "Claude Code", supported: true } } : { id: MONO, name: "Example Mono" };
    };
    const user = userEvent.setup();
    mount(`/monos/${MONO}/profile`);
    await screen.findByText("PRs merged", {}, { timeout: 3000 }).catch(() => undefined);
    await waitFor(() => expect(reads).toBeGreaterThanOrEqual(1));
    const dialog = await open(user);
    await waitFor(() => expect(reads).toBeGreaterThanOrEqual(2));
    await waitFor(() => expect(within(dialog).getByRole("button", { name: "Reset conversation" })).toBeEnabled());
    expect(within(dialog).queryByRole("checkbox")).toBeNull();
    await user.click(within(dialog).getByRole("button", { name: "Reset conversation" }));
    await waitFor(() => expect(called("mono.reset")).toHaveLength(1));
    expect(called("mono.reset")[0]?.params).toMatchObject({ deleteFiles: false });
  });

  it("keeps Confirm off while the profile loads", async () => {
    let release: () => void = () => undefined;
    const gate = new Promise<void>((resolve) => (release = resolve));
    let reads = 0;
    handlers["mono.profile"] = (async () => {
      reads += 1;
      if (reads > 1) await gate;
      return { id: MONO, name: "Example Mono", resetFiles: { label: "Claude Code", supported: true } };
    }) as never;
    const user = userEvent.setup();
    mount(`/monos/${MONO}/profile`);
    await waitFor(() => expect(reads).toBeGreaterThanOrEqual(1));
    await screen.findByRole("button", { name: "Reset conversation" });
    const dialog = await open(user);
    await waitFor(() => expect(reads).toBeGreaterThanOrEqual(2));
    expect(within(dialog).getByRole("button", { name: "Reset conversation" })).toBeDisabled();
    release();
    await waitFor(() => expect(within(dialog).getByRole("button", { name: "Reset conversation" })).toBeEnabled());
  });

  it("falls back to deleteFiles false when the profile cannot be read", async () => {
    let reads = 0;
    handlers["mono.profile"] = () => {
      reads += 1;
      if (reads > 1) throw new FixtureError("Unavailable");
      return { id: MONO, name: "Example Mono", resetFiles: { label: "Claude Code", supported: true } };
    };
    const user = userEvent.setup();
    mount(`/monos/${MONO}/profile`);
    await waitFor(() => expect(reads).toBeGreaterThanOrEqual(1));
    const dialog = await open(user);
    await waitFor(() => expect(within(dialog).getByRole("button", { name: "Reset conversation" })).toBeEnabled());
    expect(within(dialog).queryByRole("checkbox")).toBeNull();
    await user.click(within(dialog).getByRole("button", { name: "Reset conversation" }));
    await waitFor(() => expect(called("mono.reset")).toHaveLength(1));
    expect(called("mono.reset")[0]?.params).toMatchObject({ deleteFiles: false });
  });

  it("says so when the passkey prompt is cancelled, and sends nothing", async () => {
    const { ReauthCancelledError } = await import("@/features/accounts/queries");
    api.requestMonoResetTicket.mockRejectedValue(new ReauthCancelledError());
    const user = userEvent.setup();
    mount(`/monos/${MONO}/profile`);
    const dialog = await open(user);
    await user.click(within(dialog).getByRole("button", { name: "Reset conversation" }));
    expect(await within(dialog).findByText(/Passkey confirmation was cancelled/)).toBeInTheDocument();
    expect(called("mono.reset")).toHaveLength(0);
  });

  it("explains a desktop that asks for a fresh passkey", async () => {
    handlers["mono.reset"] = () => {
      throw new FixtureError("Confirm with your passkey", "fresh_required");
    };
    const user = userEvent.setup();
    mount(`/monos/${MONO}/profile`);
    const dialog = await open(user);
    await user.click(within(dialog).getByRole("button", { name: "Reset conversation" }));
    expect(await within(dialog).findByText(/Could not reset the conversation\. Confirm with your passkey and try again\./)).toBeInTheDocument();
  });
});
