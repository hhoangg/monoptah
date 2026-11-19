// Throwaway spike Worker for S-06. GET /send-cost builds one complete push request (key generation for a fake
// subscription is done once per isolate, then ECDH + HKDF + AES-GCM encryption + ES256 VAPID signing per request) and
// returns its size. It makes no outgoing request. It exists so the maintainer can read the production CPU time of one
// send in Workers Logs (see FINDINGS.md, PENDING-MAC). Never deployed by the agent.
import { b64uEncode, buildPushRequest, generateVapidKeys, importVapidKeys } from "../src/webpush.ts";

interface Fixture {
  subscription: { endpoint: string; keys: { p256dh: string; auth: string } };
  publicKey: string;
  keyPair: CryptoKeyPair;
}

let fixture: Promise<Fixture> | undefined;

async function makeFixture(): Promise<Fixture> {
  const ua = (await crypto.subtle.generateKey({ name: "ECDH", namedCurve: "P-256" }, true, ["deriveBits"])) as CryptoKeyPair;
  const vapid = await generateVapidKeys();
  return {
    subscription: {
      endpoint: "https://fcm.googleapis.com/fcm/send/obviously-fake-id",
      keys: {
        p256dh: b64uEncode(new Uint8Array((await crypto.subtle.exportKey("raw", ua.publicKey)) as ArrayBuffer)),
        auth: b64uEncode(crypto.getRandomValues(new Uint8Array(16))),
      },
    },
    publicKey: vapid.publicKey,
    keyPair: await importVapidKeys(vapid.publicKey, vapid.privateKey),
  };
}

export default {
  async fetch(): Promise<Response> {
    // The fixture is built on the first request and reused, so later requests measure only the per-send work.
    fixture ??= makeFixture();
    const f = await fixture;
    const request = await buildPushRequest({
      subscription: f.subscription,
      payload: new TextEncoder().encode(JSON.stringify({ title: "Monoptah", body: "Your session finished", tag: "session-1" })),
      ttl: 3600,
      urgency: "high",
      vapid: { publicKey: f.publicKey, keyPair: f.keyPair, subject: "mailto:admin@example.test" },
    });
    return Response.json({ bodyBytes: request.init.body?.length ?? 0, headers: Object.keys(request.init.headers) });
  },
} satisfies ExportedHandler;
