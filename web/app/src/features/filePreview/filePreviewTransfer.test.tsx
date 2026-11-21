// F-60a: the same previews through encrypted transfers (`file.prepare`, capability `fileTransfer`): the desktop answers with an
// object and a key, the bytes come from `/api/transfers/...` (stubbed here from the mock desktop's in-memory server),
// are decrypted in the page, and a reopen of the same content is served from the tab cache.
import { QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { KNOWN_CAPABILITIES } from "@monoptah/protocol";
import { FIXTURE_HANDLERS, type MemoryBackend, type MockDesktop } from "@monoptah/mock-desktop";
import { IDBFactory } from "fake-indexeddb";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { MemoryPersistAdapter, registerPersistAdapter, type PersistAdapter } from "@/data";
import { resetLinkConnections } from "@/link";
import { clearLinkStores } from "@/link/store.ts";
import { resetSessionActivity } from "@/features/sessions/activityStore";
import { resetHighlighting } from "@/features/markdown/highlight/plugin";
import { DESKTOP_ID, connectMockDesktop, seedWorkspace, type MockLinkOptions } from "@/features/sessions/dev/mockLink";
import { resetMonoSeen } from "@/features/sessions/monoSeen";
import { stubTranscriptLayout, VIEW } from "@/features/transcript/layoutStub";
import { createQueryClient } from "@/lib/queryClient";
import { createAppRouter } from "@/router";
import { clearHooks } from "@/ui/hooks";
import { clearSlots } from "@/ui/slots";
import { contentCache } from "./transfer/cache";
import "@/features/monos/register";

const SESSION = "sess-local-files";
const WITH_TRANSFER = [...KNOWN_CAPABILITIES];

beforeAll(() => {
  window.matchMedia = ((query: string) => ({ matches: false, media: query, onchange: null, addEventListener() {}, removeEventListener() {}, addListener() {}, removeListener() {}, dispatchEvent: () => false })) as never;
  stubTranscriptLayout();
  Object.defineProperty(HTMLElement.prototype, "clientHeight", {
    configurable: true,
    get(this: HTMLElement) {
      return this.dataset.testid === "transcript-scroller" ? VIEW : 600;
    },
  });
  Object.defineProperty(HTMLElement.prototype, "clientWidth", { configurable: true, get: () => 800 });
});

const original = { ...FIXTURE_HANDLERS };
let previous: PersistAdapter;
let prepares: Array<{ conversationId?: string; path: string; asset?: string }>;
let legacy: string[];
let downloads: string[];
let backend: MemoryBackend;
const realFetch = globalThis.fetch;

beforeEach(() => {
  globalThis.indexedDB = new IDBFactory();
  localStorage.clear();
  resetMonoSeen();
  contentCache.clear();
  previous = registerPersistAdapter(new MemoryPersistAdapter());
  prepares = [];
  legacy = [];
  downloads = [];
  const prepare = FIXTURE_HANDLERS["file.prepare"];
  FIXTURE_HANDLERS["file.prepare"] = ((params: (typeof prepares)[number], ctx: never) => {
    prepares.push(params);
    return (prepare as (p: unknown, c: unknown) => unknown)(params, ctx);
  }) as never;
  const mentioned = FIXTURE_HANDLERS["chat.mentionedFile"];
  FIXTURE_HANDLERS["chat.mentionedFile"] = ((params: { path: string }, ctx: never) => {
    legacy.push(params.path);
    return (mentioned as (p: unknown, c: unknown) => unknown)(params, ctx);
  }) as never;
  URL.createObjectURL = vi.fn(() => "blob:https://app.test/1");
  URL.revokeObjectURL = vi.fn();
});
afterEach(() => {
  Object.assign(FIXTURE_HANDLERS, original);
  globalThis.fetch = realFetch;
  resetLinkConnections();
  clearLinkStores();
  registerPersistAdapter(previous);
  resetSessionActivity();
  resetHighlighting();
  clearSlots();
  clearHooks();
  localStorage.clear();
});

function mount(options: MockLinkOptions = {}) {
  const link = connectMockDesktop({
    capabilities: WITH_TRANSFER,
    ...options,
    seed: (desktop: MockDesktop) => {
      seedWorkspace(desktop);
      desktop.world.addAssistantText(SESSION, "Outside the project: `/tmp/outside-note.txt`, `/tmp/large-fixture.bin`, `/tmp/large-fixture.txt` and `/tmp/not-there.txt`; secret `/Users/example/.ssh/config.json`.");
      options.seed?.(desktop);
    },
  });
  backend = link.desktop.transfers.backend as MemoryBackend;
  // The Worker's download route, served from the mock desktop's in-memory server.
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = new URL(String(input), "https://app.test");
    if (!url.pathname.startsWith("/api/transfers/")) return realFetch(input, init);
    downloads.push(url.pathname + url.search);
    const sealed = backend.objects.get(decodeURIComponent(url.pathname.split("/").pop() ?? ""));
    return sealed ? new Response(sealed as BodyInit, { status: 200 }) : new Response("{}", { status: 404 });
  }) as typeof fetch;
  const queryClient = createQueryClient();
  const router = createAppRouter({ history: createMemoryHistory({ initialEntries: [`/d/${DESKTOP_ID}/s/${SESSION}`] }), queryClient });
  render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return link;
}

const mentionChip = async (text: string) => {
  const chips = await screen.findAllByTestId("file-mention");
  const found = chips.find((chip) => chip.textContent === text || chip.getAttribute("data-path") === text);
  if (!found) throw new Error(`No mention ${text}; there are ${chips.map((chip) => chip.textContent).join(" | ")}`);
  return found;
};
const dialog = () => screen.findByTestId("file-preview-dialog");

describe("files through encrypted transfers", () => {
  it("shows an html report with its image, asking the desktop with file.prepare and never chat.mentionedFile", async () => {
    const user = userEvent.setup();
    mount();
    await user.click(await mentionChip("report.html"));
    const frame = await within(await dialog()).findByTestId("preview-html");
    expect(frame.getAttribute("sandbox")).toBe("");
    expect(prepares.map((request) => request.asset)).toEqual([undefined, "img/chart.png"]);
    expect(prepares[0]).toMatchObject({ conversationId: SESSION, path: "report.html" });
    expect(legacy).toEqual([]);
    expect(downloads).toHaveLength(2);
    for (const url of downloads) expect(url).toMatch(new RegExp(`^/api/transfers/${DESKTOP_ID}/[0-9a-f]{64}\\.[0-9a-f]{16}\\?device=`));
    expect(within(await dialog()).getByTestId("file-preview-path")).toHaveTextContent("/example/alpha/report.html");
  });

  it("opens a file outside the shared projects for a Control device, and a reopen is served from the cache", async () => {
    const user = userEvent.setup();
    const link = mount();
    await user.click(await mentionChip("/tmp/outside-note.txt"));
    const lines = await within(await dialog()).findByTestId("preview-lines");
    expect(lines.textContent).toContain("A note outside every project.");
    expect(downloads).toHaveLength(1);
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByTestId("file-preview-dialog")).toBeNull());
    await user.click(await mentionChip("/tmp/outside-note.txt"));
    expect(await within(await dialog()).findByTestId("preview-lines")).toBeInTheDocument();
    // The desktop is asked again (a changed file has a new hash); the bytes are not downloaded again.
    expect(prepares.filter((request) => request.path === "/tmp/outside-note.txt")).toHaveLength(2);
    expect(downloads).toHaveLength(1);
    expect(link.desktop.transfers.uploads).toHaveLength(1);
  });

  it("refuses a read-only device a file outside its projects, with the reason", async () => {
    const user = userEvent.setup();
    mount({ permission: "read" });
    await user.click(await mentionChip("/tmp/outside-note.txt"));
    const problem = await within(await dialog()).findByTestId("file-preview-error");
    expect(problem).toHaveAttribute("data-problem", "not_mentioned");
    expect(problem).toHaveTextContent("This device cannot open this file");
    expect(downloads).toEqual([]);
  });

  it("hides sensitive files from every device", async () => {
    const user = userEvent.setup();
    mount();
    await user.click(await mentionChip("/Users/example/.ssh/config.json"));
    expect(await within(await dialog()).findByTestId("file-preview-error")).toHaveAttribute("data-problem", "denied");
  });

  it("says a missing file is not found", async () => {
    const user = userEvent.setup();
    mount();
    await user.click(await mentionChip("/tmp/not-there.txt"));
    expect(await within(await dialog()).findByTestId("file-preview-error")).toHaveAttribute("data-problem", "not_found");
  });

  it("offers a binary file over 4 MB as a download", async () => {
    const user = userEvent.setup();
    mount();
    await user.click(await mentionChip("/tmp/large-fixture.bin"));
    const box = await dialog();
    const body = await within(box).findByTestId("preview-binary");
    const names: string[] = [];
    vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(function (this: HTMLAnchorElement) {
      names.push(this.download);
    });
    await user.click(within(body).getByTestId("file-preview-download"));
    expect(names).toEqual(["large-fixture.bin"]);
    expect(body).toHaveTextContent("5 MB - /tmp/large-fixture.bin");
    expect(downloads).toHaveLength(1);
  });

  it("says the desktop is preparing the file until the download starts, and waits as long as the upload takes", async () => {
    const user = userEvent.setup();
    let release: () => void = () => undefined;
    const gate = new Promise<void>((resolve) => (release = resolve));
    const slow = FIXTURE_HANDLERS["file.prepare"];
    FIXTURE_HANDLERS["file.prepare"] = (async (params: unknown, ctx: never) => {
      await gate;
      return (slow as (p: unknown, c: unknown) => unknown)(params, ctx);
    }) as never;
    mount();
    await user.click(await mentionChip("/tmp/outside-note.txt"));
    const note = await within(await dialog()).findByTestId("file-preview-preparing");
    expect(note).toHaveTextContent("Your desktop is preparing this file");
    expect(downloads).toEqual([]);
    release();
    expect(await within(await dialog()).findByTestId("preview-lines")).toBeInTheDocument();
    expect(screen.queryByTestId("file-preview-preparing")).toBeNull();
  });

  it("shows the progress of a large download while it comes, then the file", async () => {
    const user = userEvent.setup();
    const link = mount();
    let release: () => void = () => undefined;
    const gate = new Promise<void>((resolve) => (release = resolve));
    const served = globalThis.fetch;
    // The same route, but the body stops after 2 MiB until the test lets the rest through.
    globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
      const response = await served(input, init);
      if (!String(input).includes("/api/transfers/")) return response;
      const bytes = new Uint8Array(await response.arrayBuffer());
      return new Response(
        new ReadableStream<Uint8Array>({
          async start(controller) {
            controller.enqueue(bytes.slice(0, 2 * 1024 * 1024));
            await gate;
            controller.enqueue(bytes.slice(2 * 1024 * 1024));
            controller.close();
          },
        }),
        { status: 200 },
      );
    }) as typeof fetch;
    await user.click(await mentionChip("/tmp/large-fixture.bin"));
    const bar = await screen.findByTestId("file-preview-progress", {}, { timeout: 5_000 });
    expect(within(bar).getByRole("progressbar")).toHaveAttribute("max");
    expect(bar).toHaveTextContent(/MB of 5 MB/);
    release();
    expect(await within(await dialog()).findByTestId("preview-binary")).toBeInTheDocument();
    expect(link.desktop.transfers.uploads).toHaveLength(1);
  }, 20_000);

  it("opens a text file over 4 MB", async () => {
    const user = userEvent.setup();
    mount();
    await user.click(await mentionChip("/tmp/large-fixture.txt"));
    const lines = await within(await dialog()).findByTestId("preview-lines", {}, { timeout: 10_000 });
    expect(lines).toBeInTheDocument();
    expect(downloads).toHaveLength(1);
  }, 20_000);

  it("keeps working with a desktop that has no fileTransfer: the paged chat.mentionedFile", async () => {
    const user = userEvent.setup();
    mount({ capabilities: WITH_TRANSFER.filter((name) => name !== "fileTransfer") });
    await user.click(await mentionChip("report.html"));
    await within(await dialog()).findByTestId("preview-html");
    expect(prepares).toEqual([]);
    expect(legacy.length).toBeGreaterThan(0);
    expect(downloads).toEqual([]);
  });

  it("tells the user to update an old desktop that refuses a file nothing in the chat named", async () => {
    const user = userEvent.setup();
    mount({ capabilities: WITH_TRANSFER.filter((name) => name !== "fileTransfer" && name !== "mentionedFiles2") });
    await user.click(await mentionChip("/tmp/outside-note.txt"));
    expect(await within(await dialog()).findByTestId("file-preview-error")).toHaveAttribute("data-problem", "update_desktop");
  });
});
