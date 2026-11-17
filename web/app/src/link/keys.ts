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
import { KEYS_STORE, idbDelete, idbGet, idbPut } from "./idb.ts";
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

export class DeviceKeyError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "DeviceKeyError";
  }
}

const aad = (keyId: string) => new TextEncoder().encode(keyId);

function info(record: DeviceKeyRecord): DeviceKeyInfo {
  return { keyId: record.keyId, deviceId: record.deviceId, publicKey: record.publicKey.slice() };
}

// --- in-memory private key (one per page) ---

let unwrapped: { keyId: string; privateKey: Uint8Array; publicKey: Uint8Array } | null = null;
let pending: Promise<KeyPair> | null = null;

function zeroMemory(): void {
  unwrapped?.privateKey.fill(0);
  unwrapped = null;
  pending = null;
}

if (typeof globalThis.addEventListener === "function") {
  globalThis.addEventListener("pagehide", zeroMemory);
}

/** Zeroes and drops the in-memory private key now (what `pagehide` does). */
export const lockDeviceKey = zeroMemory;

// --- storage ---

async function loadWrappingKey(create: boolean): Promise<CryptoKey | undefined> {
  const stored = await idbGet<WrappingKeyRecord>(KEYS_STORE, WRAPPING_KEY_RECORD);
  if (stored) return stored.key;
  if (!create) return undefined;
  const key = await crypto.subtle.generateKey({ name: "AES-GCM", length: 256 }, false, ["encrypt", "decrypt"]);
  await idbPut(KEYS_STORE, WRAPPING_KEY_RECORD, { v: 1, key } satisfies WrappingKeyRecord);
  return key;
}

async function createRecord(): Promise<{ record: DeviceKeyRecord; pair: KeyPair }> {
  const wrapping = await loadWrappingKey(true);
  if (!wrapping) throw new DeviceKeyError("no wrapping key");
  const pair = generateKeyPair();
  const keyId = crypto.randomUUID();
  const iv = randomBytes(IV_BYTES);
  const sealed = await crypto.subtle.encrypt({ name: "AES-GCM", iv, additionalData: aad(keyId) }, wrapping, new Uint8Array(pair.privateKey));
  const record: DeviceKeyRecord = { v: 1, keyId, publicKey: pair.publicKey.slice(), iv, ciphertext: new Uint8Array(sealed) };
  await idbPut(KEYS_STORE, DEVICE_KEY_RECORD, record);
  return { record, pair };
}

async function unwrap(record: DeviceKeyRecord): Promise<Uint8Array> {
  const wrapping = await loadWrappingKey(false);
  if (!wrapping) throw new DeviceKeyError("the device key cannot be unlocked");
  try {
    const plain = await crypto.subtle.decrypt({ name: "AES-GCM", iv: new Uint8Array(record.iv), additionalData: aad(record.keyId) }, wrapping, new Uint8Array(record.ciphertext));
    return new Uint8Array(plain);
  } catch {
    throw new DeviceKeyError("the device key cannot be unlocked");
  }
}

async function loadOrCreate(): Promise<KeyPair> {
  const record = await idbGet<DeviceKeyRecord>(KEYS_STORE, DEVICE_KEY_RECORD);
  if (!record) {
    const created = await createRecord();
    unwrapped = { keyId: created.record.keyId, privateKey: created.pair.privateKey, publicKey: created.pair.publicKey };
    return { privateKey: unwrapped.privateKey, publicKey: unwrapped.publicKey.slice() };
  }
  if (unwrapped && unwrapped.keyId === record.keyId) {
    return { privateKey: unwrapped.privateKey, publicKey: record.publicKey.slice() };
  }
  const privateKey = await unwrap(record);
  unwrapped = { keyId: record.keyId, privateKey, publicKey: record.publicKey };
  return { privateKey, publicKey: record.publicKey.slice() };
}

/**
 * Loads the device keypair, creating and storing it on first use. `privateKey` is the module-scoped
 * buffer itself (zeroed on `pagehide`): use it at once for the handshake and never keep it, copy
 * it, or put it in a store.
 */
export function getDeviceKeyPair(): Promise<KeyPair> {
  const inFlight = (pending ??= loadOrCreate().finally(() => {
    pending = null;
  }));
  return inFlight;
}

/** Same as getDeviceKeyPair but without exposing the private key. */
export async function loadOrCreateDeviceKey(): Promise<DeviceKeyInfo> {
  await getDeviceKeyPair();
  const record = await idbGet<DeviceKeyRecord>(KEYS_STORE, DEVICE_KEY_RECORD);
  if (!record) throw new DeviceKeyError("the device key is missing");
  return info(record);
}

/** The stored key identity without creating one (null before the first loadOrCreateDeviceKey). */
export async function getDeviceKeyInfo(): Promise<DeviceKeyInfo | null> {
  const record = await idbGet<DeviceKeyRecord>(KEYS_STORE, DEVICE_KEY_RECORD);
  return record ? info(record) : null;
}

/** Records the server-assigned device id after POST /api/devices. The AAD stays the keyId. */
export async function bindDeviceId(deviceId: string): Promise<void> {
  const record = await idbGet<DeviceKeyRecord>(KEYS_STORE, DEVICE_KEY_RECORD);
  if (!record) throw new DeviceKeyError("no device key to bind");
  await idbPut(KEYS_STORE, DEVICE_KEY_RECORD, { ...record, deviceId } satisfies DeviceKeyRecord);
}

/** "Forget this device": deletes the key record and the AES key and wipes memory. Sign-out does not call this. */
export async function forgetDeviceKey(): Promise<void> {
  zeroMemory();
  await idbDelete(KEYS_STORE, DEVICE_KEY_RECORD);
  await idbDelete(KEYS_STORE, WRAPPING_KEY_RECORD);
}

/** Asks the browser not to evict our storage. Resolves false when unsupported or refused; never throws. */
export async function requestPersistentStorage(): Promise<boolean> {
  try {
    return (await navigator.storage?.persist?.()) === true;
  } catch {
    return false;
  }
}

