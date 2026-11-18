// An API actor that needs no browser: a software ES256 authenticator (attestation "none", user verification set)
// built on node:crypto, and a plain `fetch` client with a cookie jar. It talks to the real Worker API, so
// registration, re-auth and login go through the same verification as a browser with a passkey. The CDP path
// (lib/actor.ts, lib/webauthn.ts) stays the one for the UI specs; this one is for specs that do not open a page.
import { createHash, createPrivateKey, createSign, generateKeyPairSync, randomBytes, type KeyObject } from "node:crypto";
import type { ApiResponse, CallOptions, Json } from "./actor.ts";
import { SESSION_COOKIE } from "./actor.ts";

const b64u = (bytes: Uint8Array): string => Buffer.from(bytes).toString("base64url");
const sha256 = (data: Uint8Array): Buffer => createHash("sha256").update(data).digest();

// --- the few CBOR shapes an attestation needs ---
function head(major: number, value: number): Buffer {
  if (value < 24) return Buffer.from([(major << 5) | value]);
  if (value < 256) return Buffer.from([(major << 5) | 24, value]);
  const out = Buffer.alloc(3);
  out[0] = (major << 5) | 25;
  out.writeUInt16BE(value, 1);
  return out;
}
const cborInt = (n: number): Buffer => (n >= 0 ? head(0, n) : head(1, -1 - n));
const cborBytes = (b: Uint8Array): Buffer => Buffer.concat([head(2, b.length), b]);
const cborText = (t: string): Buffer => Buffer.concat([head(3, Buffer.byteLength(t)), Buffer.from(t)]);
const cborMap = (entries: [Buffer, Buffer][]): Buffer => Buffer.concat([head(5, entries.length), ...entries.flat()]);

export interface SoftActorOptions {
  origin: string;
  rpId: string;
  /** Sent as `CF-Connecting-IP`, so no two actors share a per-IP rate limit. */
  ip: string;
}

export class SoftActor {
  readonly origin: string;
  readonly ip: string;
  readonly #rpId: string;
  readonly #jar = new Map<string, string>();
  readonly #credentialId = randomBytes(32);
  readonly #key: KeyObject;
  readonly #x: Buffer;
  readonly #y: Buffer;
  #counter = 0;
  #userHandle: string | null = null;

  constructor(options: SoftActorOptions) {
    this.origin = options.origin;
    this.ip = options.ip;
    this.#rpId = options.rpId;
    const { privateKey, publicKey } = generateKeyPairSync("ec", { namedCurve: "P-256" });
    this.#key = createPrivateKey(privateKey.export({ type: "pkcs8", format: "pem" }));
    const jwk = publicKey.export({ format: "jwk" });
    this.#x = Buffer.from(String(jwk.x), "base64url");
    this.#y = Buffer.from(String(jwk.y), "base64url");
  }

  /** API call with the session cookie, Origin, CSRF header and a per-actor IP. */
  async call(method: string, path: string, options: CallOptions = {}): Promise<ApiResponse<Json>> {
    const { body, csrf = true } = options;
    const headers: Record<string, string> = { "Content-Type": "application/json", "CF-Connecting-IP": this.ip, Origin: this.origin };
    if (csrf) headers["X-Monoptah-CSRF"] = "1";
    const cookie = this.cookieHeader();
    if (cookie) headers["Cookie"] = cookie;
    const init: RequestInit = { method, headers };
    if (method !== "GET" && method !== "HEAD") init.body = JSON.stringify(body ?? {});
    const res = await fetch(`${this.origin}${path}`, init);
    for (const line of res.headers.getSetCookie()) {
      const pair = line.split(";")[0] ?? "";
      const eq = pair.indexOf("=");
      if (eq <= 0) continue;
      const name = pair.slice(0, eq);
      const value = pair.slice(eq + 1);
      if (value === "" || /;\s*max-age=0/i.test(line)) this.#jar.delete(name);
      else this.#jar.set(name, value);
    }
    let json: unknown = null;
    try {
      json = await res.json();
    } catch {
      // not JSON
    }
    return { status: res.status, json: json as Json, retryAfter: res.headers.get("Retry-After") };
  }

  /** `name=value` pairs for a `Cookie` header (never log it). */
  cookieHeader(): string {
    return [...this.#jar].map(([name, value]) => `${name}=${value}`).join("; ");
  }

  /** The session cookie as {name, value}, or undefined when there is none. */
  sessionCookie(): { name: string; value: string } | undefined {
    const value = this.#jar.get(SESSION_COOKIE);
    return value === undefined ? undefined : { name: SESSION_COOKIE, value };
  }

  #clientData(type: string, challenge: string): Buffer {
    return Buffer.from(JSON.stringify({ type, challenge, origin: this.origin, crossOrigin: false }));
  }

  #authData(withCredential: boolean): Buffer {
    this.#counter += 1;
    const counter = Buffer.alloc(4);
    counter.writeUInt32BE(this.#counter);
    // UP | UV, plus AT when the credential data follows.
    const flags = Buffer.from([withCredential ? 0x45 : 0x05]);
    const parts = [sha256(Buffer.from(this.#rpId)), flags, counter];
    if (withCredential) {
      const cose = cborMap([
        [cborInt(1), cborInt(2)],
        [cborInt(3), cborInt(-7)],
        [cborInt(-1), cborInt(1)],
        [cborInt(-2), cborBytes(this.#x)],
        [cborInt(-3), cborBytes(this.#y)],
      ]);
      const idLength = Buffer.alloc(2);
      idLength.writeUInt16BE(this.#credentialId.length);
      parts.push(Buffer.alloc(16), idLength, this.#credentialId, cose);
    }
    return Buffer.concat(parts);
  }

  /** Registers an account with an invite. Returns the verify response (`user`, `recoveryCodes`). */
  async register(invite: string, handle: string): Promise<ApiResponse<Json>> {
    const options = await this.call("POST", "/api/auth/register/options", { body: { invite, handle, displayName: handle } });
    if (options.status !== 200) return options;
    const creation = options.json["options"] as { challenge: string; user: { id: string } };
    this.#userHandle = creation.user.id;
    const clientDataJSON = this.#clientData("webauthn.create", creation.challenge);
    const attestationObject = Buffer.concat([
      head(5, 3),
      cborText("fmt"), cborText("none"),
      cborText("attStmt"), head(5, 0),
      cborText("authData"), cborBytes(this.#authData(true)),
    ]);
    const response = {
      id: b64u(this.#credentialId),
      rawId: b64u(this.#credentialId),
      type: "public-key",
      clientExtensionResults: {},
      response: { clientDataJSON: b64u(clientDataJSON), attestationObject: b64u(attestationObject), transports: ["internal"] },
    };
    return this.call("POST", "/api/auth/register/verify", { body: { challengeId: options.json["challengeId"], response } });
  }

  #assertion(challenge: string): unknown {
    const clientDataJSON = this.#clientData("webauthn.get", challenge);
    const authenticatorData = this.#authData(false);
    const signature = createSign("sha256")
      .update(Buffer.concat([authenticatorData, sha256(clientDataJSON)]))
      .sign({ key: this.#key, dsaEncoding: "der" });
    return {
      id: b64u(this.#credentialId),
      rawId: b64u(this.#credentialId),
      type: "public-key",
      clientExtensionResults: {},
      response: {
        clientDataJSON: b64u(clientDataJSON),
        authenticatorData: b64u(authenticatorData),
        signature: b64u(signature),
        ...(this.#userHandle ? { userHandle: this.#userHandle } : {}),
      },
    };
  }

  async #ceremony(kind: "login" | "reauth"): Promise<ApiResponse<Json>> {
    const options = await this.call("POST", `/api/auth/${kind}/options`, { body: {} });
    if (options.status !== 200) return options;
    const challenge = (options.json["options"] as { challenge: string }).challenge;
    return this.call("POST", `/api/auth/${kind}/verify`, { body: { challengeId: options.json["challengeId"], response: this.#assertion(challenge) } });
  }

  /** Discoverable passkey login. */
  login(): Promise<ApiResponse<Json>> {
    return this.#ceremony("login");
  }

  /** Passkey re-auth: starts the 5 minute fresh window. */
  reauth(): Promise<ApiResponse<Json>> {
    return this.#ceremony("reauth");
  }
}
