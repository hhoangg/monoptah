// Desktop key pinning (R-10, security.md 5). The first directory public key seen for a desktop is
// pinned; a different key later is "changed" and the link must block ("Desktop key changed").
// The only way to replace a pin is repinFromVerify with a fingerprint that came from the desktop
// itself (the `/desktops/:id/verify#fp=` page). There is deliberately no "trust new key" call.
import { PINS_STORE, idbAddAll, idbClear, idbGet, idbUpdate } from "./idb.ts";
import { fingerprint, normalizeFingerprint } from "./fingerprint.ts";

export interface PinRecord {
  v: 1;
  desktopId: string;
  publicKey: Uint8Array;
  /** Lowercase hex SHA-256 of `publicKey`. */
  fingerprint: string;
  pinnedAt: number;
}

export type PinCheck = "pinned-now" | "match" | "changed";

export type RepinResult =
  | { ok: true; fingerprint: string }
  | { ok: false; reason: "invalid_fingerprint" | "mismatch" };

function sameKey(a: Uint8Array, b: Uint8Array): boolean {
  return a.length === b.length && a.every((byte, i) => byte === b[i]);
}

export async function getPin(desktopId: string): Promise<PinRecord | undefined> {
  return idbGet<PinRecord>(PINS_STORE, desktopId);
}

/** First success pins (trust on first use); a different key is "changed" and the pin is kept. */
export function checkPin(desktopId: string, directoryPublicKey: Uint8Array): Promise<PinCheck> {
  return idbUpdate<PinRecord, PinCheck>(PINS_STORE, desktopId, (current) => {
    if (!current) {
      const record: PinRecord = {
        v: 1,
        desktopId,
        publicKey: directoryPublicKey.slice(),
        fingerprint: fingerprint(directoryPublicKey),
        pinnedAt: Date.now(),
      };
      return { next: record, result: "pinned-now" };
    }
    return { result: sameKey(current.publicKey, directoryPublicKey) ? "match" : "changed" };
  });
}

export interface PinInspection {
  status: "new" | "match" | "changed";
  /** Fingerprint of the stored pin; null when there is none. */
  pinnedFingerprint: string | null;
}

/** Reads the pin without writing: a first-use key is not pinned until a handshake with it succeeded (`commitPin`). */
export async function inspectPin(desktopId: string, directoryPublicKey: Uint8Array): Promise<PinInspection> {
  const current = await getPin(desktopId);
  if (!current) return { status: "new", pinnedFingerprint: null };
  return { status: sameKey(current.publicKey, directoryPublicKey) ? "match" : "changed", pinnedFingerprint: current.fingerprint };
}

/**
 * Pins a first-use key after the handshake with it succeeded. Atomic (`add`): when a pin appeared
 * meanwhile, the stored one wins and the result says whether it equals `publicKey`.
 */
export async function commitPin(desktopId: string, publicKey: Uint8Array): Promise<"pinned-now" | "match" | "changed"> {
  const record: PinRecord = { v: 1, desktopId, publicKey: publicKey.slice(), fingerprint: fingerprint(publicKey), pinnedAt: Date.now() };
  if (await idbAddAll(PINS_STORE, [[desktopId, record]])) return "pinned-now";
  const current = await getPin(desktopId);
  return current && sameKey(current.publicKey, publicKey) ? "match" : "changed";
}

/**
 * Replaces the pin only when the full SHA-256 fingerprint of `directoryPublicKey` equals the full
 * fingerprint from the verify link fragment (64 hex chars, case-insensitive). A truncated or
 * malformed fragment, or any mismatch, keeps the old pin.
 */
export async function repinFromVerify(desktopId: string, fragmentFingerprintHex: string, directoryPublicKey: Uint8Array): Promise<RepinResult> {
  const expected = normalizeFingerprint(fragmentFingerprintHex);
  if (!expected) return { ok: false, reason: "invalid_fingerprint" };
  const actual = fingerprint(directoryPublicKey);
  if (actual !== expected) return { ok: false, reason: "mismatch" };
  const record: PinRecord = { v: 1, desktopId, publicKey: directoryPublicKey.slice(), fingerprint: actual, pinnedAt: Date.now() };
  return idbUpdate<PinRecord, RepinResult>(PINS_STORE, desktopId, () => ({ next: record, result: { ok: true, fingerprint: actual } }));
}

/** "Forget this device": drops every pin. */
export function forgetPins(): Promise<void> {
  return idbClear(PINS_STORE);
}
