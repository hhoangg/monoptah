// The WebAuthn helpers and the virtual authenticator, without any route: the authenticator must produce
// ceremonies the library accepts, and the helpers must refuse the negative cases (origin, RP ID, UV, counter).
import { env } from "cloudflare:workers";
import { describe, expect, it } from "vitest";
import { buildAuthenticationOptions, buildRegistrationOptions, counterDecreased, verifyAuthentication, verifyRegistration } from "../src/modules/passkeys/webauthn.ts";
import { VirtualAuthenticator } from "./support/authenticator.ts";
import { encodeCbor } from "./support/cbor.ts";

const good = { origin: env.RP_ORIGIN, rpId: env.RP_ID };

async function register(key: VirtualAuthenticator, ceremony: Parameters<VirtualAuthenticator["create"]>[1] = good) {
  const options = await buildRegistrationOptions(env, { userId: crypto.randomUUID(), userName: "alice", displayName: "Alice" });
  const response = await key.create(options, ceremony);
  return { options, response, verified: await verifyRegistration(env, response, options.challenge) };
}

describe("cbor encoder", () => {
  it("encodes integers, strings, bytes and maps", () => {
    expect([...encodeCbor(0)]).toEqual([0x00]);
    expect([...encodeCbor(24)]).toEqual([0x18, 24]);
    expect([...encodeCbor(-7)]).toEqual([0x26]);
    expect([...encodeCbor("fmt")]).toEqual([0x63, 0x66, 0x6d, 0x74]);
    expect([...encodeCbor(new Uint8Array([1, 2]))]).toEqual([0x42, 1, 2]);
    expect([...encodeCbor(new Map([[1, 2]]))]).toEqual([0xa1, 0x01, 0x02]);
  });
});

describe("registration options", () => {
  it("require a resident key and user verification, ask for no attestation, and leave the attachment unset", async () => {
    const options = await buildRegistrationOptions(env, { userId: "u1", userName: "alice", displayName: "Alice" });
    expect(options.authenticatorSelection?.residentKey).toBe("required");
    expect(options.authenticatorSelection?.userVerification).toBe("required");
    expect(options.authenticatorSelection?.authenticatorAttachment).toBeUndefined();
    expect(options.attestation).toBe("none");
    expect(options.rp).toEqual({ name: "Monoptah", id: env.RP_ID });
  });
});

describe("registration verification", () => {
  it("accepts a virtual authenticator", async () => {
    const key = await VirtualAuthenticator.generate();
    const { verified } = await register(key);
    expect(verified?.credentialId).toBe(key.credentialId);
    expect(verified?.counter).toBe(0);
    expect(verified?.publicKey.length).toBeGreaterThan(0);
  });

  it("refuses a wrong origin, a wrong RP ID, a missing UV flag and a wrong challenge", async () => {
    expect((await register(await VirtualAuthenticator.generate(), { ...good, origin: "https://evil.example" })).verified).toBeNull();
    expect((await register(await VirtualAuthenticator.generate(), { ...good, rpId: "evil.example" })).verified).toBeNull();
    expect((await register(await VirtualAuthenticator.generate(), { ...good, userVerified: false })).verified).toBeNull();
    const key = await VirtualAuthenticator.generate();
    const options = await buildRegistrationOptions(env, { userId: "u", userName: "bob", displayName: "Bob" });
    expect(await verifyRegistration(env, await key.create(options, good), "another-challenge")).toBeNull();
  });
});

describe("authentication verification", () => {
  async function registered() {
    const key = await VirtualAuthenticator.generate();
    const { verified } = await register(key);
    if (!verified) throw new Error("registration failed");
    return { key, credential: { id: verified.credentialId, publicKey: verified.publicKey, counter: verified.counter } };
  }

  it("accepts an assertion and returns the new counter", async () => {
    const { key, credential } = await registered();
    const options = await buildAuthenticationOptions(env);
    expect(options.allowCredentials).toBeUndefined();
    expect(options.userVerification).toBe("required");
    const result = await verifyAuthentication(env, await key.get(options, good), options.challenge, credential);
    expect(result).toMatchObject({ ok: true, newCounter: 1 });
  });

  it("refuses a wrong origin, a wrong RP ID and a missing UV flag", async () => {
    const { key, credential } = await registered();
    const options = await buildAuthenticationOptions(env);
    for (const bad of [{ ...good, origin: "https://evil.example" }, { ...good, rpId: "evil.example" }, { ...good, userVerified: false }]) {
      expect(await verifyAuthentication(env, await key.get(options, bad), options.challenge, credential)).toEqual({ ok: false, reason: "invalid" });
    }
  });

  it("refuses a counter that does not increase, and accepts 0 staying 0", async () => {
    const { key, credential } = await registered();
    const options = await buildAuthenticationOptions(env);
    expect(await verifyAuthentication(env, await key.get(options, { ...good, counter: 5 }), options.challenge, credential)).toMatchObject({ ok: true, newCounter: 5 });
    const stored = { ...credential, counter: 5 };
    expect(await verifyAuthentication(env, await key.get(options, { ...good, counter: 5 }), options.challenge, stored)).toEqual({ ok: false, reason: "counter" });
    expect(await verifyAuthentication(env, await key.get(options, { ...good, counter: 3 }), options.challenge, stored)).toEqual({ ok: false, reason: "counter" });
    expect(await verifyAuthentication(env, await key.get(options, { ...good, counter: 0 }), options.challenge, credential)).toMatchObject({ ok: true, newCounter: 0 });
    expect(counterDecreased(0, 0)).toBe(false);
    expect(counterDecreased(0, 1)).toBe(false);
    expect(counterDecreased(3, 0)).toBe(true);
  });

  it("refuses a user handle that is not the expected user", async () => {
    const { key, credential } = await registered();
    const options = await buildAuthenticationOptions(env);
    const assertion = await key.get(options, good);
    expect(await verifyAuthentication(env, assertion, options.challenge, credential, "someone-else")).toEqual({ ok: false, reason: "invalid" });
  });
});
