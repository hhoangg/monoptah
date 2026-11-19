// Session folders in the project's session list at 390 (phone), 768 (tablet) and 1280 (desktop), against the real
// router, the link and the mock desktop: grouping, folding, live updates, and the edits a control device gets.
import { QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { act, cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/features/backgrounds", () => ({
  BackgroundControls: () => null,
  BACKGROUND_CONTAINER_CLASS: "relative isolate",
  ChatBackground: () => null,
  useFirstBackgroundProject: () => null,
}));
import { MemoryPersistAdapter, catalogKey, registerPersistAdapter, type PersistAdapter } from "@/data";
import { getLinkConnection, resetLinkConnections } from "@/link";
import { clearLinkStores } from "@/link/store.ts";
import { createQueryClient } from "@/lib/queryClient";
import { createAppRouter } from "@/router";
import { AppShell } from "@/features/layout/AppShell";
import { reloadLayoutPrefs } from "@/features/layout/layoutPrefs";
import { resetTabsStore } from "@/features/layout/tabs/tabsStore";
import { clearPanels } from "@/features/layout/panels";
import { clearShortcuts } from "@/features/layout/shortcuts";
import { clearHooks } from "@/ui/hooks";
import { clearSlots, registerSlot } from "@/ui/slots";
import { resetSessionActivity } from "./activityStore";
import { DESKTOP_ID, connectMockDesktop, seedWorkspace, type MockLink, type MockLinkOptions } from "./dev/mockLink";
import { FOLDER_COLLAPSE_KEY, resetFolderCollapse } from "./folders";

let viewport = 1280;
function stubViewport(width: number) {
  viewport = width;
  Object.defineProperty(window, "innerWidth", { configurable: true, writable: true, value: width });
  window.matchMedia = ((query: string) => {
    const found = /min-width:\s*([\d.]+)(px|rem)/.exec(query);
    const min = found ? Number(found[1]) * (found[2] === "rem" ? 16 : 1) : null;
    return {
      get matches() {
        return min !== null && viewport >= min;
      },
      media: query,
      onchange: null,
      addEventListener: () => {},
      removeEventListener: () => {},
      addListener: () => {},
      removeListener: () => {},
      dispatchEvent: () => false,
    };
  }) as unknown as typeof window.matchMedia;
}

beforeAll(() => {
  Element.prototype.hasPointerCapture ??= () => false;
  Element.prototype.setPointerCapture ??= () => {};
  Element.prototype.releasePointerCapture ??= () => {};
  Element.prototype.scrollIntoView ??= () => {};
});

let previous: PersistAdapter;
beforeEach(() => {
  clearPanels();
  clearShortcuts();
  localStorage.clear();
  resetFolderCollapse();
  resetTabsStore();
  reloadLayoutPrefs();
  previous = registerPersistAdapter(new MemoryPersistAdapter());
  registerSlot("app-shell", AppShell);
});
afterEach(() => {
  cleanup();
  resetLinkConnections();
  clearLinkStores();
  registerPersistAdapter(previous);
  resetSessionActivity();
  clearSlots();
  clearHooks();
  clearPanels();
  clearShortcuts();
  localStorage.clear();
  stubViewport(1280);
});

type Call = { method: string; params: Record<string, unknown> };

/** Records every request the app sends. */
function spy(): Call[] {
  const connection = getLinkConnection(DESKTOP_ID);
  const calls: Call[] = [];
  const original = connection.request.bind(connection) as unknown as (method: string, params: Record<string, unknown>, ...rest: unknown[]) => unknown;
  connection.request = ((method: string, params: Record<string, unknown>, ...rest: unknown[]) => {
    calls.push({ method, params });
    return original(method, params, ...rest);
  }) as typeof connection.request;
  return calls;
}

function mount(width: number, options: MockLinkOptions = {}): { link: MockLink; queryClient: ReturnType<typeof createQueryClient> } {
  stubViewport(width);
  const link = connectMockDesktop({ ...options, seed: (desktop) => seedWorkspace(desktop, { sessions: 4, working: 0 }) });
  const queryClient = createQueryClient();
  const router = createAppRouter({ history: createMemoryHistory({ initialEntries: [`/d/${DESKTOP_ID}/p/proj-alpha`] }), queryClient });
  render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return { link, queryClient };
}

const folderSection = () => screen.findByRole("region", { name: "Folder Parser work" });
const cardIn = (scope: HTMLElement, id: string) => within(scope).getAllByTestId("session-card").find((element) => element.dataset.sessionId === id);

describe.each([390, 768, 1280])("folders in the session list at %i px", (width) => {
  it("groups the folder's sessions under its header and keeps them out of the loose list", async () => {
    mount(width);
    const section = await folderSection();
    const header = within(section).getByRole("button", { name: /^Parser work/ });
    expect(header).toHaveAttribute("aria-expanded", "true");
    expect(within(header).getByLabelText("2 sessions")).toBeInTheDocument();
    expect(cardIn(section, "sess-local-1")).toBeDefined();
    expect(cardIn(section, "sess-local-approval")).toBeDefined();
    const loose = screen.getByRole("list", { name: "Sessions" });
    expect(within(loose).queryByText("Fix the example parser")).toBeNull();
    expect(within(loose).getAllByTestId("session-card").length).toBeGreaterThan(0);
    // The folder comes before the loose sessions.
    expect(section.compareDocumentPosition(loose) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("folds on this device, remembers it, and follows the desktop when it changes the state", async () => {
    const user = userEvent.setup();
    const { link, queryClient } = mount(width);
    const section = await folderSection();
    await user.click(within(section).getByRole("button", { name: /^Parser work/ }));
    expect(within(section).getByRole("button", { name: /^Parser work/ })).toHaveAttribute("aria-expanded", "false");
    expect(within(section).queryAllByTestId("session-card")).toHaveLength(0);
    expect(JSON.parse(localStorage.getItem(`${FOLDER_COLLAPSE_KEY}.${DESKTOP_ID}`) ?? "{}")).toEqual({ "folder-parser": { desktop: false, local: true } });
    // The desktop folds it itself: the desktop's state is shown again once it differs from what was seen.
    const folder = link.desktop.world.folders[0]!;
    folder.collapsed = true;
    await act(async () => {
      await queryClient.invalidateQueries({ queryKey: catalogKey(DESKTOP_ID) });
    });
    expect(within(await folderSection()).getByRole("button", { name: /^Parser work/ })).toHaveAttribute("aria-expanded", "false");
    folder.collapsed = false;
    await act(async () => {
      await queryClient.invalidateQueries({ queryKey: catalogKey(DESKTOP_ID) });
    });
    await waitFor(() => expect(within(screen.getByRole("region", { name: "Folder Parser work" })).getByRole("button", { name: /^Parser work/ })).toHaveAttribute("aria-expanded", "true"));
  });

  it("shows a folder the desktop makes, renames and removes on the next catalog refresh", async () => {
    const { link, queryClient } = mount(width);
    await folderSection();
    const refresh = () => act(async () => void (await queryClient.invalidateQueries({ queryKey: catalogKey(DESKTOP_ID) })));
    link.desktop.world.createFolder("proj-alpha", ["sess-local-files"], "Reports");
    await refresh();
    expect(await screen.findByRole("region", { name: "Folder Reports" })).toBeInTheDocument();
    link.desktop.world.renameFolder("proj-alpha", link.desktop.world.folders[0]!.id, "Write-ups");
    await refresh();
    expect(await screen.findByRole("region", { name: "Folder Write-ups" })).toBeInTheDocument();
    link.desktop.world.deleteFolder("proj-alpha", "folder-parser");
    await refresh();
    await waitFor(() => expect(screen.queryByRole("region", { name: "Folder Parser work" })).toBeNull());
    // Its sessions are back in the loose list.
    await waitFor(() => expect(within(screen.getByRole("list", { name: "Sessions" })).getByText("Fix the example parser")).toBeInTheDocument());
  });

  it("lets a control device add a session to a folder, create one, take one out, rename and ungroup", async () => {
    const user = userEvent.setup();
    mount(width);
    const section = await folderSection();
    const calls = spy();
    const loose = await screen.findByRole("list", { name: "Sessions" });
    const looseCard = within(loose).getAllByTestId("session-card")[0]!;
    const looseId = looseCard.dataset.sessionId!;

    await user.click(within(looseCard).getByRole("button", { name: /Actions for/ }));
    await user.click(await screen.findByRole("menuitem", { name: "Add to folder" }));
    const dialog = await screen.findByRole("dialog", { name: "Add to folder" });
    await user.click(within(dialog).getByRole("button", { name: /^Parser work/ }));
    await waitFor(() => expect(calls.find((c) => c.method === "folder.move")?.params).toEqual({ projectId: "proj-alpha", conversationId: looseId, folderId: "folder-parser" }));
    await waitFor(() => expect(cardIn(within(screen.getByRole("region", { name: "Folder Parser work" })).getByRole("list", { name: "Sessions in Parser work" }), looseId)).toBeDefined());

    // Take it out again from its card.
    const inFolder = cardIn(screen.getByRole("region", { name: "Folder Parser work" }), looseId)!;
    await user.click(within(inFolder).getByRole("button", { name: /Actions for/ }));
    await user.click(await screen.findByRole("menuitem", { name: "Remove from folder" }));
    await waitFor(() => expect(calls.filter((c) => c.method === "folder.move").at(-1)?.params).toEqual({ projectId: "proj-alpha", conversationId: looseId, folderId: null }));
    await waitFor(() => expect(cardIn(screen.getByRole("list", { name: "Sessions" }), looseId)).toBeDefined());

    // A new folder from a session.
    const again = cardIn(screen.getByRole("list", { name: "Sessions" }), looseId)!;
    await user.click(within(again).getByRole("button", { name: /Actions for/ }));
    await user.click(await screen.findByRole("menuitem", { name: "Add to folder" }));
    const create = await screen.findByRole("dialog", { name: "Add to folder" });
    await user.type(within(create).getByRole("textbox", { name: "New folder" }), "Fresh ideas");
    await user.click(within(create).getByRole("button", { name: "Create folder" }));
    await waitFor(() => expect(calls.find((c) => c.method === "folder.create")?.params).toEqual({ projectId: "proj-alpha", conversationIds: [looseId], name: "Fresh ideas" }));
    expect(await screen.findByRole("region", { name: "Folder Fresh ideas" })).toBeInTheDocument();

    // Rename and ungroup from the folder's menu.
    await user.click(within(section).getByRole("button", { name: "Actions for folder Parser work" }));
    await user.click(await screen.findByRole("menuitem", { name: "Rename" }));
    const field = await screen.findByRole("textbox", { name: "Name" });
    await user.clear(field);
    await user.type(field, "Parser");
    await user.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(calls.find((c) => c.method === "folder.rename")?.params).toEqual({ projectId: "proj-alpha", folderId: "folder-parser", name: "Parser" }));
    const renamed = await screen.findByRole("region", { name: "Folder Parser" });
    await user.click(within(renamed).getByRole("button", { name: "Actions for folder Parser" }));
    await user.click(await screen.findByRole("menuitem", { name: "Ungroup" }));
    await user.click(await screen.findByRole("button", { name: "Ungroup" }));
    await waitFor(() => expect(calls.find((c) => c.method === "folder.delete")?.params).toEqual({ projectId: "proj-alpha", folderId: "folder-parser" }));
    await waitFor(() => expect(screen.queryByRole("region", { name: "Folder Parser" })).toBeNull());
  });

  it("shows a read-only device the folders and none of the folder edits", async () => {
    const user = userEvent.setup();
    mount(width, { permission: "read" });
    const section = await folderSection();
    expect(within(section).queryByRole("button", { name: /Actions for folder/ })).toBeNull();
    await user.click(within(section).getAllByRole("button", { name: /Actions for/ })[0]!);
    const menu = await screen.findByRole("menu");
    expect(within(menu).getByRole("menuitem", { name: "Open" })).toBeInTheDocument();
    expect(within(menu).queryByRole("menuitem", { name: /folder/i })).toBeNull();
  });
});

describe("folder edits are offered only where the desktop can do them", () => {
  it("hides the folder items on a desktop that sends no folders", async () => {
    const user = userEvent.setup();
    mount(390, { seed: (desktop) => void desktop });
    await folderSection();
    // An older desktop's catalog has no `folders`: drop them from every catalog answer.
    const connection = getLinkConnection(DESKTOP_ID);
    const original = connection.request.bind(connection) as unknown as (method: string, params: unknown, ...rest: unknown[]) => Promise<Record<string, unknown>>;
    connection.request = (async (method: string, params: unknown, ...rest: unknown[]) => {
      const result = await original(method, params, ...rest);
      if (method !== "workspace.list") return result;
      const { folders: _folders, ...older } = result;
      return older;
    }) as unknown as typeof connection.request;
    cleanup();
    const queryClient = createQueryClient();
    const router = createAppRouter({ history: createMemoryHistory({ initialEntries: [`/d/${DESKTOP_ID}/p/proj-alpha`] }), queryClient });
    render(
      <QueryClientProvider client={queryClient}>
        <RouterProvider router={router} />
      </QueryClientProvider>,
    );
    const card = (await screen.findAllByTestId("session-card"))[0]!;
    await user.click(within(card).getByRole("button", { name: /Actions for/ }));
    const menu = await screen.findByRole("menu");
    expect(within(menu).getByRole("menuitem", { name: "Rename" })).toBeInTheDocument();
    expect(within(menu).queryByRole("menuitem", { name: /folder/i })).toBeNull();
  });

  it("tells the user when taking a session out of a folder fails", async () => {
    const user = userEvent.setup();
    mount(390);
    const section = await folderSection();
    const connection = getLinkConnection(DESKTOP_ID);
    const original = connection.request.bind(connection) as unknown as (method: string, params: unknown, ...rest: unknown[]) => Promise<unknown>;
    connection.request = ((method: string, params: unknown, ...rest: unknown[]) => (method === "folder.move" ? Promise.reject(new Error("refused")) : original(method, params, ...rest))) as typeof connection.request;
    await user.click(within(cardIn(section, "sess-local-1")!).getByRole("button", { name: /Actions for/ }));
    await user.click(await screen.findByRole("menuitem", { name: "Remove from folder" }));
    const dialog = await screen.findByRole("dialog", { name: "Could not update the folders" });
    await user.click(within(dialog).getByRole("button", { name: "OK" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("forgets the collapse choice of a folder that is gone", async () => {
    const user = userEvent.setup();
    const { link, queryClient } = mount(390);
    const section = await folderSection();
    await user.click(within(section).getByRole("button", { name: /^Parser work/ }));
    const key = `${FOLDER_COLLAPSE_KEY}.${DESKTOP_ID}`;
    expect(Object.keys(JSON.parse(localStorage.getItem(key) ?? "{}"))).toEqual(["folder-parser"]);
    link.desktop.world.deleteFolder("proj-alpha", "folder-parser");
    await act(async () => {
      await queryClient.invalidateQueries({ queryKey: catalogKey(DESKTOP_ID) });
    });
    await waitFor(() => expect(JSON.parse(localStorage.getItem(key) ?? "{}")).toEqual({}));
  });
});
