// Reads a file through `file.prepare` and the encrypted object in R2, as a `MentionReader` (the shape the previews
// already consume), so html, markdown and images need no second code path. The desktop is asked every time (a changed
// file has a new hash); the bytes come from the tab cache when this hash was opened before.
import type { ResultOf } from "@monoptah/protocol";
import { MAX_PREVIEW_BYTES, PreviewLimitError, type MentionContent, type MentionReader } from "../mentionedFile.ts";
import { fetchPrepared, TransferError, type PreparedFile } from "./client.ts";
import type { ContentCache } from "./cache.ts";

export type PrepareParams = { path: string; asset?: string; source?: boolean };
export type Prepare = (params: PrepareParams, signal?: AbortSignal) => Promise<PreparedFile>;

export interface TransferReaderDeps {
  desktopId: string;
  prepare: Prepare;
  /** This browser's device id (the Worker checks its grant). */
  deviceId: () => Promise<string>;
  fetch?: typeof fetch;
  cache?: ContentCache;
}

type Meta = MentionContent["meta"];

function metaOf(file: PreparedFile): Meta {
  const meta: Meta = { path: file.path, name: file.name, size: file.size, kind: file.kind, mime: file.mime, encoding: "none", data: "", offset: 0, total: file.size };
  if (file.assets) meta.assets = file.assets;
  if (file.project) meta.project = file.project;
  return meta;
}

/** The content a preview takes: text decoded for the text kinds, the bytes as one chunk for the others. */
export function contentOf(file: PreparedFile, bytes: Uint8Array): MentionContent {
  const meta = metaOf(file);
  if (file.kind === "text" || file.kind === "markdown" || file.kind === "html") {
    return { meta, text: new TextDecoder("utf-8", { fatal: false }).decode(bytes), chunks: [], length: bytes.length };
  }
  return { meta, text: "", chunks: [bytes], length: bytes.length };
}

export function transferReader(deps: TransferReaderDeps): MentionReader {
  return async (params, options = {}) => {
    const { signal, budget, onProgress } = options;
    if (budget && !budget.takeAsset()) throw new PreviewLimitError();
    let reserved = false;
    for (let attempt = 0; ; attempt++) {
      signal?.throwIfAborted();
      const file = await deps.prepare(params, signal);
      // A binary file is only offered for download; everything else is shown, so it must fit the preview.
      if (file.kind !== "binary" && file.size > MAX_PREVIEW_BYTES) throw new PreviewLimitError();
      if (budget && !reserved) {
        if (!budget.reserve(file.size)) throw new PreviewLimitError();
        reserved = true;
      }
      try {
        const deviceId = await deps.deviceId();
        const bytes = await fetchPrepared(file, {
          desktopId: deps.desktopId,
          deviceId,
          ...(signal ? { signal } : {}),
          ...(onProgress ? { onProgress } : {}),
          ...(deps.fetch ? { fetch: deps.fetch } : {}),
          ...(deps.cache ? { cache: deps.cache } : {}),
        });
        return contentOf(file, bytes);
      } catch (error) {
        // The object expired between the desktop's check and the download: ask again, the desktop uploads it again.
        if (error instanceof TransferError && error.problem === "gone" && attempt === 0) continue;
        throw error;
      }
    }
  };
}

export type { ResultOf };
