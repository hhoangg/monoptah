// Turns a picked image file into the stored dithered PNG. Shared by the worker (OffscreenCanvas) and the
// main-thread fallback (a detached <canvas>): the two differ only in how the canvas is made.
import { fitSize, renderPixels } from "./dither";

type Canvas2D = CanvasRenderingContext2D | OffscreenCanvasRenderingContext2D;

export interface CanvasLike {
  width: number;
  height: number;
  getContext(kind: "2d", options?: CanvasRenderingContext2DSettings): Canvas2D | null;
  toBlob(): Promise<Blob>;
}

/** Decodes `file`, scales it to fit, dithers it and encodes a PNG (lossless: JPEG would blur the dots). */
export async function processImage(file: Blob, makeCanvas: (width: number, height: number) => CanvasLike): Promise<Blob> {
  const probe = await createImageBitmap(file);
  const { width, height } = fitSize(probe.width, probe.height);
  let bitmap = probe;
  try {
    if (width < probe.width || height < probe.height) {
      // A large photo is decoded again at the size it is kept at: the browser scales it with its own filter, which
      // is far better than drawImage shrinking a 12 megapixel bitmap.
      probe.close();
      bitmap = await createImageBitmap(file, { resizeWidth: width, resizeHeight: height, resizeQuality: "high" });
    }
    const canvas = makeCanvas(width, height);
    const ctx = canvas.getContext("2d", { willReadFrequently: true });
    if (!ctx) throw new Error("Canvas is not available");
    ctx.drawImage(bitmap, 0, 0, width, height);
    const image = ctx.getImageData(0, 0, width, height);
    const pixels = renderPixels({ width, height, pixels: image.data });
    ctx.putImageData(new ImageData(pixels as Uint8ClampedArray<ArrayBuffer>, width, height), 0, 0);
    return await canvas.toBlob();
  } finally {
    bitmap.close();
  }
}

export function offscreenCanvas(width: number, height: number): CanvasLike {
  const canvas = new OffscreenCanvas(width, height);
  return {
    width,
    height,
    getContext: (kind, options) => canvas.getContext(kind, options),
    toBlob: () => canvas.convertToBlob({ type: "image/png" }),
  };
}

export function domCanvas(width: number, height: number): CanvasLike {
  const canvas = document.createElement("canvas");
  canvas.width = width;
  canvas.height = height;
  return {
    width,
    height,
    getContext: (kind, options) => canvas.getContext(kind, options),
    toBlob: () =>
      new Promise<Blob>((resolve, reject) => {
        canvas.toBlob((blob) => (blob ? resolve(blob) : reject(new Error("Could not encode the image"))), "image/png");
      }),
  };
}
