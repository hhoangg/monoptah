// The TRANSPORT layer on top of the two Noise CipherStates (architecture.md 5.2, 5.4):
// plaintext kinds (MSG, CHUNK, REKEY), chunking of large messages, reassembly of
// incoming chunks, and the REKEY schedule. Used by the mock desktop (responder) and by
// the test client (initiator); it has no role-specific code.
import {
  MAX_RESPONSE_BYTES,
  REKEY_AFTER_MESSAGES,
  REKEY_AFTER_MS,
  Reassembler,
  encodePlaintext,
  splitMessage,
  type PushResult,
} from "@monoptah/protocol";
import type { CipherState } from "./noise.ts";

const NO_AD = new Uint8Array(0);

export interface SecureChannelOptions {
  /** Encrypts what this side sends. */
  send: CipherState;
  /** Decrypts what this side receives. */
  recv: CipherState;
  /** Send REKEY after this many messages (default 2^16, architecture.md 5.4). Lower it in tests. */
  rekeyAfterMessages?: number;
  /** Send REKEY after this much time (default 1 hour). */
  rekeyAfterMs?: number;
  /** Largest message this side sends, in bytes (default 8 MiB). */
  maxOutgoingBytes?: number;
  /** Largest message this side accepts, in bytes (default 8 MiB). */
  maxIncomingBytes?: number;
  now?: () => number;
}

export class SecureChannel {
  readonly #send: CipherState;
  readonly #recv: CipherState;
  readonly #rekeyAfterMessages: number;
  readonly #rekeyAfterMs: number;
  readonly #maxOutgoingBytes: number;
  readonly #now: () => number;
  readonly #reassembler: Reassembler;
  #sentSinceRekey = 0;
  #rekeyedAt: number;
  #nextMsgId = 0;

  /** REKEY messages this side has sent and received (for tests). */
  rekeysSent = 0;
  rekeysReceived = 0;

  constructor(options: SecureChannelOptions) {
    this.#send = options.send;
    this.#recv = options.recv;
    this.#rekeyAfterMessages = options.rekeyAfterMessages ?? REKEY_AFTER_MESSAGES;
    this.#rekeyAfterMs = options.rekeyAfterMs ?? REKEY_AFTER_MS;
    this.#maxOutgoingBytes = options.maxOutgoingBytes ?? MAX_RESPONSE_BYTES;
    this.#now = options.now ?? Date.now;
    this.#rekeyedAt = this.#now();
    this.#reassembler = new Reassembler({ now: this.#now, ...(options.maxIncomingBytes === undefined ? {} : { maxBytes: options.maxIncomingBytes }) });
  }

  /** Messages currently being reassembled. */
  get inFlight(): number {
    return this.#reassembler.inFlight;
  }

  /** Throws `ReassemblyError("timeout")` when a message has been in flight too long. Call from a timer. */
  checkTimeouts(): void {
    this.#reassembler.checkTimeouts();
  }

  /**
   * Turns one serialized message into the TRANSPORT bodies to send, in order: MSG or
   * CHUNKs, each encrypted, with a REKEY (and a `rekey()` of the sending state)
   * inserted whenever the schedule says so. Throws `SplitError` above the size limit.
   */
  seal(message: Uint8Array): Uint8Array[] {
    const bodies: Uint8Array[] = [];
    const msgId = this.#nextMsgId;
    this.#nextMsgId = (this.#nextMsgId + 1) >>> 0;
    for (const plaintext of splitMessage(message, msgId, this.#maxOutgoingBytes)) {
      if (this.#rekeyDue()) bodies.push(this.#sealRekey());
      bodies.push(this.#sealOne(plaintext));
    }
    return bodies;
  }

  /** Encrypts one already-encoded plaintext as it is, with no chunking or rekey check. For tests that send malformed plaintexts. */
  sealRaw(plaintext: Uint8Array): Uint8Array {
    return this.#sealOne(plaintext);
  }

  /** Decrypts one TRANSPORT body. A `rekey` result has already rekeyed the receiving state. */
  open(body: Uint8Array): PushResult {
    const plaintext = this.#recv.decryptWithAd(NO_AD, body);
    const result = this.#reassembler.push(plaintext);
    if (result.type === "rekey") {
      this.#recv.rekey();
      this.rekeysReceived += 1;
    }
    return result;
  }

  /** Drops partial messages and zeroes the cipher keys (session ended; the channel is unusable afterwards). */
  reset(): void {
    this.#reassembler.reset();
    this.#send.destroy();
    this.#recv.destroy();
  }

  #rekeyDue(): boolean {
    return this.#sentSinceRekey >= this.#rekeyAfterMessages || this.#now() - this.#rekeyedAt >= this.#rekeyAfterMs;
  }

  #sealOne(plaintext: Uint8Array): Uint8Array {
    const body = this.#send.encryptWithAd(NO_AD, plaintext);
    this.#sentSinceRekey += 1;
    return body;
  }

  /** The REKEY message is encrypted with the old key; only afterwards does the sender rekey. */
  #sealRekey(): Uint8Array {
    const body = this.#sealOne(encodePlaintext({ kind: "rekey" }));
    this.#send.rekey();
    this.#sentSinceRekey = 0;
    this.#rekeyedAt = this.#now();
    this.rekeysSent += 1;
    return body;
  }
}
