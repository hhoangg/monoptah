// Noise_IK_25519_ChaChaPoly_SHA256 (Noise Protocol Framework, revision 34) on @noble.
//
// Both roles: initiator (the device) and responder (the desktop). The module holds
// no application code: it knows the Noise objects (CipherState, SymmetricState,
// HandshakeState), the IK pattern, and two helpers that define the Monoptah
// identities (`prologue`, `fingerprint`). Primitives are @noble only:
//   - x25519 (DH, 32-byte keys)        @noble/curves
//   - ChaCha20-Poly1305 (IETF AEAD)    @noble/ciphers
//   - SHA-256, HMAC-SHA-256 (HKDF)     @noble/hashes
//
// Spec sections cited below: 5 (processing rules: CipherState, SymmetricState,
// HandshakeState), 4.1 (DH), 4.2 (cipher functions), 4.3 (hash and HKDF),
// 7.4 (IK), 7.2 (Rekey), 11.3 (nonce 2^64-1 is reserved).
import { x25519 } from "@noble/curves/ed25519.js";
import { chacha20poly1305 } from "@noble/ciphers/chacha.js";
import { hmac } from "@noble/hashes/hmac.js";
import { sha256 } from "@noble/hashes/sha2.js";
import { bytesToHex, concatBytes, randomBytes } from "@noble/hashes/utils.js";
import { PROLOGUE_PREFIX, uuidToBytes } from "@monoptah/protocol";

export const PROTOCOL_NAME = "Noise_IK_25519_ChaChaPoly_SHA256";
export const DHLEN = 32;
export const HASHLEN = 32;
export const TAG_LEN = 16;
/** Largest Noise message (spec section 3). */
export const MAX_NOISE_MESSAGE = 65_535;
/** 2^64 - 1: the nonce value that must never be used (spec 5.1). */
export const MAX_NONCE = 2n ** 64n - 1n;

const EMPTY = new Uint8Array(0);

export type NoiseErrorKind =
  | "decrypt"
  | "nonce_exhausted"
  | "bad_key"
  | "bad_message"
  | "message_too_large"
  | "bad_state"
  | "bad_dh";

export class NoiseError extends Error {
  readonly kind: NoiseErrorKind;
  constructor(kind: NoiseErrorKind, message: string) {
    super(message);
    this.name = "NoiseError";
    this.kind = kind;
  }
}

export interface KeyPair {
  /** 32 bytes. */
  privateKey: Uint8Array;
  /** 32 bytes. */
  publicKey: Uint8Array;
}

/** GENERATE_KEYPAIR (spec 4.1). With `privateKey` the pair is derived, which is how tests inject keys. */
export function generateKeyPair(privateKey: Uint8Array = randomBytes(32)): KeyPair {
  if (privateKey.length !== DHLEN) throw new NoiseError("bad_key", `a private key is ${DHLEN} bytes, got ${privateKey.length}`);
  return { privateKey: privateKey.slice(), publicKey: x25519.getPublicKey(privateKey) };
}

/** DH (spec 4.1). @noble rejects the all-zero shared secret (low-order points), which surfaces as `bad_dh`. */
function dh(privateKey: Uint8Array, publicKey: Uint8Array): Uint8Array {
  try {
    return x25519.getSharedSecret(privateKey, publicKey);
  } catch (error) {
    throw new NoiseError("bad_dh", `invalid DH input: ${error instanceof Error ? error.message : String(error)}`);
  }
}

// --- Hash and HKDF (spec 4.3 and 5.2) ---

/** HKDF with HMAC-SHA-256 as the Noise spec defines it (not RFC 5869 extract/expand with info). Returns `n` (2 or 3) outputs. */
export function hkdf(chainingKey: Uint8Array, inputKeyMaterial: Uint8Array, n: 2 | 3): Uint8Array[] {
  const tempKey = hmac(sha256, chainingKey, inputKeyMaterial);
  const out1 = hmac(sha256, tempKey, Uint8Array.of(0x01));
  const out2 = hmac(sha256, tempKey, concatBytes(out1, Uint8Array.of(0x02)));
  if (n === 2) return [out1, out2];
  const out3 = hmac(sha256, tempKey, concatBytes(out2, Uint8Array.of(0x03)));
  return [out1, out2, out3];
}

// --- CipherState (spec 5.1) ---

/** ChaChaPoly nonce (spec 12.3): 32 bits of zeros then the 64-bit counter, little endian. */
function chachaNonce(n: bigint): Uint8Array {
  const nonce = new Uint8Array(12);
  new DataView(nonce.buffer).setBigUint64(4, n, true);
  return nonce;
}

function encrypt(k: Uint8Array, n: bigint, ad: Uint8Array, plaintext: Uint8Array): Uint8Array {
  return chacha20poly1305(k, chachaNonce(n), ad).encrypt(plaintext);
}

function decrypt(k: Uint8Array, n: bigint, ad: Uint8Array, ciphertext: Uint8Array): Uint8Array {
  return chacha20poly1305(k, chachaNonce(n), ad).decrypt(ciphertext);
}

export class CipherState {
  #k: Uint8Array | null;
  #n = 0n;

  /** Without a key the state passes data through unchanged (a handshake before the first DH). */
  constructor(key: Uint8Array | null = null) {
    this.#k = null;
    if (key !== null) this.initializeKey(key);
  }

  /** InitializeKey: installs `key` and restarts the nonce at 0. */
  initializeKey(key: Uint8Array): void {
    if (key.length !== 32) throw new NoiseError("bad_key", `a cipher key is 32 bytes, got ${key.length}`);
    this.#k = key.slice();
    this.#n = 0n;
  }

  hasKey(): boolean {
    return this.#k !== null;
  }

  /** The nonce the next message uses. Rekey does not reset it (spec 4.2). */
  get nonce(): bigint {
    return this.#n;
  }

  /** SetNonce (spec 5.1), for tests and for resuming. */
  setNonce(n: bigint): void {
    if (n < 0n || n > MAX_NONCE) throw new NoiseError("bad_state", "nonce out of range");
    this.#n = n;
  }

  encryptWithAd(ad: Uint8Array, plaintext: Uint8Array): Uint8Array {
    if (this.#k === null) return plaintext;
    if (this.#n === MAX_NONCE) throw new NoiseError("nonce_exhausted", "nonce 2^64-1 is reserved; rekey or end the session");
    const out = encrypt(this.#k, this.#n, ad, plaintext);
    this.#n += 1n;
    return out;
  }

  /** The nonce advances only when the message authenticates (spec 5.1). */
  decryptWithAd(ad: Uint8Array, ciphertext: Uint8Array): Uint8Array {
    if (this.#k === null) return ciphertext;
    if (this.#n === MAX_NONCE) throw new NoiseError("nonce_exhausted", "nonce 2^64-1 is reserved; rekey or end the session");
    let out: Uint8Array;
    try {
      out = decrypt(this.#k, this.#n, ad, ciphertext);
    } catch {
      throw new NoiseError("decrypt", "message failed to authenticate");
    }
    this.#n += 1n;
    return out;
  }

  /** Rekey (spec 4.2): k = first 32 bytes of ENCRYPT(k, 2^64-1, empty, 32 zero bytes). The nonce is kept. */
  rekey(): void {
    if (this.#k === null) throw new NoiseError("bad_state", "cannot rekey a CipherState without a key");
    this.#k = encrypt(this.#k, MAX_NONCE, EMPTY, new Uint8Array(32)).slice(0, 32);
  }
}

// --- SymmetricState (spec 5.2) ---

class SymmetricState {
  #ck: Uint8Array;
  #h: Uint8Array;
  readonly #cipher = new CipherState();

  constructor(protocolName: string) {
    const name = new TextEncoder().encode(protocolName);
    if (name.length <= HASHLEN) {
      this.#h = new Uint8Array(HASHLEN);
      this.#h.set(name);
    } else {
      this.#h = sha256(name);
    }
    this.#ck = this.#h;
  }

  get handshakeHash(): Uint8Array {
    return this.#h;
  }

  mixKey(inputKeyMaterial: Uint8Array): void {
    const [ck, tempK] = hkdf(this.#ck, inputKeyMaterial, 2) as [Uint8Array, Uint8Array];
    this.#ck = ck;
    this.#cipher.initializeKey(tempK);
  }

  mixHash(data: Uint8Array): void {
    this.#h = sha256(concatBytes(this.#h, data));
  }

  encryptAndHash(plaintext: Uint8Array): Uint8Array {
    const ciphertext = this.#cipher.encryptWithAd(this.#h, plaintext);
    this.mixHash(ciphertext);
    return ciphertext;
  }

  decryptAndHash(ciphertext: Uint8Array): Uint8Array {
    const plaintext = this.#cipher.decryptWithAd(this.#h, ciphertext);
    this.mixHash(ciphertext);
    return plaintext;
  }

  /** Split (spec 5.2): two CipherStates, the first for initiator -> responder. */
  split(): [CipherState, CipherState] {
    const [k1, k2] = hkdf(this.#ck, EMPTY, 2) as [Uint8Array, Uint8Array];
    return [new CipherState(k1), new CipherState(k2)];
  }
}

// --- HandshakeState for IK (spec 5.3, 7.4) ---
//
//   IK:  <- s
//        ...
//        -> e, es, s, ss
//        <- e, ee, se
//
// The responder static key is a pre-message known to both sides.

type Token = "e" | "s" | "ee" | "es" | "se" | "ss";
const IK_MESSAGES: readonly (readonly Token[])[] = [
  ["e", "es", "s", "ss"],
  ["e", "ee", "se"],
];

export interface HandshakeOptions {
  initiator: boolean;
  /** Prologue bytes, mixed into the hash first; both sides must use identical bytes. */
  prologue: Uint8Array;
  /** Own static key pair. */
  s: KeyPair;
  /** The responder static public key. Required for the initiator; the responder ignores it. */
  rs?: Uint8Array;
  /** Fixed ephemeral private key for deterministic tests. Default: random. */
  ephemeralPrivateKey?: Uint8Array;
}

export interface SplitResult {
  /** Encrypts what this side sends. */
  send: CipherState;
  /** Decrypts what this side receives. */
  recv: CipherState;
}

export class HandshakeState {
  readonly #symmetric = new SymmetricState(PROTOCOL_NAME);
  readonly #initiator: boolean;
  readonly #s: KeyPair;
  #e: KeyPair | null = null;
  #rs: Uint8Array | null;
  #re: Uint8Array | null = null;
  readonly #ephemeralPrivateKey: Uint8Array | undefined;
  #messageIndex = 0;
  #split: SplitResult | null = null;
  /** Set by the first error thrown from a handshake call (spec 5.3: a failed handshake must be abandoned). */
  #poisoned: NoiseError | null = null;

  constructor(options: HandshakeOptions) {
    this.#initiator = options.initiator;
    this.#s = options.s;
    this.#ephemeralPrivateKey = options.ephemeralPrivateKey;
    this.#symmetric.mixHash(options.prologue);
    // Pre-message `<- s`: both sides hash the responder static key.
    if (options.initiator) {
      if (options.rs === undefined || options.rs.length !== DHLEN) {
        throw new NoiseError("bad_key", "the initiator needs the responder static public key (32 bytes)");
      }
      this.#rs = options.rs.slice();
      this.#symmetric.mixHash(this.#rs);
    } else {
      this.#rs = null;
      this.#symmetric.mixHash(this.#s.publicKey);
    }
  }

  get isComplete(): boolean {
    return this.#messageIndex === IK_MESSAGES.length;
  }

  /** H after the handshake (spec 5.2 GetHandshakeHash), a unique channel binding. */
  get handshakeHash(): Uint8Array {
    return this.#symmetric.handshakeHash;
  }

  /** The peer static public key: known up front for the initiator, learned from message 1 by the responder. */
  get remoteStatic(): Uint8Array | null {
    return this.#rs === null ? null : this.#rs.slice();
  }

  #myTurn(): boolean {
    // Message 0 is initiator -> responder, then they alternate.
    return this.#initiator === (this.#messageIndex % 2 === 0);
  }

  /**
   * Runs one step. After any thrown error the state is unusable: symmetric state may be half updated,
   * so every later call throws instead of continuing from a corrupt transcript.
   */
  #guard<T>(step: () => T): T {
    if (this.#poisoned !== null) throw new NoiseError("bad_state", `the handshake failed earlier (${this.#poisoned.kind}) and must be abandoned`);
    try {
      return step();
    } catch (error) {
      this.#poisoned = error instanceof NoiseError ? error : new NoiseError("bad_state", error instanceof Error ? error.message : String(error));
      throw error;
    }
  }

  /** WriteMessage: produces the next handshake message carrying `payload`. */
  writeMessage(payload: Uint8Array = EMPTY): Uint8Array {
    return this.#guard(() => this.#write(payload));
  }

  #write(payload: Uint8Array): Uint8Array {
    if (this.isComplete) throw new NoiseError("bad_state", "the handshake is complete");
    if (!this.#myTurn()) throw new NoiseError("bad_state", "it is not this side's turn to write");
    const parts: Uint8Array[] = [];
    for (const token of IK_MESSAGES[this.#messageIndex] as readonly Token[]) {
      switch (token) {
        case "e":
          this.#e = generateKeyPair(this.#ephemeralPrivateKey);
          parts.push(this.#e.publicKey);
          this.#symmetric.mixHash(this.#e.publicKey);
          break;
        case "s":
          parts.push(this.#symmetric.encryptAndHash(this.#s.publicKey));
          break;
        default:
          this.#mixDh(token);
      }
    }
    parts.push(this.#symmetric.encryptAndHash(payload));
    const message = concatBytes(...parts);
    if (message.length > MAX_NOISE_MESSAGE) throw new NoiseError("message_too_large", `handshake message of ${message.length} bytes`);
    this.#advance();
    return message;
  }

  /** ReadMessage: consumes the next handshake message and returns its payload. Throws on any failure. */
  readMessage(message: Uint8Array): Uint8Array {
    return this.#guard(() => this.#read(message));
  }

  #read(message: Uint8Array): Uint8Array {
    if (this.isComplete) throw new NoiseError("bad_state", "the handshake is complete");
    if (this.#myTurn()) throw new NoiseError("bad_state", "it is not this side's turn to read");
    if (message.length > MAX_NOISE_MESSAGE) throw new NoiseError("message_too_large", `handshake message of ${message.length} bytes`);
    let offset = 0;
    const take = (length: number): Uint8Array => {
      if (message.length - offset < length) throw new NoiseError("bad_message", "handshake message is too short");
      const out = message.slice(offset, offset + length);
      offset += length;
      return out;
    };
    for (const token of IK_MESSAGES[this.#messageIndex] as readonly Token[]) {
      switch (token) {
        case "e":
          this.#re = take(DHLEN);
          this.#symmetric.mixHash(this.#re);
          break;
        case "s": {
          // The static key is encrypted once a key exists (always true for IK message 1).
          const length = DHLEN + TAG_LEN;
          this.#rs = this.#symmetric.decryptAndHash(take(length));
          break;
        }
        default:
          this.#mixDh(token);
      }
    }
    const payload = this.#symmetric.decryptAndHash(message.slice(offset));
    this.#advance();
    return payload;
  }

  /** Split (spec 5.2): valid once the handshake is complete. */
  split(): SplitResult {
    return this.#guard(() => this.#splitNow());
  }

  #splitNow(): SplitResult {
    if (!this.isComplete) throw new NoiseError("bad_state", "the handshake is not complete");
    if (this.#split === null) {
      const [c1, c2] = this.#symmetric.split();
      this.#split = this.#initiator ? { send: c1, recv: c2 } : { send: c2, recv: c1 };
    }
    return this.#split;
  }

  #advance(): void {
    this.#messageIndex += 1;
  }

  /** DH token: the first letter names the initiator key, the second the responder key (spec 5.3). */
  #mixDh(token: "ee" | "es" | "se" | "ss"): void {
    const localLetter = this.#initiator ? token[0] : token[1];
    const remoteLetter = this.#initiator ? token[1] : token[0];
    const localPrivate = localLetter === "e" ? this.#need(this.#e, "ephemeral").privateKey : this.#s.privateKey;
    const remotePublic = remoteLetter === "e" ? this.#need(this.#re, "remote ephemeral") : this.#need(this.#rs, "remote static");
    this.#symmetric.mixKey(dh(localPrivate, remotePublic));
  }

  #need<T>(value: T | null, what: string): T {
    if (value === null) throw new NoiseError("bad_state", `${what} key is not available yet`);
    return value;
  }
}

// --- Monoptah identities (architecture.md 5.3) ---

type Id16 = string | Uint8Array;

function id16(value: Id16, what: string): Uint8Array {
  const bytes = typeof value === "string" ? uuidToBytes(value) : value;
  if (bytes.length !== 16) throw new NoiseError("bad_key", `${what} must be 16 bytes`);
  return bytes;
}

/** `"monoptah-web/1" || desktopId(16) || deviceId(16) || channel(u16 big endian)`. Ids are UUID strings or 16 bytes. */
export function prologue(desktopId: Id16, deviceId: Id16, channel: number): Uint8Array {
  if (!Number.isInteger(channel) || channel < 0 || channel > 0xffff) throw new NoiseError("bad_state", `channel must be a u16, got ${channel}`);
  return concatBytes(
    new TextEncoder().encode(PROLOGUE_PREFIX),
    id16(desktopId, "desktopId"),
    id16(deviceId, "deviceId"),
    Uint8Array.of(channel >> 8, channel & 0xff),
  );
}

/** Lowercase hex SHA-256 of a public key: the device and desktop fingerprint (security.md 5). */
export function fingerprint(publicKey: Uint8Array): string {
  return bytesToHex(sha256(publicKey));
}
