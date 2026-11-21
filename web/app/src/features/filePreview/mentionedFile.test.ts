import { describe, expect, it, vi } from "vitest";
import {
  ASSET_LOADING_URI,
  ASSET_PLACEHOLDER_URI,
  loadHtmlAssets,
  buildPreviewParts,
  blobText,
  MAX_PREVIEW_BYTES,
  PREVIEW_CSP,
  PreviewLimitError,
  assetPath,
  buildPreviewDocument,
  decodeBase64,
  encodeBase64,
  formatSize,
  imageDataUri,
  imageMime,
  loadHtmlDocument,
  previewProblem,
  readMention,
  type MentionRequester,
  type MentionedPage,
} from "./mentionedFile";

const page = (over: Partial<MentionedPage>): MentionedPage => ({ path: "/p/a.txt", name: "a.txt", size: 0, kind: "text", encoding: "utf8", data: "", offset: 0, total: 0, ...over });

describe("readMention paging", () => {
  it("follows nextOffset and concatenates utf8 pages", async () => {
    const offsets: number[] = [];
    const request: MentionRequester = async (params) => {
      const offset = params.offset ?? 0;
      offsets.push(offset);
      const parts = ["one ", "two ", "three"];
      const index = offset / 4;
      return page({ data: parts[index] as string, offset, total: 13, ...(index < 2 ? { nextOffset: offset + 4 } : {}) });
    };
    const content = await readMention(request, { path: "a.txt" });
    expect(content.text).toBe("one two three");
    expect(offsets).toEqual([0, 4, 8]);
    expect(content.meta.data).toBe("");
  });

  it("joins base64 pages as bytes, not as one string", async () => {
    const first = new Uint8Array([1, 2, 3, 4]);
    const second = new Uint8Array([5, 6]);
    const request: MentionRequester = async (params) =>
      (params.offset ?? 0) === 0
        ? page({ kind: "image", mime: "image/png", encoding: "base64", data: encodeBase64(first), total: 6, nextOffset: 4 })
        : page({ kind: "image", mime: "image/png", encoding: "base64", data: encodeBase64(second), offset: 4, total: 6 });
    const content = await readMention(request, { path: "a.png" });
    expect(content.length).toBe(6);
    expect([...content.chunks.flatMap((chunk) => [...chunk])]).toEqual([1, 2, 3, 4, 5, 6]);
  });

  it("refuses a page out of order or a nextOffset that does not advance", async () => {
    await expect(readMention(async () => page({ offset: 5, total: 9 }), { path: "a" })).rejects.toThrow("out of order");
    await expect(readMention(async () => page({ total: 9, nextOffset: 0 }), { path: "a" })).rejects.toThrow("bad page offset");
  });

  it("counts the pages of the main document against the 40 MiB cap: a bigger file stops after the first page", async () => {
    const request = vi.fn(async () => page({ data: "x", total: MAX_PREVIEW_BYTES + 1, nextOffset: 1 }));
    await expect(readMention(request, { path: "a.txt" })).rejects.toBeInstanceOf(PreviewLimitError);
    expect(request).toHaveBeenCalledTimes(1);
  });

  it("stops following pages once more arrived than the file has (a desktop that lies about its total)", async () => {
    const request = vi.fn(async (params: { offset?: number }) => page({ data: "abcd", offset: params.offset ?? 0, total: 6, nextOffset: (params.offset ?? 0) + 4 }));
    await expect(readMention(request, { path: "a.txt" })).rejects.toBeInstanceOf(PreviewLimitError);
    expect(request.mock.calls.length).toBeLessThanOrEqual(2);
  });

  it("passes the source flag on every page request", async () => {
    const request = vi.fn(async (params: { offset?: number }) => page({ data: "ab", offset: params.offset ?? 0, total: 4, ...(params.offset ? {} : { nextOffset: 2 }) }));
    await readMention(request, { path: "a.html", source: true });
    expect(request.mock.calls.map(([params]) => params)).toEqual([{ path: "a.html", source: true }, { path: "a.html", source: true, offset: 2 }]);
  });

  it("stops when aborted", async () => {
    const controller = new AbortController();
    const request = vi.fn(async () => {
      controller.abort();
      return page({ total: 9, nextOffset: 4 });
    });
    await expect(readMention(request, { path: "a" }, { signal: controller.signal })).rejects.toThrow();
    expect(request).toHaveBeenCalledTimes(1);
  });
});

describe("base64", () => {
  it("round-trips bytes across slice boundaries and rejects damaged data", () => {
    const bytes = Uint8Array.from({ length: 0x6000 * 2 + 7 }, (_, i) => i % 251);
    expect(decodeBase64(encodeBase64(bytes))).toEqual(bytes);
    expect(() => decodeBase64("not base64!")).toThrow();
  });
});

describe("buildPreviewDocument", () => {
  const uris = new Map([[0, "data:image/png;base64,AAAA"]]);

  it("replaces every placeholder by its data URI, an unknown index by an empty one, and keeps everything else", () => {
    const html = '<img src="monoptah-asset:0"><div style="background:url(monoptah-asset:0)"></div><img src="monoptah-asset:7"><img srcset="monoptah-asset:0 2x">';
    const out = buildPreviewDocument(html, uris);
    expect(out).not.toContain("monoptah-asset:");
    expect(out.match(/data:image\/png;base64,AAAA/g)).toHaveLength(3);
    expect(out).toContain(`src="${ASSET_PLACEHOLDER_URI}"`);
  });

  it("matches the whole index, so asset:1 never eats the start of asset:10", () => {
    const many = new Map([[1, "data:image/png;base64,ONE="], [10, "data:image/png;base64,TEN="]]);
    expect(buildPreviewDocument('<img src="monoptah-asset:10"><img src="monoptah-asset:1">', many)).toBe(
      `<meta http-equiv="Content-Security-Policy" content="${PREVIEW_CSP}"><img src="data:image/png;base64,TEN="><img src="data:image/png;base64,ONE=">`,
    );
  });

  it("prefixes the CSP meta, after a leading doctype so the page stays in standards mode", () => {
    const meta = `<meta http-equiv="Content-Security-Policy" content="default-src 'none'; img-src data:; style-src-attr 'unsafe-inline'; font-src data:; media-src data:">`;
    expect(buildPreviewDocument("<p>x</p>", uris)).toBe(`${meta}<p>x</p>`);
    expect(buildPreviewDocument("<!DOCTYPE html><p>x</p>", uris)).toBe(`<!DOCTYPE html>${meta}<p>x</p>`);
  });

  it("builds a data URI that cannot carry a quote, and only for image types", () => {
    const uri = imageDataUri("image/png", new Uint8Array([255, 254, 253, 34, 39, 60, 62]));
    expect(uri).toMatch(/^data:image\/png;base64,[A-Za-z0-9+/=]+$/);
    expect(imageDataUri('image/png" onerror="x', new Uint8Array([1]))).toBeNull();
    expect(imageDataUri("text/html", new Uint8Array([1]))).toBeNull();
    expect(imageMime("image/svg+xml; charset=utf-8")).toBe("image/svg+xml");
    expect(imageMime("application/pdf")).toBeNull();
  });

  it("keeps an inline data: image the desktop already checked, next to a placeholder", () => {
    const inline = "data:image/png;base64,iVBORw0KGgo=";
    const out = buildPreviewDocument(`<img src="${inline}"><img src="monoptah-asset:0">`, uris);
    expect(out).toContain(`<img src="${inline}">`);
    expect(out).toContain('<img src="data:image/png;base64,AAAA">');
  });

  it("does not treat `$&` in a data URI replacement as a pattern", () => {
    expect(buildPreviewDocument("<img src='monoptah-asset:0'>", new Map([[0, "data:image/png;base64,$&$1"]]))).toContain("src='data:image/png;base64,$&$1'");
  });
});

describe("loadHtmlAssets", () => {
  const png = encodeBase64(new Uint8Array([137, 80, 78, 71]));
  const imagePage = () => page({ kind: "image", mime: "image/png", encoding: "base64", data: png, total: 4 });
  const htmlOf = (n: number) => Array.from({ length: n }, (_, i) => `<img src="monoptah-asset:${i}">`).join("");

  it("has the document at once, with a loading placeholder per pending image, before any asset has finished", async () => {
    const release: Array<() => void> = [];
    const request: MentionRequester = (params) =>
      params.asset ? new Promise((resolve) => release.push(() => resolve(imagePage()))) : Promise.resolve(page({ kind: "html", data: htmlOf(2), total: 1, assets: ["a.png", "b.png"] }));
    const changes = vi.fn();
    const load = loadHtmlAssets(request, "r.html", { text: `<p>Report text</p>${htmlOf(2)}`, assets: ["a.png", "b.png"] }, { onChange: changes });
    const first = load.view();
    const firstHtml = await blobText(first.document);
    expect(first.document.type).toBe("text/html;charset=utf-8");
    expect(firstHtml).toContain("Report text");
    expect(firstHtml.split(`src="${ASSET_LOADING_URI}"`)).toHaveLength(3);
    expect(first).toMatchObject({ pending: 2, skipped: 0 });
    await vi.waitFor(() => expect(release).toHaveLength(2));
    release[0]!();
    await vi.waitFor(() => expect(load.view().pending).toBe(1));
    const second = await blobText(load.view().document);
    expect(second).toContain(`src="data:image/png;base64,${png}"`);
    expect(second.split(`src="${ASSET_LOADING_URI}"`)).toHaveLength(2);
    expect(changes).toHaveBeenCalled();
    release[1]!();
    await load.done;
    expect(load.view()).toMatchObject({ pending: 0, skipped: 0 });
    load.dispose();
  });

  it("keeps the image bytes out of one big string: the document is a Blob assembled from a Blob per image", async () => {
    const big = encodeBase64(new Uint8Array(2_000_000));
    const request: MentionRequester = async () => page({ kind: "image", mime: "image/png", encoding: "base64", data: big, total: 2_000_000 });
    const assets = Array.from({ length: 30 }, (_, i) => `i${i}.png`);
    const load = loadHtmlAssets(request, "r.html", { text: htmlOf(30), assets });
    await load.done;
    const { document } = load.view();
    expect(document).toBeInstanceOf(Blob);
    // 30 images of about 2.7 MB of base64 each: far over anything a page string could carry, and nothing is dropped.
    expect(document.size).toBeGreaterThan(30 * big.length);
    expect(load.view()).toMatchObject({ pending: 0, skipped: 0 });
    load.dispose();
  });

  it("assembles the parts without copying: buildPreviewParts keeps a Blob as a part", () => {
    const image = new Blob(["data:image/png;base64,AAAA"]);
    const parts = buildPreviewParts("<!doctype html><p>a</p><img src='monoptah-asset:0'><img src='monoptah-asset:9'>", new Map([[0, image]]));
    expect(parts).toContain(image);
    expect(parts.join("")).toContain(ASSET_PLACEHOLDER_URI);
    expect(String(parts[0])).toBe("<!doctype html>");
  });

  it("shows a failed image as the Image not loaded placeholder and counts it live", async () => {
    const request: MentionRequester = async (params) => {
      if (params.asset === "gone.png") throw Object.assign(new Error("x"), { code: "not_found" });
      return imagePage();
    };
    const load = loadHtmlAssets(request, "r.html", { text: htmlOf(2), assets: ["gone.png", "ok.png"] });
    await load.done;
    expect(load.view()).toMatchObject({ skipped: 1, pending: 0 });
    const html = await blobText(load.view().document);
    expect(html).toContain(`src="${ASSET_PLACEHOLDER_URI}"`);
    expect(html).toContain(`src="data:image/png;base64,${png}"`);
    load.dispose();
  });

  it("stops and keeps nothing for an asset that arrives after dispose", async () => {
    let release: () => void = () => undefined;
    const request: MentionRequester = (params) => (params.asset ? new Promise((resolve) => (release = () => resolve(imagePage()))) : Promise.resolve(page({ kind: "html", total: 1 })));
    const changes = vi.fn();
    const load = loadHtmlAssets(request, "r.html", { text: htmlOf(1), assets: ["a.png"] }, { onChange: changes });
    await vi.waitFor(() => expect(release).not.toBe(undefined));
    load.dispose();
    release();
    await load.done;
    expect(changes).not.toHaveBeenCalled();
    expect(load.view().pending).toBe(1);
  });
});

describe("loadHtmlDocument", () => {
  const png = encodeBase64(new Uint8Array([137, 80, 78, 71]));
  const htmlPage = (assets: string[], html: string) => page({ kind: "html", data: html, total: html.length, assets });
  const imagePage = () => page({ kind: "image", mime: "image/png", encoding: "base64", data: png, total: 4 });

  it("fetches each used asset once through the asset param and inlines it", async () => {
    const seen: Array<string | undefined> = [];
    const request: MentionRequester = async (params) => {
      seen.push(params.asset);
      return params.asset ? imagePage() : htmlPage(["a.png", "b.png"], "");
    };
    const content = await readMention(async () => htmlPage(["a.png", "b.png"], '<img src="monoptah-asset:0"><img src="monoptah-asset:0">'), { path: "r.html" });
    const { document, skipped } = await loadHtmlDocument(request, "r.html", content);
    expect(seen).toEqual(["a.png"]);
    expect(document).toContain(`src="data:image/png;base64,${png}"`);
    expect(skipped).toBe(0);
  });

  it("puts a visible placeholder, not an empty or broken image, where an asset fails or is not an image, and counts it", async () => {
    const request: MentionRequester = async (params) => {
      if (params.asset === "gone.png") throw Object.assign(new Error("x"), { code: "not_found" });
      return page({ kind: "binary", encoding: "none", total: 3 });
    };
    const content = await readMention(async () => htmlPage(["gone.png", "bin.dat"], '<img src="monoptah-asset:0"><img src="monoptah-asset:1">'), { path: "r.html" });
    const { document, skipped } = await loadHtmlDocument(request, "r.html", content);
    expect(document.split(`src="${ASSET_PLACEHOLDER_URI}"`)).toHaveLength(3);
    expect(document).not.toContain('src="data:,"');
    expect(skipped).toBe(2);
  });

  it("has no page cap: 70 assets and a page of many megabytes all load", async () => {
    const assets = Array.from({ length: 70 }, (_, i) => `i${i}.png`);
    const html = assets.map((_, i) => `<img src="monoptah-asset:${i}">`).join("");
    const big = encodeBase64(new Uint8Array(1_000_000));
    // 70 images of 1 MB each: far over the old 40 MiB page total, each one still inside the per-file limit.
    const request: MentionRequester = async (params) => (params.asset ? page({ kind: "image", mime: "image/png", encoding: "base64", data: big, total: 1_000_000 }) : htmlPage(assets, html));
    const content = await readMention(request, { path: "r.html" });
    const { document, skipped } = await loadHtmlDocument(request, "r.html", content);
    expect(skipped).toBe(0);
    expect(document.match(/data:image\/png;base64/g)).toHaveLength(70);
    expect(document).not.toContain(ASSET_PLACEHOLDER_URI);
  });

  it("reads at most 3 assets at a time, the desktop's allowance for one device", async () => {
    const assets = Array.from({ length: 8 }, (_, i) => `i${i}.png`);
    const html = assets.map((_, i) => `<img src="monoptah-asset:${i}">`).join("");
    let flying = 0;
    let most = 0;
    const request: MentionRequester = async (params) => {
      if (!params.asset) return htmlPage(assets, html);
      flying++;
      most = Math.max(most, flying);
      await new Promise((resolve) => setTimeout(resolve, 5));
      flying--;
      return imagePage();
    };
    const content = await readMention(request, { path: "r.html" });
    const { skipped } = await loadHtmlDocument(request, "r.html", content);
    expect(skipped).toBe(0);
    expect(most).toBe(3);
  });

  it("asks again when the desktop says busy, and keeps every asset", async () => {
    vi.useFakeTimers();
    try {
      const assets = ["a.png", "b.png", "c.png"];
      const html = assets.map((_, i) => `<img src="monoptah-asset:${i}">`).join("");
      const refused = new Map<string, number>();
      const request: MentionRequester = async (params) => {
        if (!params.asset) return htmlPage(assets, html);
        const times = refused.get(params.asset) ?? 0;
        refused.set(params.asset, times + 1);
        if (times < 2) throw Object.assign(new Error("Desktop is busy"), { code: "busy" });
        return imagePage();
      };
      const content = await readMention(request, { path: "r.html" });
      const loading = loadHtmlDocument(request, "r.html", content);
      await vi.runAllTimersAsync();
      const { document, skipped } = await loading;
      expect(skipped).toBe(0);
      expect(document.match(/data:image\/png;base64/g)).toHaveLength(3);
      expect([...refused.values()]).toEqual([3, 3, 3]);
    } finally {
      vi.useRealTimers();
    }
  });

  it("waits for a desktop that stays busy for as long as the preview is open, says so, and never turns the image into a placeholder", async () => {
    vi.useFakeTimers();
    try {
      const assets = ["a.png", "b.png"];
      const html = assets.map((_, i) => `<img src="monoptah-asset:${i}">`).join("");
      let busyFor = 40;
      const request: MentionRequester = async (params) => {
        if (!params.asset) return htmlPage(assets, html);
        if (params.asset === "a.png" && busyFor-- > 0) throw Object.assign(new Error("Desktop is busy"), { code: "busy" });
        return imagePage();
      };
      const content = await readMention(request, { path: "r.html" });
      const waiting: boolean[] = [];
      const loading = loadHtmlDocument(request, "r.html", content, undefined, (now) => waiting.push(now));
      // Far longer than the old 24 s of retries.
      await vi.advanceTimersByTimeAsync(10 * 60_000);
      const { document, skipped } = await loading;
      expect(skipped).toBe(0);
      expect(document).not.toContain(ASSET_PLACEHOLDER_URI);
      expect(waiting[0]).toBe(true);
      expect(waiting.at(-1)).toBe(false);
    } finally {
      vi.useRealTimers();
    }
  });

  it("honours the retryAfter the desktop gives, within bounds", async () => {
    vi.useFakeTimers();
    try {
      const assets = ["a.png"];
      const html = '<img src="monoptah-asset:0">';
      const asked: number[] = [];
      const request: MentionRequester = async (params) => {
        if (!params.asset) return htmlPage(assets, html);
        asked.push(Date.now());
        if (asked.length === 1) throw Object.assign(new Error("Desktop is busy"), { code: "busy", retryAfter: 90 });
        return imagePage();
      };
      const content = await readMention(request, { path: "r.html" });
      const loading = loadHtmlDocument(request, "r.html", content);
      await vi.advanceTimersByTimeAsync(59_000);
      expect(asked).toHaveLength(1);
      // 90 s asked, the pause is capped at 60 s so the desktop is asked again with a fresh answer.
      await vi.advanceTimersByTimeAsync(1_500);
      expect((await loading).skipped).toBe(0);
      expect(asked).toHaveLength(2);
      expect(asked[1]! - asked[0]!).toBe(60_000);
    } finally {
      vi.useRealTimers();
    }
  });

  it("stops waiting the moment the preview closes", async () => {
    const assets = ["a.png"];
    const request: MentionRequester = async (params) => {
      if (!params.asset) return htmlPage(assets, '<img src="monoptah-asset:0">');
      throw Object.assign(new Error("Desktop is busy"), { code: "busy" });
    };
    const content = await readMention(request, { path: "r.html" });
    const controller = new AbortController();
    const loading = loadHtmlDocument(request, "r.html", content, controller.signal);
    await new Promise((resolve) => setTimeout(resolve, 20));
    controller.abort();
    await expect(loading).rejects.toBeDefined();
  });

  it("asks again after a transient failure (no answer, a network error) and keeps the image", async () => {
    vi.useFakeTimers();
    try {
      const assets = ["a.png", "b.png"];
      const html = assets.map((_, i) => `<img src="monoptah-asset:${i}">`).join("");
      const tried = new Map<string, number>();
      const request: MentionRequester = async (params) => {
        if (!params.asset) return htmlPage(assets, html);
        const n = (tried.get(params.asset) ?? 0) + 1;
        tried.set(params.asset, n);
        if (n === 1) throw new Error("Desktop did not answer");
        if (n === 2 && params.asset === "b.png") throw Object.assign(new Error("The desktop did not answer in time"), { code: "timeout" });
        return imagePage();
      };
      const content = await readMention(request, { path: "r.html" });
      const loading = loadHtmlDocument(request, "r.html", content);
      await vi.advanceTimersByTimeAsync(30_000);
      const { document, skipped } = await loading;
      expect(skipped).toBe(0);
      expect(document.match(/data:image\/png;base64/g)).toHaveLength(2);
      expect(tried.get("b.png")).toBe(3);
    } finally {
      vi.useRealTimers();
    }
  });

  it("gives a transient failure that never passes a few tries, then a placeholder, and does not retry a refusal", async () => {
    vi.useFakeTimers();
    try {
      const assets = ["flaky.png", "denied.png"];
      const html = assets.map((_, i) => `<img src="monoptah-asset:${i}">`).join("");
      const tried = new Map<string, number>();
      const request: MentionRequester = async (params) => {
        if (!params.asset) return htmlPage(assets, html);
        tried.set(params.asset, (tried.get(params.asset) ?? 0) + 1);
        throw params.asset === "flaky.png" ? new Error("Desktop did not answer") : Object.assign(new Error("hidden"), { code: "path_denied" });
      };
      const content = await readMention(request, { path: "r.html" });
      const loading = loadHtmlDocument(request, "r.html", content);
      await vi.advanceTimersByTimeAsync(60_000);
      const { document, skipped } = await loading;
      expect(skipped).toBe(2);
      expect(tried.get("flaky.png")).toBe(4);
      expect(tried.get("denied.png")).toBe(1);
      expect(document.split(`src="${ASSET_PLACEHOLDER_URI}"`)).toHaveLength(3);
    } finally {
      vi.useRealTimers();
    }
  });

  it("shows a placeholder for an asset over the per-file limit, and loads the others", async () => {
    const assets = ["huge.png", "ok.png"];
    const html = assets.map((_, i) => `<img src="monoptah-asset:${i}">`).join("");
    const request: MentionRequester = async (params) => {
      if (!params.asset) return htmlPage(assets, html);
      return params.asset === "huge.png" ? page({ kind: "image", mime: "image/png", encoding: "base64", data: "", total: MAX_PREVIEW_BYTES + 1, nextOffset: 1 }) : imagePage();
    };
    const content = await readMention(request, { path: "r.html" });
    const { document, skipped } = await loadHtmlDocument(request, "r.html", content);
    expect(skipped).toBe(1);
    expect(document).toContain(`src="${ASSET_PLACEHOLDER_URI}"`);
    expect(document).toContain(`src="data:image/png;base64,${png}"`);
  });
});

describe("assetPath", () => {
  it("accepts a plain relative path and drops ./, query and fragment", () => {
    expect(assetPath("img/a.png")).toBe("img/a.png");
    expect(assetPath("./img/a%20b.png?raw=1#x")).toBe("img/a b.png");
  });

  it("keeps ../, absolute and file:// references as the desktop's keys", () => {
    expect(assetPath("../img/a.png")).toBe("../img/a.png");
    expect(assetPath("a/../b.png")).toBe("a/../b.png");
    expect(assetPath("/home/me/p/a.PNG?v=1")).toBe("/home/me/p/a.PNG");
    expect(assetPath("file:///home/me/a%20b.png#x")).toBe("/home/me/a b.png");
    expect(assetPath("FILE://localhost/home/me/a.png")).toBe("/home/me/a.png");
  });

  it("refuses what is never an image path", () => {
    for (const bad of ["~/a.png", "https://x.invalid/a.png", "file://host/a.png", "data:image/png;base64,AA", "a\\b.png", "a%00.png", "", "%E0%A4%A", "a.txt", "dir/", "noext", "/"]) {
      expect(assetPath(bad), bad).toBeNull();
    }
  });
});

describe("messages", () => {
  it("formats sizes", () => {
    expect(formatSize(12)).toBe("12 B");
    expect(formatSize(2048)).toBe("2 KB");
    expect(formatSize(48_213)).toBe("47 KB");
    expect(formatSize(5 * 1024 * 1024)).toBe("5 MB");
  });

  it("maps each desktop code to its message", () => {
    expect(previewProblem({ code: "path_denied" }).title).toBe("This file is hidden by Monoptah");
    expect(previewProblem({ code: "forbidden" })).toMatchObject({ kind: "not_mentioned", title: "This file is outside this conversation" });
    expect(previewProblem({ code: "too_large" }).title).toBe("This file is too large to preview");
    expect(previewProblem({ code: "not_found" }).title).toBe("This file could not be found");
    expect(previewProblem({ code: "busy" })).toMatchObject({ kind: "busy", retry: true });
    expect(previewProblem({ code: "not_selected" }).title).toBe("Not on this device's projects");
    expect(previewProblem(new PreviewLimitError())).toMatchObject({ kind: "too_large", retry: false });
    expect(previewProblem(new Error("x"))).toMatchObject({ kind: "failed", retry: true });
  });

  it("tells an old desktop's refusal from a real one: forbidden without mentionedFiles2 means update", () => {
    const update = { kind: "update_desktop", title: "Update Monoptah on your desktop", retry: false };
    // A desktop that does not advertise `mentionedFiles2` refuses a file made by a command with the same `forbidden`.
    expect(previewProblem({ code: "forbidden" }, { desktopCurrent: false })).toMatchObject(update);
    expect(previewProblem({ code: "forbidden" }, { desktopCurrent: true })).toMatchObject({ kind: "not_mentioned" });
    // A desktop that does not know the method at all.
    expect(previewProblem({ code: "unsupported" })).toMatchObject(update);
    // The other reasons never turn into an update notice.
    for (const code of ["path_denied", "not_found", "too_large"]) {
      expect(previewProblem({ code }, { desktopCurrent: false }).kind).not.toBe("update_desktop");
    }
    // A blocked file is never described as unmentioned or missing: the three short reasons stay apart.
    expect(previewProblem({ code: "not_found" }).title).toBe("This file could not be found");
    expect(previewProblem({ code: "path_denied" }).kind).toBe("denied");
  });
});
