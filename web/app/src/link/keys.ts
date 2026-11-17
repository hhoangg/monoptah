// Device static key storage (R-10, security.md 6).
//
// Design: the X25519 private key exists before the server knows the device (the flow is create
// keypair -> POST /api/devices -> bind the returned deviceId), so the AES-GCM AAD is a locally
// generated stable `keyId` (UUID), not the deviceId. The server deviceId is stored beside it once
// known (bindDeviceId). A ciphertext copied into a record with another keyId fails to decrypt.
//
// - The private key is stored only as AES-GCM-256 ciphertext (random 96-bit IV per write).
// - The AES key is a non-extractable CryptoKey stored in IndexedDB (structured clone).
// - The unwrapped private key lives in one module-scoped Uint8Array; `pagehide` zeroes it and drops
//   the reference, the next use unwraps again.
// - Nothing here logs a key, and errors never carry key material.
import { randomBytes } from "@noble/hashes/utils.js";
import { KEYS_STORE, idbAddAll, idbDelete, idbGet, idbUpdate } from "./idb.ts";
import { generateKeyPair, type KeyPair } from "./noise.ts";

export const DEVICE_KEY_RECORD = "device";
export const WRAPPING_KEY_RECORD = "wrapping";
const IV_BYTES = 12;

export interface DeviceKeyRecord {
  v: 1;
  /** Local stable id, the AES-GCM AAD. */
  keyId: string;
  /** Server-assigned device id, absent until bindDeviceId. */
  deviceId?: string;
  /** Public key in clear (32 bytes). */
  publicKey: Uint8Array;
  iv: Uint8Array;
  /** AES-GCM(privateKey), AAD = keyId. */
  ciphertext: Uint8Array;
}

interface WrappingKeyRecord {
  v: 1;
  key: CryptoKey;
}

/** What callers may keep: no private key. */
export interface DeviceKeyInfo {
  keyId: string;
  deviceId: string | undefined;
  publicKey: Uint8Array;
}

export type DeviceKeyErrorCode =
  /** The stored key cannot be decrypted (the wrapping key is missing or does not match): not transient. */
  | "unreadable"
  /** A `pagehide` locked the key while it was loading: transient, try again. */
  | "locked"
  | "missing"
  /** The stored key is not the one that was registered. */
  | "mismatch";

export class DeviceKeyError extends Error {
  readonly code: DeviceKeyErrorCode;
  constructor(message: string, code: DeviceKeyErrorCode = "unreadable") {
    super(message);
    this.name = "DeviceKeyError";
    this.code = code;
  }
}

const aad = (keyId: string) => new TextEncoder().encode(keyId);

function info(record: DeviceKeyRecord): DeviceKeyInfo {
  return { keyId: record.keyId, deviceId: record.deviceId, publicKey: record.publicKey.slice() };
}

function sameBytes(a: Uint8Array, b: Uint8Array): boolean {
  return a.length === b.length && a.every((byte, i) => byte === b[i]);
}

// --- cross-tab lock ---

export const DEVICE_KEY_LOCK = "monoptah-device-key";

interface LockManagerLike {
  request<R>(name: string, callback: () => Promise<R>): Promise<R>;
}

/**
 * Runs `fn` under the Web Lock `monoptah-device-key` (one holder across every tab of this origin), so
 * key creation and device registration never interleave between tabs. Without Web Locks it just runs
 * `fn`: the atomic `add` writes of `createRecord` and the key check of `bindDeviceId` are the fallback.
 * Not re-entrant: never call a locking function from inside `fn`.
 */
export function withDeviceKeyLock<T>(fn: () => Promise<T>): Promise<T> {
  const locks = (globalThis.navigator as { locks?: LockManagerLike } | undefined)?.locks;
  if (!locks || typeof locks.request !== "function") return fn();
  return locks.request(DEVICE_KEY_LOCK, fn);
}

// --- in-memory private key (one per page) ---

let unwrapped: { keyId: string; privateKey: Uint8Array; publicKey: Uint8Array } | null = null;
/** Bumped by every zeroing, so a load that was running during a `pagehide` does not store its key. */
let generation = 0;

function zeroMemory(): void {
  generation += 1;
  unwrapped?.privateKey.fill(0);
  unwrapped = null;
}

if (typeof globalThis.addEventListener === "function") {
  globalThis.addEventListener("pagehide", zeroMemory);
}

/** Zeroes and drops the in-memory private key now (what `pagehide` does). */
export const lockDeviceKey = zeroMemory;

// --- storage ---

/** Creates the AES key (when missing) and the sealed device record; both are written in one transaction with `add`. Null when another tab won. */
async function createRecord(): Promise<{ record: DeviceKeyRecord; pair: KeyPair } | null> {
  const existing = await idbGet<WrappingKeyRecord>(KEYS_STORE, WRAPPING_KEY_RECORD);
  const wrapping = existing?.key ?? (await crypto.subtle.generateKey({ name: "AES-GCM", length: 256 }, false, ["encrypt", "decrypt"]));
  const seed = randomBytes(32);
  const pair = generateKeyPair(seed);
  seed.fill(0);
  const keyId = crypto.randomUUID();
  const iv = randomBytes(IV_BYTES);
  const plain = new Uint8Array(pair.privateKey);
  let sealed: ArrayBuffer;
  try {
    sealed = await crypto.subtle.encrypt({ name: "AES-GCM", iv, additionalData: aad(keyId) }, wrapping, plain);
  } catch (error) {
    pair.privateKey.fill(0);
    throw error;
  } finally {
    plain.fill(0);
  }
  const record: DeviceKeyRecord = { v: 1, keyId, publicKey: pair.publicKey.slice(), iv, ciphertext: new Uint8Array(sealed) };
  const entries: [string, unknown][] = existing ? [[DEVICE_KEY_RECORD, record]] : [[WRAPPING_KEY_RECORD, { v: 1, key: wrapping } satisfies WrappingKeyRecord], [DEVICE_KEY_RECORD, record]];
  if (await idbAddAll(KEYS_STORE, entries)) return { record, pair };
  pair.privateKey.fill(0);
  return null;
}

async function unwrap(record: DeviceKeyRecord): Promise<Uint8Array> {
  const stored = await idbGet<WrappingKeyRecord>(KEYS_STORE, WRAPPING_KEY_RECORD);
  if (!stored) throw new DeviceKeyError("the device key cannot be unlocked");
  try {
    const plain = await crypto.subtle.decrypt({ name: "AES-GCM", iv: new Uint8Array(record.iv), additionalData: aad(record.keyId) }, stored.key, new Uint8Array(record.ciphertext));
    return new Uint8Array(plain);
  } catch {
    throw new DeviceKeyError("the device key cannot be unlocked");
  }
}

async function loadOrCreate(): Promise<KeyPair> {
  const gen = generation;
  const stillValid = (fresh?: Uint8Array): void => {
    if (gen === generation) return;
    fresh?.fill(0);
    throw new DeviceKeyError("the device key was locked while it loaded", "locked");
  };
  // A lost race for the first write re-reads what the winner stored.
  for (let attempt = 0; attempt < 3; attempt++) {
    const record = await idbGet<DeviceKeyRecord>(KEYS_STORE, DEVICE_KEY_RECORD);
    stillValid();
    if (record) {
      if (unwrapped && unwrapped.keyId === record.keyId) return { privateKey: unwrapped.privateKey, publicKey: record.publicKey.slice() };
      const privateKey = await unwrap(record);
      stillValid(privateKey);
      unwrapped = { keyId: record.keyId, privateKey, publicKey: record.publicKey };
      return { privateKey, publicKey: record.publicKey.slice() };
    }
    const created = await createRecord();
    if (!created) continue;
    stillValid(created.pair.privateKey);
    unwrapped = { keyId: created.record.keyId, privateKey: created.pair.privateKey, publicKey: created.pair.publicKey };
    return { privateKey: unwrapped.privateKey, publicKey: unwrapped.publicKey.slice() };
  }
  throw new DeviceKeyError("the device key could not be created", "unreadable");
}

/**
 * Loads the device keypair, creating and storing it on first use (under the cross-tab lock).
 * `privateKey` is the module-scoped buffer itself (zeroed on `pagehide`): use it at once for the
 * handshake and never keep it, copy it, or put it in a store.
 */
export function getDeviceKeyPair(): Promise<KeyPair> {
  return withDeviceKeyLock(loadOrCreate);
}

/** `loadOrCreateDeviceKey` for a caller that already holds `withDeviceKeyLock` (the lock is not re-entrant). */
export async function loadOrCreateDeviceKeyHeld(): Promise<DeviceKeyInfo> {
  await loadOrCreate();
  const record = await idbGet<DeviceKeyRecord>(KEYS_STORE, DEVICE_KEY_RECORD);
  if (!record) throw new DeviceKeyError("the device key is missing", "missing");
  return info(record);
}

/** Same as getDeviceKeyPair but without exposing the private key. */
export function loadOrCreateDeviceKey(): Promise<DeviceKeyInfo> {
  return withDeviceKeyLock(loadOrCreateDeviceKeyHeld);
}

/** The stored key identity without creating one (null before the first loadOrCreateDeviceKey). */
export async function getDeviceKeyInfo(): Promise<DeviceKeyInfo | null> {
  const record = await idbGet<DeviceKeyRecord>(KEYS_STORE, DEVICE_KEY_RECORD);
  return record ? info(record) : null;
}

/**
 * Records the server-assigned device id after POST /api/devices, in one transaction, and only when the
 * stored key is the one that was registered (`publicKey`). Throws `DeviceKeyError` code `mismatch`
 * when another tab replaced the record meanwhile. The AAD stays the keyId.
 */
export async function bindDeviceId(deviceId: string, publicKey: Uint8Array): Promise<void> {
  const outcome = await idbUpdate<DeviceKeyRecord, "ok" | "missing" | "mismatch">(KEYS_STORE, DEVICE_KEY_RECORD, (current) => {
    if (!current) return { result: "missing" };
    if (!sameBytes(current.publicKey, publicKey)) return { result: "mismatch" };
    return { next: { ...current, deviceId } satisfies DeviceKeyRecord, result: "ok" };
  });
  if (outcome === "missing") throw new DeviceKeyError("no device key to bind", "missing");
  if (outcome === "mismatch") throw new DeviceKeyError("the stored device key is not the registered one", "mismatch");
}

/** "Forget this device": deletes the key record and the AES key and wipes memory. Sign-out does not call this. */
export function forgetDeviceKey(): Promise<void> {
  return withDeviceKeyLock(async () => {
    zeroMemory();
    await idbDelete(KEYS_STORE, DEVICE_KEY_RECORD);
    await idbDelete(KEYS_STORE, WRAPPING_KEY_RECORD);
  });
}

/** Asks the browser not to evict our storage. Resolves false when unsupported or refused; never throws. */
export async function requestPersistentStorage(): Promise<boolean> {
  try {
    return (await navigator.storage?.persist?.()) === true;
  } catch {
    return false;
  }
}

