// A large photo is decoded again at the size it is kept at, a small one is decoded once.
import { afterEach, describe, expect, it, vi } from "vitest";
import { MAX_EDGE } from "./dither";
import { processImage, type CanvasLike } from "./process";

function bitmap(width: number, height: number) {
  return { width, height, close: vi.fn() };
}

const canvas = (): CanvasLike => ({
  width: 0,
  height: 0,
  getContext: () => ({ drawImage() {}, getImageData: (_x: number, _y: number, w: number, h: number) => ({ data: new Uint8ClampedArray(w * h * 4) }), putImageData() {} }) as never,
  toBlob: () => Promise.resolve(new Blob(["p"], { type: "image/png" })),
});

class FakeImageData {
  data: Uint8ClampedArray;
  constructor(data: Uint8ClampedArray) {
    this.data = data;
  }
}

afterEach(() => vi.unstubAllGlobals());

describe("processImage", () => {
  it("decodes a large photo again at the size it is kept at", async () => {
    const probe = bitmap(MAX_EDGE * 4, MAX_EDGE * 2);
    const sized = bitmap(MAX_EDGE, MAX_EDGE / 2);
    const create = vi.fn().mockResolvedValueOnce(probe).mockResolvedValueOnce(sized);
    vi.stubGlobal("createImageBitmap", create);
    vi.stubGlobal("ImageData", FakeImageData);
    await processImage(new Blob(["x"]), canvas);
    expect(create).toHaveBeenCalledTimes(2);
    expect(create.mock.calls[1]?.[1]).toEqual({ resizeWidth: MAX_EDGE, resizeHeight: MAX_EDGE / 2, resizeQuality: "high" });
    expect(probe.close).toHaveBeenCalled();
    expect(sized.close).toHaveBeenCalled();
  });

  it("decodes a photo that already fits once", async () => {
    const small = bitmap(300, 200);
    const create = vi.fn().mockResolvedValue(small);
    vi.stubGlobal("createImageBitmap", create);
    vi.stubGlobal("ImageData", FakeImageData);
    await processImage(new Blob(["x"]), canvas);
    expect(create).toHaveBeenCalledTimes(1);
    expect(small.close).toHaveBeenCalled();
  });
});
