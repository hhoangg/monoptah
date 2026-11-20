// The alias layer: a path (and an asset of an html page) resolves to an encrypted object, which is downloaded,
// decrypted and handed to the preview as the content it expects.
import { describe, expect, it, vi } from "vitest";
import { type MemoryBackend, MockTransfers } from "@monoptah/mock-desktop";
import { loadHtmlDocument, PreviewBudget, PreviewLimitError, previewProblem } from "../mentionedFile";
import { ContentCache } from "./cache";
import { TransferError } from "./client";
import { transferReader, type PrepareParams } from "./reader";

const DESKTOP = "11111111-1111-4111-8111-111111111111";

/** `expire`: the first this many downloads answer 404, as an object deleted just after the desktop's check would. */
function setup(control = false, expire = 0) {
  const transfers = new MockTransfers();
  const backend = transfers.backend as MemoryBackend;
  const prepared: PrepareParams[] = [];
  const downloads: string[] = [];
  const fakeFetch = (async (input: RequestInfo | URL) => {
    const url = String(input);
    downloads.push(url);
    if (downloads.length <= expire) return new Response("{}", { status: 404 });
    const sealed = backend.objects.get(decodeURIComponent(url.split("/").pop()!.split("?")[0]!));
    return sealed ? new Response(sealed as BodyInit, { status: 200 }) : new Response("{}", { status: 404 });
  }) as unknown as typeof fetch;
  const read = transferReader({
    desktopId: DESKTOP,
    deviceId: async () => "22222222-2222-4222-8222-222222222222",
    prepare: (params) => {
      prepared.push(params);
      return transfers.prepare(params, control);
    },
    fetch: fakeFetch,
    cache: new ContentCache(),
  });
  return { transfers, backend, prepared, downloads, read };
}

describe("transferReader", () => {
  it("reads a text file as text and an image as bytes", async () => {
    const { read } = setup();
    const text = await read({ path: "src/example.ts" });
    expect(text.meta).toMatchObject({ kind: "text", name: "example.ts", size: text.length, encoding: "none" });
    expect(text.text).toContain("export const answer");
    expect(text.chunks).toEqual([]);
    const image = await read({ path: "img/chart.png" });
    expect(image.meta).toMatchObject({ kind: "image", mime: "image/png" });
    expect(image.chunks).toHaveLength(1);
    expect(image.chunks[0]?.[0]).toBe(0x89);
  });

  it("resolves the relative assets of an html page through the same layer and inlines them", async () => {
    const { read, prepared } = setup();
    const page = await read({ path: "report.html" });
    expect(page.meta.assets).toEqual(["img/chart.png"]);
    const { document, limited } = await loadHtmlDocument({ read }, "report.html", page);
    expect(limited).toBe(false);
    expect(document).not.toContain("monoptah-asset:");
    expect(document.match(/data:image\/png;base64,/g)).toHaveLength(2);
    expect(prepared.map((p) => p.asset)).toEqual([undefined, "img/chart.png"]);
  });

  it("shows a file over 4 MB: one download, every chunk decrypted", async () => {
    const { read, downloads } = setup(true);
    const text = await read({ path: "/tmp/large-fixture.txt" });
    expect(text.length).toBeGreaterThan(4_000_000);
    expect(text.text.length).toBe(text.length);
    expect(downloads).toHaveLength(1);
    const binary = await read({ path: "/tmp/large-fixture.bin" });
    expect(binary.meta.kind).toBe("binary");
    expect(binary.chunks[0]?.length).toBe(binary.length);
  });

  it("asks the desktop again on a reopen but downloads nothing: the cache holds the content by hash", async () => {
    const { read, prepared, downloads, transfers } = setup();
    const progress = vi.fn();
    await read({ path: "report.html" }, { onProgress: progress });
    expect(downloads).toHaveLength(1);
    const again = await read({ path: "report.html" }, { onProgress: progress });
    expect(again.text).toContain("Example report");
    expect(prepared).toHaveLength(2);
    expect(downloads).toHaveLength(1);
    expect(transfers.uploads).toHaveLength(1);
  });

  it("asks again once when the object expired between the desktop's check and the download", async () => {
    const { read, prepared, downloads } = setup(false, 1);
    const page = await read({ path: "report.html" });
    expect(page.text).toContain("Example report");
    expect(prepared).toHaveLength(2);
    expect(downloads).toHaveLength(2);
  });

  it("gives up when the object is gone twice", async () => {
    const { read, prepared } = setup(false, 2);
    await expect(read({ path: "report.html" })).rejects.toMatchObject({ problem: "gone" });
    expect(prepared).toHaveLength(2);
  });

  it("charges the preview budget and refuses what does not fit", async () => {
    const { read } = setup(true);
    const budget = new PreviewBudget();
    await read({ path: "report.html", asset: "img/chart.png" }, { budget });
    expect(budget.assets).toBe(1);
    const tight = new PreviewBudget();
    tight.bytes = 40 * 1024 * 1024 - 1;
    await expect(read({ path: "report.html", asset: "img/chart.png" }, { budget: tight })).rejects.toBeInstanceOf(PreviewLimitError);
  });

  it("reports the desktop's refusals as they are", async () => {
    const { read } = setup(false);
    await expect(read({ path: "/tmp/outside-note.txt" })).rejects.toMatchObject({ code: "forbidden" });
    await expect(read({ path: "/example/alpha/.env" })).rejects.toMatchObject({ code: "path_denied" });
  });
});

describe("what the user sees", () => {
  it("explains each transfer problem", () => {
    const problem = (kind: "gone" | "signed_out" | "damaged" | "network" | "too_large") => previewProblem(new TransferError(kind, "x"));
    expect(problem("gone")).toMatchObject({ kind: "expired", retry: true });
    expect(problem("signed_out")).toMatchObject({ kind: "signed_out", retry: false });
    expect(problem("damaged")).toMatchObject({ kind: "damaged", retry: true });
    expect(problem("network")).toMatchObject({ kind: "failed", retry: true });
    expect(problem("too_large")).toMatchObject({ kind: "too_large" });
  });

  it("tells a read-only device it is outside its shared projects, and an old desktop to update", () => {
    const forbidden = { code: "forbidden" };
    expect(previewProblem(forbidden, { fileTransfer: true })).toMatchObject({ kind: "not_mentioned", title: "This device cannot open this file" });
    expect(previewProblem(forbidden, { desktopCurrent: false })).toMatchObject({ kind: "update_desktop" });
    expect(previewProblem(forbidden, {})).toMatchObject({ kind: "not_mentioned", title: "This file is outside this conversation" });
    expect(previewProblem({ code: "unsupported" })).toMatchObject({ kind: "update_desktop" });
  });
});
