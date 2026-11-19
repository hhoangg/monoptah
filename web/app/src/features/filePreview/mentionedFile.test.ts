import { describe, expect, it, vi } from "vitest";
import {
  EMPTY_DATA_URI,
  MAX_PREVIEW_ASSETS,
  MAX_PREVIEW_BYTES,
  PREVIEW_CSP,
  PreviewBudget,
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
    expect(out).toContain(`src="${EMPTY_DATA_URI}"`);
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

  it("does not treat `$&` in a data URI replacement as a pattern", () => {
    expect(buildPreviewDocument("<img src='monoptah-asset:0'>", new Map([[0, "data:image/png;base64,$&$1"]]))).toContain("src='data:image/png;base64,$&$1'");
  });
});

describe("loadHtmlDocument", () => {
  const png = encodeBase64(new Uint8Array([137, 80, 78, 71]));
  const htmlPage = (assets: string[], html: string) => page({ kind: "html", data: html, total: html.length, assets });

  it("fetches each used asset once through the asset param and inlines it", async () => {
    const seen: Array<string | undefined> = [];
    const request: MentionRequester = async (params) => {
      seen.push(params.asset);
      return params.asset ? page({ kind: "image", mime: "image/png", encoding: "base64", data: png, total: 4 }) : htmlPage(["a.png", "b.png"], "");
    };
    const content = await readMention(async () => htmlPage(["a.png", "b.png"], '<img src="monoptah-asset:0"><img src="monoptah-asset:0">'), { path: "r.html" });
    const { document, limited } = await loadHtmlDocument(request, "r.html", content);
    expect(seen).toEqual(["a.png"]);
    expect(document).toContain(`src="data:image/png;base64,${png}"`);
    expect(limited).toBe(false);
  });

  it("leaves an empty image for an asset that fails or is not an image, without failing the page", async () => {
    const request: MentionRequester = async (params) => {
      if (params.asset === "gone.png") throw Object.assign(new Error("x"), { code: "not_found" });
      return page({ kind: "binary", encoding: "none", total: 3 });
    };
    const content = await readMention(async () => htmlPage(["gone.png", "bin.dat"], '<img src="monoptah-asset:0"><img src="monoptah-asset:1">'), { path: "r.html" });
    const { document } = await loadHtmlDocument(request, "r.html", content);
    expect(document.match(/src="data:,"/g)).toHaveLength(2);
  });

  it("caps a preview at 64 assets and says so", async () => {
    const assets = Array.from({ length: 70 }, (_, i) => `i${i}.png`);
    const html = assets.map((_, i) => `<img src="monoptah-asset:${i}">`).join("");
    const request: MentionRequester = async (params) => (params.asset ? page({ kind: "image", mime: "image/png", encoding: "base64", data: png, total: 4 }) : htmlPage(assets, html));
    const content = await readMention(request, { path: "r.html" });
    const { document, limited } = await loadHtmlDocument(request, "r.html", content);
    expect(limited).toBe(true);
    expect(document.match(/data:image\/png;base64/g)).toHaveLength(MAX_PREVIEW_ASSETS);
    expect(document.match(/src="data:,"/g)).toHaveLength(6);
  });
});

describe("PreviewBudget", () => {
  it("allows 64 assets and 40 MiB", () => {
    const budget = new PreviewBudget();
    for (let i = 0; i < MAX_PREVIEW_ASSETS; i++) expect(budget.takeAsset()).toBe(true);
    expect(budget.takeAsset()).toBe(false);
    expect(budget.limited).toBe(true);
    const bytes = new PreviewBudget();
    expect(bytes.reserve(MAX_PREVIEW_BYTES)).toBe(true);
    expect(bytes.reserve(1)).toBe(false);
    expect(bytes.limited).toBe(true);
  });

  it("makes readMention skip an asset over the byte cap before its data is read", async () => {
    const budget = new PreviewBudget();
    budget.reserve(MAX_PREVIEW_BYTES - 10);
    const request = vi.fn(async () => page({ kind: "image", encoding: "base64", data: "", total: 100, nextOffset: 50 }));
    await expect(readMention(request, { path: "a.html", asset: "x.png" }, { budget })).rejects.toBeInstanceOf(PreviewLimitError);
    expect(request).toHaveBeenCalledTimes(1);
  });
});

describe("assetPath", () => {
  it("accepts a plain relative path and drops ./, query and fragment", () => {
    expect(assetPath("img/a.png")).toBe("img/a.png");
    expect(assetPath("./img/a%20b.png?raw=1#x")).toBe("img/a b.png");
  });

  it("refuses anything that is not relative and inside the folder", () => {
    for (const bad of ["../a.png", "a/../../b.png", "/etc/a.png", "~/a.png", "https://x.invalid/a.png", "data:image/png;base64,AA", "a\\b.png", "a%00.png", "", "%E0%A4%A"]) {
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
    expect(previewProblem({ code: "forbidden" }).title).toBe("This file is not mentioned in this conversation");
    expect(previewProblem({ code: "too_large" }).title).toBe("This file is too large to preview");
    expect(previewProblem({ code: "not_found" }).title).toBe("This file could not be found");
    expect(previewProblem({ code: "busy" })).toMatchObject({ kind: "busy", retry: true });
    expect(previewProblem({ code: "not_selected" }).title).toBe("Not on this device's projects");
    expect(previewProblem(new Error("x"))).toMatchObject({ kind: "failed", retry: true });
  });
});
