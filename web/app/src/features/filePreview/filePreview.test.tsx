// F-45 and F-60: clicking a file mention in a session chat or a Mono chat, against the real link wired to the mock
// desktop (its `chat.mentionedFile` fixtures: an html report with an image, markdown, png, svg, text, a binary, a
// hidden `.env`, a file that is too large or gone).
import { QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { FIXTURE_HANDLERS, FixtureError, MENTION_MESSAGE, MOCK_PAGE_BYTES, type MockDesktop } from "@monoptah/mock-desktop";
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
import "@/features/monos/register";

const MONO = "mono-example";
const SESSION = "sess-local-files";

beforeAll(() => {
  window.matchMedia = ((query: string) => ({ matches: false, media: query, onchange: null, addEventListener() {}, removeEventListener() {}, addListener() {}, removeListener() {}, dispatchEvent: () => false })) as never;
  stubTranscriptLayout();
  // The code lines of the viewer need a height too (the transcript stub gives only its scroller one).
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
let requests: Array<{ conversationId: string; path: string; asset?: string; offset?: number }>;
const blobs = new Map<string, Blob>();
let blobCount = 0;
const revoked: string[] = [];

beforeEach(() => {
  globalThis.indexedDB = new IDBFactory();
  localStorage.clear();
  resetMonoSeen();
  previous = registerPersistAdapter(new MemoryPersistAdapter());
  requests = [];
  blobs.clear();
  revoked.length = 0;
  blobCount = 0;
  const handler = FIXTURE_HANDLERS["chat.mentionedFile"];
  FIXTURE_HANDLERS["chat.mentionedFile"] = ((params: (typeof requests)[number], ctx: never) => {
    requests.push(params);
    return (handler as (p: unknown, c: unknown) => unknown)(params, ctx);
  }) as never;
  URL.createObjectURL = vi.fn((blob: Blob) => {
    const url = `blob:https://app.test/${++blobCount}`;
    blobs.set(url, blob);
    return url;
  });
  URL.revokeObjectURL = vi.fn((url: string) => void revoked.push(url));
});
afterEach(() => {
  Object.assign(FIXTURE_HANDLERS, original);
  resetLinkConnections();
  clearLinkStores();
  registerPersistAdapter(previous);
  resetSessionActivity();
  resetHighlighting();
  clearSlots();
  clearHooks();
  localStorage.clear();
});

function seedMonoChat(desktop: MockDesktop) {
  const base = desktop.world.sessions.get("sess-local-1")!;
  desktop.world.sessions.set(`mono:${MONO}`, { ...base, id: `mono:${MONO}`, turns: structuredClone(base.turns), approvals: [], script: [] } as never);
  desktop.world.addAssistantText(`mono:${MONO}`, MENTION_MESSAGE);
}

function mount(path: string, options: MockLinkOptions = {}) {
  const link = connectMockDesktop({
    ...options,
    seed: (desktop) => {
      seedWorkspace(desktop);
      seedMonoChat(desktop);
      desktop.world.addAssistantText(SESSION, "Also see `private/unmentioned-by-desktop.txt` for the rest.");
      desktop.world.addAssistantText(`mono:${MONO}`, "Also see `private/unmentioned-by-desktop.txt` for the rest.");
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
  return { router, link };
}

const mentionChip = async (text: string | RegExp) => {
  const chips = await screen.findAllByTestId("file-mention");
  const found = chips.find((chip) => (typeof text === "string" ? chip.textContent === text || chip.getAttribute("data-path") === text : text.test(chip.textContent ?? "")));
  if (!found) throw new Error(`No mention ${String(text)}; there are ${chips.map((chip) => chip.textContent).join(" | ")}`);
  return found;
};
const dialog = () => screen.findByTestId("file-preview-dialog");
const blobText = (blob: Blob) =>
  new Promise<string>((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result));
    reader.onerror = () => reject(reader.error);
    reader.readAsText(blob);
  });

describe("mentions in a session chat", () => {
  it("makes an inline code file a clickable chip with the icon of its file type", async () => {
    mount(`/s/${SESSION}`);
    const chip = await mentionChip("report.html");
    expect(chip.tagName).toBe("BUTTON");
    expect(chip.querySelector('[data-kind="markup"]')).not.toBeNull();
    expect((await mentionChip("docs/findings.md")).querySelector('[data-kind="markdown"]')).not.toBeNull();
    expect((await mentionChip("src/example.ts:2")).querySelector('[data-kind="code"]')).not.toBeNull();
  });

  it("opens an html report in a sandboxed iframe from a blob: URL, with its image inlined", async () => {
    const user = userEvent.setup();
    mount(`/s/${SESSION}`);
    await user.click(await mentionChip("report.html"));
    const frame = await within(await dialog()).findByTestId("preview-html");
    expect(frame.tagName).toBe("IFRAME");
    // Exactly no sandbox tokens: not allow-scripts, allow-same-origin, allow-popups, allow-top-navigation, allow-forms.
    expect(frame.getAttribute("sandbox")).toBe("");
    expect(frame.getAttribute("referrerpolicy")).toBe("no-referrer");
    expect(frame.hasAttribute("srcdoc")).toBe(false);
    // An opaque white page: the document inside is transparent unless its own styles say otherwise.
    expect(frame).toHaveClass("bg-[#fff]", "[color-scheme:light]");
    const src = frame.getAttribute("src") ?? "";
    expect(src).toMatch(/^blob:/);
    const html = await blobText(blobs.get(src)!);
    expect(blobs.get(src)!.type).toBe("text/html;charset=utf-8");
    expect(html).toContain('<meta http-equiv="Content-Security-Policy" content="default-src \'none\'; img-src data:; style-src-attr \'unsafe-inline\'; font-src data:; media-src data:">');
    expect(html).not.toContain("monoptah-asset:");
    expect(html.match(/data:image\/png;base64,[A-Za-z0-9+/=]+/g)).toHaveLength(2);
    expect(html).toContain("Example report");
    expect(requests.map((request) => request.asset)).toEqual([undefined, "img/chart.png"]);
    expect(within(await dialog()).getByTestId("file-preview-path")).toHaveTextContent("/example/alpha/report.html");
  });

  it("revokes the blob URL of the report when the preview closes, and Escape closes it", async () => {
    const user = userEvent.setup();
    mount(`/s/${SESSION}`);
    await user.click(await mentionChip("report.html"));
    const frame = await within(await dialog()).findByTestId("preview-html");
    const src = frame.getAttribute("src")!;
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByTestId("file-preview-dialog")).toBeNull());
    await waitFor(() => expect(revoked).toContain(src));
  });

  it("renders a markdown file with its relative image from the desktop and never loads a remote image", async () => {
    const user = userEvent.setup();
    mount(`/s/${SESSION}`);
    await user.click(await mentionChip("docs/findings.md"));
    const body = await within(await dialog()).findByTestId("preview-markdown");
    expect(within(body).getByText("Example findings")).toBeInTheDocument();
    const image = await within(body).findByTestId("preview-md-image");
    expect(image.getAttribute("src")).toMatch(/^data:image\/png;base64,[A-Za-z0-9+/=]+$/);
    expect(image).toHaveAttribute("alt", "chart");
    expect(body.querySelectorAll("img")).toHaveLength(1);
    expect(body.querySelector('[src*="example.invalid"], [href*="example.invalid"]')).toBeNull();
    expect(within(body).getByText("remote")).toBeInTheDocument();
    expect(requests.filter((request) => request.asset)).toEqual([expect.objectContaining({ path: "docs/findings.md", asset: "img/chart.png" })]);
  });

  it("opens an image in a viewer with zoom, from a data: URI", async () => {
    const user = userEvent.setup();
    mount(`/s/${SESSION}`);
    await user.click(await mentionChip("img/chart.png"));
    const image = await within(await dialog()).findByTestId("preview-image");
    expect(image.tagName).toBe("IMG");
    expect(image.getAttribute("src")).toMatch(/^data:image\/png;base64,[A-Za-z0-9+/=]+$/);
    expect(image).toHaveAttribute("data-zoom", "fit");
    await user.click(screen.getByRole("button", { name: "Zoom in" }));
    const zoomed = Number(image.getAttribute("data-zoom"));
    expect(zoomed).toBeGreaterThan(0);
    await user.click(screen.getByRole("button", { name: "Zoom in" }));
    expect(Number(image.getAttribute("data-zoom"))).toBeGreaterThan(zoomed);
    await user.click(screen.getByRole("button", { name: "Zoom out" }));
    expect(Number(image.getAttribute("data-zoom"))).toBeLessThan(zoomed * 1.5);
    await user.click(screen.getByRole("button", { name: "Fit to window" }));
    expect(image).toHaveAttribute("data-zoom", "fit");
  });

  it("zooms with ctrl + wheel and does not let the page zoom", async () => {
    const user = userEvent.setup();
    mount(`/s/${SESSION}`);
    await user.click(await mentionChip("img/chart.png"));
    const image = await within(await dialog()).findByTestId("preview-image");
    const pane = screen.getByTestId("preview-image-pane");
    const event = new WheelEvent("wheel", { deltaY: -100, ctrlKey: true, bubbles: true, cancelable: true });
    pane.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(true);
    await waitFor(() => expect(image).not.toHaveAttribute("data-zoom", "fit"));
  });

  it("shows an svg only through an img element", async () => {
    const user = userEvent.setup();
    mount(`/s/${SESSION}`);
    await user.click(await mentionChip("diagram"));
    const open = await dialog();
    const image = await within(open).findByTestId("preview-image");
    expect(image.tagName).toBe("IMG");
    expect(image.getAttribute("src")).toMatch(/^data:image\/svg\+xml;base64,[A-Za-z0-9+/=]+$/);
    // No app-origin blob: URL is ever made for desktop bytes that are not the sandboxed html document.
    expect([...blobs.values()].map((blob) => blob.type)).not.toContain("image/svg+xml");
    expect([...blobs.values()].filter((blob) => blob.type.startsWith("image/"))).toEqual([]);
    expect(open.querySelector("svg:not([aria-hidden]), object, embed, iframe")).toBeNull();
    expect(open.querySelector("circle, rect[fill='#4f8cff']")).toBeNull();
  });

  it("opens a text file outside the project with highlighted lines and jumps to the line of #L3", async () => {
    const user = userEvent.setup();
    mount(`/s/${SESSION}`);
    await user.click(await mentionChip("~/Documents/x/notes.py"));
    const lines = await within(await dialog()).findByTestId("preview-lines");
    await waitFor(() => expect(lines.querySelector(".md-token")).not.toBeNull());
    expect(lines.textContent).toContain('print("example notes")');
    const marked = within(lines).getAllByTestId("file-line").filter((row) => row.getAttribute("data-highlighted") === "true");
    expect(marked).toHaveLength(1);
    expect(marked[0]).toHaveAttribute("data-index", "2");
    expect(requests.at(-1)).toMatchObject({ path: "~/Documents/x/notes.py" });
    expect(requests.at(-1)?.path).not.toContain("#");
  });

  it("opens a project text file in the Files route, with the line, and does not ask for a preview", async () => {
    const user = userEvent.setup();
    const { router } = mount(`/s/${SESSION}`);
    await user.click(await mentionChip("src/example.ts:2"));
    await waitFor(() => expect(router.state.location.pathname).toBe(`/d/${DESKTOP_ID}/p/proj-alpha/files/src/example.ts`));
    expect(router.state.location.search).toEqual({ line: 2 });
    expect(screen.queryByTestId("file-preview-dialog")).toBeNull();
    expect(requests).toEqual([]);
  });

  it("says a binary file cannot be previewed, with its size and path, and copies the path", async () => {
    const user = userEvent.setup();
    mount(`/s/${SESSION}`);
    const write = vi.spyOn(navigator.clipboard, "writeText");
    await user.click(await mentionChip("bin/tool.bin"));
    const binary = await within(await dialog()).findByTestId("preview-binary");
    expect(binary).toHaveTextContent("Cannot preview this file");
    expect(binary).toHaveTextContent("47 KB");
    expect(binary).toHaveTextContent("/example/alpha/bin/tool.bin");
    await user.click(within(binary).getByRole("button", { name: "Copy path" }));
    expect(write).toHaveBeenCalledWith("/example/alpha/bin/tool.bin");
  });

  it.each([
    [".env", "path_denied", "This file is hidden by Monoptah"],
    ["big.log", "too_large", "This file is too large to preview"],
    ["missing.txt", "not_found", "This file could not be found"],
    ["private/unmentioned-by-desktop.txt", "forbidden", "This file is outside this conversation"],
  ])("explains %s (%s) in a Mono chat, where every file opens the preview", async (mention, _code, message) => {
    const user = userEvent.setup();
    mount(`/monos/${MONO}`);
    await user.click(await mentionChip(mention));
    const problem = await within(await dialog()).findByTestId("file-preview-error");
    expect(problem).toHaveTextContent(message);
    expect(within(problem).queryByRole("button", { name: "Try again" })).toBeNull();
  });

  it("explains a hidden .env in a session chat too", async () => {
    const user = userEvent.setup();
    mount(`/s/${SESSION}`);
    await user.click(await mentionChip(".env"));
    expect(await within(await dialog()).findByTestId("file-preview-error")).toHaveAttribute("data-problem", "denied");
    expect(await dialog()).toHaveTextContent("This file is hidden by Monoptah");
  });

  it("offers Try again on a busy desktop, and shows the file when it works again", async () => {
    const user = userEvent.setup();
    const handler = FIXTURE_HANDLERS["chat.mentionedFile"];
    let busy = true;
    FIXTURE_HANDLERS["chat.mentionedFile"] = ((params: never, ctx: never) => {
      if (busy) throw new FixtureError("Busy", "busy");
      return (handler as (p: unknown, c: unknown) => unknown)(params, ctx);
    }) as never;
    mount(`/s/${SESSION}`);
    await user.click(await mentionChip("bin/tool.bin"));
    const problem = await within(await dialog()).findByTestId("file-preview-error");
    expect(problem).toHaveAttribute("data-problem", "busy");
    busy = false;
    await user.click(within(problem).getByRole("button", { name: "Try again" }));
    expect(await within(await dialog()).findByTestId("preview-binary")).toBeInTheDocument();
  });

  it("tells an old desktop's refusal apart: forbidden without mentionedFiles2 says to update the desktop", async () => {
    const user = userEvent.setup();
    mount(`/monos/${MONO}`, { capabilities: ["cloud", "mentionedFiles"] });
    await user.click(await mentionChip("private/unmentioned-by-desktop.txt"));
    const problem = await within(await dialog()).findByTestId("file-preview-error");
    expect(problem).toHaveAttribute("data-problem", "update_desktop");
    expect(problem).toHaveTextContent("Update Monoptah on your desktop");
    expect(within(problem).queryByRole("button", { name: "Try again" })).toBeNull();
  });

  it("shows an update hint for a file outside the project, and asks nothing, when the desktop does not advertise mentionedFiles", async () => {
    const user = userEvent.setup();
    mount(`/s/${SESSION}`, { capabilities: ["approvals", "cloud"] });
    await user.click(await mentionChip("~/Documents/x/notes.py"));
    const hint = await within(await dialog()).findByTestId("capability-notice");
    expect(hint).toHaveTextContent("Update Monoptah on your desktop to use File previews");
    expect(requests).toEqual([]);
  });

  it("routes any file of the project to the Files route without the capability, and only an outside file gets the notice", async () => {
    const user = userEvent.setup();
    const { router } = mount(`/s/${SESSION}`, { capabilities: ["approvals", "cloud"] });
    await user.click(await mentionChip("report.html"));
    await waitFor(() => expect(router.state.location.pathname).toBe(`/d/${DESKTOP_ID}/p/proj-alpha/files/report.html`));
    expect(screen.queryByTestId("file-preview-dialog")).toBeNull();
    expect(requests).toEqual([]);
  });

  it("still routes a project text file to the Files route without the capability", async () => {
    const user = userEvent.setup();
    const { router } = mount(`/s/${SESSION}`, { capabilities: ["approvals", "cloud"] });
    await user.click(await mentionChip("src/example.ts:2"));
    await waitFor(() => expect(router.state.location.pathname).toContain("/files/src/example.ts"));
  });
});

describe("mentions in a Mono chat", () => {
  it("makes the file mentions clickable and opens an html report", async () => {
    const user = userEvent.setup();
    mount(`/monos/${MONO}`);
    const chip = await mentionChip("report.html");
    expect(chip.tagName).toBe("BUTTON");
    await user.click(chip);
    const frame = await within(await dialog()).findByTestId("preview-html");
    expect(frame.getAttribute("sandbox")).toBe("");
    expect(requests[0]).toMatchObject({ conversationId: `mono:${MONO}`, path: "report.html" });
  });

  it("previews even a source file (a Mono chat has no project), and pages a long one to the end", async () => {
    const user = userEvent.setup();
    mount(`/monos/${MONO}`);
    await user.click(await mentionChip("src/example.ts:2"));
    const lines = await within(await dialog()).findByTestId("preview-lines");
    expect(lines.textContent).toContain("export const answer");
    expect(requests.at(-1)).toMatchObject({ path: "src/example.ts" });
  });

  it("asks for the raw source of a markdown file when the mention has a line, and shows it as highlighted lines", async () => {
    const user = userEvent.setup();
    mount(`/monos/${MONO}`, { seed: (desktop) => desktop.world.addAssistantText(`mono:${MONO}`, "The finding is at `docs/findings.md:3`.") });
    await user.click(await mentionChip("docs/findings.md:3"));
    const lines = await within(await dialog()).findByTestId("preview-lines");
    expect(lines.textContent).toContain("# Example findings");
    expect(requests[0]).toMatchObject({ path: "docs/findings.md", source: true });
    const marked = within(lines).getAllByTestId("file-line").filter((row) => row.getAttribute("data-highlighted") === "true");
    expect(marked).toHaveLength(1);
    expect(marked[0]).toHaveAttribute("data-index", "2");
  });

  it("does not ask for the source without a line, so html and markdown stay rendered", async () => {
    const user = userEvent.setup();
    mount(`/monos/${MONO}`);
    await user.click(await mentionChip("docs/findings.md"));
    await within(await dialog()).findByTestId("preview-markdown");
    expect(requests[0]).not.toHaveProperty("source");
  });

  it("follows nextOffset until the whole file is in", async () => {
    const user = userEvent.setup();
    mount(`/monos/${MONO}`);
    await user.click(await mentionChip("long-notes.txt"));
    const lines = await within(await dialog()).findByTestId("preview-lines");
    await waitFor(() => expect(lines.textContent).toContain("001 Example line"));
    const offsets = requests.filter((request) => request.path === "long-notes.txt").map((request) => request.offset ?? 0);
    expect(offsets.length).toBeGreaterThan(2);
    expect(offsets).toEqual(offsets.map((_, index) => index * MOCK_PAGE_BYTES));
    // The list is virtualized: its height is the whole file, 400 lines of 20px.
    expect(lines.firstElementChild).toHaveStyle({ height: "8000px" });
  });
});
