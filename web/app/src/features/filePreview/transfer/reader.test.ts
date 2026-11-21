// The alias layer: a path (and an asset of an html page) resolves to an encrypted object, which is downloaded,
// decrypted and handed to the preview as the content it expects.
import { describe, expect, it, vi } from "vitest";
import { MemoryBackend, MockTransfers, seal, sealedSize } from "@monoptah/mock-desktop";
import { createHash } from "node:crypto";
import { loadHtmlDocument, ASSET_PLACEHOLDER_URI, PreviewLimitError, previewProblem, readAsset } from "../mentionedFile";
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
    const { document, skipped } = await loadHtmlDocument({ read }, "report.html", page);
    expect(skipped).toBe(0);
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

  it("refuses a file over the per-file limit, whatever the page already loaded", async () => {
    const { read, transfers } = setup(true);
    const prepare = transfers.prepare.bind(transfers);
    transfers.prepare = async (params, control) => ({ ...(await prepare(params, control)), size: 40 * 1024 * 1024 + 1 });
    await expect(read({ path: "report.html", asset: "img/chart.png" })).rejects.toBeInstanceOf(PreviewLimitError);
  });

  it("reports the desktop's refusals as they are", async () => {
    const { read } = setup(false);
    await expect(read({ path: "/tmp/outside-note.txt" })).rejects.toMatchObject({ code: "forbidden" });
    await expect(read({ path: "/example/alpha/.env" })).rejects.toMatchObject({ code: "path_denied" });
  });
});

/**
 * A desktop that serves an html page and its images the way the real one does: at most 3 `file.prepare` in flight for the
 * device (a 4th is answered `busy` at once), a prepare takes a while, and the sealed bytes come from the server.
 */
function manyAssets(sizes: number[], slots = 3) {
  const key = new Uint8Array(32).fill(7);
  const noncePrefix = new Uint8Array(8).fill(9);
  const names = sizes.map((_, i) => `img/n${i}.png`);
  const page = new TextEncoder().encode(names.map((_, i) => `<img src="monoptah-asset:${i}">`).join(""));
  const files = new Map<string, { bytes: Uint8Array; kind: "html" | "image" }>([["report.html", { bytes: page, kind: "html" }]]);
  names.forEach((name, i) => {
    const bytes = new Uint8Array(sizes[i] as number);
    bytes.set([0x89, 0x50, 0x4e, 0x47]);
    bytes[bytes.length - 1] = i;
    files.set(name, { bytes, kind: "image" });
  });
  const backend = new MemoryBackend();
  const stats = { flying: 0, most: 0, busy: 0, prepares: 0 };
  const prepare = async (params: PrepareParams) => {
    stats.prepares++;
    if (stats.flying >= slots) {
      stats.busy++;
      throw Object.assign(new Error("Desktop is busy"), { code: "busy" });
    }
    stats.flying++;
    stats.most = Math.max(stats.most, stats.flying);
    try {
      await new Promise((resolve) => setTimeout(resolve, 5));
      const file = files.get(params.asset ?? params.path) as { bytes: Uint8Array; kind: "html" | "image" };
      const hash = createHash("sha256").update(file.bytes).digest("hex");
      const object = `${hash}.0000000000000000`;
      await backend.put(object, seal(key, noncePrefix, file.bytes));
      return {
        path: `/p/${params.asset ?? params.path}`,
        name: (params.asset ?? params.path).split("/").pop() as string,
        kind: file.kind,
        mime: file.kind === "html" ? "text/html" : "image/png",
        size: file.bytes.length,
        hash,
        object,
        key: Buffer.from(key).toString("base64"),
        nonce: Buffer.from(noncePrefix).toString("base64"),
        chunkBytes: 1024 * 1024,
        reused: false,
        ...(file.kind === "html" ? { assets: names } : {}),
      };
    } finally {
      stats.flying--;
    }
  };
  const fakeFetch = (async (input: RequestInfo | URL) => {
    const sealed = backend.objects.get(decodeURIComponent(String(input).split("/").pop()!.split("?")[0]!));
    return sealed ? new Response(sealed as BodyInit, { status: 200 }) : new Response("{}", { status: 404 });
  }) as unknown as typeof fetch;
  const reader = () => transferReader({ desktopId: DESKTOP, deviceId: async () => "22222222-2222-4222-8222-222222222222", prepare, fetch: fakeFetch, cache: new ContentCache() });
  return { read: reader(), reader, stats, sealedSize };
}

/** Moves the fake clock on until `work` is done: the work also waits on real crypto, which a single timer flush cannot see. */
async function pumpTimers<T>(work: Promise<T>): Promise<T> {
  let done = false;
  const result = work.finally(() => {
    done = true;
  });
  // A bound in real time (only the timers are faked), so a hang fails with its reason instead of the bare test timeout.
  const deadline = Date.now() + 12_000;
  while (!done) {
    if (Date.now() > deadline) throw new Error("pumpTimers: the work was still not done after 12 s");
    await vi.advanceTimersByTimeAsync(10_000);
    await new Promise((resolve) => setImmediate(resolve));
  }
  return result;
}

describe("an html page with many assets over encrypted transfers", () => {
  it("loads all 8 assets, one of them 3 MiB, though the desktop allows only 3 prepares at once", async () => {
    const { read, stats } = manyAssets([92_000, 91_000, 248_000, 244_000, 29_000, 23_000, 198_000, 3 * 1024 * 1024]);
    const page = await read({ path: "report.html" });
    const { document, skipped } = await loadHtmlDocument({ read }, "report.html", page);
    expect(skipped).toBe(0);
    expect(document.match(/data:image\/png;base64,/g)).toHaveLength(8);
    expect(document).not.toContain(ASSET_PLACEHOLDER_URI);
    expect(stats.busy).toBe(0);
    expect(stats.most).toBeLessThanOrEqual(3);
  });

  it("keeps every asset when other readers use the same slots: busy is waited out, not turned into a lost image", { timeout: 20_000 }, async () => {
    const { read, stats } = manyAssets([1000, 2000, 3000, 4000, 5000, 6000, 7000, 8000], 2);
    const page = await read({ path: "report.html" });
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    try {
      const { document, skipped } = await pumpTimers(loadHtmlDocument({ read }, "report.html", page));
      expect(skipped).toBe(0);
      expect(document.match(/data:image\/png;base64,/g)).toHaveLength(8);
      expect(stats.busy).toBeGreaterThan(0);
    } finally {
      vi.useRealTimers();
    }
  });

  it("two previews of one desktop share the allowance of 3: together they never exceed it", async () => {
    const { read, reader, stats } = manyAssets(Array.from({ length: 8 }, () => 2000));
    const second = reader();
    const first = await read({ path: "report.html" });
    const [one, two] = await Promise.all([loadHtmlDocument({ read }, "report.html", first), loadHtmlDocument({ read: second }, "report.html", first)]);
    expect(one.skipped + two.skipped).toBe(0);
    expect(stats.busy).toBe(0);
    expect(stats.most).toBeLessThanOrEqual(3);
  });

  it("gates only the desktop's prepare: downloads of the same asset set are not held by the gate", async () => {
    const { read, stats } = manyAssets(Array.from({ length: 6 }, () => 3000));
    const source = { read };
    await Promise.all(Array.from({ length: 6 }, (_, i) => readAsset(source, { path: "report.html", asset: `img/n${i}.png` })));
    expect(stats.most).toBeLessThanOrEqual(3);
  });

  it("limits readAsset to the same 3 at a time for the markdown images too", async () => {
    const { read, stats } = manyAssets(Array.from({ length: 6 }, () => 1000));
    const source = { read };
    const reads = Array.from({ length: 6 }, (_, i) => readAsset(source, { path: "report.html", asset: `img/n${i}.png` }));
    await Promise.all(reads);
    expect(stats.busy).toBe(0);
    expect(stats.most).toBeLessThanOrEqual(3);
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
