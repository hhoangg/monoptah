// The download half of `file.prepare`: the desktop answered with an object name and a key; the bytes come from the
// Worker (`GET /api/transfers/:desktopId/:object`, cookie session plus this device's grant), are decrypted in the
// browser and checked against the content hash the desktop announced. Reopening a file already in the tab cache
// costs no download.
import type { ResultOf } from "@monoptah/protocol";
import { cacheKey, contentCache, type ContentCache } from "./cache.ts";
import { decryptStream, importTransferKey, sha256Hex, TransferCryptoError } from "./decrypt.ts";

export type PreparedFile = ResultOf<"file.prepare">;

export type TransferProblem = "gone" | "signed_out" | "network" | "damaged" | "too_large";

export class TransferError extends Error {
  readonly problem: TransferProblem;
  constructor(problem: TransferProblem, message: string) {
    super(message);
    this.name = "TransferError";
    this.problem = problem;
  }
}

export interface FetchObjectOptions {
  desktopId: string;
  deviceId: string;
  signal?: AbortSignal;
  onProgress?: (received: number, total: number) => void;
  fetch?: typeof fetch;
  cache?: ContentCache;
}

export function objectUrl(desktopId: string, object: string, deviceId: string): string {
  return `/api/transfers/${encodeURIComponent(desktopId)}/${encodeURIComponent(object)}?device=${encodeURIComponent(deviceId)}`;
}

/** The plaintext of a prepared file: from the tab cache by hash, else downloaded, decrypted and verified. */
export async function fetchPrepared(file: PreparedFile, options: FetchObjectOptions): Promise<Uint8Array> {
  const cache = options.cache ?? contentCache;
  const key = cacheKey(options.desktopId, file.hash);
  const cached = cache.get(key);
  if (cached && cached.length === file.size) {
    options.onProgress?.(1, 1);
    return cached;
  }
  const transfer = await importTransferKey(file.key, file.nonce, file.chunkBytes).catch((error: unknown) => {
    throw error instanceof TransferCryptoError ? new TransferError("damaged", error.message) : error;
  });
  const doFetch = options.fetch ?? fetch;
  let response: Response;
  try {
    // The session cookie goes with the request, and the browser never keeps the ciphertext.
    const init: Record<string, unknown> = { credentials: "same-origin", cache: "no-store", ...(options.signal ? { signal: options.signal } : {}) };
    response = await doFetch(objectUrl(options.desktopId, file.object, options.deviceId), init as RequestInit);
  } catch (error) {
    if (options.signal?.aborted) throw error;
    throw new TransferError("network", "The file could not be downloaded");
  }
  if (response.status === 404) throw new TransferError("gone", "The file is no longer on the server");
  if (response.status === 401 || response.status === 403) throw new TransferError("signed_out", "Sign in again to open this file");
  if (!response.ok || response.body === null) throw new TransferError("network", "The file could not be downloaded");
  let plain: Uint8Array;
  try {
    plain = await decryptStream(transfer, response.body, file.size, {
      ...(options.signal ? { signal: options.signal } : {}),
      ...(options.onProgress ? { onProgress: options.onProgress } : {}),
    });
  } catch (error) {
    if (error instanceof TransferCryptoError) throw new TransferError("damaged", error.message);
    if (options.signal?.aborted) throw error;
    throw new TransferError("network", "The file could not be downloaded");
  }
  // The key opened the object, and now the content must be the one the desktop named.
  if ((await sha256Hex(plain)) !== file.hash) throw new TransferError("damaged", "The file does not match what the desktop announced");
  cache.set(key, plain);
  return plain;
}
