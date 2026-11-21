// The html preview shows the page at once, swaps each image in as it arrives, says how many images are placeholders or
// waiting on a busy desktop, and gives back every object URL when it closes.
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { HtmlPreview } from "./HtmlPreview";
import { ASSET_LOADING_URI, blobText, encodeBase64, type MentionedPage, type MentionRequester } from "./mentionedFile";

const blobs = new Map<string, Blob>();
const revoked: string[] = [];
let count = 0;
beforeEach(() => {
  blobs.clear();
  revoked.length = 0;
  count = 0;
  URL.createObjectURL = vi.fn((blob: Blob) => {
    const url = `blob:https://app.test/${++count}`;
    blobs.set(url, blob);
    return url;
  });
  URL.revokeObjectURL = vi.fn((url: string) => void revoked.push(url));
});
afterEach(() => vi.restoreAllMocks());

const text = '<p>Report text</p><img src="monoptah-asset:0"><img src="monoptah-asset:1">';
const assets = ["a.png", "b.png"];
const png = (): MentionedPage => ({ path: "/p/a.png", name: "a.png", size: 4, kind: "image", mime: "image/png", encoding: "base64", data: encodeBase64(new Uint8Array([137, 80, 78, 71])), offset: 0, total: 4 });
/** jsdom never loads the frame: say it has, so the preview may replace the document. */
const frameLoads = () => fireEvent.load(screen.getByTestId("preview-html"));
const frameDocument = async () => blobText(blobs.get(screen.getByTestId("preview-html").getAttribute("src") ?? "")!);

describe("HtmlPreview", () => {
  it("shows the page and its text before any image has arrived, each image as a loading placeholder", async () => {
    const request: MentionRequester = () => new Promise(() => undefined);
    render(<HtmlPreview source={request} path="r.html" name="r.html" text={text} assets={assets} />);
    const html = await frameDocument();
    expect(html).toContain("Report text");
    expect(html.split(`src="${ASSET_LOADING_URI}"`)).toHaveLength(3);
    expect(screen.queryByTestId("preview-skipped-notice")).toBeNull();
  });

  it("swaps an image in when it arrives, without waiting for the other one", async () => {
    let finish: () => void = () => undefined;
    const request: MentionRequester = (params) => (params.asset === "a.png" ? Promise.resolve(png()) : new Promise((resolve) => (finish = () => resolve(png()))));
    render(<HtmlPreview source={request} path="r.html" name="r.html" text={text} assets={assets} />);
    await waitFor(async () => (frameLoads(), expect(await frameDocument()).toContain("data:image/png;base64,")), { timeout: 3000 });
    const html = await frameDocument();
    expect(html.split(`src="${ASSET_LOADING_URI}"`)).toHaveLength(2);
    finish();
    await waitFor(async () => (frameLoads(), expect(await frameDocument()).not.toContain(ASSET_LOADING_URI)), { timeout: 3000 });
  });

  it("counts the images that could not be had, as they fail", async () => {
    const request: MentionRequester = () => Promise.reject(Object.assign(new Error("x"), { code: "not_found" }));
    render(<HtmlPreview source={request} path="r.html" name="r.html" text={text} assets={assets} />);
    expect(await screen.findByTestId("preview-skipped-notice", {}, { timeout: 3000 })).toHaveTextContent("2 not loaded");
  });

  it("says it is waiting while the desktop is busy, and keeps the page", async () => {
    const request: MentionRequester = () => Promise.reject(Object.assign(new Error("busy"), { code: "busy" }));
    const { unmount } = render(<HtmlPreview source={request} path="r.html" name="r.html" text={text} assets={assets} />);
    expect(await screen.findByTestId("file-preview-waiting", {}, { timeout: 3000 })).toHaveTextContent("Waiting...");
    expect(await frameDocument()).toContain("Report text");
    unmount();
  });

  it("revokes every object URL of its document when it closes, and the ones it replaced", async () => {
    const request: MentionRequester = () => Promise.resolve(png());
    const { unmount } = render(<HtmlPreview source={request} path="r.html" name="r.html" text={text} assets={assets} />);
    await waitFor(async () => (frameLoads(), expect(await frameDocument()).not.toContain(ASSET_LOADING_URI)), { timeout: 3000 });
    const made = [...blobs.keys()];
    // The document was made again as each image arrived, each time a new object URL.
    expect(made.length).toBeGreaterThanOrEqual(2);
    unmount();
    await waitFor(() => expect(revoked.length).toBeGreaterThan(0));
    await waitFor(() => expect(new Set(revoked)).toEqual(new Set(made)), { timeout: 8000 });
  }, 15_000);

  it("stops reloading the frame once the reader has taken over it, and offers the ready images behind a Show button", async () => {
    let finish: () => void = () => undefined;
    const request: MentionRequester = (params) => (params.asset === "a.png" ? Promise.resolve(png()) : new Promise((resolve) => (finish = () => resolve(png()))));
    render(<HtmlPreview source={request} path="r.html" name="r.html" text={text} assets={assets} />);
    // While untouched the first swap is automatic.
    await waitFor(async () => (frameLoads(), expect(await frameDocument()).toContain("data:image/png;base64,")), { timeout: 3000 });
    const frame = screen.getByTestId("preview-html");
    const shown = frame.getAttribute("src");
    // The pointer over the frame, after the page has been on screen a moment: it is the reader's now.
    await new Promise((resolve) => setTimeout(resolve, 900));
    frame.dispatchEvent(new Event("pointerenter"));
    finish();
    expect(await screen.findByTestId("preview-html-ready", {}, { timeout: 3000 })).toHaveTextContent("1 ready");
    // No reload under the reader's hands, however long it waits.
    await new Promise((resolve) => setTimeout(resolve, 1200));
    frameLoads();
    expect(screen.getByTestId("preview-html").getAttribute("src")).toBe(shown);
    expect(await frameDocument()).toContain(ASSET_LOADING_URI);
    // One reload, when asked for.
    fireEvent.click(screen.getByTestId("preview-html-ready-show"));
    await waitFor(async () => expect(await frameDocument()).not.toContain(ASSET_LOADING_URI));
    expect(screen.getByTestId("preview-html").getAttribute("src")).not.toBe(shown);
    expect(screen.queryByTestId("preview-html-ready")).toBeNull();
  }, 15_000);

  it("starts from nothing when another page takes its place", async () => {
    const request: MentionRequester = () => Promise.reject(Object.assign(new Error("x"), { code: "not_found" }));
    const { rerender } = render(<HtmlPreview source={request} path="r.html" name="r.html" text={text} assets={assets} />);
    expect(await screen.findByTestId("preview-skipped-notice", {}, { timeout: 3000 })).toBeInTheDocument();
    const never: MentionRequester = () => new Promise(() => undefined);
    rerender(<HtmlPreview source={never} path="r.html" name="r.html" text={'<p>Other</p><img src="monoptah-asset:0">'} assets={["c.png"]} />);
    expect(screen.queryByTestId("preview-skipped-notice")).toBeNull();
    const html = await frameDocument();
    expect(html).toContain("Other");
    expect(html.split(`src="${ASSET_LOADING_URI}"`)).toHaveLength(2);
  });

  it("counts a pointer that was already over the frame when the page appeared, once the grace is over", async () => {
    let finish: () => void = () => undefined;
    const request: MentionRequester = (params) => (params.asset === "a.png" ? Promise.resolve(png()) : new Promise((resolve) => (finish = () => resolve(png()))));
    render(<HtmlPreview source={request} path="r.html" name="r.html" text={text} assets={assets} />);
    const frame = screen.getByTestId("preview-html");
    await waitFor(async () => (frameLoads(), expect(await frameDocument()).toContain("data:image/png;base64,")), { timeout: 3000 });
    // The pointer comes in at once, inside the grace: nothing yet.
    frame.dispatchEvent(new Event("pointerenter"));
    expect(screen.queryByTestId("preview-html-ready")).toBeNull();
    await new Promise((resolve) => setTimeout(resolve, 1000));
    finish();
    expect(await screen.findByTestId("preview-html-ready", {}, { timeout: 3000 })).toHaveTextContent("1 ready");
  }, 15_000);

  it("keeps waiting, not loaded and ready in the one compact row, and the images list over the frame, so the frame does not lose height", async () => {
    let finish: () => void = () => undefined;
    const three = '<img src="monoptah-asset:0"><img src="monoptah-asset:1"><img src="monoptah-asset:2"><img src="monoptah-asset:3">';
    const request: MentionRequester = (params) => {
      if (params.asset === "gone.png") return Promise.reject(Object.assign(new Error("x"), { code: "not_found" }));
      if (params.asset === "busy.png") return Promise.reject(Object.assign(new Error("busy"), { code: "busy" }));
      if (params.asset === "ok.png") return Promise.resolve(png());
      return new Promise((resolve) => (finish = () => resolve(png())));
    };
    render(<HtmlPreview source={request} path="r.html" name="r.html" text={three} assets={["gone.png", "busy.png", "ok.png", "late.png"]} />);
    await waitFor(async () => (frameLoads(), expect(await frameDocument()).toContain("data:image/png;base64,")), { timeout: 3000 });
    await new Promise((resolve) => setTimeout(resolve, 900));
    screen.getByTestId("preview-html").dispatchEvent(new Event("pointerenter"));
    finish();
    const row = await screen.findByTestId("preview-html-controls");
    await waitFor(() => expect(row).toContainElement(screen.getByTestId("preview-html-ready")), { timeout: 3000 });
    for (const id of ["preview-html-images-toggle", "file-preview-waiting", "preview-skipped-notice", "preview-html-ready", "preview-html-ready-show"]) expect(row).toContainElement(screen.getByTestId(id));
    expect(row).toHaveClass("h-9", "overflow-hidden", "whitespace-nowrap");
    // The list opens over the frame (absolute), not between the row and the frame.
    fireEvent.click(screen.getByTestId("preview-html-images-toggle"));
    expect(screen.getByTestId("preview-html-images-list")).toHaveClass("absolute");
    expect(row.parentElement!.children[1]).toBe(screen.getByTestId("preview-html-images-list"));
  }, 15_000);
});
