// Shared S-06 test suite: run by the Node test (webpush.test.ts) and the workerd test (webpush.workers.test.ts), so
// both runtimes prove that every WebCrypto step of src/webpush.ts works.
import { describe, expect, it } from "vitest";
import {
  b64uDecode,
  b64uEncode,
  buildPushRequest,
  classifyPushResponse,
  createVapidJwt,
  decryptPayload,
  encryptPayload,
  generateVapidKeys,
  HEADER_LENGTH,
  importEcKeyPair,
  importVapidKeys,
  MAX_PLAINTEXT_BYTES,
  parseRetryAfter,
  pushHeaders,
  retryDelaySeconds,
  audienceOf,
  vapidAuthorization,
} from "../src/webpush.ts";

// RFC 8291 Appendix A and section 5 (https://www.rfc-editor.org/rfc/rfc8291#appendix-A), whitespace removed.
const RFC = {
  plaintext: "V2hlbiBJIGdyb3cgdXAsIEkgd2FudCB0byBiZSBhIHdhdGVybWVsb24", // "When I grow up, I want to be a watermelon"
  asPublic: "BP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8",
  asPrivate: "yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw",
  uaPublic: "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4",
  uaPrivate: "q1dXpw3UpT5VOmu_cf_v6ih07Aems3njxI-JWgLcM94",
  salt: "DGv6ra1nlYgDCS1FRnbzlw",
  authSecret: "BTBZMqHH6r4Tts7J_aSIgg",
  header: "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8",
  ciphertext: "8pfeW0KbunFT06SuDKoJH9Ql87S1QUrdirN6GcG7sFz1y1sqLgVi1VhjVkHsUoEsbI_0LpXMuGvnzQ",
  // Section 5 body (the three lines concatenated).
  body: "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPTpK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN",
};

const decoder = new TextDecoder();
const encoder = new TextEncoder();

export function defineWebPushSuite(runtime: string, now: () => number): void {
  describe(`web push in ${runtime}`, () => {
    it("reproduces the RFC 8291 Appendix A encrypted message byte for byte", async () => {
      const sender = await importEcKeyPair(b64uDecode(RFC.asPublic), b64uDecode(RFC.asPrivate), "ECDH");
      const body = await encryptPayload(b64uDecode(RFC.plaintext), b64uDecode(RFC.uaPublic), b64uDecode(RFC.authSecret), {
        salt: b64uDecode(RFC.salt),
        senderKeyPair: sender,
      });
      expect(b64uEncode(body.slice(0, HEADER_LENGTH))).toBe(RFC.header);
      expect(b64uEncode(body.slice(HEADER_LENGTH))).toBe(RFC.ciphertext);
      expect(b64uEncode(body)).toBe(RFC.body);
      // The RFC prints "Content-Length: 145" in section 5, but its own header (86) + ciphertext (58) is 144 octets.
      expect(body.length).toBe(144);
    });

    it("decrypts the RFC example as the user agent", async () => {
      const ua = await importEcKeyPair(b64uDecode(RFC.uaPublic), b64uDecode(RFC.uaPrivate), "ECDH");
      const plain = await decryptPayload(b64uDecode(RFC.body), ua, b64uDecode(RFC.authSecret));
      expect(decoder.decode(plain)).toBe("When I grow up, I want to be a watermelon");
    });

    it("round-trips random messages with fresh salt and ephemeral key, with and without padding", async () => {
      const ua = (await crypto.subtle.generateKey({ name: "ECDH", namedCurve: "P-256" }, true, ["deriveBits"])) as CryptoKeyPair;
      const uaPublic = new Uint8Array((await crypto.subtle.exportKey("raw", ua.publicKey)) as ArrayBuffer);
      const auth = crypto.getRandomValues(new Uint8Array(16));
      for (const padding of [0, 7, 500]) {
        const message = encoder.encode(JSON.stringify({ title: "Session finished", body: "Tap to open", n: padding }));
        const body = await encryptPayload(message, uaPublic, auth, { padding });
        expect(body.length).toBe(HEADER_LENGTH + message.length + 1 + padding + 16);
        expect(await decryptPayload(body, ua, auth)).toEqual(message);
      }
      const a = await encryptPayload(encoder.encode("x"), uaPublic, auth);
      const b = await encryptPayload(encoder.encode("x"), uaPublic, auth);
      expect(b64uEncode(a)).not.toBe(b64uEncode(b));
    });

    it("rejects an oversize payload, a bad key and a bad auth secret", async () => {
      const ua = await importEcKeyPair(b64uDecode(RFC.uaPublic), b64uDecode(RFC.uaPrivate), "ECDH");
      const uaPublic = new Uint8Array((await crypto.subtle.exportKey("raw", ua.publicKey)) as ArrayBuffer);
      const auth = b64uDecode(RFC.authSecret);
      expect((await encryptPayload(new Uint8Array(MAX_PLAINTEXT_BYTES), uaPublic, auth)).length).toBe(4096);
      await expect(encryptPayload(new Uint8Array(MAX_PLAINTEXT_BYTES + 1), uaPublic, auth)).rejects.toThrow(/too large/);
      await expect(encryptPayload(new Uint8Array(1), uaPublic.slice(0, 64), auth)).rejects.toThrow(/p256dh/);
      const offCurve = new Uint8Array(uaPublic);
      offCurve[64] = (offCurve[64] ?? 0) ^ 1;
      await expect(encryptPayload(new Uint8Array(1), offCurve, auth)).rejects.toThrow();
      await expect(encryptPayload(new Uint8Array(1), uaPublic, new Uint8Array(15))).rejects.toThrow(/auth/);
    });

    it("signs a VAPID JWT that verifies with the public key and carries the right claims", async () => {
      const keys = await generateVapidKeys();
      const pair = await importVapidKeys(keys.publicKey, keys.privateKey);
      const nowSeconds = 1_800_000_000;
      const aud = audienceOf("https://fcm.googleapis.com/fcm/send/abc");
      expect(aud).toBe("https://fcm.googleapis.com");
      const jwt = await createVapidJwt({ audience: aud, subject: "mailto:admin@example.test", nowSeconds }, pair.privateKey);
      const [h, c, s] = jwt.split(".") as [string, string, string];
      expect(JSON.parse(decoder.decode(b64uDecode(h)))).toEqual({ typ: "JWT", alg: "ES256" });
      expect(JSON.parse(decoder.decode(b64uDecode(c)))).toEqual({ aud, exp: nowSeconds + 12 * 3600, sub: "mailto:admin@example.test" });
      const signature = b64uDecode(s);
      expect(signature.length).toBe(64);
      const verifyKey = await crypto.subtle.importKey("raw", b64uDecode(keys.publicKey), { name: "ECDSA", namedCurve: "P-256" }, false, ["verify"]);
      expect(await crypto.subtle.verify({ name: "ECDSA", hash: "SHA-256" }, verifyKey, signature, encoder.encode(`${h}.${c}`))).toBe(true);
      expect(await crypto.subtle.verify({ name: "ECDSA", hash: "SHA-256" }, verifyKey, signature, encoder.encode(`${h}.${c}x`))).toBe(false);
      expect(vapidAuthorization(jwt, keys.publicKey)).toBe(`vapid t=${jwt}, k=${keys.publicKey}`);
    });

    it("enforces the VAPID claim rules", async () => {
      const keys = await generateVapidKeys();
      const pair = await importVapidKeys(keys.publicKey, keys.privateKey);
      const base = { audience: "https://push.example.test", subject: "mailto:a@example.test", nowSeconds: 1_800_000_000 };
      await expect(createVapidJwt({ ...base, expiresAt: base.nowSeconds + 86_400 }, pair.privateKey)).resolves.toBeTypeOf("string");
      await expect(createVapidJwt({ ...base, expiresAt: base.nowSeconds + 86_401 }, pair.privateKey)).rejects.toThrow(/24 hours/);
      await expect(createVapidJwt({ ...base, expiresAt: base.nowSeconds }, pair.privateKey)).rejects.toThrow(/future/);
      await expect(createVapidJwt({ ...base, subject: "admin@example.test" }, pair.privateKey)).rejects.toThrow(/sub/);
      await expect(createVapidJwt({ ...base, audience: "https://push.example.test/path" }, pair.privateKey)).rejects.toThrow(/aud/);
      expect(() => audienceOf("http://push.example.test/x")).toThrow(/https/);
      expect(audienceOf("https://web.push.apple.com:443/abc")).toBe("https://web.push.apple.com");
    });

    it("validates TTL, Urgency and Topic headers", () => {
      expect(pushHeaders({ ttl: 0 })).toEqual({ TTL: "0" });
      expect(pushHeaders({ ttl: 86_400, urgency: "high", topic: "session-42_x-Y" })).toEqual({ TTL: "86400", Urgency: "high", Topic: "session-42_x-Y" });
      for (const urgency of ["very-low", "low", "normal", "high"] as const) expect(pushHeaders({ ttl: 1, urgency }).Urgency).toBe(urgency);
      expect(() => pushHeaders({ ttl: undefined as unknown as number })).toThrow(/TTL/);
      expect(() => pushHeaders({ ttl: -1 })).toThrow(/TTL/);
      expect(() => pushHeaders({ ttl: 1.5 })).toThrow(/TTL/);
      expect(() => pushHeaders({ ttl: 1, urgency: "urgent" as unknown as "high" })).toThrow(/Urgency/);
      expect(pushHeaders({ ttl: 1, topic: "a".repeat(32) }).Topic).toHaveLength(32);
      expect(() => pushHeaders({ ttl: 1, topic: "a".repeat(33) })).toThrow(/Topic/);
      expect(() => pushHeaders({ ttl: 1, topic: "" })).toThrow(/Topic/);
      expect(() => pushHeaders({ ttl: 1, topic: "has space" })).toThrow(/Topic/);
      expect(() => pushHeaders({ ttl: 1, topic: "a+b/c=" })).toThrow(/Topic/);
    });

    it("builds a complete request: encrypted body, VAPID authorization and RFC 8030 headers", async () => {
      const ua = (await crypto.subtle.generateKey({ name: "ECDH", namedCurve: "P-256" }, true, ["deriveBits"])) as CryptoKeyPair;
      const auth = crypto.getRandomValues(new Uint8Array(16));
      const subscription = {
        endpoint: "https://updates.push.services.mozilla.com/wpush/v2/obviously-fake-id",
        keys: { p256dh: b64uEncode(new Uint8Array((await crypto.subtle.exportKey("raw", ua.publicKey)) as ArrayBuffer)), auth: b64uEncode(auth) },
      };
      const vapid = await generateVapidKeys();
      const request = await buildPushRequest({
        subscription,
        payload: encoder.encode("hello"),
        ttl: 60,
        urgency: "normal",
        topic: "demo",
        vapid: { publicKey: vapid.publicKey, keyPair: await importVapidKeys(vapid.publicKey, vapid.privateKey), subject: "mailto:a@example.test" },
      });
      expect(request.url).toBe(subscription.endpoint);
      expect(request.init.method).toBe("POST");
      expect(request.init.headers).toMatchObject({ TTL: "60", Urgency: "normal", Topic: "demo", "Content-Encoding": "aes128gcm", "Content-Type": "application/octet-stream" });
      expect(request.init.headers["Authorization"]).toMatch(/^vapid t=[\w-]+\.[\w-]+\.[\w-]+, k=[\w-]{87}$/);
      expect(decoder.decode(await decryptPayload(request.init.body ?? new Uint8Array(), ua, auth))).toBe("hello");
      const empty = await buildPushRequest({ subscription, ttl: 0, vapid: { publicKey: vapid.publicKey, keyPair: await importVapidKeys(vapid.publicKey, vapid.privateKey), subject: "mailto:a@example.test" } });
      expect(empty.init.body).toBeUndefined();
      expect(empty.init.headers["Content-Encoding"]).toBeUndefined();
    });

    it("classifies push service responses", () => {
      expect(classifyPushResponse(201)).toEqual({ action: "ok" });
      expect(classifyPushResponse(404)).toEqual({ action: "delete-subscription" });
      expect(classifyPushResponse(410)).toEqual({ action: "delete-subscription" });
      expect(classifyPushResponse(429, "120")).toEqual({ action: "retry", retryAfterSeconds: 120 });
      expect(classifyPushResponse(429)).toEqual({ action: "retry", retryAfterSeconds: null });
      expect(classifyPushResponse(503, "Thu, 01 Jan 2026 00:01:00 GMT", Date.parse("2026-01-01T00:00:00Z"))).toEqual({ action: "retry", retryAfterSeconds: 60 });
      expect(classifyPushResponse(500)).toEqual({ action: "retry", retryAfterSeconds: null });
      expect(classifyPushResponse(400)).toEqual({ action: "drop", reason: "bad-request" });
      expect(classifyPushResponse(401)).toEqual({ action: "drop", reason: "auth" });
      expect(classifyPushResponse(403)).toEqual({ action: "drop", reason: "auth" });
      expect(classifyPushResponse(413)).toEqual({ action: "drop", reason: "payload-too-large" });
      expect(classifyPushResponse(418)).toEqual({ action: "drop", reason: "unexpected" });
      expect(parseRetryAfter("garbage")).toBeNull();
      expect(parseRetryAfter("Thu, 01 Jan 2026 00:00:00 GMT", Date.parse("2026-01-02T00:00:00Z"))).toBe(0);
      expect(retryDelaySeconds(1, null)).toBe(30);
      expect(retryDelaySeconds(3, null)).toBe(120);
      expect(retryDelaySeconds(20, null)).toBe(3600);
      expect(retryDelaySeconds(1, 600)).toBe(600);
    });

    it("measures the time of one send (encrypt + sign) in this runtime", async () => {
      const ua = (await crypto.subtle.generateKey({ name: "ECDH", namedCurve: "P-256" }, true, ["deriveBits"])) as CryptoKeyPair;
      const subscription = {
        endpoint: "https://fcm.googleapis.com/fcm/send/obviously-fake-id",
        keys: {
          p256dh: b64uEncode(new Uint8Array((await crypto.subtle.exportKey("raw", ua.publicKey)) as ArrayBuffer)),
          auth: b64uEncode(crypto.getRandomValues(new Uint8Array(16))),
        },
      };
      const vapid = await generateVapidKeys();
      const keyPair = await importVapidKeys(vapid.publicKey, vapid.privateKey);
      const payload = encoder.encode(JSON.stringify({ title: "Monoptah", body: "Your session finished", tag: "session-1" }));
      const send = () => buildPushRequest({ subscription, payload, ttl: 3600, urgency: "high", vapid: { publicKey: vapid.publicKey, keyPair, subject: "mailto:a@example.test" } });
      for (let i = 0; i < 5; i += 1) await send();
      const n = 200;
      const samples: number[] = [];
      for (let i = 0; i < n; i += 1) {
        const t = now();
        await send();
        samples.push(now() - t);
      }
      const t = now();
      for (let i = 0; i < n; i += 1) await send();
      const batchMean = (now() - t) / n;
      samples.sort((a, b) => a - b);
      const mean = samples.reduce((s, v) => s + v, 0) / n;
      console.log(`S06-TIMING ${runtime} ${JSON.stringify({ n, meanMs: mean, p95Ms: samples[Math.floor(n * 0.95)], maxMs: samples[n - 1], batchMeanMs: batchMean })}`);
      expect(samples.length).toBe(n);
    });
  });
}
