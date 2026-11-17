// Generates noise_ik_vectors.json from the TypeScript Noise implementation.
//   node --experimental-strip-types crates/monocode-core/src/mobile/web_link/testdata/gen-noise-vectors.ts > crates/monocode-core/src/mobile/web_link/testdata/noise_ik_vectors.json
// Run `pnpm install --frozen-lockfile` in web/ first. All keys are obviously fake test patterns.
import { CipherState, HandshakeState, fingerprint, generateKeyPair, prologue } from "../../../../../../web/mock-desktop/src/noise.ts";

const hex = (b: Uint8Array): string => Buffer.from(b).toString("hex");
const fill = (start: number): Uint8Array => Uint8Array.from({ length: 32 }, (_, i) => start + i);
const utf8 = (s: string): Uint8Array => new TextEncoder().encode(s);

const initiatorStaticPriv = fill(0x01);
const initiatorEphemeralPriv = fill(0x21);
const responderStaticPriv = fill(0x41);
const responderEphemeralPriv = fill(0x61);
const wrongResponderPriv = fill(0x81);
const desktopId = Uint8Array.from({ length: 16 }, (_, i) => 0xa0 + i);
const deviceId = Uint8Array.from({ length: 16 }, (_, i) => 0xb0 + i);
const channel = 7;

const initiatorStatic = generateKeyPair(initiatorStaticPriv);
const responderStatic = generateKeyPair(responderStaticPriv);
const wrongResponder = generateKeyPair(wrongResponderPriv);
const pro = prologue(desktopId, deviceId, channel);

const hs1Payload = utf8('{"name":"Test Browser","ua":"test"}');
const hs2Payload = utf8('{"proto":1}');

const ini = new HandshakeState({ initiator: true, prologue: pro, s: initiatorStatic, rs: responderStatic.publicKey, ephemeralPrivateKey: initiatorEphemeralPriv });
const res = new HandshakeState({ initiator: false, prologue: pro, s: responderStatic, ephemeralPrivateKey: responderEphemeralPriv });
const hs1 = ini.writeMessage(hs1Payload);
res.readMessage(hs1);
const hs2 = res.writeMessage(hs2Payload);
ini.readMessage(hs2);
const iSplit = ini.split();
const rSplit = res.split();

type Step = { plaintext: string; ciphertext: string };
function seq(sender: CipherState, receiver: CipherState, texts: string[]): Step[] {
  return texts.map((t) => {
    const ct = sender.encryptWithAd(new Uint8Array(0), utf8(t));
    const back = receiver.decryptWithAd(new Uint8Array(0), ct);
    if (hex(back) !== hex(utf8(t))) throw new Error("self-check failed");
    return { plaintext: hex(utf8(t)), ciphertext: hex(ct) };
  });
}

const i2rBefore = seq(iSplit.send, rSplit.recv, ["i2r message 0", "i2r message 1", "i2r message 2"]);
const r2iBefore = seq(rSplit.send, iSplit.recv, ["r2i message 0", "r2i message 1", "r2i message 2"]);
iSplit.send.rekey();
rSplit.recv.rekey();
rSplit.send.rekey();
iSplit.recv.rekey();
const i2rAfter = seq(iSplit.send, rSplit.recv, ["i2r after rekey 0", "i2r after rekey 1"]);
const r2iAfter = seq(rSplit.send, iSplit.recv, ["r2i after rekey 0", "r2i after rekey 1"]);

if (hex(ini.handshakeHash) !== hex(res.handshakeHash)) throw new Error("handshake hash mismatch");

// Negative cases: the TS responder must reject them.
function rejects(fn: () => void): boolean {
  try {
    fn();
    return false;
  } catch {
    return true;
  }
}
const wrongPro = prologue(desktopId, deviceId, channel + 1);
const wrongPrologue = {
  prologue: hex(wrongPro),
  hs1: hex(new HandshakeState({ initiator: true, prologue: wrongPro, s: initiatorStatic, rs: responderStatic.publicKey, ephemeralPrivateKey: initiatorEphemeralPriv }).writeMessage(hs1Payload)),
};
const wrongStaticHs1 = new HandshakeState({ initiator: true, prologue: pro, s: initiatorStatic, rs: wrongResponder.publicKey, ephemeralPrivateKey: initiatorEphemeralPriv }).writeMessage(hs1Payload);
const wrongResponderStatic = { responder_static_public: hex(wrongResponder.publicKey), hs1: hex(wrongStaticHs1) };
if (!rejects(() => new HandshakeState({ initiator: false, prologue: pro, s: responderStatic }).readMessage(Buffer.from(wrongPrologue.hs1, "hex")))) throw new Error("wrong prologue accepted");
if (!rejects(() => new HandshakeState({ initiator: false, prologue: pro, s: responderStatic }).readMessage(wrongStaticHs1))) throw new Error("wrong responder static accepted");

const out = {
  $comment:
    "Noise IK cross-implementation vectors generated from web/mock-desktop/src/noise.ts. Regenerate with: node --experimental-strip-types crates/monocode-core/src/mobile/web_link/testdata/gen-noise-vectors.ts > crates/monocode-core/src/mobile/web_link/testdata/noise_ik_vectors.json. Never edit by hand. All keys are fake test patterns. Both directions are covered: the Rust responder and initiator must reproduce these bytes.",
  protocol: "Noise_IK_25519_ChaChaPoly_SHA256",
  inputs: {
    desktop_id: hex(desktopId),
    device_id: hex(deviceId),
    channel,
    prologue: hex(pro),
    initiator_static_private: hex(initiatorStaticPriv),
    initiator_static_public: hex(initiatorStatic.publicKey),
    initiator_ephemeral_private: hex(initiatorEphemeralPriv),
    responder_static_private: hex(responderStaticPriv),
    responder_static_public: hex(responderStatic.publicKey),
    responder_ephemeral_private: hex(responderEphemeralPriv),
    hs1_payload: hex(hs1Payload),
    hs2_payload: hex(hs2Payload),
  },
  outputs: {
    hs1: hex(hs1),
    hs2: hex(hs2),
    handshake_hash: hex(ini.handshakeHash),
    initiator_fingerprint: fingerprint(initiatorStatic.publicKey),
    responder_fingerprint: fingerprint(responderStatic.publicKey),
  },
  transport: {
    i2r_before_rekey: i2rBefore,
    r2i_before_rekey: r2iBefore,
    i2r_after_rekey: i2rAfter,
    r2i_after_rekey: r2iAfter,
  },
  wrong_prologue: wrongPrologue,
  wrong_responder_static: wrongResponderStatic,
};
process.stdout.write(JSON.stringify(out, null, 2) + "\n");
