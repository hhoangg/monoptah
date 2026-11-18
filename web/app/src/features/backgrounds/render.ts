import { domCanvas, processImage } from "./process";
import type { WorkerRequest, WorkerResponse } from "./dither.worker";

let nextId = 0;

/** True when the dither can run in a Web Worker. */
export function canUseWorker(): boolean {
  return typeof Worker !== "undefined" && typeof OffscreenCanvas !== "undefined";
}

function inWorker(file: Blob): Promise<Blob> {
  return new Promise((resolve, reject) => {
    const worker = new Worker(new URL("./dither.worker.ts", import.meta.url), { type: "module" });
    const id = (nextId += 1);
    const finish = () => worker.terminate();
    worker.onmessage = (event: MessageEvent<WorkerResponse>) => {
      if (event.data.id !== id) return;
      finish();
      if ("blob" in event.data) resolve(event.data.blob);
      else reject(new Error(event.data.error));
    };
    worker.onerror = () => {
      finish();
      reject(new Error("The image worker failed"));
    };
    worker.postMessage({ id, file } satisfies WorkerRequest);
  });
}

/**
 * The dithered, faded PNG of a picked photo. A Web Worker with OffscreenCanvas when available, otherwise the
 * main thread (a brief pause, tolerable for one picked photo).
 */
export async function renderBackground(file: Blob): Promise<Blob> {
  if (canUseWorker()) {
    try {
      return await inWorker(file);
    } catch {
      // Fall through to the main thread: a blocked or failing worker should not lose the photo.
    }
  }
  return processImage(file, domCanvas);
}
