import { IDBFactory } from "fake-indexeddb";
import { beforeEach, describe, expect, it } from "vitest";
import { displayFingerprint, fingerprint, normalizeFingerprint } from "./fingerprint.ts";
import { checkPin, forgetPins, getPin, repinFromVerify } from "./pins.ts";

const keyA = new Uint8Array(32).fill(1);
const keyB = new Uint8Array(32).fill(2);
const DESKTOP = "0123456789abcdef0123456789abcdef";

beforeEach(() => {
  globalThis.indexedDB = new IDBFactory();
});

describe("desktop key pins", () => {
  it("pins on first success, then matches", async () => {
    expect(await checkPin(DESKTOP, keyA)).toBe("pinned-now");
    expect(await checkPin(DESKTOP, keyA)).toBe("match");
    expect((await getPin(DESKTOP))?.fingerprint).toBe(fingerprint(keyA));
  });

  it("reports a changed key and never replaces the pin", async () => {
    await checkPin(DESKTOP, keyA);
    expect(await checkPin(DESKTOP, keyB)).toBe("changed");
    expect(await checkPin(DESKTOP, keyB)).toBe("changed");
    expect(await checkPin(DESKTOP, keyA)).toBe("match");
  });

  it("keeps pins per desktop", async () => {
    await checkPin(DESKTOP, keyA);
    expect(await checkPin("fedcba9876543210fedcba9876543210", keyB)).toBe("pinned-now");
  });

  it("re-pins from verify only on an exact full fingerprint match (case-insensitive)", async () => {
    await checkPin(DESKTOP, keyA);
    const result = await repinFromVerify(DESKTOP, fingerprint(keyB).toUpperCase(), keyB);
    expect(result).toEqual({ ok: true, fingerprint: fingerprint(keyB) });
    expect(await checkPin(DESKTOP, keyB)).toBe("match");
    expect(await checkPin(DESKTOP, keyA)).toBe("changed");
  });

  it("refuses a mismatching fingerprint and keeps the old pin", async () => {
    await checkPin(DESKTOP, keyA);
    expect(await repinFromVerify(DESKTOP, fingerprint(keyA), keyB)).toEqual({ ok: false, reason: "mismatch" });
    expect(await checkPin(DESKTOP, keyA)).toBe("match");
  });

  it("refuses a truncated (32 char) fingerprint", async () => {
    await checkPin(DESKTOP, keyA);
    const truncated = fingerprint(keyB).slice(0, 32);
    expect(await repinFromVerify(DESKTOP, truncated, keyB)).toEqual({ ok: false, reason: "invalid_fingerprint" });
    expect(await checkPin(DESKTOP, keyA)).toBe("match");
  });

  it("forgetPins drops every pin", async () => {
    await checkPin(DESKTOP, keyA);
    await forgetPins();
    expect(await getPin(DESKTOP)).toBeUndefined();
    expect(await checkPin(DESKTOP, keyB)).toBe("pinned-now");
  });
});

describe("fingerprint display", () => {
  it("shows the first 32 hex chars as 8 groups of 4", () => {
    const hex = fingerprint(keyA);
    const shown = displayFingerprint(hex);
    expect(shown.split(" ")).toHaveLength(8);
    expect(shown.replaceAll(" ", "")).toBe(hex.slice(0, 32));
  });

  it("normalizes only full 64 hex fingerprints", () => {
    const hex = fingerprint(keyA);
    expect(normalizeFingerprint(hex.toUpperCase())).toBe(hex);
    expect(normalizeFingerprint(hex.slice(0, 32))).toBeNull();
    expect(normalizeFingerprint("z".repeat(64))).toBeNull();
  });
});
