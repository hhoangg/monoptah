// The decrypted files this tab already opened, by content hash, so opening the same file again is instant (the desktop
// is still asked, because a changed file has a new hash). Memory only: a decrypted file is never written to disk, a
// service worker or any storage. Bounded in bytes and entries; the least recently used goes first.
export const CACHE_MAX_BYTES = 96 * 1024 * 1024;
export const CACHE_MAX_ENTRIES = 48;
/** A file larger than this is not kept (it would push out everything else). */
export const CACHE_MAX_ENTRY_BYTES = 48 * 1024 * 1024;

export class ContentCache {
  readonly #entries = new Map<string, Uint8Array>();
  #bytes = 0;

  get(key: string): Uint8Array | undefined {
    const found = this.#entries.get(key);
    if (found === undefined) return undefined;
    // Most recently used goes last.
    this.#entries.delete(key);
    this.#entries.set(key, found);
    return found;
  }

  has(key: string): boolean {
    return this.#entries.has(key);
  }

  set(key: string, bytes: Uint8Array): void {
    if (bytes.length > CACHE_MAX_ENTRY_BYTES) return;
    const old = this.#entries.get(key);
    if (old) {
      this.#bytes -= old.length;
      this.#entries.delete(key);
    }
    this.#entries.set(key, bytes);
    this.#bytes += bytes.length;
    for (const [oldest, value] of this.#entries) {
      if (this.#bytes <= CACHE_MAX_BYTES && this.#entries.size <= CACHE_MAX_ENTRIES) break;
      if (oldest === key) continue;
      this.#entries.delete(oldest);
      this.#bytes -= value.length;
    }
  }

  clear(): void {
    this.#entries.clear();
    this.#bytes = 0;
  }

  get size(): number {
    return this.#entries.size;
  }

  get bytes(): number {
    return this.#bytes;
  }
}

/** The cache of this tab. */
export const contentCache = new ContentCache();

/** The key of a file: the desktop and the content hash. */
export function cacheKey(desktopId: string, hash: string): string {
  return `${desktopId}:${hash}`;
}
