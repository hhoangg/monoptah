import { describe, expect, it } from "vitest";
import { MAX_FRAME, MIN_FRAME, buildFrame, checkFrameSize, readDeviceId, writeDeviceId } from "../src/frame.ts";

describe("frame size check", () => {
  it("accepts 20 and 65,555, rejects around them", () => {
    expect(checkFrameSize(MIN_FRAME)).toBe("ok");
    expect(checkFrameSize(MIN_FRAME - 1)).toBe("too_small");
    expect(checkFrameSize(MAX_FRAME)).toBe("ok");
    expect(checkFrameSize(MAX_FRAME + 1)).toBe("too_big");
    expect(MAX_FRAME).toBe(65_555);
  });
});

describe("device id header", () => {
  it("round trips and overwrites", () => {
    const f = buildFrame(64, "dev-aaaa");
    expect(readDeviceId(f)).toBe("dev-aaaa");
    writeDeviceId(f, "d2");
    expect(readDeviceId(f)).toBe("d2");
    expect(f.length).toBe(64);
  });
});
