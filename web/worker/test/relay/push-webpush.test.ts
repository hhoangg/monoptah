// P-02: RFC 8291 encryption against the Appendix A vector, VAPID JWT, Retry-After parsing.
import { beforeEach, describe, expect, it } from "vitest";
import { toBase64Url } from "../../src/lib/crypto.ts";
import { fromBase64Url, JWT_LIFETIME_SECONDS, resetVapidCaches, vapidAuthorization, vapidJwt } from "../../src/modules/push/vapid.ts";
import { encryptPayload, MAX_BACKOFF_SECONDS, parseRetryAfter } from "../../src/modules/push/webpush.ts";
import { decryptAs, ecdhPrivateFromParts, generatePair, b64 } from "./support/push-support.ts";

// RFC 8291 section 5 and Appendix A (public test vector, not a secret).
const AS_PUBLIC = "BP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8";
const AS_PRIVATE = "yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw";
const UA_PUBLIC = "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4";
const UA_PRIVATE = "q1dXpw3UpT5VOmu_cf_v6ih07Aems3njxI-JWgLcM94";
const AUTH_SECRET = "BTBZMqHH6r4Tts7J_aSIgg";
const SALT = "DGv6ra1nlYgDCS1FRnbzlw";
const EXPECTED =
  "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27ml" +
  "mlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPT" +
  "pK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN";
const PLAINTEXT = "When I grow up, I want to be a watermelon";

describe("RFC 8291 Appendix A", () => {
  it("produces the exact bytes of the example message", async () => {
    const asPublic = fromBase64Url(AS_PUBLIC);
    const body = await encryptPayload(new TextEncoder().encode(PLAINTEXT), fromBase64Url(UA_PUBLIC), fromBase64Url(AUTH_SECRET), {
      salt: fromBase64Url(SALT),
      ephemeral: { privateKey: await ecdhPrivateFromParts(asPublic, AS_PRIVATE), publicKey: asPublic },
    });
    expect(toBase64Url(body)).toBe(EXPECTED);
    // 86 byte header + 41 byte message + delimiter + 16 byte tag (the RFC example prints Content-Length 145).
    expect(body.length).toBe(144);
  });

  it("the test-side receiver decrypts the example message (the helper follows the RFC too)", async () => {
    const uaPublic = fromBase64Url(UA_PUBLIC);
    const plain = await decryptAs(await ecdhPrivateFromParts(uaPublic, UA_PRIVATE), uaPublic, fromBase64Url(AUTH_SECRET), b64(EXPECTED));
    expect(new TextDecoder().decode(plain)).toBe(PLAINTEXT);
  });

  it("rejects malformed subscription keys", async () => {
    await expect(encryptPayload(new Uint8Array(1), new Uint8Array(64), new Uint8Array(16))).rejects.toThrow();
    await expect(encryptPayload(new Uint8Array(1), fromBase64Url(UA_PUBLIC), new Uint8Array(15))).rejects.toThrow();
  });
});

describe("VAPID JWT", () => {
  beforeEach(resetVapidCaches);

  it("is an ES256 JWT with the right aud, exp and sub that verifies with the public key", async () => {
    const pair = await generatePair();
    const key = { keyId: "k1", publicKey: pair.publicKey, privateKey: pair.privateKey };
    const now = 1_800_000_000;
    const header = await vapidAuthorization(key, "mailto:ops@example.invalid", "https://fcm.googleapis.com/fcm/send/abc?x=1", now);
    const match = /^vapid t=([\w-]+\.[\w-]+\.[\w-]+), k=([\w-]+)$/.exec(header);
    expect(match).not.toBeNull();
    const [, jwt, k] = match as RegExpExecArray;
    expect(k).toBe(pair.publicKey);
    const [h, c, s] = (jwt as string).split(".") as [string, string, string];
    expect(JSON.parse(new TextDecoder().decode(fromBase64Url(h)))).toEqual({ typ: "JWT", alg: "ES256" });
    expect(JSON.parse(new TextDecoder().decode(fromBase64Url(c)))).toEqual({ aud: "https://fcm.googleapis.com", exp: now + JWT_LIFETIME_SECONDS, sub: "mailto:ops@example.invalid" });
    const verifyKey = await crypto.subtle.importKey("raw", pair.publicRaw, { name: "ECDSA", namedCurve: "P-256" }, false, ["verify"]);
    const signature = fromBase64Url(s);
    expect(signature.length).toBe(64);
    expect(await crypto.subtle.verify({ name: "ECDSA", hash: "SHA-256" }, verifyKey, signature, new TextEncoder().encode(`${h}.${c}`))).toBe(true);
    const other = await generatePair();
    const wrongKey = await crypto.subtle.importKey("raw", other.publicRaw, { name: "ECDSA", namedCurve: "P-256" }, false, ["verify"]);
    expect(await crypto.subtle.verify({ name: "ECDSA", hash: "SHA-256" }, wrongKey, signature, new TextEncoder().encode(`${h}.${c}`))).toBe(false);
  });

  it("is cached per audience and key until one hour before expiry", async () => {
    const pair = await generatePair();
    const key = { keyId: "k2", publicKey: pair.publicKey, privateKey: pair.privateKey };
    const now = 1_800_000_000;
    const first = await vapidJwt(key, "mailto:a@example.invalid", "https://fcm.googleapis.com", now);
    expect(await vapidJwt(key, "mailto:a@example.invalid", "https://fcm.googleapis.com", now + JWT_LIFETIME_SECONDS - 3601)).toBe(first);
    expect(await vapidJwt(key, "mailto:a@example.invalid", "https://fcm.googleapis.com", now + JWT_LIFETIME_SECONDS - 3600)).not.toBe(first);
    expect(await vapidJwt(key, "mailto:a@example.invalid", "https://web.push.apple.com", now)).not.toBe(first);
  });

  it("refuses an unusable key", async () => {
    await expect(vapidJwt({ keyId: "bad", publicKey: "REPLACE_WITH_VAPID_PUBLIC_KEY_BASE64URL", privateKey: "x" }, "mailto:a@example.invalid", "https://fcm.googleapis.com")).rejects.toThrow();
  });
});

describe("Retry-After", () => {
  const now = Date.parse("2030-01-01T00:00:00Z");
  it("reads seconds and HTTP dates and caps at one hour", () => {
    expect(parseRetryAfter("120", now)).toBe(120);
    expect(parseRetryAfter("999999", now)).toBe(MAX_BACKOFF_SECONDS);
    expect(parseRetryAfter("Tue, 01 Jan 2030 00:05:00 GMT", now)).toBe(300);
    expect(parseRetryAfter("Tue, 01 Jan 2030 09:00:00 GMT", now)).toBe(MAX_BACKOFF_SECONDS);
    expect(parseRetryAfter("Mon, 31 Dec 2029 00:00:00 GMT", now)).toBe(1);
    expect(parseRetryAfter(null, now)).toBe(60);
    expect(parseRetryAfter("soon", now)).toBe(60);
  });
});
