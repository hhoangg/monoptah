// U-05: the files route against the real link wired to the mock desktop, whose file methods are replaced
// by a small project tree (and restored after each test).
import { QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { FIXTURE_HANDLERS, FixtureError } from "@monoptah/mock-desktop";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { MemoryPersistAdapter, registerPersistAdapter, type PersistAdapter } from "@/data";
import { AppShell } from "@/features/layout/AppShell";
import { clearPanels, registerPanel, usePanels } from "@/features/layout/panels";
import { clearShortcuts, registerShortcut } from "@/features/layout/shortcuts";
import { DESKTOP_ID, connectMockDesktop, seedWorkspace } from "@/features/sessions/dev/mockLink";
import { resetSessionActivity } from "@/features/sessions/activityStore";
import { resetHighlighting } from "@/features/markdown/highlight/plugin";
import { resetLinkConnections } from "@/link";
import { clearLinkStores } from "@/link/store.ts";
import { createQueryClient } from "@/lib/queryClient";
import { createAppRouter } from "@/router";
import { clearHooks, registerSessionCapabilities } from "@/ui/hooks";
import { clearSlots, registerSlot } from "@/ui/slots";
import { WRAP_KEY, reloadFilePrefs } from "./filePrefs";
import { FilesPanel } from "./FilesPanel";
import { closeQuickOpen, currentProject, startQuickOpen, useQuickOpen } from "./quickOpen";

function stubViewport(width: number) {
  window.matchMedia = ((query: string) => {
    const min = /min-width:\s*(\d+)px/.exec(query);
    return {
      matches: min ? width >= Number(min[1]) : false,
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
  // jsdom has no layout: give every element a size so the virtualizer renders rows.
  Object.defineProperty(HTMLElement.prototype, "clientHeight", { configurable: true, get: () => 600 });
  Object.defineProperty(HTMLElement.prototype, "clientWidth", { configurable: true, get: () => 800 });
});

type Entry = { name: string; isDir: boolean; ignored: boolean };
const dir = (name: string, ignored = false): Entry => ({ name, isDir: true, ignored });
const file = (name: string, ignored = false): Entry => ({ name, isDir: false, ignored });

const TREE: Record<string, Entry[]> = {
  "": [file("README.md"), dir("src"), dir("node_modules", true), dir("docs"), file(".env")],
  src: [file("main.ts"), file("util.ts"), file("evil.txt"), file("evil.ts"), file("big.txt")],
  node_modules: [file("dep.js")],
  docs: [file("guide.md"), file("raw.md")],
};
const TEXT: Record<string, string> = {
  "README.md": "# Hello\n\nSome **bold** text\n",
  "src/main.ts": "const answer: number = 42;\n",
  "src/util.ts": "export const x = 1;\n",
  "src/evil.txt": "<img src=x onerror=alert(1)>\n",
  "src/big.txt": "partial\n",
  "src/evil.ts": 'const s = "<img src=x onerror=alert(1)>";\n',
  "docs/guide.md": "# Guide\n",
  "docs/raw.md": "# Raw\n\n<img src=x onerror=alert(1)>\n\n<script>alert(2)</script>\n\n<b>bold html</b>\n",
};

const handlers = FIXTURE_HANDLERS as unknown as Record<string, (p: never, c: never) => unknown>;
const originals = { files: handlers["project.files"], file: handlers["project.file"], search: handlers["project.search"] };
let calls: { files: string[]; search: string[] };

beforeEach(() => {
  calls = { files: [], search: [] };
  handlers["project.files"] = ((p: { path?: string }) => {
    calls.files.push(p.path ?? "");
    const entries = TREE[p.path ?? ""];
    if (!entries) throw new FixtureError("No such folder");
    return { entries };
  }) as never;
  handlers["project.file"] = ((p: { path: string }) => {
    if (p.path === ".env") throw new FixtureError("This file is hidden", "path_denied");
    const text = TEXT[p.path];
    if (text === undefined) throw new FixtureError("No such file");
    return { path: p.path, text, truncated: p.path === "src/big.txt" };
  }) as never;
  handlers["project.search"] = ((p: { query: string }) => {
    calls.search.push(p.query);
    const all = ["src/main.ts", "src/util.ts", "README.md"];
    return { results: all.filter((path) => path.toLowerCase().includes(p.query.toLowerCase())).map((path) => ({ path, name: path.split("/").pop() ?? path })) };
  }) as never;
});

let previous: PersistAdapter;
beforeEach(() => {
  localStorage.clear();
  reloadFilePrefs();
  previous = registerPersistAdapter(new MemoryPersistAdapter());
});
afterEach(() => {
  handlers["project.files"] = originals.files as never;
  handlers["project.file"] = originals.file as never;
  handlers["project.search"] = originals.search as never;
  vi.useRealTimers();
  closeQuickOpen();
  resetLinkConnections();
  clearLinkStores();
  registerPersistAdapter(previous);
  resetSessionActivity();
  resetHighlighting();
  clearSlots();
  clearHooks();
  clearPanels();
  clearShortcuts();
  localStorage.clear();
  reloadFilePrefs();
});

function mount(path: string, { width = 1280, shell = false, route }: { width?: number; shell?: boolean; route?: string } = {}) {
  stubViewport(width);
  if (shell) registerSlot("app-shell", AppShell);
  connectMockDesktop({ seed: seedWorkspace });
  const queryClient = createQueryClient();
  const router = createAppRouter({ history: createMemoryHistory({ initialEntries: [`/d/${DESKTOP_ID}${route ?? `/p/proj-alpha/files/${path}`}`] }), queryClient });
  render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return { router };
}

describe("file tree (F-59)", () => {
  it("loads the root, opens folders lazily and dims ignored entries", async () => {
    const user = userEvent.setup();
    mount("");
    const tree = await screen.findByRole("tree", { name: "Project files" });
    await within(tree).findByRole("treeitem", { name: /README\.md/ });
    const ignored = within(tree).getByRole("treeitem", { name: /node_modules/ });
    expect(ignored).toHaveAttribute("data-ignored", "true");
    expect(within(tree).getByRole("treeitem", { name: /^src$/ })).not.toHaveAttribute("data-ignored");
    // Folders first.
    const names = within(tree).getAllByRole("treeitem").map((item) => item.textContent);
    expect(names.indexOf("docs")).toBeLessThan(names.indexOf("README.md"));
    // The folders are prefetched, so no folder request is needed when one opens.
    await waitFor(() => expect(calls.files).toContain("src"));
    expect(within(tree).queryByRole("treeitem", { name: /main\.ts/ })).toBeNull();
    await user.click(within(tree).getByRole("treeitem", { name: /^src$/ }));
    expect(await within(tree).findByRole("treeitem", { name: /main\.ts/ })).toBeInTheDocument();
    expect(calls.files.filter((path) => path === "src")).toHaveLength(1);
  });

  it("prefetches only the first four folders", async () => {
    TREE["f5"] = [file("a")];
    for (const name of ["f1", "f2", "f3", "f4"]) TREE[name] = [file("a")];
    const root = TREE[""] as Entry[];
    const saved = [...root];
    root.splice(0, root.length, dir("f1"), dir("f2"), dir("f3"), dir("f4"), dir("f5"));
    try {
      mount("");
      await screen.findByRole("treeitem", { name: /^f5$/ });
      await waitFor(() => expect(calls.files.filter((path) => path !== "")).toHaveLength(4));
      expect(calls.files).not.toContain("f5");
    } finally {
      root.splice(0, root.length, ...saved);
    }
  });

  it("refreshes what is loaded", async () => {
    const user = userEvent.setup();
    mount("");
    await screen.findByRole("treeitem", { name: /README\.md/ });
    const before = calls.files.filter((path) => path === "").length;
    await user.click(screen.getByRole("button", { name: "Refresh files" }));
    await waitFor(() => expect(calls.files.filter((path) => path === "").length).toBeGreaterThan(before));
  });
});

describe("file viewer (F-60)", () => {
  it("shows highlighted code as token spans and never as markup", async () => {
    mount("src/main.ts");
    const lines = await screen.findByTestId("file-lines");
    await waitFor(() => expect(lines.querySelector(".md-token")).not.toBeNull());
    expect(lines.textContent).toContain("const answer: number = 42;");
    expect(lines.querySelectorAll("span.md-token").length).toBeGreaterThan(1);
  });

  it("renders agent-controlled text as text, with no element created from it", async () => {
    mount("src/evil.txt");
    const lines = await screen.findByTestId("file-lines");
    await waitFor(() => expect(lines.textContent).toContain("<img src=x onerror=alert(1)>"));
    expect(lines.querySelector("img")).toBeNull();
    expect(document.querySelector("img[onerror]")).toBeNull();
  });

  it("renders a string that looks like markup on the highlighted path as text", async () => {
    mount("src/evil.ts");
    const lines = await screen.findByTestId("file-lines");
    await waitFor(() => expect(lines.querySelector(".md-token")).not.toBeNull());
    expect(lines.textContent).toContain("<img src=x onerror=alert(1)>");
    expect(lines.querySelector("img")).toBeNull();
    expect(document.querySelector("img[onerror]")).toBeNull();
  });

  it("previews markdown with raw HTML without creating an element from it", async () => {
    const user = userEvent.setup();
    mount("docs/raw.md");
    await screen.findByTestId("file-lines");
    await user.click(screen.getByRole("radio", { name: "Preview" }));
    const preview = await screen.findByTestId("markdown-preview");
    await waitFor(() => expect(within(preview).getByRole("heading", { name: "Raw" })).toBeInTheDocument());
    expect(preview.querySelector("img, script, b")).toBeNull();
    expect(document.querySelector("img[onerror], script")).toBeNull();
  });

  it("starts each file in its own state (the viewer is keyed by path)", async () => {
    const user = userEvent.setup();
    const { router } = mount("README.md");
    await screen.findByTestId("file-lines");
    await user.click(screen.getByRole("radio", { name: "Preview" }));
    await screen.findByTestId("markdown-preview");
    await act(async () => router.history.push(`/d/${DESKTOP_ID}/p/proj-alpha/files/docs/guide.md`));
    await screen.findByTestId("file-lines");
    expect(screen.getByRole("radio", { name: "Code" })).toBeChecked();
    expect(screen.queryByTestId("markdown-preview")).toBeNull();
  });

  it("remembers the wrap toggle", async () => {
    const user = userEvent.setup();
    mount("src/main.ts");
    const toggle = await screen.findByRole("button", { name: "Wrap lines" });
    await screen.findByTestId("file-lines");
    expect(toggle).toHaveAttribute("aria-pressed", "false");
    expect(screen.getByTestId("file-lines")).toHaveAttribute("data-wrap", "false");
    await user.click(toggle);
    expect(toggle).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByTestId("file-lines")).toHaveAttribute("data-wrap", "true");
    expect(localStorage.getItem(WRAP_KEY)).toBe("1");
    // The stored value is what a reload reads back.
    reloadFilePrefs();
    expect(screen.getByRole("button", { name: "Wrap lines" })).toHaveAttribute("aria-pressed", "true");
    await user.click(screen.getByRole("button", { name: "Wrap lines" }));
    expect(localStorage.getItem(WRAP_KEY)).toBe("0");
  });

  it("reads the stored wrap preference", async () => {
    localStorage.setItem(WRAP_KEY, "1");
    reloadFilePrefs();
    mount("src/main.ts");
    await screen.findByTestId("file-lines");
    expect(screen.getByRole("button", { name: "Wrap lines" })).toHaveAttribute("aria-pressed", "true");
  });

  it("shows a notice for a truncated file", async () => {
    mount("src/big.txt");
    expect(await screen.findByText("Only the first part of this file is shown.")).toBeInTheDocument();
  });

  it("shows the hidden message for a denied file", async () => {
    mount(".env");
    expect(await screen.findByText("This file is hidden by Monoptah")).toBeInTheDocument();
    expect(screen.queryByTestId("file-lines")).toBeNull();
  });

  it("shows the missing-file illustration for another error", async () => {
    mount("src/gone.ts");
    expect(await screen.findByText("This file could not be opened")).toBeInTheDocument();
    expect(screen.getByTestId("file-problem").querySelector("svg")).not.toBeNull();
    expect(screen.queryByText("This file is hidden by Monoptah")).toBeNull();
  });

  it("previews markdown through the safe renderer", async () => {
    const user = userEvent.setup();
    mount("README.md");
    await screen.findByTestId("file-lines");
    await user.click(screen.getByRole("radio", { name: "Preview" }));
    const preview = await screen.findByTestId("markdown-preview");
    expect(within(preview).getByRole("heading", { name: "Hello" })).toBeInTheDocument();
    expect(preview.querySelector("strong")).toHaveTextContent("bold");
  });

  it("opens a folder path as the tree with the folder opened", async () => {
    mount("src");
    const tree = await screen.findByRole("tree", { name: "Project files" });
    expect(await within(tree).findByRole("treeitem", { name: /main\.ts/ })).toBeInTheDocument();
    expect(screen.queryByTestId("file-viewer")).toBeNull();
  });
});

describe("search (F-61)", () => {
  it("sends the query 150 ms after the last key", async () => {
    mount("");
    const box = await screen.findByRole("searchbox", { name: "Search files" });
    await screen.findByRole("treeitem", { name: /README\.md/ });
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    fireEvent.change(box, { target: { value: "ma" } });
    await act(async () => {
      vi.advanceTimersByTime(149);
    });
    expect(calls.search).toEqual([]);
    fireEvent.change(box, { target: { value: "mai" } });
    await act(async () => {
      vi.advanceTimersByTime(149);
    });
    expect(calls.search).toEqual([]);
    await act(async () => {
      vi.advanceTimersByTime(1);
    });
    // The relay may use timers too: keep the fake clock running until the result shows.
    for (let i = 0; i < 50 && !screen.queryByRole("button", { name: /src\/main\.ts/ }); i++) {
      await act(async () => {
        await vi.advanceTimersByTimeAsync(10);
      });
    }
    expect(screen.getByRole("button", { name: /src\/main\.ts/ })).toBeInTheDocument();
    expect(calls.search).toEqual(["mai"]);
  });
});

describe("quick open (Mod+P)", () => {
  it("opens from the shortcut, ranks hits and navigates on Enter", async () => {
    const user = userEvent.setup();
    registerShortcut("quick-open", startQuickOpen);
    const { router } = mount("", { shell: true });
    await screen.findByRole("treeitem", { name: /README\.md/ });
    fireEvent.keyDown(window, { key: "p", ctrlKey: true });
    const input = await screen.findByRole("combobox", { name: "Find a file" });
    await user.type(input, "util");
    const option = await screen.findByRole("option", { name: /src\/util\.ts/ });
    expect(option).toHaveAttribute("aria-selected", "true");
    await user.keyboard("{Enter}");
    await waitFor(() => expect(router.state.location.pathname).toBe(`/d/${DESKTOP_ID}/p/proj-alpha/files/src/util.ts`));
    await waitFor(() => expect(screen.queryByRole("combobox", { name: "Find a file" })).toBeNull());
    expect(await screen.findByTestId("file-viewer")).toBeInTheDocument();
  });

  it("moves the selection with the arrow keys", async () => {
    const user = userEvent.setup();
    registerShortcut("quick-open", startQuickOpen);
    mount("", { shell: true });
    await screen.findByRole("treeitem", { name: /README\.md/ });
    fireEvent.keyDown(window, { key: "p", ctrlKey: true });
    const input = await screen.findByRole("combobox", { name: "Find a file" });
    await user.type(input, "s");
    const options = await screen.findAllByRole("option");
    expect(options.length).toBeGreaterThan(1);
    await user.keyboard("{ArrowDown}");
    await waitFor(() => expect(screen.getAllByRole("option")[1]).toHaveAttribute("aria-selected", "true"));
  });
});

describe("quick open: which project, and when", () => {
  const pressModP = () => fireEvent.keyDown(window, { key: "p", ctrlKey: true });
  const localOnly = (capability: string, context: { target?: { kind: string } | null }) =>
    !["files", "changes", "git"].includes(capability) || !context.target || context.target.kind === "local";

  beforeEach(() => {
    registerPanel("files", { title: "Files", component: FilesPanel });
    registerShortcut("quick-open", startQuickOpen);
  });

  it("searches the session's project on a session route with no earlier Files use", async () => {
    mount("", { shell: true, route: "/s/sess-local-1" });
    await waitFor(() => expect(usePanels.getState().scope).toEqual({ desktopId: DESKTOP_ID, projectId: "proj-alpha" }));
    pressModP();
    expect(await screen.findByRole("combobox", { name: "Find a file" })).toBeInTheDocument();
    expect(useQuickOpen.getState().target).toEqual({ desktopId: DESKTOP_ID, projectId: "proj-alpha" });
    expect(await screen.findByTestId("files-panel")).toBeInTheDocument();
  });

  it("opens from a project screen where no Files surface is mounted", async () => {
    mount("", { shell: true, route: "/p/proj-alpha" });
    await waitFor(() => expect(usePanels.getState().scope?.projectId).toBe("proj-alpha"));
    expect(screen.queryByTestId("files-panel")).toBeNull();
    pressModP();
    expect(await screen.findByRole("combobox", { name: "Find a file" })).toBeInTheDocument();
  });

  it("does nothing, and opens no Files panel, for a session the predicate refuses", async () => {
    registerSessionCapabilities(localOnly);
    mount("", { shell: true, route: "/s/sess-codespace-1" });
    await waitFor(() => expect(usePanels.getState().scope?.projectId).toBe("proj-alpha"));
    expect(usePanels.getState().scopeTarget).toEqual({ kind: "codespace" });
    pressModP();
    await new Promise((resolve) => setTimeout(resolve, 150));
    expect(screen.queryByRole("combobox", { name: "Find a file" })).toBeNull();
    expect(usePanels.getState().open).toBeNull();
    expect(useQuickOpen.getState().target).toBeNull();
  });

  it("uses the project in view, not a Files panel left open on another one, and never pops up later", async () => {
    const { router } = mount("", { shell: true, route: "/p/proj-alpha" });
    await waitFor(() => expect(usePanels.getState().scope?.projectId).toBe("proj-alpha"));
    pressModP();
    await screen.findByRole("combobox", { name: "Find a file" });
    fireEvent.keyDown(screen.getByRole("combobox", { name: "Find a file" }), { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("combobox", { name: "Find a file" })).toBeNull());
    // The Files panel of alpha is still the open one; beta is on screen now, so alpha's panel hides.
    await act(async () => router.history.push(`/d/${DESKTOP_ID}/p/proj-beta`));
    await waitFor(() => expect(screen.queryByTestId("files-panel")).toBeNull());
    expect(usePanels.getState().open?.projectId).toBe("proj-alpha");
    expect(currentProject()).toEqual({ desktopId: DESKTOP_ID, projectId: "proj-beta" });
    pressModP();
    await screen.findByRole("combobox", { name: "Find a file" });
    expect(useQuickOpen.getState().target?.projectId).toBe("proj-beta");
  });

  it("drops a pending target that no surface shows, so returning to the project does not pop the dialog up", async () => {
    const { router } = mount("", { shell: true, route: "/p/proj-alpha" });
    await waitFor(() => expect(usePanels.getState().scope?.projectId).toBe("proj-alpha"));
    pressModP();
    await screen.findByRole("combobox", { name: "Find a file" });
    await act(async () => router.history.push(`/d/${DESKTOP_ID}/p/proj-beta`));
    await waitFor(() => expect(screen.queryByTestId("files-panel")).toBeNull());
    expect(useQuickOpen.getState().target).toBeNull();
    await act(async () => router.history.push(`/d/${DESKTOP_ID}/p/proj-alpha`));
    await screen.findByTestId("files-panel");
    await new Promise((resolve) => setTimeout(resolve, 200));
    expect(screen.queryByRole("combobox", { name: "Find a file" })).toBeNull();
  });

  it("has no project to search outside a project or session screen", async () => {
    mount("", { shell: true, route: "/settings" });
    await screen.findByTestId("app-shell");
    expect(currentProject("/settings")).toBeUndefined();
    pressModP();
    await new Promise((resolve) => setTimeout(resolve, 100));
    expect(usePanels.getState().open).toBeNull();
  });

  it("at 768, a press on the dialog's scrim closes quick open and leaves the Files overlay alone", async () => {
    const user = userEvent.setup({ pointerEventsCheck: 0 });
    mount("", { shell: true, width: 768, route: "/p/proj-alpha" });
    await waitFor(() => expect(usePanels.getState().scope?.projectId).toBe("proj-alpha"));
    pressModP();
    await screen.findByTestId("panel-overlay");
    await screen.findByRole("dialog", { name: "Quick open" });
    const scrim = document.querySelector('[data-slot="dialog-overlay"]');
    expect(scrim).not.toBeNull();
    await user.click(scrim as Element);
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Quick open" })).toBeNull());
    expect(usePanels.getState().open?.kind).toBe("files");
    expect(screen.getByTestId("panel-overlay")).toBeInTheDocument();
    expect(useQuickOpen.getState().target).toBeNull();
  });

  it("opens the top hit of what is typed when Enter comes inside the debounce", async () => {
    const user = userEvent.setup();
    const { router } = mount("", { shell: true });
    await screen.findByRole("treeitem", { name: /README\.md/ });
    pressModP();
    const input = await screen.findByRole("combobox", { name: "Find a file" });
    await user.type(input, "util");
    await screen.findByRole("option", { name: /src\/util\.ts/ });
    // A new query, and Enter before its 150 ms are over: the old query's top hit (util) must not open.
    await user.clear(input);
    await user.type(input, "READ{Enter}");
    await waitFor(() => expect(router.state.location.pathname).toBe(`/d/${DESKTOP_ID}/p/proj-alpha/files/README.md`));
  });
});

describe("files route and the session-capabilities predicate", () => {
  it("shows a notice and reads nothing when files is refused", async () => {
    registerSessionCapabilities((capability) => capability !== "files");
    mount("src/main.ts");
    expect(await screen.findByText("Files are not available for this project.")).toBeInTheDocument();
    expect(screen.queryByTestId("files-browser")).toBeNull();
    expect(calls.files).toEqual([]);
  });
});

describe("file tree keyboard (WAI-ARIA tree)", () => {
  it("has one tab stop and moves with the arrow keys, Right and Left", async () => {
    const user = userEvent.setup();
    mount("");
    const tree = await screen.findByRole("tree", { name: "Project files" });
    await within(tree).findByRole("treeitem", { name: /README\.md/ });
    const items = () => within(tree).getAllByRole("treeitem");
    expect(items().filter((item) => item.tabIndex === 0)).toHaveLength(1);
    expect(items()[0]).toHaveAttribute("tabindex", "0");
    items()[0]!.focus();
    await user.keyboard("{ArrowDown}");
    expect(items()[1]).toHaveFocus();
    expect(items().filter((item) => item.tabIndex === 0)).toEqual([items()[1]]);
    await user.keyboard("{End}");
    expect(items().at(-1)).toHaveFocus();
    await user.keyboard("{Home}");
    expect(items()[0]).toHaveFocus();
    // The first entry is a folder (folders sort first): Right opens it, Right again enters it, Left goes back up.
    expect(items()[0]).toHaveAttribute("aria-expanded", "false");
    await user.keyboard("{ArrowRight}");
    expect(items()[0]).toHaveAttribute("aria-expanded", "true");
    await within(tree).findByRole("treeitem", { name: /guide\.md/ });
    await user.keyboard("{ArrowRight}");
    expect(items()[1]).toHaveFocus();
    expect(items()[1]).toHaveAttribute("aria-level", "2");
    await user.keyboard("{ArrowLeft}");
    expect(items()[0]).toHaveFocus();
    await user.keyboard("{ArrowLeft}");
    expect(items()[0]).toHaveAttribute("aria-expanded", "false");
  });
});
