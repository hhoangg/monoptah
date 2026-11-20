import { describe, expect, it, vi } from "vitest";
import { type MemoryBackend, MockTransfers } from "@monoptah/mock-desktop";
import { ContentCache } from "./cache";
import { fetchPrepared, objectUrl, TransferError } from "./client";

const DESKTOP = "11111111-1111-4111-8111-111111111111";
const DEVICE = "22222222-2222-4222-8222-222222222222";

function setup() {
  const transfers = new MockTransfers();
  const backend = transfers.backend as MemoryBackend;
  const calls: string[] = [];
  const fakeFetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    calls.push(url);
    expect(init?.credentials).toBe("same-origin");
    const object = decodeURIComponent(url.split("/").pop()!.split("?")[0]!);
    const sealed = backend.objects.get(object);
    if (!sealed) return new Response("{}", { status: 404 });
    return new Response(sealed as BodyInit, { status: 200 });
  }) as unknown as typeof fetch;
  return { transfers, backend, calls, fakeFetch, cache: new ContentCache() };
}

describe("objectUrl", () => {
  it("names the desktop, the object and this device, and nothing from the path", () => {
    expect(objectUrl(DESKTOP, `${"a".repeat(64)}.${"b".repeat(16)}`, DEVICE)).toBe(`/api/transfers/${DESKTOP}/${"a".repeat(64)}.${"b".repeat(16)}?device=${DEVICE}`);
  });
});

describe("fetchPrepared", () => {
  it("downloads, decrypts and verifies the content hash", async () => {
    const { transfers, fakeFetch, cache, calls } = setup();
    const file = await transfers.prepare({ path: "report.html" }, false);
    const seen: Array<[number, number]> = [];
    const bytes = await fetchPrepared(file, { desktopId: DESKTOP, deviceId: DEVICE, fetch: fakeFetch, cache, onProgress: (a, b) => seen.push([a, b]) });
    expect(new TextDecoder().decode(bytes)).toContain("Example report");
    expect(calls).toHaveLength(1);
    expect(calls[0]).toContain(`/api/transfers/${DESKTOP}/${file.object}?device=${DEVICE}`);
    expect(seen.at(-1)?.[0]).toBe(seen.at(-1)?.[1]);
  });

  it("serves a file it already holds from the cache by hash, without a download", async () => {
    const { transfers, fakeFetch, cache, calls } = setup();
    const file = await transfers.prepare({ path: "report.html" }, false);
    await fetchPrepared(file, { desktopId: DESKTOP, deviceId: DEVICE, fetch: fakeFetch, cache });
    // The desktop is asked again and answers with the same hash (and, for another key, another object name).
    const again = await transfers.prepare({ path: "report.html" }, false);
    expect(again.reused).toBe(true);
    const bytes = await fetchPrepared(again, { desktopId: DESKTOP, deviceId: DEVICE, fetch: fakeFetch, cache });
    expect(calls).toHaveLength(1);
    expect(new TextDecoder().decode(bytes)).toContain("Example report");
  });

  it("does not mix the cache of two desktops", async () => {
    const { transfers, fakeFetch, cache, calls } = setup();
    const file = await transfers.prepare({ path: "report.html" }, false);
    await fetchPrepared(file, { desktopId: DESKTOP, deviceId: DEVICE, fetch: fakeFetch, cache });
    await fetchPrepared(file, { desktopId: "33333333-3333-4333-8333-333333333333", deviceId: DEVICE, fetch: fakeFetch, cache });
    expect(calls).toHaveLength(2);
  });

  it("maps a 404, a sign-out and a failure to what the user can act on", async () => {
    const { transfers, backend, cache } = setup();
    const file = await transfers.prepare({ path: "report.html" }, false);
    const answer = (status: number) => (async () => new Response("{}", { status })) as unknown as typeof fetch;
    const problem = async (fetchImpl: typeof fetch) => {
      try {
        await fetchPrepared(file, { desktopId: DESKTOP, deviceId: DEVICE, fetch: fetchImpl, cache });
      } catch (error) {
        return error instanceof TransferError ? error.problem : "other";
      }
      return "none";
    };
    expect(await problem(answer(404))).toBe("gone");
    expect(await problem(answer(401))).toBe("signed_out");
    expect(await problem(answer(403))).toBe("signed_out");
    expect(await problem(answer(500))).toBe("network");
    expect(await problem((async () => Promise.reject(new TypeError("offline"))) as unknown as typeof fetch)).toBe("network");
    backend.objects.clear();
  });

  it("refuses bytes that do not open, or that open to another content than announced", async () => {
    const { transfers, backend, cache } = setup();
    const file = await transfers.prepare({ path: "report.html" }, false);
    const sealed = backend.objects.get(file.object)!;
    const flipped = sealed.slice();
    flipped[5] = (flipped[5] ?? 0) ^ 1;
    const serve = (bytes: Uint8Array) => (async () => new Response(bytes as BodyInit, { status: 200 })) as unknown as typeof fetch;
    await expect(fetchPrepared(file, { desktopId: DESKTOP, deviceId: DEVICE, fetch: serve(flipped), cache })).rejects.toMatchObject({ problem: "damaged" });
    await expect(fetchPrepared(file, { desktopId: DESKTOP, deviceId: DEVICE, fetch: serve(sealed.slice(0, sealed.length - 4)), cache })).rejects.toMatchObject({ problem: "damaged" });
    // The desktop announces another hash than the content it sealed.
    await expect(fetchPrepared({ ...file, hash: "0".repeat(64) }, { desktopId: DESKTOP, deviceId: DEVICE, fetch: serve(sealed), cache })).rejects.toMatchObject({ problem: "damaged" });
    expect(cache.size).toBe(0);
  });

  it("rejects a damaged key before any request", async () => {
    const { transfers, cache, fakeFetch } = setup();
    const file = await transfers.prepare({ path: "report.html" }, false);
    await expect(fetchPrepared({ ...file, key: "AAAA" }, { desktopId: DESKTOP, deviceId: DEVICE, fetch: fakeFetch, cache })).rejects.toMatchObject({ problem: "damaged" });
    expect(fakeFetch).not.toHaveBeenCalled();
  });

  it("downloads a file over 4 MB in one request and opens every chunk", async () => {
    const { transfers, fakeFetch, cache, calls } = setup();
    const file = await transfers.prepare({ path: "/tmp/large-fixture.bin" }, true);
    expect(file.size).toBeGreaterThan(4 * 1024 * 1024);
    const seen: number[] = [];
    const bytes = await fetchPrepared(file, { desktopId: DESKTOP, deviceId: DEVICE, fetch: fakeFetch, cache, onProgress: (received) => seen.push(received) });
    expect(bytes.length).toBe(file.size);
    expect(calls).toHaveLength(1);
    expect(seen.length).toBeGreaterThan(0);
  });
});
