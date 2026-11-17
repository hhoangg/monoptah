// One browser context on the worker origin with its own virtual authenticator, an IP and a page-side API helper.
// Requests go through the page (`fetch` in the page), so the browser sets Origin and the real cookie jar holds the
// session. The only header added by hand is the CSRF header, unless a test turns it off.
import type { BrowserContext, Page } from "@playwright/test";
import type { VirtualAuthenticatorHandle } from "./webauthn.ts";

export interface ApiResponse<T = unknown> {
  status: number;
  /** Parsed JSON body, or null when the body is not JSON. */
  json: T;
  retryAfter: string | null;
}

export interface CallOptions {
  body?: unknown;
  /** Send `X-Monoptah-CSRF: 1` (default true). */
  csrf?: boolean;
}

/** Loose view of the JSON bodies the tests read. The Worker is the source of truth; the tests assert on it. */
export type Json = Record<string, any>; // eslint-disable-line @typescript-eslint/no-explicit-any

export const SESSION_COOKIE = "__Host-mc_session";

interface PublicKeyCredentialJSONLike {
  toJSON(): unknown;
}
interface WebAuthnStatics {
  parseCreationOptionsFromJSON?: (options: unknown) => PublicKeyCredentialCreationOptions;
  parseRequestOptionsFromJSON?: (options: unknown) => PublicKeyCredentialRequestOptions;
}

export class Actor {
  readonly context: BrowserContext;
  readonly page: Page;
  readonly authenticator: VirtualAuthenticatorHandle;
  readonly origin: string;
  readonly ip: string;

  constructor(context: BrowserContext, page: Page, authenticator: VirtualAuthenticatorHandle, origin: string, ip: string) {
    this.context = context;
    this.page = page;
    this.authenticator = authenticator;
    this.origin = origin;
    this.ip = ip;
  }

  /** API call from the page with a per-actor `CF-Connecting-IP`. */
  async call(method: string, path: string, options: CallOptions = {}): Promise<ApiResponse<Json>> {
    const { body, csrf = true } = options;
    return this.page.evaluate(
      async ({ method, path, body, csrf, ip }) => {
        const headers: Record<string, string> = { "Content-Type": "application/json", "CF-Connecting-IP": ip };
        if (csrf) headers["X-Monoptah-CSRF"] = "1";
        const init: RequestInit = { method, headers, credentials: "same-origin" };
        if (method !== "GET" && method !== "HEAD") init.body = JSON.stringify(body ?? {});
        const res = await fetch(path, init);
        let json: unknown = null;
        try {
          json = await res.json();
        } catch {
          // not JSON
        }
        return { status: res.status, json: json as Json, retryAfter: res.headers.get("Retry-After") };
      },
      { method, path, body, csrf, ip: this.ip },
    );
  }

  /** `navigator.credentials.create` with server options JSON; returns `credential.toJSON()`. Throws on refusal. */
  async createCredential(optionsJson: unknown): Promise<unknown> {
    return this.page.evaluate(async (optionsJson) => {
      const statics = PublicKeyCredential as unknown as WebAuthnStatics;
      const b64uToBuf = (text: string): ArrayBuffer => {
        const padded = text.replaceAll("-", "+").replaceAll("_", "/").padEnd(Math.ceil(text.length / 4) * 4, "=");
        return Uint8Array.from(atob(padded), (c) => c.charCodeAt(0)).buffer;
      };
      const bufToB64u = (buf: ArrayBuffer): string => {
        let s = "";
        for (const b of new Uint8Array(buf)) s += String.fromCharCode(b);
        return btoa(s).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/, "");
      };
      let publicKey: PublicKeyCredentialCreationOptions;
      if (typeof statics.parseCreationOptionsFromJSON === "function") {
        publicKey = statics.parseCreationOptionsFromJSON(optionsJson);
      } else {
        const o = optionsJson as { challenge: string; user: { id: string; name: string; displayName: string }; excludeCredentials?: { id: string; type: "public-key" }[] } & Record<string, unknown>;
        publicKey = {
          ...(o as unknown as PublicKeyCredentialCreationOptions),
          challenge: b64uToBuf(o.challenge),
          user: { ...o.user, id: b64uToBuf(o.user.id) },
          excludeCredentials: (o.excludeCredentials ?? []).map((c) => ({ ...c, id: b64uToBuf(c.id) })),
        };
      }
      const cred = (await navigator.credentials.create({ publicKey })) as PublicKeyCredential | null;
      if (!cred) throw new Error("navigator.credentials.create returned null");
      if (typeof (cred as unknown as PublicKeyCredentialJSONLike).toJSON === "function") return (cred as unknown as PublicKeyCredentialJSONLike).toJSON();
      const r = cred.response as AuthenticatorAttestationResponse;
      return {
        id: cred.id,
        rawId: bufToB64u(cred.rawId),
        type: cred.type,
        authenticatorAttachment: cred.authenticatorAttachment ?? undefined,
        clientExtensionResults: cred.getClientExtensionResults(),
        response: { clientDataJSON: bufToB64u(r.clientDataJSON), attestationObject: bufToB64u(r.attestationObject), transports: r.getTransports() },
      };
    }, optionsJson);
  }

  /** `navigator.credentials.get` with server options JSON; returns `credential.toJSON()`. Throws on refusal. */
  async getAssertion(optionsJson: unknown): Promise<unknown> {
    return this.page.evaluate(async (optionsJson) => {
      const statics = PublicKeyCredential as unknown as WebAuthnStatics;
      const b64uToBuf = (text: string): ArrayBuffer => {
        const padded = text.replaceAll("-", "+").replaceAll("_", "/").padEnd(Math.ceil(text.length / 4) * 4, "=");
        return Uint8Array.from(atob(padded), (c) => c.charCodeAt(0)).buffer;
      };
      const bufToB64u = (buf: ArrayBuffer): string => {
        let s = "";
        for (const b of new Uint8Array(buf)) s += String.fromCharCode(b);
        return btoa(s).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/, "");
      };
      let publicKey: PublicKeyCredentialRequestOptions;
      if (typeof statics.parseRequestOptionsFromJSON === "function") {
        publicKey = statics.parseRequestOptionsFromJSON(optionsJson);
      } else {
        const o = optionsJson as { challenge: string; allowCredentials?: { id: string; type: "public-key" }[] } & Record<string, unknown>;
        publicKey = {
          ...(o as unknown as PublicKeyCredentialRequestOptions),
          challenge: b64uToBuf(o.challenge),
          allowCredentials: (o.allowCredentials ?? []).map((c) => ({ ...c, id: b64uToBuf(c.id) })),
        };
      }
      const cred = (await navigator.credentials.get({ publicKey })) as PublicKeyCredential | null;
      if (!cred) throw new Error("navigator.credentials.get returned null");
      if (typeof (cred as unknown as PublicKeyCredentialJSONLike).toJSON === "function") return (cred as unknown as PublicKeyCredentialJSONLike).toJSON();
      const r = cred.response as AuthenticatorAssertionResponse;
      return {
        id: cred.id,
        rawId: bufToB64u(cred.rawId),
        type: cred.type,
        authenticatorAttachment: cred.authenticatorAttachment ?? undefined,
        clientExtensionResults: cred.getClientExtensionResults(),
        response: {
          clientDataJSON: bufToB64u(r.clientDataJSON),
          authenticatorData: bufToB64u(r.authenticatorData),
          signature: bufToB64u(r.signature),
          userHandle: r.userHandle ? bufToB64u(r.userHandle) : undefined,
        },
      };
    }, optionsJson);
  }

  /** Whether the page's Chromium has the JSON helpers (otherwise the manual base64url fallback runs). */
  async hasJsonHelpers(): Promise<boolean> {
    return this.page.evaluate(() => {
      const s = PublicKeyCredential as unknown as WebAuthnStatics;
      return typeof s.parseCreationOptionsFromJSON === "function" && typeof s.parseRequestOptionsFromJSON === "function";
    });
  }

  /** Registers an account with an invite. Returns the verify response (`user`, `recoveryCodes`). */
  async register(invite: string, handle: string): Promise<ApiResponse<Json>> {
    const options = await this.call("POST", "/api/auth/register/options", { body: { invite, handle, displayName: handle } });
    if (options.status !== 200) return options;
    const response = await this.createCredential(options.json["options"]);
    return this.call("POST", "/api/auth/register/verify", { body: { challengeId: options.json["challengeId"], response } });
  }

  /** Discoverable passkey login (empty allowCredentials). Returns the verify response; the caller asserts. */
  async login(): Promise<ApiResponse<Json>> {
    const options = await this.call("POST", "/api/auth/login/options", { body: {} });
    if (options.status !== 200) return options;
    const response = await this.getAssertion(options.json["options"]);
    return this.call("POST", "/api/auth/login/verify", { body: { challengeId: options.json["challengeId"], response } });
  }

  /** Passkey re-auth: starts the 5 minute fresh window. */
  async reauth(): Promise<ApiResponse<Json>> {
    const options = await this.call("POST", "/api/auth/reauth/options", { body: {} });
    if (options.status !== 200) return options;
    const response = await this.getAssertion(options.json["options"]);
    return this.call("POST", "/api/auth/reauth/verify", { body: { challengeId: options.json["challengeId"], response } });
  }

  /** The session cookie as the browser stores it, or undefined when there is none. */
  async sessionCookie() {
    const cookies = await this.context.cookies(this.origin);
    return cookies.find((c) => c.name === SESSION_COOKIE);
  }
}
