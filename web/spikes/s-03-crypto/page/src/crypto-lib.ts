// S-03 measurement code: noble throughput, WebCrypto matrix, IndexedDB key storage, wrapped key.
import { x25519 } from "@noble/curves/ed25519.js";
import { chacha20poly1305 } from "@noble/ciphers/chacha.js";

const MSG = 64 * 1024;
const DB = "s03";
const STORE = "keys";

export interface BenchResult {
  messageBytes: number;
  iterations: number;
  encryptMiBs: number;
  decryptMiBs: number;
  x25519SharedSecretMs: number;
}

/** X25519 + ChaCha20-Poly1305 throughput on 64 KiB messages, `iterations` encrypt and decrypt rounds. */
export function bench(iterations: number): BenchResult {
  const a = x25519.keygen();
  const b = x25519.keygen();
  const t0 = performance.now();
  const shared = x25519.getSharedSecret(a.secretKey, b.publicKey);
  const x25519SharedSecretMs = performance.now() - t0;
  const msg = new Uint8Array(MSG);
  for (let i = 0; i < MSG; i += 65536) crypto.getRandomValues(msg.subarray(i, i + 65536));
  const nonce = new Uint8Array(12);
  const sealed: Uint8Array[] = [];
  const e0 = performance.now();
  for (let i = 0; i < iterations; i++) {
    new DataView(nonce.buffer).setUint32(0, i);
    sealed.push(chacha20poly1305(shared, nonce).encrypt(msg));
  }
  const encMs = performance.now() - e0;
  const d0 = performance.now();
  for (let i = 0; i < iterations; i++) {
    new DataView(nonce.buffer).setUint32(0, i);
    const pt = chacha20poly1305(shared, nonce).decrypt(sealed[i]!);
    if (pt[0] !== msg[0] || pt.length !== MSG) throw new Error("decrypt mismatch");
  }
  const decMs = performance.now() - d0;
  const mib = (iterations * MSG) / (1024 * 1024);
  return {
    messageBytes: MSG,
    iterations,
    encryptMiBs: round(mib / (encMs / 1000)),
    decryptMiBs: round(mib / (decMs / 1000)),
    x25519SharedSecretMs: round(x25519SharedSecretMs),
  };
}

const round = (n: number) => Math.round(n * 100) / 100;

async function attempt(fn: () => Promise<unknown>): Promise<string> {
  try { await fn(); return "ok"; } catch (e) { return `${(e as Error).name}: ${(e as Error).message}`.slice(0, 120); }
}

/** WebCrypto availability matrix. Each entry is "ok" or the error. */
export async function matrix(): Promise<Record<string, string>> {
  const s = crypto.subtle;
  const out: Record<string, string> = {};
  out["secureContext"] = String(isSecureContext);
  out["X25519 deriveBits"] = await attempt(async () => {
    const a = (await s.generateKey({ name: "X25519" }, false, ["deriveBits"])) as CryptoKeyPair;
    const b = (await s.generateKey({ name: "X25519" }, false, ["deriveBits"])) as CryptoKeyPair;
    const bits = await s.deriveBits({ name: "X25519", public: b.publicKey } as unknown as AlgorithmIdentifier, a.privateKey, 256);
    if (bits.byteLength !== 32) throw new Error("bad length");
  });
  out["Ed25519 sign/verify"] = await attempt(async () => {
    const k = (await s.generateKey({ name: "Ed25519" }, false, ["sign", "verify"])) as CryptoKeyPair;
    const sig = await s.sign({ name: "Ed25519" }, k.privateKey, new Uint8Array(8));
    if (!(await s.verify({ name: "Ed25519" }, k.publicKey, sig, new Uint8Array(8)))) throw new Error("verify false");
  });
  out["AES-GCM encrypt/decrypt"] = await attempt(async () => {
    const k = await s.generateKey({ name: "AES-GCM", length: 256 }, false, ["encrypt", "decrypt"]);
    const iv = new Uint8Array(12);
    const ct = await s.encrypt({ name: "AES-GCM", iv }, k, new Uint8Array(16));
    await s.decrypt({ name: "AES-GCM", iv }, k, ct);
  });
  out["ECDH P-256 deriveBits"] = await attempt(async () => {
    const a = await s.generateKey({ name: "ECDH", namedCurve: "P-256" }, false, ["deriveBits"]);
    const b = await s.generateKey({ name: "ECDH", namedCurve: "P-256" }, false, ["deriveBits"]);
    await s.deriveBits({ name: "ECDH", public: b.publicKey }, a.privateKey, 256);
  });
  return out;
}

function openDb(): Promise<IDBDatabase> {
  return new Promise((ok, fail) => {
    const req = indexedDB.open(DB, 1);
    req.onupgradeneeded = () => req.result.createObjectStore(STORE);
    req.onsuccess = () => ok(req.result);
    req.onerror = () => fail(req.error ?? new Error("indexedDB.open failed"));
  });
}
function idb<T>(mode: IDBTransactionMode, run: (st: IDBObjectStore) => IDBRequest<T>): Promise<T> {
  return openDb().then((db) => new Promise<T>((ok, fail) => {
    const tx = db.transaction(STORE, mode);
    const req = run(tx.objectStore(STORE));
    tx.oncomplete = () => { db.close(); ok(req.result); };
    tx.onerror = () => { db.close(); fail(tx.error ?? new Error("tx failed")); };
    tx.onabort = () => { db.close(); fail(tx.error ?? new Error("tx aborted")); };
  }));
}

const hex = (u: Uint8Array) => Array.from(u, (x) => x.toString(16).padStart(2, "0")).join("");

interface WrappedRecord { keyId: string; iv: Uint8Array; ciphertext: Uint8Array; publicKey: Uint8Array }

/**
 * architecture.md 3.4: random non-extractable AES-GCM-256 CryptoKey in IndexedDB, the X25519 private
 * key stored only as AES-GCM ciphertext (AAD = keyId). Returns hex of the public key and (test-only,
 * to prove the key is never stored in clear) the private key.
 */
export async function createKey(): Promise<{ publicKeyHex: string; privateKeyHex: string; extractable: boolean }> {
  const aes = await crypto.subtle.generateKey({ name: "AES-GCM", length: 256 }, false, ["encrypt", "decrypt"]);
  const kp = x25519.keygen();
  const keyId = crypto.randomUUID();
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const ct = new Uint8Array(await crypto.subtle.encrypt({ name: "AES-GCM", iv, additionalData: new TextEncoder().encode(keyId) }, aes, Uint8Array.from(kp.secretKey)));
  const rec: WrappedRecord = { keyId, iv, ciphertext: ct, publicKey: kp.publicKey };
  await idb("readwrite", (st) => { st.put(aes, "aes"); return st.put(rec, "wrapped"); });
  return { publicKeyHex: hex(kp.publicKey), privateKeyHex: hex(kp.secretKey), extractable: aes.extractable };
}

export interface CheckResult {
  found: boolean;
  keyUsable: boolean;
  extractable: boolean | null;
  exportBlocked: boolean;
  publicKeyMatches: boolean;
  publicKeyHex: string;
  error?: string;
}

/** Load the stored AES key, unwrap the X25519 key, re-derive the public key and compare. */
export async function checkKey(): Promise<CheckResult> {
  const aes = await idb<CryptoKey | undefined>("readonly", (st) => st.get("aes"));
  const rec = await idb<WrappedRecord | undefined>("readonly", (st) => st.get("wrapped"));
  if (!aes || !rec) return { found: false, keyUsable: false, extractable: null, exportBlocked: false, publicKeyMatches: false, publicKeyHex: "" };
  const exportBlocked = await attempt(() => crypto.subtle.exportKey("raw", aes)).then((r) => r !== "ok");
  try {
    const priv = new Uint8Array(await crypto.subtle.decrypt({ name: "AES-GCM", iv: Uint8Array.from(rec.iv), additionalData: new TextEncoder().encode(rec.keyId) }, aes, Uint8Array.from(rec.ciphertext)));
    const pub = x25519.getPublicKey(priv);
    priv.fill(0);
    return { found: true, keyUsable: true, extractable: aes.extractable, exportBlocked, publicKeyMatches: hex(pub) === hex(rec.publicKey), publicKeyHex: hex(pub) };
  } catch (e) {
    return { found: true, keyUsable: false, extractable: aes.extractable, exportBlocked, publicKeyMatches: false, publicKeyHex: "", error: String(e) };
  }
}

/** True when `needle` occurs in any stored value (typed arrays, buffers, strings), searched recursively. */
export async function storedContains(needleHex: string): Promise<{ contains: boolean; scannedValues: number }> {
  const needle = Uint8Array.from(needleHex.match(/../g) ?? [], (h) => parseInt(h, 16));
  const keys = await idb<IDBValidKey[]>("readonly", (st) => st.getAllKeys());
  let scanned = 0;
  let contains = false;
  const has = (hay: Uint8Array) => {
    outer: for (let i = 0; i + needle.length <= hay.length; i++) {
      for (let j = 0; j < needle.length; j++) if (hay[i + j] !== needle[j]) continue outer;
      return true;
    }
    return false;
  };
  const walk = (v: unknown): void => {
    scanned++;
    if (v instanceof Uint8Array) { if (has(v)) contains = true; }
    else if (v instanceof ArrayBuffer) { if (has(new Uint8Array(v))) contains = true; }
    else if (typeof v === "string") { if (v.toLowerCase().includes(needleHex.toLowerCase())) contains = true; }
    else if (v && typeof v === "object" && !(v instanceof CryptoKey)) for (const x of Object.values(v)) walk(x);
  };
  for (const k of keys) walk(await idb("readonly", (st) => st.get(k)));
  return { contains, scannedValues: scanned };
}

export async function clearKey(): Promise<void> {
  await idb("readwrite", (st) => st.clear());
}

export interface PersistResult { persistedBefore: boolean | string; persistResult: boolean | string; persistedAfter: boolean | string; estimate: { quota?: number; usage?: number } | string }

export async function persist(): Promise<PersistResult> {
  const sm = navigator.storage;
  // Firefox shows a permission prompt for persist() that nobody answers in automation: do not hang on it.
  const run = async <T>(fn: () => Promise<T>): Promise<T | string> => {
    try {
      return await Promise.race([fn(), new Promise<string>((ok) => setTimeout(() => ok("no answer in 4 s (permission prompt?)"), 4000))]);
    } catch (e) { return `error: ${String(e)}`; }
  };
  if (!sm) return { persistedBefore: "unsupported", persistResult: "unsupported", persistedAfter: "unsupported", estimate: "unsupported" };
  const persistedBefore = await run(() => sm.persisted());
  const persistResult = await run(() => sm.persist());
  const persistedAfter = await run(() => sm.persisted());
  const estimate = await run(() => sm.estimate());
  return { persistedBefore, persistResult, persistedAfter, estimate };
}

/** Round trip of the wrap in memory (no storage): wrap, unwrap, same public key. */
export async function wrapRoundTrip(): Promise<boolean> {
  const aes = await crypto.subtle.generateKey({ name: "AES-GCM", length: 256 }, false, ["encrypt", "decrypt"]);
  const kp = x25519.keygen();
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const aad = new TextEncoder().encode("local-key-id");
  const ct = await crypto.subtle.encrypt({ name: "AES-GCM", iv, additionalData: aad }, aes, Uint8Array.from(kp.secretKey));
  const priv = new Uint8Array(await crypto.subtle.decrypt({ name: "AES-GCM", iv, additionalData: aad }, aes, ct));
  return hex(x25519.getPublicKey(priv)) === hex(kp.publicKey);
}
