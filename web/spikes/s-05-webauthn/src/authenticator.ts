// Spike-local software WebAuthn authenticator for S-05: ES256 (-7), RS256 (-257) and Ed25519 (-8) over WebCrypto only,
// so it runs in Node 22+ and in workerd. It answers `create` with a "none" attestation and `get` with a signed assertion,
// in the JSON shapes of @simplewebauthn. It is modelled on web/worker/test/support/authenticator.ts (ES256 only); the
// CBOR encoder is imported from there, read-only.
import type { AuthenticationResponseJSON, RegistrationResponseJSON } from "@simplewebauthn/server";
import { encodeCbor, type CborValue } from "../../../worker/test/support/cbor.ts";

export type Algorithm = "ES256" | "RS256" | "Ed25519";

const FLAG_UP = 0x01;
const FLAG_UV = 0x04;
const FLAG_AT = 0x40;
const encoder = new TextEncoder();

export function b64u(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/, "");
}

export function fromB64u(text: string): Uint8Array<ArrayBuffer> {
  const padded = text.replaceAll("-", "+").replaceAll("_", "/").padEnd(Math.ceil(text.length / 4) * 4, "=");
  return Uint8Array.from(atob(padded), (c) => c.charCodeAt(0));
}

function concat(...parts: Uint8Array[]): Uint8Array<ArrayBuffer> {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let offset = 0;
  for (const part of parts) {
    out.set(part, offset);
    offset += part.length;
  }
  return out;
}

function uint32(value: number): Uint8Array {
  const out = new Uint8Array(4);
  new DataView(out.buffer).setUint32(0, value);
  return out;
}

async function sha256(data: Uint8Array<ArrayBuffer>): Promise<Uint8Array<ArrayBuffer>> {
  return new Uint8Array(await crypto.subtle.digest("SHA-256", data));
}

/** WebCrypto gives ECDSA signatures as raw r || s; WebAuthn wants ASN.1 DER. */
function rawToDer(raw: Uint8Array): Uint8Array {
  const half = raw.length / 2;
  const integer = (bytes: Uint8Array): number[] => {
    let start = 0;
    while (start < bytes.length - 1 && bytes[start] === 0) start += 1;
    const body = Array.from(bytes.slice(start));
    if ((body[0] ?? 0) & 0x80) body.unshift(0);
    return [0x02, body.length, ...body];
  };
  const body = [...integer(raw.slice(0, half)), ...integer(raw.slice(half))];
  return new Uint8Array([0x30, body.length, ...body]);
}

export interface Ceremony {
  origin: string;
  rpId: string;
  /** Overrides the signature counter (default: previous + 1, create: 0). */
  counter?: number;
}

export interface Options {
  challenge: string;
  user?: { id: string };
}

export class SpikeAuthenticator {
  readonly credentialId: string;
  counter = 0;
  userHandle: string | null = null;

  readonly algorithm: Algorithm;
  private readonly keys: CryptoKeyPair;
  private readonly cose: Map<number | string, CborValue>;

  private constructor(algorithm: Algorithm, keys: CryptoKeyPair, cose: Map<number | string, CborValue>) {
    this.algorithm = algorithm;
    this.keys = keys;
    this.cose = cose;
    this.credentialId = b64u(crypto.getRandomValues(new Uint8Array(32)));
  }

  static async generate(algorithm: Algorithm): Promise<SpikeAuthenticator> {
    if (algorithm === "ES256") {
      const keys = (await crypto.subtle.generateKey({ name: "ECDSA", namedCurve: "P-256" }, true, ["sign", "verify"])) as CryptoKeyPair;
      const raw = new Uint8Array((await crypto.subtle.exportKey("raw", keys.publicKey)) as ArrayBuffer);
      const cose = new Map<number | string, CborValue>([[1, 2], [3, -7], [-1, 1], [-2, raw.slice(1, 33)], [-3, raw.slice(33, 65)]]);
      return new SpikeAuthenticator(algorithm, keys, cose);
    }
    if (algorithm === "RS256") {
      const keys = (await crypto.subtle.generateKey(
        { name: "RSASSA-PKCS1-v1_5", modulusLength: 2048, publicExponent: new Uint8Array([1, 0, 1]), hash: "SHA-256" },
        true,
        ["sign", "verify"],
      )) as CryptoKeyPair;
      const jwk = (await crypto.subtle.exportKey("jwk", keys.publicKey)) as JsonWebKey;
      const cose = new Map<number | string, CborValue>([[1, 3], [3, -257], [-1, fromB64u(jwk.n ?? "")], [-2, fromB64u(jwk.e ?? "")]]);
      return new SpikeAuthenticator(algorithm, keys, cose);
    }
    const keys = (await crypto.subtle.generateKey({ name: "Ed25519" }, true, ["sign", "verify"])) as CryptoKeyPair;
    const raw = new Uint8Array((await crypto.subtle.exportKey("raw", keys.publicKey)) as ArrayBuffer);
    const cose = new Map<number | string, CborValue>([[1, 1], [3, -8], [-1, 6], [-2, raw]]);
    return new SpikeAuthenticator(algorithm, keys, cose);
  }

  private clientData(type: "webauthn.create" | "webauthn.get", challenge: string, origin: string): Uint8Array<ArrayBuffer> {
    return encoder.encode(JSON.stringify({ type, challenge, origin, crossOrigin: false }));
  }

  async create(options: Options, ceremony: Ceremony): Promise<RegistrationResponseJSON> {
    this.userHandle = options.user?.id ?? null;
    this.counter = ceremony.counter ?? 0;
    const clientDataJSON = this.clientData("webauthn.create", options.challenge, ceremony.origin);
    const credentialId = fromB64u(this.credentialId);
    const authData = concat(
      await sha256(encoder.encode(ceremony.rpId)),
      new Uint8Array([FLAG_UP | FLAG_UV | FLAG_AT]),
      uint32(this.counter),
      new Uint8Array(16),
      new Uint8Array([credentialId.length >> 8, credentialId.length & 0xff]),
      credentialId,
      encodeCbor(this.cose),
    );
    const attestationObject = encodeCbor(new Map<number | string, CborValue>([["fmt", "none"], ["attStmt", new Map()], ["authData", authData]]));
    return {
      id: this.credentialId,
      rawId: this.credentialId,
      type: "public-key",
      clientExtensionResults: {},
      response: { clientDataJSON: b64u(clientDataJSON), attestationObject: b64u(attestationObject), transports: ["internal"] },
    };
  }

  async get(options: Options, ceremony: Ceremony): Promise<AuthenticationResponseJSON> {
    this.counter = ceremony.counter ?? this.counter + 1;
    const clientDataJSON = this.clientData("webauthn.get", options.challenge, ceremony.origin);
    const authData = concat(await sha256(encoder.encode(ceremony.rpId)), new Uint8Array([FLAG_UP | FLAG_UV]), uint32(this.counter));
    const signed = concat(authData, await sha256(clientDataJSON));
    let signature: Uint8Array;
    if (this.algorithm === "ES256") {
      signature = rawToDer(new Uint8Array(await crypto.subtle.sign({ name: "ECDSA", hash: "SHA-256" }, this.keys.privateKey, signed)));
    } else if (this.algorithm === "RS256") {
      signature = new Uint8Array(await crypto.subtle.sign({ name: "RSASSA-PKCS1-v1_5" }, this.keys.privateKey, signed));
    } else {
      signature = new Uint8Array(await crypto.subtle.sign({ name: "Ed25519" }, this.keys.privateKey, signed));
    }
    return {
      id: this.credentialId,
      rawId: this.credentialId,
      type: "public-key",
      clientExtensionResults: {},
      response: {
        clientDataJSON: b64u(clientDataJSON),
        authenticatorData: b64u(authData),
        signature: b64u(signature),
        ...(this.userHandle === null ? {} : { userHandle: this.userHandle }),
      },
    };
  }
}
