// A software WebAuthn authenticator for tests (and for a `wrangler dev` smoke script): ES256 over WebCrypto only,
// so it runs in workerd and in Node 22+. It answers `create` with a "none" attestation and `get` with a signed
// assertion, in the JSON shapes of @simplewebauthn (`RegistrationResponseJSON`, `AuthenticationResponseJSON`).
//
//   const key = await VirtualAuthenticator.generate();
//   const registration = await key.create(options, { origin: "http://localhost:8787", rpId: "localhost" });
//   const assertion = await key.get(loginOptions, { origin, rpId });
//
// Negative tests override the origin, the RP ID, the counter and the user verification flag per call.
import type { AuthenticationResponseJSON, RegistrationResponseJSON } from "@simplewebauthn/server";
import { encodeCbor, type CborValue } from "./cbor.ts";

const FLAG_UP = 0x01;
const FLAG_UV = 0x04;
const FLAG_BE = 0x08;
const FLAG_BS = 0x10;
const FLAG_AT = 0x40;

const encoder = new TextEncoder();

export function b64u(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/, "");
}

export function fromB64u(text: string): Uint8Array {
  const padded = text.replaceAll("-", "+").replaceAll("_", "/").padEnd(Math.ceil(text.length / 4) * 4, "=");
  return Uint8Array.from(atob(padded), (c) => c.charCodeAt(0));
}

async function sha256(data: Uint8Array): Promise<Uint8Array> {
  return new Uint8Array(await crypto.subtle.digest("SHA-256", data));
}

function concat(...parts: Uint8Array[]): Uint8Array {
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

/** WebCrypto gives ECDSA signatures as raw r || s; WebAuthn wants ASN.1 DER. */
export function rawToDer(raw: Uint8Array): Uint8Array {
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

export interface CeremonyOptions {
  /** The origin written into `clientDataJSON`. Required: tests state it, so a wrong one is deliberate. */
  origin: string;
  /** The RP ID hashed into the authenticator data. Defaults to the one in the options when given. */
  rpId: string;
  /** Sets the signature counter written into the response (and remembers it). Default: previous + 1 (create: 0). */
  counter?: number;
  /** `false` clears the UV flag. Default true. */
  userVerified?: boolean;
  /** Extra flag bits, for example backup eligibility. */
  backupEligible?: boolean;
  backedUp?: boolean;
}

export interface CreationOptionsLike {
  challenge: string;
  rp?: { id?: string };
  user: { id: string };
}

export interface RequestOptionsLike {
  challenge: string;
  rpId?: string;
  allowCredentials?: { id: string }[];
}

export class VirtualAuthenticator {
  /** Base64url credential id. */
  readonly credentialId: string;
  /** Base64url user handle, set by `create` (the WebAuthn user id of the options). */
  userHandle: string | null = null;
  /** The last counter this authenticator reported. */
  counter = 0;

  private readonly keys: CryptoKeyPair;
  private readonly publicRaw: Uint8Array;

  private constructor(keys: CryptoKeyPair, publicRaw: Uint8Array, credentialId: Uint8Array) {
    this.keys = keys;
    this.publicRaw = publicRaw;
    this.credentialId = b64u(credentialId);
  }

  static async generate(): Promise<VirtualAuthenticator> {
    const keys = (await crypto.subtle.generateKey({ name: "ECDSA", namedCurve: "P-256" }, true, ["sign", "verify"])) as CryptoKeyPair;
    const publicRaw = new Uint8Array((await crypto.subtle.exportKey("raw", keys.publicKey)) as ArrayBuffer);
    return new VirtualAuthenticator(keys, publicRaw, crypto.getRandomValues(new Uint8Array(32)));
  }

  private flags(options: CeremonyOptions, extra = 0): number {
    let flags = FLAG_UP | extra;
    if (options.userVerified ?? true) flags |= FLAG_UV;
    if (options.backupEligible || options.backedUp) flags |= FLAG_BE;
    if (options.backedUp) flags |= FLAG_BS;
    return flags;
  }

  private clientData(type: "webauthn.create" | "webauthn.get", challenge: string, origin: string): Uint8Array {
    return encoder.encode(JSON.stringify({ type, challenge, origin, crossOrigin: false }));
  }

  /** Answers `navigator.credentials.create` with a "none" attestation. */
  async create(options: CreationOptionsLike, ceremony: CeremonyOptions): Promise<RegistrationResponseJSON> {
    this.userHandle = options.user.id;
    this.counter = ceremony.counter ?? 0;
    const clientDataJSON = this.clientData("webauthn.create", options.challenge, ceremony.origin);
    const credentialId = fromB64u(this.credentialId);
    const cose = new Map<number | string, CborValue>([
      [1, 2],
      [3, -7],
      [-1, 1],
      [-2, this.publicRaw.slice(1, 33)],
      [-3, this.publicRaw.slice(33, 65)],
    ]);
    const authData = concat(
      await sha256(encoder.encode(ceremony.rpId)),
      new Uint8Array([this.flags(ceremony, FLAG_AT)]),
      uint32(this.counter),
      new Uint8Array(16),
      new Uint8Array([credentialId.length >> 8, credentialId.length & 0xff]),
      credentialId,
      encodeCbor(cose),
    );
    const attestationObject = encodeCbor(new Map<number | string, CborValue>([["fmt", "none"], ["attStmt", new Map()], ["authData", authData]]));
    return {
      id: this.credentialId,
      rawId: this.credentialId,
      type: "public-key",
      authenticatorAttachment: "platform",
      clientExtensionResults: {},
      response: { clientDataJSON: b64u(clientDataJSON), attestationObject: b64u(attestationObject), transports: ["internal"] },
    };
  }

  /** Answers `navigator.credentials.get` with a signed assertion (discoverable: the user handle is included). */
  async get(options: RequestOptionsLike, ceremony: CeremonyOptions): Promise<AuthenticationResponseJSON> {
    const allowed = options.allowCredentials;
    if (allowed && allowed.length > 0 && !allowed.some((c) => c.id === this.credentialId)) throw new Error("No allowed credential on this authenticator");
    this.counter = ceremony.counter ?? this.counter + 1;
    const clientDataJSON = this.clientData("webauthn.get", options.challenge, ceremony.origin);
    const authData = concat(await sha256(encoder.encode(ceremony.rpId)), new Uint8Array([this.flags(ceremony)]), uint32(this.counter));
    const signed = concat(authData, await sha256(clientDataJSON));
    const raw = new Uint8Array(await crypto.subtle.sign({ name: "ECDSA", hash: "SHA-256" }, this.keys.privateKey, signed));
    return {
      id: this.credentialId,
      rawId: this.credentialId,
      type: "public-key",
      authenticatorAttachment: "platform",
      clientExtensionResults: {},
      response: {
        clientDataJSON: b64u(clientDataJSON),
        authenticatorData: b64u(authData),
        signature: b64u(rawToDer(raw)),
        ...(this.userHandle === null ? {} : { userHandle: this.userHandle }),
      },
    };
  }
}
