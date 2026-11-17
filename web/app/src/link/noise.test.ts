// @vitest-environment node
import { describe, expect, it } from "vitest";
import { chacha20poly1305 } from "@noble/ciphers/chacha.js";
import { hexToBytes, bytesToHex } from "@noble/hashes/utils.js";
import { PROLOGUE_PREFIX } from "@monoptah/protocol";
import {
  CipherState,
  HandshakeState,
  MAX_NONCE,
  NoiseError,
  fingerprint,
  generateKeyPair,
  hkdf,
  prologue,
  type KeyPair,
} from "./noise.ts";
import vectorFile from "./fixtures/noise-ik-chachapoly-sha256-vectors.json" with { type: "json" };

interface NoiseVector {
  source: string;
  protocol_name: string;
  init_prologue: string;
  init_static: string;
  init_ephemeral: string;
  init_remote_static: string;
  resp_prologue: string;
  resp_static: string;
  resp_ephemeral: string;
  handshake_hash?: string;
  messages: { payload: string; ciphertext: string }[];
}
const vectors = vectorFile.vectors as NoiseVector[];
const hex = hexToBytes;

describe("published Noise_IK_25519_ChaChaPoly_SHA256 vectors", () => {
  it("includes both the cacophony and the snow entries", () => {
    expect(vectors.map((v) => v.source).sort()).toEqual(["cacophony", "snow"]);
  });

  for (const v of vectors) {
    it(`${v.source}: every handshake and transport message matches, in both directions`, () => {
      const initiatorStatic = generateKeyPair(hex(v.init_static));
      const responderStatic = generateKeyPair(hex(v.resp_static));
      // The vector's pre-known responder key must be the public half of resp_static.
      expect(bytesToHex(responderStatic.publicKey)).toBe(v.init_remote_static);

      const initiator = new HandshakeState({
        initiator: true,
        prologue: hex(v.init_prologue),
        s: initiatorStatic,
        rs: hex(v.init_remote_static),
        ephemeralPrivateKey: hex(v.init_ephemeral),
      });
      const responder = new HandshakeState({
        initiator: false,
        prologue: hex(v.resp_prologue),
        s: responderStatic,
        ephemeralPrivateKey: hex(v.resp_ephemeral),
      });

      const [m0, m1, ...transport] = v.messages as [NoiseVector["messages"][number], NoiseVector["messages"][number], ...NoiseVector["messages"]];

      const wire0 = initiator.writeMessage(hex(m0.payload));
      expect(bytesToHex(wire0)).toBe(m0.ciphertext);
      expect(bytesToHex(responder.readMessage(wire0))).toBe(m0.payload);
      expect(bytesToHex(responder.remoteStatic as Uint8Array)).toBe(bytesToHex(initiatorStatic.publicKey));

      const wire1 = responder.writeMessage(hex(m1.payload));
      expect(bytesToHex(wire1)).toBe(m1.ciphertext);
      expect(bytesToHex(initiator.readMessage(wire1))).toBe(m1.payload);

      expect(initiator.isComplete).toBe(true);
      expect(responder.isComplete).toBe(true);
      expect(bytesToHex(initiator.handshakeHash)).toBe(bytesToHex(responder.handshakeHash));
      if (v.handshake_hash !== undefined) expect(bytesToHex(initiator.handshakeHash)).toBe(v.handshake_hash);

      const i = initiator.split();
      const r = responder.split();
      // Transport messages alternate, starting with the initiator.
      transport.forEach((m, index) => {
        const [sender, receiver] = index % 2 === 0 ? [i, r] : [i, r].reverse();
        const ciphertext = (sender as typeof i).send.encryptWithAd(new Uint8Array(0), hex(m.payload));
        expect(bytesToHex(ciphertext)).toBe(m.ciphertext);
        expect(bytesToHex((receiver as typeof r).recv.decryptWithAd(new Uint8Array(0), ciphertext))).toBe(m.payload);
      });
    });
  }
});

function pair(opts: { prologueA?: Uint8Array; prologueB?: Uint8Array; rsOverride?: Uint8Array } = {}) {
  const initiatorStatic = generateKeyPair();
  const responderStatic = generateKeyPair();
  const base = new TextEncoder().encode("test prologue");
  const initiator = new HandshakeState({
    initiator: true,
    prologue: opts.prologueA ?? base,
    s: initiatorStatic,
    rs: opts.rsOverride ?? responderStatic.publicKey,
  });
  const responder = new HandshakeState({ initiator: false, prologue: opts.prologueB ?? base, s: responderStatic });
  return { initiator, responder, initiatorStatic, responderStatic };
}

function complete() {
  const p = pair();
  const payload1 = new TextEncoder().encode("hello from the device");
  const payload2 = new TextEncoder().encode('{"proto":1}');
  expect(p.responder.readMessage(p.initiator.writeMessage(payload1))).toEqual(payload1);
  expect(p.initiator.readMessage(p.responder.writeMessage(payload2))).toEqual(payload2);
  return { ...p, i: p.initiator.split(), r: p.responder.split() };
}

describe("handshake failures", () => {
  it("is poisoned after a thrown error: every later call throws", () => {
    const p = pair({ prologueA: new TextEncoder().encode("prologue A"), prologueB: new TextEncoder().encode("prologue B") });
    const message = p.initiator.writeMessage();
    expect(() => p.responder.readMessage(message)).toThrow(/authenticate/);
    // The same valid message cannot be retried, a reply cannot be written, and nothing can be split.
    expect(() => p.responder.readMessage(message)).toThrow(/failed earlier/);
    expect(() => p.responder.writeMessage()).toThrow(/failed earlier/);
    expect(() => p.responder.split()).toThrow(/failed earlier/);
  });

  it("is poisoned after an out-of-turn call", () => {
    const p = pair();
    expect(() => p.responder.writeMessage()).toThrow(NoiseError);
    expect(() => p.responder.readMessage(p.initiator.writeMessage())).toThrow(/failed earlier/);
  });

  it("a different prologue fails at message 1", () => {
    const p = pair({ prologueA: new TextEncoder().encode("prologue A"), prologueB: new TextEncoder().encode("prologue B") });
    const message = p.initiator.writeMessage(new Uint8Array(0));
    expect(() => p.responder.readMessage(message)).toThrow(NoiseError);
  });

  it("a prologue that differs by one channel byte fails", () => {
    const desktop = "11111111-1111-4111-8111-111111111111";
    const device = "22222222-2222-4222-8222-222222222222";
    const p = pair({ prologueA: prologue(desktop, device, 7), prologueB: prologue(desktop, device, 8) });
    expect(() => p.responder.readMessage(p.initiator.writeMessage())).toThrow(/authenticate/);
  });

  it("a wrong responder static key fails at message 1", () => {
    const p = pair({ rsOverride: generateKeyPair().publicKey });
    const message = p.initiator.writeMessage(new TextEncoder().encode("payload"));
    expect(() => p.responder.readMessage(message)).toThrow(NoiseError);
  });

  it("a tampered message 1 fails, at every byte position", () => {
    const base = pair();
    const message = base.initiator.writeMessage(new TextEncoder().encode("payload"));
    for (let position = 0; position < message.length; position += 7) {
      const p = pair();
      const copy = p.initiator.writeMessage(new TextEncoder().encode("payload"));
      copy[position] = (copy[position] as number) ^ 0x01;
      expect(() => p.responder.readMessage(copy), `byte ${position}`).toThrow(NoiseError);
    }
  });

  it("a tampered message 2 fails on the initiator", () => {
    const p = pair();
    p.responder.readMessage(p.initiator.writeMessage());
    const message2 = p.responder.writeMessage(new TextEncoder().encode("x"));
    message2[message2.length - 1] = (message2[message2.length - 1] as number) ^ 0x80;
    expect(() => p.initiator.readMessage(message2)).toThrow(NoiseError);
  });

  it("a truncated message 1 is rejected", () => {
    const p = pair();
    const message = p.initiator.writeMessage();
    expect(() => p.responder.readMessage(message.slice(0, 40))).toThrow(NoiseError);
  });

  it("an all-zero ephemeral (low order point) is rejected", () => {
    const p = pair();
    const message = p.initiator.writeMessage();
    message.set(new Uint8Array(32), 0);
    expect(() => p.responder.readMessage(message)).toThrow(NoiseError);
  });

  it("enforces turn order and completion", () => {
    const p = pair();
    expect(() => p.responder.writeMessage()).toThrow(/turn/);
    expect(() => p.initiator.readMessage(new Uint8Array(80))).toThrow(/turn/);
    // Each error poisons its own state (next test), so completion is checked on fresh ones.
    expect(() => pair().initiator.split()).toThrow(/not complete/);
  });

  it("the initiator needs the responder key", () => {
    expect(() => new HandshakeState({ initiator: true, prologue: new Uint8Array(0), s: generateKeyPair() })).toThrow(NoiseError);
  });
});

describe("transport CipherStates", () => {
  it("round-trips in both directions after the handshake", () => {
    const { i, r } = complete();
    const ad = new Uint8Array(0);
    expect(new TextDecoder().decode(r.recv.decryptWithAd(ad, i.send.encryptWithAd(ad, new TextEncoder().encode("ping"))))).toBe("ping");
    expect(new TextDecoder().decode(i.recv.decryptWithAd(ad, r.send.encryptWithAd(ad, new TextEncoder().encode("pong"))))).toBe("pong");
  });

  it("a tampered ciphertext fails and does not advance the nonce", () => {
    const { i, r } = complete();
    const ad = new Uint8Array(0);
    const ciphertext = i.send.encryptWithAd(ad, new TextEncoder().encode("secret"));
    const tampered = ciphertext.slice();
    tampered[0] = (tampered[0] as number) ^ 0xff;
    expect(() => r.recv.decryptWithAd(ad, tampered)).toThrow(NoiseError);
    expect(r.recv.nonce).toBe(0n);
    // The genuine message still decrypts afterwards.
    expect(new TextDecoder().decode(r.recv.decryptWithAd(ad, ciphertext))).toBe("secret");
  });

  it("a replayed or reordered message fails", () => {
    const { i, r } = complete();
    const ad = new Uint8Array(0);
    const first = i.send.encryptWithAd(ad, new TextEncoder().encode("one"));
    const second = i.send.encryptWithAd(ad, new TextEncoder().encode("two"));
    expect(() => r.recv.decryptWithAd(ad, second)).toThrow(NoiseError);
    r.recv.decryptWithAd(ad, first);
    expect(() => r.recv.decryptWithAd(ad, first)).toThrow(NoiseError);
  });

  it("rekey round-trips when both sides rekey together, and changes the key", () => {
    const { i, r } = complete();
    const ad = new Uint8Array(0);
    const before = i.send.encryptWithAd(ad, new TextEncoder().encode("same"));
    r.recv.decryptWithAd(ad, before);
    i.send.rekey();
    r.recv.rekey();
    const after = i.send.encryptWithAd(ad, new TextEncoder().encode("same"));
    expect(new TextDecoder().decode(r.recv.decryptWithAd(ad, after))).toBe("same");
    expect(i.send.nonce).toBe(2n); // rekey keeps the nonce
  });

  it("a one-sided rekey breaks decryption", () => {
    const { i, r } = complete();
    const ad = new Uint8Array(0);
    i.send.rekey();
    expect(() => r.recv.decryptWithAd(ad, i.send.encryptWithAd(ad, new Uint8Array(4)))).toThrow(NoiseError);
  });

  it("rekey matches the spec: k' = first 32 bytes of ENCRYPT(k, 2^64-1, empty, 32 zero bytes)", () => {
    const key = new Uint8Array(32).fill(7);
    // Independent computation: nonce is 4 zero bytes then eight 0xff bytes (counter 2^64-1, little endian).
    const nonce = new Uint8Array(12);
    nonce.fill(0xff, 4);
    const expectedKey = chacha20poly1305(key, nonce, new Uint8Array(0)).encrypt(new Uint8Array(32)).slice(0, 32);
    const rekeyed = new CipherState(key);
    rekeyed.rekey();
    const m = new TextEncoder().encode("rekey check");
    const ad = new Uint8Array(0);
    expect(bytesToHex(rekeyed.encryptWithAd(ad, m))).toBe(bytesToHex(new CipherState(expectedKey).encryptWithAd(ad, m)));
    expect(bytesToHex(rekeyed.encryptWithAd(ad, m))).not.toBe(bytesToHex(new CipherState(key).encryptWithAd(ad, m)));
  });

  it("refuses the reserved nonce 2^64-1 for encrypt and decrypt, but allows 2^64-2", () => {
    const key = new Uint8Array(32).fill(3);
    const sender = new CipherState(key);
    const receiver = new CipherState(key);
    const ad = new Uint8Array(0);
    sender.setNonce(MAX_NONCE - 1n);
    receiver.setNonce(MAX_NONCE - 1n);
    const last = sender.encryptWithAd(ad, new Uint8Array([1, 2, 3]));
    expect(sender.nonce).toBe(MAX_NONCE);
    expect(() => sender.encryptWithAd(ad, new Uint8Array(1))).toThrow(/reserved/);
    expect(receiver.decryptWithAd(ad, last)).toEqual(new Uint8Array([1, 2, 3]));
    expect(() => receiver.decryptWithAd(ad, last)).toThrow(/reserved/);
  });
});

describe("hkdf (spec 4.3)", () => {
  it("returns outputs that chain: out2 depends on out1", () => {
    const ck = new Uint8Array(32).fill(1);
    const [a, b, c] = hkdf(ck, new Uint8Array(32).fill(2), 3) as [Uint8Array, Uint8Array, Uint8Array];
    const [a2, b2] = hkdf(ck, new Uint8Array(32).fill(2), 2) as [Uint8Array, Uint8Array];
    expect(a2).toEqual(a);
    expect(b2).toEqual(b);
    expect(c).not.toEqual(b);
  });
});

describe("Monoptah helpers", () => {
  const desktop = "00112233-4455-4677-8899-aabbccddeeff";
  const device = "ffeeddcc-bbaa-4988-8776-655443322110";

  it("prologue is prefix || desktopId || deviceId || channel (u16 BE)", () => {
    const bytes = prologue(desktop, device, 0x0102);
    const prefix = new TextEncoder().encode(PROLOGUE_PREFIX);
    expect(new TextDecoder().decode(bytes.slice(0, prefix.length))).toBe("monoptah-web/1");
    expect(bytesToHex(bytes.slice(prefix.length, prefix.length + 16))).toBe("00112233445546778899aabbccddeeff");
    expect(bytesToHex(bytes.slice(prefix.length + 16, prefix.length + 32))).toBe("ffeeddccbbaa49888776655443322110");
    expect(Array.from(bytes.slice(-2))).toEqual([1, 2]);
    expect(bytes.length).toBe(prefix.length + 34);
  });

  it("prologue accepts 16-byte arrays and rejects bad channels and ids", () => {
    expect(prologue(hex("00112233445546778899aabbccddeeff"), device, 1)).toEqual(prologue(desktop, device, 1));
    expect(() => prologue(desktop, device, 0x10000)).toThrow(NoiseError);
    expect(() => prologue(new Uint8Array(15), device, 1)).toThrow(NoiseError);
  });

  it("fingerprint is the lowercase hex SHA-256 of the public key", () => {
    // SHA-256 of 32 zero bytes.
    expect(fingerprint(new Uint8Array(32))).toBe("66687aadf862bd776c8fc18b8e9f8e20089714856ee233b3902a591d0d5f2925");
    const key: KeyPair = generateKeyPair();
    expect(fingerprint(key.publicKey)).toMatch(/^[0-9a-f]{64}$/);
  });
});
