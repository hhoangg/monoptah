# S-06 Web Push from Workers - findings

Date of research and runs: 2026-11-17. Ticket: S-06 in `web/docs/roadmap.md`. Evidence (logs, bundles, RFC texts) in `/tmp/evidence/ws-q/t4/`, not committed.

## Acceptance table

| Item | Status | Evidence |
|---|---|---|
| In-house RFC 8291 aes128gcm + RFC 8292 VAPID sender, WebCrypto only | PASS | `src/webpush.ts` (no npm push library, no `node:` import). ECDH P-256 (`deriveBits`), HKDF-SHA-256 (`deriveBits`), AES-128-GCM (`encrypt`), ECDSA P-256/SHA-256 (`sign`), `importKey` raw and jwk. |
| Encryption matches the RFC 8291 vector byte for byte | PASS (Node and workerd) | `test/suite.ts` "reproduces the RFC 8291 Appendix A encrypted message byte for byte": fixed salt `DGv6ra1nlYgDCS1FRnbzlw`, application server key pair (public `BP4z9KsN...jl7A8`, private `yfWPiYE-...oRw`), user agent public `BCVxsr7N...Iiw4`, auth secret `BTBZMqHH6r4Tts7J_aSIgg`, plaintext `V2hlbiBJ...tZWxvbg` ("When I grow up, I want to be a watermelon"). Header (86 octets), ciphertext (`8pfeW0Kb...zQ`, 58 octets) and the section 5 body all match. Values quoted from https://www.rfc-editor.org/rfc/rfc8291#appendix-A (text fetched 2026-11-17). |
| Decrypt round trip | PASS | "decrypts the RFC example as the user agent", and random round trips with padding 0, 7, 500. |
| VAPID JWT verifies and has the right claims | PASS | ES256, header `{"typ":"JWT","alg":"ES256"}`, claims `aud` (origin of the endpoint), `exp` (default now + 12 h, at most 24 h, RFC 8292 section 2), `sub` (`mailto:` or `https:`); signature 64 octets, verified with `subtle.verify` using the public key; tampering fails. `Authorization: vapid t=<jwt>, k=<base64url public key>`. |
| Header rules TTL, Urgency, Topic | PASS | `pushHeaders` in `src/webpush.ts`: TTL required non-negative integer, Urgency one of `very-low`, `low`, `normal`, `high`, Topic 1 to 32 chars of the base64url alphabet (RFC 8030 sections 5.2, 5.3, 5.4). |
| Run in Node AND workerd | PASS | `npx vitest run s-06` (Node 24, 10 + 3 tests), `npx vitest run -c s-06-webpush/vitest.workers.config.ts` (workerd, compatibility_date 2026-09-01, no flags, 10 tests). Also the Worker `worker/index.ts` under `wrangler dev --local` with `no_nodejs_compat` + `no_nodejs_compat_v2`: `/send-cost` returns a 172 byte body; the bundle (9.39 KiB, gzip 3.15 KiB) has no `node:` import. |
| CPU / time per send, local | measured, NOT production | One send = ECDH + 3 HKDF + AES-GCM + ES256 sign (key import excluded: the VAPID key is imported once). Node 24 `performance.now()`, 200 sends: mean 0.61-0.66 ms, p95 0.73-0.80 ms, max about 2.4 ms. workerd (vitest pool): timer resolution is whole ms, so only the batch mean is meaningful: 0.245 ms per send over 200 sends (p95/max read 1). Raw: `/tmp/evidence/ws-q/t4/s-06-webpush-workers.log`, `node-tests.log`. |
| CPU ms per send, production | PENDING-MAC | Steps below. Free cap is 10 ms CPU, I/O wait (the `fetch` to the push service) excluded (https://developers.cloudflare.com/workers/platform/limits/). |
| 404/410 handling, TTL, Urgency, Topic collapse documented | PASS (documentation) | Section "Expected push service behaviour" below, plus `classifyPushResponse` and `retryDelaySeconds` with tests. |
| Minimum iOS version, silent push revocation, gesture | documented from Apple/WebKit | iOS and iPadOS 16.4 for Home Screen web apps; Safari does not support invisible push and revokes permission if a notification is not shown immediately; permission and `subscribe` must run in a user gesture. Sources below. Real confirmation PENDING-DEVICE. |
| Real notification received on iOS home-screen PWA, Android Chrome, desktop Chrome, Firefox, Safari | PENDING-DEVICE / PENDING-MAC | Runbook below. |
| Behaviour when permission is requested outside a gesture | documented, device confirmation PENDING-DEVICE | Apple says to request it from a click or tap handler and call the subscribe method immediately from the handler. The test page has `?auto=1` to record what each browser does. |

## Reproduce

From `web/spikes`: `npx vitest run s-06` and `npx vitest run -c s-06-webpush/vitest.workers.config.ts --reporter=verbose` (the `S06-TIMING` lines carry the timings).

## API summary (`src/webpush.ts`)

- `encryptPayload(plaintext, uaPublic, authSecret, { salt?, senderKeyPair?, padding? })` returns the full body: RFC 8188 header (salt 16, record size 4096, key id length 65, sender public key 65) then one record (`plaintext || 0x02 || zero padding`) encrypted with AES-128-GCM. Maximum plaintext with padding: 3993 octets (so the body is at most 4096, the size every push service must accept, RFC 8030 section 7.2 text: push services MUST NOT answer 413 for bodies of 4096 bytes or less).
- `createVapidJwt`, `vapidAuthorization`, `audienceOf`, `importVapidKeys`, `generateVapidKeys`.
- `pushHeaders`, `buildPushRequest` (returns `{ url, init }` for `fetch`).
- `classifyPushResponse(status, retryAfter?, nowMs?)` returns `ok`, `delete-subscription` (404, 410), `retry` with `retryAfterSeconds` (429, 5xx), or `drop` with a reason (400, 401/403, 413, other); `retryDelaySeconds(attempt, retryAfter)`: 30 s doubling, cap 1 h, never below Retry-After.
- Note: the RFC 8291 section 5 example prints `Content-Length: 145`, but its own header (86) plus ciphertext (58) is 144 octets, and the printed body decodes to 144 octets. Our output equals the printed body, so the number in the RFC example is a typo, not a difference in encryption.

## Expected push service behaviour

RFC 8030 (https://www.rfc-editor.org/rfc/rfc8030, text fetched 2026-11-17):

- 201 Created: accepted.
- TTL header is required, a non-negative integer in seconds; a service that does not get it MUST answer 400 (section 5.2). The service may keep the message for less time and says so in a TTL response header. After the TTL the service MUST NOT deliver.
- Urgency: `very-low`, `low`, `normal`, `high`; a message without the header is `normal`; the service must not forward the header to the user agent (section 5.3).
- Topic: at most 32 characters of the URL-safe base64 alphabet, otherwise 400 (section 5.4). A new message with the same topic replaces the queued one (collapse) and replaces its stored TTL and Urgency.
- 404: the subscription does not exist or has expired; a service MUST answer 404 to a send to an expired subscription (section 7.3). 410: the RFC uses it for a failed delivery receipt (section 6.3); in practice the services use 410 for a gone subscription (vendor docs below).
- 413 only for bodies over 4096 bytes. 429 may carry `Retry-After` (section 8.4: "SHOULD also include a Retry-After header").
- 5xx: service trouble; retry with backoff.

Vendors:

- Chrome / FCM and the generic protocol guide (https://web.dev/articles/push-notifications-web-push-protocol): 201 accepted; 429 with Retry-After; 400 bad header; 404 "subscription is expired and can't be used"; 410 "no longer valid and should be removed from application server"; 413 too large. TTL 0 means deliver now or drop. Urgency default `normal`.
- Apple (https://developer.apple.com/documentation/usernotifications/sending-web-push-notifications-in-web-apps-and-browsers): 201 success, 400 bad request, 403 authentication error (reasons `BadJwtToken`, `BadAuthorizationHeader`, `BadVapidPublicKey`, `VapidPkHashMismatch`), 404 invalid path, 405 method, 410 "device token has expired", 413 too large (reason `PayloadTooLarge`, 4 KB), 429 too many requests (`TooManyRequests`), 500, 503. Reasons `BadTtl`, `BadUrgency`, `BadWebPushTopic`, `BadWebPushRequest` for bad requests. The `Topic` is "a maximum of 32 characters from the URL or filename-safe Base64"; the TTL may keep a notification for "30 days or fewer"; do not refresh the VAPID JWT more often than once per hour (so cache one JWT per audience for up to 12 h rather than signing per send; a possible CPU saving); VAPID public key must match the key used at `subscribe`. Allow `*.push.apple.com`.
- Mozilla autopush (https://mozilla-services.github.io/autopush-rs/http.html): 201; 404 "Push subscription is invalid"; 410 "Push subscription is no longer available"; Topic any 32 byte alphanumeric string incl. `_` `-`, replaces an unreceived message; body limit 4096 bytes (bridged GCM/FCM connections base64-encode data and may limit it to 2744 bytes: our payloads stay far below).
- Mapping implemented: delete the subscription on 404 and 410 (also Apple 404 means the stored endpoint path is invalid, so it is unusable as well); retry 429 and 5xx with backoff honoring Retry-After; do not retry 400, 401/403, 413 and do not delete the subscription for them (they mean our request or VAPID setup is wrong; a 403 `VapidPkHashMismatch` after VAPID key rotation means the subscription must be re-created, which the `vapid_key_id` column of `push_subscriptions` is for).

## iOS and Safari (sources: Apple and WebKit)

- Minimum: iOS and iPadOS 16.4 for Home Screen web apps; macOS Safari 16 on macOS 13 or later (https://developer.apple.com/documentation/usernotifications/sending-web-push-notifications-in-web-apps-and-browsers; https://webkit.org/blog/13878/web-push-for-web-apps-on-ios-and-ipados/). No Apple Developer Program membership needed.
- Only a web app added to the Home Screen can request permission on iOS; the request must follow "direct user interaction". Apple: call the push subscription method immediately from the gesture's event handler.
- "Safari doesn't support invisible push notifications. Present push notifications to the user immediately after your service worker receives them. If you don't, Safari revokes the push notification permission for your site." So the service worker must always call `showNotification` in the `push` handler (`userVisibleOnly: true`), including on unexpected payloads. The Monoptah service worker must never swallow a push.
- Push goes through APNs hosts `*.push.apple.com`; VAPID is required for Safari web push (the requirements list a VAPID key pair).

## Device runbook (PENDING-DEVICE / PENDING-MAC)

Real delivery needs the test page on an https origin (a secure context; `localhost` works only on the same machine for desktop browsers) and, for iOS, the page added to the Home Screen. Files: `device/index.html`, `device/app.js`, `device/sw.js`, `device/manifest.webmanifest`, `device/style.css`, `device/icon.svg`; the sender is `send.ts`. The page has no inline script or style and no `innerHTML`; `test/device.test.ts` checks it against the CSP of `web/worker/src/csp.ts` (script-src 'self', style-src 'self', trusted types).

1. On the Mac: `cd web/spikes && node s-06-webpush/send.ts keygen` (writes a throwaway VAPID key pair to the OS temp dir, mode 0600, prints the public key; never commit it).
2. Host `device/` on https. Options: a throwaway Cloudflare Pages/Workers static deploy (`npx wrangler pages deploy s-06-webpush/device --project-name monoptah-spike-s06`, maintainer's account), or a Cloudflare Tunnel to `python3 -m http.server -d s-06-webpush/device 8080`. The page should be served with the production headers if you want to prove the CSP: copy the `Content-Security-Policy` from `buildCsp(host)` into a `_headers` file next to the page for the throwaway deploy only.
3. On each target open the page, paste the VAPID public key, press "Enable notifications" (this runs in the click handler), accept the prompt, copy the subscription JSON into `sub-<target>.json` on the Mac.
4. Send: `node s-06-webpush/send.ts send sub-<target>.json --title Test --body "S-06 <target>" --urgency high --ttl 60`. The tool prints the HTTP status and the classification; 201 means the push service accepted it. Record whether the notification appeared. Targets: iOS Home Screen PWA (iOS 16.4+; add via Share, "Add to Home Screen", open from the icon, then subscribe), Android Chrome, desktop Chrome, Firefox, Safari (macOS 13+).
5. TTL, Urgency, Topic: with the device offline (airplane mode) send with `--ttl 300 --topic demo --body first`, again with `--body second`, then go online: expect only "second" (collapse). With `--ttl 0` while offline expect the message dropped.
6. 404/410: unsubscribe in the browser (or revoke the notification permission), send again, record the status (expected 404 or 410, classified `delete-subscription`).
7. Silent push on iOS: `node s-06-webpush/send.ts send sub-ios.json --silent` (the test service worker deliberately shows nothing for `{"silent":true}`). Send two or three; record whether iOS revokes the permission (the page shows `permission=` in its status line after reload; Settings, Notifications).
8. Permission outside a gesture: open the page with `?auto=1` on iOS Home Screen, Safari and Chrome with a fresh permission state; the page calls `Notification.requestPermission()` on load and logs the outcome. Record what each platform does (prompt, silent denial, exception).
9. Record per target: OS and browser version, HTTP status, delivered yes/no, latency, and for the CPU row below.

## PENDING-MAC: production CPU per send

Worker `worker/index.ts`, config `wrangler.jsonc` (name `monoptah-spike-s06`, `observability.enabled` true). `GET /send-cost` builds one complete push request (ECDH, HKDF, AES-GCM, ES256 sign) for a fake subscription and makes no outgoing request.

1. `cd web/spikes && npx wrangler login`, then `npx wrangler deploy -c s-06-webpush/wrangler.jsonc`.
2. Driver: `for i in $(seq 1 200); do curl -s https://monoptah-spike-s06.<subdomain>.workers.dev/send-cost > /dev/null; done`. Discard the first request of each isolate (it builds the fixture).
3. Read CPU time in the dashboard: Workers & Pages, the Worker, Observability (invocation logs show CPU and wall time per the limits page) and Metrics, "CPU Time per execution" (quantiles; https://developers.cloudflare.com/workers/observability/metrics-and-analytics/).
4. Record median and p99 here. Also check the "Exceeded CPU Time Limits" count. For a fan-out of N devices per event, multiply by N: the free plan allows 10 ms per request, so measure N sends in one request if the product will do that.
5. `npx wrangler delete --name monoptah-spike-s06`.

## Proposed doc changes (for the doc owners; I did not edit docs)

1. `architecture.md` push section: record that the sender is verified against the RFC 8291 Appendix A vector in Node and workerd, no flags needed, and point to `web/spikes/s-06-webpush/src/webpush.ts` as the reference for the P-01 implementation (a straight port or a move).
2. `security.md` push section: always show a notification in the service worker `push` handler (Safari/iOS revokes the permission otherwise); permission and `subscribe` only from a user gesture; `userVisibleOnly: true`; minimum iOS/iPadOS 16.4, macOS Safari 16 on macOS 13+.
3. `architecture.md` / `capacity.md`: classification table (404/410 delete, 429/5xx retry with Retry-After, 400/401/403/413 drop and log), and that a 403 after VAPID key rotation means the subscription must be re-created (`vapid_key_id`).
4. Apple asks not to refresh the VAPID JWT more often than once per hour: cache one signed JWT per push service origin (audience) for up to 12 h (24 h maximum), which also removes the ECDSA sign from most sends.
5. Local cost of one send is about 0.25-0.65 ms (not production); production CPU pending.
6. `roadmap.md` S-06: the RFC 8291 section 5 example says `Content-Length: 145`, the real size is 144 octets; do not use it as an assertion.

## Sources (read 2026-11-17)

- https://www.rfc-editor.org/rfc/rfc8291 (Appendix A vector, section 5)
- https://www.rfc-editor.org/rfc/rfc8188 (aes128gcm header, record size)
- https://www.rfc-editor.org/rfc/rfc8030 (TTL, Urgency, Topic, 404, 413, 429)
- https://www.rfc-editor.org/rfc/rfc8292 (VAPID: aud, exp at most 24 h, sub, 403/401)
- https://developer.apple.com/documentation/usernotifications/sending-web-push-notifications-in-web-apps-and-browsers (read through Apple's JSON documentation endpoint, the HTML page is script-rendered)
- https://webkit.org/blog/13878/web-push-for-web-apps-on-ios-and-ipados/
- https://web.dev/articles/push-notifications-web-push-protocol
- https://mozilla-services.github.io/autopush-rs/http.html
- https://developers.cloudflare.com/workers/platform/limits/
- https://developers.cloudflare.com/workers/observability/metrics-and-analytics/
