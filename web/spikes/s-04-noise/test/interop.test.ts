import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { CipherState, MAX_NONCE, generateKeyPair, HandshakeState } from "../../../app/src/link/noise.ts";
import type { NoiseError } from "../../../app/src/link/noise.ts";
import { replay, replayNegative, scriptOfCase, scriptOfPublished } from "../replay.ts";
import type { PublishedVector, Role } from "../replay.ts";
import { buildVectors, MAX_TRANSPORT_PLAINTEXT } from "../ts-vectors.ts";
import { PROTOCOL, unbs } from "../schema.ts";
import type { VectorFile } from "../schema.ts";

const load = <T>(name: string): T => JSON.parse(readFileSync(new URL(`../vectors/${name}`, import.meta.url), "utf8")) as T;
const roles: Role[] = ["initiator", "responder"];

describe("S-04 Noise IK interop, TypeScript side", () => {
  it("replays the snow-generated vectors in both roles (JS initiator vs snow responder and the reverse)", () => {
    const file = load<VectorFile>("monoptah-ik.snow.json");
    expect(file.generator).toBe("snow");
    expect(file.protocol).toBe(PROTOCOL);
    expect(file.cases.length).toBeGreaterThanOrEqual(4);
    for (const c of file.cases) for (const role of roles) expect(() => replay(scriptOfCase(c), role), `${c.name} as ${role}`).not.toThrow();
  });

  it("replays the published IK vectors (cacophony and snow) in both roles", () => {
    const file = load<{ vectors: PublishedVector[] }>("published-ik.json");
    expect(file.vectors.map((v) => v.source).sort()).toEqual(["cacophony", "snow"]);
    for (const v of file.vectors) {
      expect(v.protocol_name).toBe(PROTOCOL);
      for (const role of roles) expect(() => replay(scriptOfPublished(v), role), `${v.source} as ${role}`).not.toThrow();
    }
  });

  it("replays its own vectors in both roles", () => {
    const file = load<VectorFile>("monoptah-ik.ts.json");
    for (const c of file.cases) for (const role of roles) expect(() => replay(scriptOfCase(c), role), `${c.name} as ${role}`).not.toThrow();
  });

  it("the committed TS vectors file is reproducible from the implementation", () => {
    const committed = load<VectorFile>("monoptah-ik.ts.json");
    expect(JSON.parse(JSON.stringify(buildVectors()))).toEqual(committed);
  });

  it("covers the required cases: empty and maximum transport messages, payloads in both handshake messages, rekey", () => {
    const file = load<VectorFile>("monoptah-ik.snow.json");
    const steps = file.cases.flatMap((c) => c.transport);
    const sizes = steps.flatMap((s) => ("rekey" in s ? [] : [unbs(s.plaintext).length]));
    expect(sizes).toContain(0);
    expect(sizes).toContain(MAX_TRANSPORT_PLAINTEXT);
    expect(steps.some((s) => "rekey" in s)).toBe(true);
    expect(file.cases.some((c) => unbs(c.handshake[0].payload).length > 0 && unbs(c.handshake[1].payload).length > 0)).toBe(true);
  });

  it("wrong prologue and wrong responder static key make the responder fail message 1 (snow vectors)", () => {
    const file = load<VectorFile>("monoptah-ik.snow.json");
    expect(file.negative.map((n) => n.kind).sort()).toEqual(["wrong_prologue", "wrong_responder_static"]);
    for (const n of file.negative) expect(() => replayNegative(n), n.name).not.toThrow();
  });

  it("wrong prologue and wrong static key also fail against the TS vectors", () => {
    const file = load<VectorFile>("monoptah-ik.ts.json");
    for (const n of file.negative) expect(() => replayNegative(n), n.name).not.toThrow();
  });

  it("a tampered vector is detected (the replay is not vacuous)", () => {
    const file = load<VectorFile>("monoptah-ik.snow.json");
    const c = file.cases[1] as VectorFile["cases"][number];
    const script = scriptOfCase(c);
    const step = script.transport.find((s) => !("rekey" in s) && s.ciphertext.length > 0);
    if (step === undefined || "rekey" in step) throw new Error("no message step");
    step.ciphertext[0] = (step.ciphertext[0] as number) ^ 1;
    for (const role of roles) expect(() => replay(script, role)).toThrow();
  });

  describe("nonce limit (spec 5.1, 11.3)", () => {
    const key = new Uint8Array(32).fill(9);
    const ad = new Uint8Array(0);

    it("refuses to encrypt and decrypt at nonce 2^64-1 but works at 2^64-2", () => {
      const a = new CipherState(key);
      const b = new CipherState(key);
      a.setNonce(MAX_NONCE - 1n);
      b.setNonce(MAX_NONCE - 1n);
      const ct = a.encryptWithAd(ad, Uint8Array.of(1, 2, 3));
      expect(a.nonce).toBe(MAX_NONCE);
      expect(Array.from(b.decryptWithAd(ad, ct))).toEqual([1, 2, 3]);
      expect(() => a.encryptWithAd(ad, Uint8Array.of(4))).toThrowError(expect.objectContaining({ kind: "nonce_exhausted" }) as NoiseError);
      expect(() => b.decryptWithAd(ad, ct)).toThrowError(expect.objectContaining({ kind: "nonce_exhausted" }) as NoiseError);
    });

    it("applies to the CipherStates that Split returns", () => {
      const r = generateKeyPair(new Uint8Array(32).fill(3));
      const i = new HandshakeState({ initiator: true, prologue: ad, s: generateKeyPair(new Uint8Array(32).fill(1)), rs: r.publicKey });
      const o = new HandshakeState({ initiator: false, prologue: ad, s: r });
      o.readMessage(i.writeMessage());
      i.readMessage(o.writeMessage());
      const { send } = i.split();
      send.setNonce(MAX_NONCE);
      expect(() => send.encryptWithAd(ad, ad)).toThrowError(expect.objectContaining({ kind: "nonce_exhausted" }) as NoiseError);
    });

    it("rekey does not reset the nonce and is allowed at the limit (it uses 2^64-1 internally)", () => {
      const a = new CipherState(key);
      a.setNonce(MAX_NONCE);
      a.rekey();
      expect(a.nonce).toBe(MAX_NONCE);
      expect(() => a.encryptWithAd(ad, ad)).toThrowError(expect.objectContaining({ kind: "nonce_exhausted" }) as NoiseError);
    });
  });
});
