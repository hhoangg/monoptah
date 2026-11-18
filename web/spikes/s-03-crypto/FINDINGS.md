# S-03 Crypto and key storage

Status: DONE for desktop engines (Chromium 159.0.8154.6, Firefox 159.0, Playwright WebKit 27.2, Linux arm64, headless); iPhone results are PENDING-DEVICE.
These are local Linux desktop engines, not iPhones; numbers are not iPhone numbers.
Evidence: `/tmp/evidence/ws-q/browser/run.log` (111 passed across S-03 and S-08), JSON in `/tmp/evidence/ws-q/browser/s03/`.
Run: from `web/`, `pnpm --filter @monoptah/spikes run browser`.

## What was built

`page/`: a CSP-compatible Vite page (no inline script) served under the exact CSP of `buildCsp` (`window.s03` for Playwright, buttons and a result `<pre>` for a human). `src/crypto-lib.ts` holds the measurements; `browser/crypto.browser.spec.ts` drives them in 3 engines, including a persistent-context restart (close and reopen the browser on the same profile directory).

## Acceptance table (roadmap S-03)

| Item | chromium 159 | firefox 159 | webkit 27.2 (Playwright) | iPhone |
|---|---|---|---|---|
| `@noble` X25519 + ChaCha20-Poly1305, 64 KiB, 300 iterations, encrypt / decrypt MiB/s | 120.9 / 132.9 | 108.4 / 115.7 | 102.5 / 119.4 | PENDING-DEVICE (target at least 1 MiB/s) |
| X25519 shared secret (noble) ms | 1 | 2 | 1 | PENDING-DEVICE |
| WebCrypto X25519 deriveBits | ok | ok | ok | PENDING-DEVICE (architecture.md 3.4 cites WebKit bug 312279 on a read-back of a non-extractable X25519 key; not retested here, the design does not use it) |
| WebCrypto Ed25519 sign/verify | ok | ok | ok | PENDING-DEVICE |
| WebCrypto AES-GCM | ok | ok | ok | PENDING-DEVICE |
| WebCrypto ECDH P-256 | ok | ok | ok | PENDING-DEVICE |
| AES-GCM non-extractable `CryptoKey` in IndexedDB usable after reload | pass | pass | pass | PENDING-DEVICE |
| ... after a browser restart (persistent context closed and reopened) | pass | pass | pass | PENDING-DEVICE (PWA restart, app switch) |
| `extractable` stays false, `exportKey` throws | pass | pass | pass | PENDING-DEVICE |
| Wrapped X25519 key round trip (AES-GCM, AAD = keyId, same public key) | pass | pass | pass | PENDING-DEVICE |
| Private key never stored in clear: IndexedDB scan of all values | not found | not found | not found | n/a |
| ... raw 32 bytes and hex of the private key searched in every file of the browser profile | not found (68 files) | not found (71 files) | not found (10 files) | n/a |
| `storage.persisted()` before / `persist()` / after | false / false / false | false / no answer in 4 s (permission prompt) / false | false / false / false | PENDING-DEVICE |
| `storage.estimate()` quota | 10 GiB | 2.9 GiB | 1000 MiB | PENDING-DEVICE |

All of this ran on `http://localhost` (a secure context), with the exact CSP in Chromium and Firefox; WebKit used the same CSP without `upgrade-insecure-requests` (it upgrades localhost subresources, see S-08 FINDINGS).
Conclusion: the storage design of architecture.md 3.4 holds on every tested desktop engine; AES `CryptoKey` persistence on iPhone is the remaining open item (PENDING-DEVICE). Note the AES key itself is stored by the browser in its profile (not extractable by script, but not secret from someone with the profile and the browser's own key store); the claim "a profile copy does not yield the private key" relies on the browser keeping that material in its own store, which this spike cannot verify beyond the byte search above.

## WebKit storage policy (https://webkit.org/blog/14403/updates-to-storage-policy/, read 2026-11-17)

- Eviction can happen when the quota is exceeded, under system storage pressure, or when a site has not been used for some time; it is per origin in least-recently-used order, where last use is the last user interaction or storage operation. An origin with an active page, or in persistent mode, is excluded.
- Quotas: per origin up to 60% of total disk for browser apps and 15% for other apps; overall up to 80% and 20%; cross-origin frames get 10% of the main frame's origin quota.
- `persist()`: WebKit grants requests "based on heuristics" and does not publish them; the article names opening as a Home Screen Web App as one such signal. Best-effort origins have no persistence guarantee.
- Installed web apps get the same origin and overall quotas as in a browser app.
- The article does NOT state a fixed 7-day cap on script-writable storage, and does not say Home Screen web apps are exempt from eviction. architecture.md 3.4 says `persist()` is requested "so Safari's 7-day eviction of script-writable storage is avoided for installed PWAs"; that sentence is not supported by this page (docs do not state it here; it comes from the older ITP rules). Whether the 7-day cap applies in iOS 26 is PENDING-DEVICE (step 6).

## PENDING-DEVICE: iPhone (installed PWA and Safari tab; iOS 18 and iOS 26)

1. Build and serve: `cd web && pnpm --filter @monoptah/spikes exec node s-08-csp/serve-all.ts` on the Mac. Port 4192 serves the page with the exact CSP (including `upgrade-insecure-requests`), 4193 without it.
2. Get https to the phone (IndexedDB/WebCrypto need a secure context): easiest is a tunnel to port 4192, for example `cloudflared tunnel --url http://localhost:4192` (or `ngrok http 4192`), which gives a real https URL. Alternatively generate a self-signed cert whose subjectAltName is the Mac LAN IP, install and trust it on the phone (Settings, General, About, Certificate Trust Settings), and serve with the https option of `startServer`.
3. In Safari (tab) open the URL. The page shows the user agent and a result box. Tap "Run all". It prints JSON with `bench` (`encryptMiBs`, `decryptMiBs`), `matrix`, `persist`, `wrapRoundTrip`, `keyCreated.publicKeyHex`, `check`. Screenshot or copy the text; record in the table above (target: encrypt and decrypt at least 1 MiB/s on a mid iPhone, else document a mitigation such as smaller chunks or a Worker).
4. Reload test: reload the page and tap "Check stored key". Expected `found`, `keyUsable`, `extractable:false`, `publicKeyMatches:true` and the same `publicKeyHex`.
5. Installed PWA: Share, Add to Home Screen (the page has no manifest; iOS still installs a web clip, or add a manifest for a true standalone test). Open it from the Home Screen: it has separate storage from the Safari tab, so tap "Run all" there. Then swipe the PWA away (restart), reopen, tap "Check stored key". Then switch to another app for a minute and back, tap "Check stored key" again.
6. Eviction re-check: leave the PWA unused for 8 days, open it, tap "Check stored key" (record if `found` is false). Repeat for the Safari tab (not installed).
7. Do this on iOS 18 and iOS 26; record model, OS and Safari versions. Tap "Clear stored key" when done.

| Device | OS | Run all (enc/dec MiB/s) | Reload | PWA restart | App switch | persist() | 8-day |
|---|---|---|---|---|---|---|---|
| iPhone Safari tab | 18.x | PENDING-DEVICE | | | | | |
| iPhone PWA | 18.x | PENDING-DEVICE | | | | | |
| iPhone Safari tab | 26.x | PENDING-DEVICE | | | | | |
| iPhone PWA | 26.x | PENDING-DEVICE | | | | | |

## Proposed doc changes (owners apply)

- architecture.md 3.4: replace the "Safari's 7-day eviction ... avoided for installed PWAs" clause with the wording of the WebKit storage-policy article (LRU eviction by origin, persistent-mode origins excluded, `persist()` granted by undisclosed heuristics such as being a Home Screen Web App); keep the re-registration fallback.
- architecture.md 3.4: note that Firefox shows a permission prompt for `persist()` and that the call can stay pending, so the app must not await it on a critical path (use a timeout; this spike uses 4 s).
- roadmap S-03 / security.md: record that WebCrypto X25519 and Ed25519 are available in Chromium 159, Firefox 159 and WebKit 27.2 on desktop (iOS unknown); the design still correctly avoids relying on it.
