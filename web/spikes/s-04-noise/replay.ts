// Replays a recorded IK transcript with web/app/src/link/noise.ts playing one role; the peer is the recorded bytes.
import { bytesToHex, hexToBytes } from "@noble/hashes/utils.js";
import { generateKeyPair, HandshakeState } from "../../app/src/link/noise.ts";
import { unbs } from "./schema.ts";
import type { NegativeCase, PositiveCase } from "./schema.ts";

export type Role = "initiator" | "responder";

export type ScriptStep = { from: Role; plaintext: Uint8Array; ciphertext: Uint8Array } | { rekey: "initiator->responder" | "responder->initiator" };

export interface Script {
  name: string;
  prologue: Uint8Array;
  initStatic: Uint8Array;
  initEphemeral: Uint8Array;
  respStatic: Uint8Array;
  respEphemeral: Uint8Array;
  handshake: [{ payload: Uint8Array; message: Uint8Array }, { payload: Uint8Array; message: Uint8Array }];
  hash: Uint8Array | null;
  transport: ScriptStep[];
}

export function scriptOfCase(c: PositiveCase): Script {
  return {
    name: c.name,
    prologue: unbs(c.prologue),
    initStatic: unbs(c.initiator.static),
    initEphemeral: unbs(c.initiator.ephemeral),
    respStatic: unbs(c.responder.static),
    respEphemeral: unbs(c.responder.ephemeral),
    handshake: [
      { payload: unbs(c.handshake[0].payload), message: unbs(c.handshake[0].message) },
      { payload: unbs(c.handshake[1].payload), message: unbs(c.handshake[1].message) },
    ],
    hash: unbs(c.handshakeHash.initiator),
    transport: c.transport.map((s) => ("rekey" in s ? s : { from: s.from, plaintext: unbs(s.plaintext), ciphertext: unbs(s.ciphertext) })),
  };
}

export interface PublishedVector {
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

/** Published vectors: messages 0 and 1 are the handshake, then transport alternating initiator, responder. */
export function scriptOfPublished(v: PublishedVector): Script {
  const m = (i: number) => {
    const entry = v.messages[i] as { payload: string; ciphertext: string };
    return { payload: hexToBytes(entry.payload), message: hexToBytes(entry.ciphertext) };
  };
  return {
    name: `published-${v.source}`,
    prologue: hexToBytes(v.init_prologue),
    initStatic: hexToBytes(v.init_static),
    initEphemeral: hexToBytes(v.init_ephemeral),
    respStatic: hexToBytes(v.resp_static),
    respEphemeral: hexToBytes(v.resp_ephemeral),
    handshake: [m(0), m(1)],
    hash: v.handshake_hash === undefined ? null : hexToBytes(v.handshake_hash),
    transport: v.messages.slice(2).map((_, k) => {
      const { payload, message } = m(k + 2);
      return { from: (k + 2) % 2 === 0 ? "initiator" : "responder", plaintext: payload, ciphertext: message } as ScriptStep;
    }),
  };
}

const EMPTY = new Uint8Array(0);

function same(what: string, got: Uint8Array, want: Uint8Array): void {
  if (bytesToHex(got) !== bytesToHex(want)) throw new Error(`${what}: bytes differ (got ${got.length} bytes, want ${want.length} bytes)`);
}

/** Throws on the first difference. Every message this side produces must equal the recorded bytes. */
export function replay(script: Script, role: Role): void {
  const initiator = role === "initiator";
  const s = generateKeyPair(initiator ? script.initStatic : script.respStatic);
  const hs = new HandshakeState({
    initiator,
    prologue: script.prologue,
    s,
    ...(initiator ? { rs: generateKeyPair(script.respStatic).publicKey } : {}),
    ephemeralPrivateKey: initiator ? script.initEphemeral : script.respEphemeral,
  });
  script.handshake.forEach((step, i) => {
    const myTurn = initiator === (i === 0);
    if (myTurn) same(`handshake message ${i + 1}`, hs.writeMessage(step.payload), step.message);
    else same(`handshake payload ${i + 1}`, hs.readMessage(step.message), step.payload);
  });
  if (!hs.isComplete) throw new Error("handshake not complete");
  if (script.hash !== null) same("handshake hash", hs.handshakeHash, script.hash);
  if (!initiator) same("remote static", hs.remoteStatic ?? EMPTY, generateKeyPair(script.initStatic).publicKey);
  const { send, recv } = hs.split();
  script.transport.forEach((step, i) => {
    if ("rekey" in step) {
      const outgoing = (step.rekey === "initiator->responder") === initiator;
      (outgoing ? send : recv).rekey();
      return;
    }
    if ((step.from === "initiator") === initiator) same(`transport step ${i} ciphertext`, send.encryptWithAd(EMPTY, step.plaintext), step.ciphertext);
    else same(`transport step ${i} plaintext`, recv.decryptWithAd(EMPTY, step.ciphertext), step.plaintext);
  });
}

/** The responder must fail reading message 1. Returns normally when it does; throws when it accepts it. */
export function replayNegative(c: NegativeCase): void {
  const init = new HandshakeState({
    initiator: true,
    prologue: unbs(c.initiatorPrologue),
    s: generateKeyPair(unbs(c.initiator.static)),
    rs: unbs(c.initiatorRemoteStatic),
    ephemeralPrivateKey: unbs(c.initiator.ephemeral),
  });
  same(`${c.name}: message 1 reproduced`, init.writeMessage(new TextEncoder().encode("negative case payload")), unbs(c.message1));
  const resp = new HandshakeState({
    initiator: false,
    prologue: unbs(c.responderPrologue),
    s: generateKeyPair(unbs(c.responderStatic)),
    ephemeralPrivateKey: new Uint8Array(32).fill(7),
  });
  let accepted = false;
  try {
    resp.readMessage(unbs(c.message1));
    accepted = true;
  } catch {
    // expected
  }
  if (accepted) throw new Error(`${c.name}: the responder accepted message 1`);
}
