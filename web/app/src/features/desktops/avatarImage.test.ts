import { describe, expect, it } from "vitest";
import { AVATAR_SOURCE_MAX_BYTES, AvatarImageError, bytesToBase64, coverSquare, prepareAvatarImage, sniffImage } from "./avatarImage";

const PNG = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0];
const JPEG = [0xff, 0xd8, 0xff, 0xe0, 0, 0, 0, 0, 0, 0, 0, 0];
const WEBP = [0x52, 0x49, 0x46, 0x46, 1, 2, 3, 4, 0x57, 0x45, 0x42, 0x50];
const text = (value: string) => Array.from(new TextEncoder().encode(value));

describe("sniffImage", () => {
  it("accepts PNG, JPEG and WebP by their first bytes", () => {
    expect(sniffImage(new Uint8Array(PNG))).toBe("image/png");
    expect(sniffImage(new Uint8Array(JPEG))).toBe("image/jpeg");
    expect(sniffImage(new Uint8Array(WEBP))).toBe("image/webp");
  });

  it("refuses SVG, HTML, GIF, a RIFF that is not WebP and short input", () => {
    expect(sniffImage(new Uint8Array(text('<svg xmlns="http://www.w3.org/2000/svg"></svg>')))).toBeNull();
    expect(sniffImage(new Uint8Array(text("<!doctype html><script>")))).toBeNull();
    expect(sniffImage(new Uint8Array(text("GIF89a......")))).toBeNull();
    expect(sniffImage(new Uint8Array([0x52, 0x49, 0x46, 0x46, 1, 2, 3, 4, 0x57, 0x41, 0x56, 0x45]))).toBeNull();
    expect(sniffImage(new Uint8Array([0xff, 0xd8]))).toBeNull();
    expect(sniffImage(new Uint8Array())).toBeNull();
  });
});

describe("coverSquare", () => {
  it("crops the longer side around the center", () => {
    expect(coverSquare(400, 200)).toEqual({ x: 100, y: 0, side: 200 });
    expect(coverSquare(200, 400)).toEqual({ x: 0, y: 100, side: 200 });
    expect(coverSquare(128, 128)).toEqual({ x: 0, y: 0, side: 128 });
  });
});

describe("bytesToBase64", () => {
  it("encodes like btoa, also past one chunk", () => {
    expect(bytesToBase64(new Uint8Array([104, 105]))).toBe("aGk=");
    const big = new Uint8Array(100_000).fill(65);
    expect(bytesToBase64(big)).toBe(btoa("A".repeat(100_000)));
  });
});

describe("prepareAvatarImage", () => {
  it("refuses an SVG before decoding it, whatever its name or type says", async () => {
    const svg = new File(['<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script></svg>'], "avatar.png", { type: "image/png" });
    await expect(prepareAvatarImage(svg)).rejects.toMatchObject({ reason: "unsupported" });
    await expect(prepareAvatarImage(svg)).rejects.toBeInstanceOf(AvatarImageError);
  });

  it("refuses a file larger than the source cap before reading it", async () => {
    const big = new File([new Uint8Array(AVATAR_SOURCE_MAX_BYTES + 1)], "big.png", { type: "image/png" });
    await expect(prepareAvatarImage(big)).rejects.toMatchObject({ reason: "too_large" });
  });

  it("says unreadable when a PNG cannot be decoded", async () => {
    // jsdom has no createImageBitmap: a valid header still cannot be decoded here.
    const png = new File([new Uint8Array(PNG)], "a.png", { type: "image/png" });
    await expect(prepareAvatarImage(png)).rejects.toMatchObject({ reason: "unreadable" });
  });
});
