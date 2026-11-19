// Runs the dither off the main thread. Needs OffscreenCanvas; the caller falls back to the main thread without it.
import { offscreenCanvas, processImage } from "./process";

export interface WorkerRequest {
  id: number;
  file: Blob;
}
export type WorkerResponse = { id: number; blob: Blob } | { id: number; error: string };

self.onmessage = (event: MessageEvent<WorkerRequest>) => {
  const { id, file } = event.data;
  processImage(file, offscreenCanvas).then(
    (blob) => self.postMessage({ id, blob } satisfies WorkerResponse),
    (error: unknown) => self.postMessage({ id, error: error instanceof Error ? error.message : "Could not process the image" } satisfies WorkerResponse),
  );
};
