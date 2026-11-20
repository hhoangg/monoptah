import { describe, expect, it } from "vitest";
import { CACHE_MAX_BYTES, CACHE_MAX_ENTRIES, CACHE_MAX_ENTRY_BYTES, ContentCache, cacheKey } from "./cache";

const blob = (size: number, fill = 1) => new Uint8Array(size).fill(fill);

describe("ContentCache", () => {
  it("returns what it keeps by key, and nothing for another key", () => {
    const cache = new ContentCache();
    cache.set("a", blob(10));
    expect(cache.get("a")).toEqual(blob(10));
    expect(cache.get("b")).toBeUndefined();
    expect(cache.has("a")).toBe(true);
    expect(cacheKey("desk", "abc")).toBe("desk:abc");
  });

  it("drops the least recently used entry first when over the byte budget", () => {
    const cache = new ContentCache();
    const part = CACHE_MAX_ENTRY_BYTES;
    cache.set("a", blob(part, 1));
    cache.set("b", blob(part, 2));
    cache.get("a");
    cache.set("c", blob(part, 3));
    expect(CACHE_MAX_BYTES).toBeLessThan(3 * part);
    expect(cache.has("a")).toBe(true);
    expect(cache.has("b")).toBe(false);
    expect(cache.has("c")).toBe(true);
    expect(cache.bytes).toBeLessThanOrEqual(CACHE_MAX_BYTES);
  });

  it("keeps at most the entry cap and never a file larger than the entry cap", () => {
    const cache = new ContentCache();
    for (let i = 0; i < CACHE_MAX_ENTRIES + 5; i++) cache.set(`k${i}`, blob(4));
    expect(cache.size).toBe(CACHE_MAX_ENTRIES);
    expect(cache.has("k0")).toBe(false);
    expect(cache.has(`k${CACHE_MAX_ENTRIES + 4}`)).toBe(true);
    cache.set("huge", blob(CACHE_MAX_ENTRY_BYTES + 1));
    expect(cache.has("huge")).toBe(false);
  });

  it("counts a replaced entry once and clears", () => {
    const cache = new ContentCache();
    cache.set("a", blob(10));
    cache.set("a", blob(30));
    expect(cache.bytes).toBe(30);
    cache.clear();
    expect(cache.size).toBe(0);
    expect(cache.bytes).toBe(0);
  });
});
