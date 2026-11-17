// Builds the TS-side vector file with the implementation under test (web/app/src/link/noise.ts).
import { generateKeyPair, HandshakeState, prologue } from "../../app/src/link/noise.ts";
import type { SplitResult } from "../../app/src/link/noise.ts";
import { bs, COMMENT, PROTOCOL } from "./schema.ts";
import type { ByteStr, NegativeCase, Party, PositiveCase, TransportStep, VectorFile } from "./schema.ts";

/** Maximum transport plaintext for one Noise message: 65535 - 16 tag bytes. */
export const MAX_TRANSPORT_PLAINTEXT = 65_519;

const fillKey = (byte: number): Uint8Array => new Uint8Array(32).fill(byte);
const enc = (text: string): Uint8Array => new TextEncoder().encode(text);

/** Deterministic non-uniform bytes. */
export function pattern(length: number, seed: number): Uint8Array {
  const out = new Uint8Array(length);
  for (let i = 0; i < length; i += 1) out[i] = (seed + i * 31) & 0xff;
  return out;
}

// Obviously fake keys: one repeated byte each.
const KEYS = {
  initiatorStatic: fillKey(0x01),
  initiatorEphemeral: fillKey(0x02),
  responderStatic: fillKey(0x03),
  responderEphemeral: fillKey(0x04),
  wrongResponderStatic: fillKey(0x05),
};

type Plan = ({ from: "initiator" | "responder"; plaintext: Uint8Array } | { rekey: "initiator->responder" | "responder->initiator" })[];

function party(role: "initiator" | "responder"): Party {
  return role === "initiator"
    ? { static: bs(KEYS.initiatorStatic), ephemeral: bs(KEYS.initiatorEphemeral) }
    : { static: bs(KEYS.responderStatic), ephemeral: bs(KEYS.responderEphemeral) };
}

function positive(name: string, pro: Uint8Array, payload1: Uint8Array, payload2: Uint8Array, plan: Plan): PositiveCase {
  const iStatic = generateKeyPair(KEYS.initiatorStatic);
  const rStatic = generateKeyPair(KEYS.responderStatic);
  const init = new HandshakeState({ initiator: true, prologue: pro, s: iStatic, rs: rStatic.publicKey, ephemeralPrivateKey: KEYS.initiatorEphemeral });
  const resp = new HandshakeState({ initiator: false, prologue: pro, s: rStatic, ephemeralPrivateKey: KEYS.responderEphemeral });
  const m1 = init.writeMessage(payload1);
  const got1 = resp.readMessage(m1);
  if (got1.length !== payload1.length) throw new Error("payload 1 mismatch");
  const m2 = resp.writeMessage(payload2);
  init.readMessage(m2);
  const iHash = init.handshakeHash.slice();
  const rHash = resp.handshakeHash.slice();
  const ic: SplitResult = init.split();
  const rc: SplitResult = resp.split();
  const transport: TransportStep[] = [];
  for (const step of plan) {
    if ("rekey" in step) {
      if (step.rekey === "initiator->responder") {
        ic.send.rekey();
        rc.recv.rekey();
      } else {
        rc.send.rekey();
        ic.recv.rekey();
      }
      transport.push({ rekey: step.rekey });
      continue;
    }
    const sender = step.from === "initiator" ? ic : rc;
    const receiver = step.from === "initiator" ? rc : ic;
    const ciphertext = sender.send.encryptWithAd(new Uint8Array(0), step.plaintext);
    const back = receiver.recv.decryptWithAd(new Uint8Array(0), ciphertext);
    if (back.length !== step.plaintext.length) throw new Error("self roundtrip failed");
    transport.push({ from: step.from, plaintext: bs(step.plaintext), ciphertext: { hex: hex(ciphertext) } });
  }
  return {
    name,
    prologue: bs(pro),
    initiator: party("initiator"),
    responder: party("responder"),
    handshake: [
      { payload: bs(payload1), message: { hex: hex(m1) } },
      { payload: bs(payload2), message: { hex: hex(m2) } },
    ],
    publicKeys: { initiatorStatic: bs(iStatic.publicKey), responderStatic: bs(rStatic.publicKey) },
    handshakeHash: { initiator: bs(iHash), responder: bs(rHash) },
    transport,
  };
}

function hex(bytes: Uint8Array): string {
  return Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
}

function negative(name: string, kind: NegativeCase["kind"], initiatorPrologue: Uint8Array, responderPrologue: Uint8Array, believedResponderStatic: Uint8Array): NegativeCase {
  const iStatic = generateKeyPair(KEYS.initiatorStatic);
  const init = new HandshakeState({
    initiator: true,
    prologue: initiatorPrologue,
    s: iStatic,
    rs: generateKeyPair(believedResponderStatic).publicKey,
    ephemeralPrivateKey: KEYS.initiatorEphemeral,
  });
  const message1: ByteStr = { hex: hex(init.writeMessage(enc("negative case payload"))) };
  return {
    name,
    kind,
    initiatorPrologue: bs(initiatorPrologue),
    responderPrologue: bs(responderPrologue),
    initiator: party("initiator"),
    initiatorRemoteStatic: bs(generateKeyPair(believedResponderStatic).publicKey),
    responderStatic: bs(KEYS.responderStatic),
    message1,
  };
}

export function buildVectors(): VectorFile {
  const empty = new Uint8Array(0);
  // Fake ids (obviously fake UUIDs) for the Monoptah prologue.
  const monoptahPrologue = prologue("00000000-0000-4000-8000-000000000001", "00000000-0000-4000-8000-000000000002", 7);
  const wrongPrologue = prologue("00000000-0000-4000-8000-000000000001", "00000000-0000-4000-8000-000000000002", 8);
  const cases: PositiveCase[] = [
    positive("plain-empty-prologue-empty-payloads", empty, empty, empty, [
      { from: "initiator", plaintext: enc("hello") },
      { from: "responder", plaintext: enc("world") },
    ]),
    positive("monoptah-prologue-with-payloads", monoptahPrologue, enc("payload in message 1"), enc("payload in message 2"), [
      { from: "initiator", plaintext: enc("first from initiator") },
      { from: "responder", plaintext: pattern(100, 3) },
      { from: "initiator", plaintext: empty },
      { from: "initiator", plaintext: pattern(1000, 9) },
      { from: "responder", plaintext: empty },
      { from: "responder", plaintext: enc("last from responder") },
    ]),
    positive("max-size-transport", monoptahPrologue, empty, empty, [
      { from: "initiator", plaintext: new Uint8Array(MAX_TRANSPORT_PLAINTEXT).fill(0xa5) },
      { from: "responder", plaintext: new Uint8Array(MAX_TRANSPORT_PLAINTEXT).fill(0x5a) },
      { from: "initiator", plaintext: enc("small after max") },
    ]),
    positive("rekey-each-direction", monoptahPrologue, enc("m1"), enc("m2"), [
      { from: "initiator", plaintext: enc("before rekey i") },
      { from: "responder", plaintext: enc("before rekey r") },
      { rekey: "initiator->responder" },
      { from: "initiator", plaintext: enc("after rekey i 1") },
      { from: "responder", plaintext: enc("r not rekeyed yet") },
      { rekey: "responder->initiator" },
      { from: "responder", plaintext: enc("after rekey r 1") },
      { from: "initiator", plaintext: enc("after rekey i 2") },
      { rekey: "initiator->responder" },
      { rekey: "initiator->responder" },
      { from: "initiator", plaintext: pattern(300, 77) },
      { from: "responder", plaintext: enc("after rekey r 2") },
    ]),
  ];
  return {
    $comment: COMMENT,
    version: 1,
    generator: "ts",
    protocol: PROTOCOL,
    cases,
    negative: [
      negative("wrong-prologue", "wrong_prologue", monoptahPrologue, wrongPrologue, KEYS.responderStatic),
      negative("wrong-responder-static", "wrong_responder_static", monoptahPrologue, monoptahPrologue, KEYS.wrongResponderStatic),
    ],
  };
}
