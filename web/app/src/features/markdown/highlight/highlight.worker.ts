/// <reference lib="webworker" />
// Copied from src/features/files/editor/codeHighlight.worker.ts at ca73ac2b
import { createHighlightEngine } from "./engine";
import type { HighlightRequest, HighlightResult } from "./types";

const engine = createHighlightEngine({ maxEntries: 1 });

self.onmessage = async ({ data }: MessageEvent<{ id: number; request: HighlightRequest }>) => {
  try {
    const result: HighlightResult | null = await engine.highlight(data.request);
    // Only plain token data crosses the structured-clone boundary.
    self.postMessage({ id: data.id, result });
  } catch (error) {
    self.postMessage({ id: data.id, error: String(error) });
  }
};
