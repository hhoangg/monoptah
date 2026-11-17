# S-09 Spike exit review

Date: 2026-11-17. Inputs: `s-01` to `s-08` FINDINGS.md and the E-01 harness (`web/e2e`). This note adds no new facts: every claim
below is copied from those files. "Local" means local workerd or desktop browsers on Linux, not production and not a phone.

## Acceptance of S-09 (roadmap)

| Acceptance | Status |
|---|---|
| Findings note per ticket | DONE: one FINDINGS.md per spike S-01 to S-08, plus this review |
| Docs updated (capacity.md, architecture.md, others) | NOT DONE in this branch: `web/docs` is not owned by WS-Q. Handed over as the proposals in "Proposed doc changes" below |
| Open questions that the spike answers are removed from section 8 | NOT DONE in this branch for the same reason. Section 8 currently says "None at this time", so nothing is left to remove; the one decision row that waited on a spike is in the proposals below |

The ticket also says "size the later phases again": see "Re-sizing note". That gives no sizes: the spikes changed no assumption that justifies a new size without production numbers. S-09 is therefore PARTIAL by ownership (docs not updated) and by data (production numbers pending).

## Go / no-go per area

Legend: GO = build as designed; GO-CONDITIONAL = build, with the listed remaining checks that can still reopen it.

| Area | Verdict | Evidence | What remains PENDING |
|---|---|---|---|
| Relay DO and hibernation | GO-CONDITIONAL | S-07 FINDINGS: 1 desktop + 3 devices, 65,555 byte frames, auto-response without waking the object, eviction with open sockets (observed 8 to 12 s, documented 10 s), attachment restore, bucket rebuilt, no timer/alarm/outbound socket. Pure and workerd tests (`test/relay.workers.test.ts`, 5 tests). S-02 dry run: 480 of 480 frames, pings add 0 handler runs | PENDING-MAC: real eviction and duration stop, deploy close code (docs name none, 1001 unconfirmed), production socket ceiling. PENDING-DEVICE: iOS 18 and 26 background drop time |
| Noise IK interop | GO (the snow-side interop crate `s-04-noise/rust` is not in CI yet, maintainer decision) | S-04 FINDINGS: every criterion PASS, TS (`web/app/src/link/noise.ts`, unmodified) vs `snow` 0.10.0 in both roles, byte equality, published cacophony and snow vectors, rekey, wrong prologue, wrong static key, nonce limit. No spec divergence | Handoff to WS-K to fold vectors into `web/protocol/vectors.json` (below) |
| WebAuthn on Workers | GO-CONDITIONAL | S-05 FINDINGS: `@simplewebauthn/server` 14.1.0 verifies ES256, RS256, Ed25519 in workerd; no `nodejs_compat` needed (three controls, automated by `node s-05-webauthn/scripts/check-nocompat.ts`, no `node:` in the bundle); counter rules recorded; local verify 0.2 to 0.4 ms (Node) vs the 10 ms cap | PENDING-MAC: production CPU per verify (the pass criterion is under 10 ms; if ES256 verify is over, architecture is reopened). PENDING-DEVICE: hybrid QR (Chrome, Safari, Firefox) and passkey in an installed iOS PWA |
| Web Push | GO-CONDITIONAL | S-06 FINDINGS: in-house RFC 8291 + 8292 sender, WebCrypto only, matches the RFC 8291 Appendix A vector byte for byte in Node and workerd, no flags, no `node:` import; header rules and 404/410 classification; local cost about 0.25 to 0.65 ms per send | PENDING-MAC: production CPU per send. PENDING-DEVICE: real notification on iOS Home Screen PWA, Android Chrome, desktop Chrome, Firefox, Safari; silent push revocation; permission outside a gesture |
| CSP and Trusted Types with the libraries | GO-CONDITIONAL | S-08 FINDINGS: patched build has zero CSP or TT violations in Chromium 159, Firefox 159, Playwright WebKit 27.2 (96 tests pass with S-03); needs two build patches (Sonner and vaul runtime `<style>`, Radix scroll lock); `style-src-attr 'unsafe-inline'` is not needed (proven with a positive control: in that variant an HTML `style` attribute and `setAttribute("style")` are blocked, so the variant is a real test); the markdown renderer is the in-house AST port per security.md, so only a stand-in is tested here and the real one is covered by WS-W tests; the build patch throws if a library update changes the shape, so it fails loudly; Workbox registration passes the `default` policy; iOS 18 fallback: accept (decision stands) | PENDING-MAC: real Safari 26. PENDING-DEVICE: iOS 18 (no TT, expected fallback) and iOS 26 |
| Browser crypto and key storage | GO-CONDITIONAL | S-03 FINDINGS: noble X25519 + ChaCha20-Poly1305 about 102 to 133 MiB/s on desktop engines; WebCrypto X25519, Ed25519, AES-GCM, P-256 ok; AES-GCM non-extractable `CryptoKey` in IndexedDB survives reload and browser restart in all 3; private key not found in IndexedDB or profile files | PENDING-DEVICE: iPhone throughput (target at least 1 MiB/s), key persistence in installed PWA, restart, app switch, `persist()`, 8-day eviction. architecture.md 3.4 wording on 7-day eviction is not supported by the WebKit article (proposal below) |
| Capacity | OPEN, no go/no-go yet | S-01 and S-02 FINDINGS: nothing measured in production. The headline (about 40 to 60 DAU) is bound by DO duration, not requests, so S-01 item 1 does not change it. S-02 may change the duration factor, which would change the headline. One correction is known without production: the fan-out multiplier (below) | PENDING-MAC: S-01 items 1 to 3 and Hono + D1 CPU, S-02 runs. The capacity verdict waits for the S-02 table |
| E-01 harness (context) | PASS for what can run today | 6 passed, 6 skipped. `scripts/e2e-web.sh` now also builds web/app (`E2E_SKIP_APP_BUILD=1` skips it) and the probes fail on unexpected answers instead of skipping. The CI draft `web-e2e.yml` is not in CI yet (maintainer decision). Skips carry `TODO(E-01:...)`: pairing needs WS-R R-03 and R-04 and WS-D1 R-05 (`web/e2e/tests/pairing.spec.ts`), UI needs WS-S W-02 (`web/e2e/tests/ui.spec.ts`) | Remove the TODO skips when those tickets land |

No area is NO-GO. No result so far reopens architecture. The one thing that could still do so is production CPU per WebAuthn verify or
per push send above 10 ms, which local numbers (two orders of magnitude under the cap for WebAuthn) make unlikely but do not prove.

## Consolidated PENDING-MAC list (run in one session on the Mac)

All steps need the maintainer's free Cloudflare account; nothing is deployed by agents. Order and UTC-day grouping below follows the
constraints in the FINDINGS (daily metrics, one run per UTC day, fresh namespace traffic). Record each result in the FINDINGS section named.

1. S-07 deploy: `s-07-hibernation/FINDINGS.md` section "PENDING-MAC: deploy behaviour..." steps 1 to 3 (build probe page, `wrangler deploy -c wrangler.deploy.jsonc`, smoke `/stats`). Everything from item 2 to item 8 that uses the spike Worker needs it.
2. S-01 item 3 and S-07 step 6, max concurrent sockets in production: `s-01-free-tier/FINDINGS.md` item 3; `local-driver.ts sockets --url wss://... --n 2000`.
3. S-01 item 2, Rate Limiting binding on the free plan: `s-01-free-tier/FINDINGS.md` item 2 (6 curl calls to `/rl`; expect 200 x3 then 429).
4. S-07 step 5, deploy with open sockets and the close code: `s-07-hibernation/FINDINGS.md` step 5.
5. S-07 step 4, real eviction and duration stop (idle hour): `s-07-hibernation/FINDINGS.md` step 4. Record in S-02 FINDINGS.
6. S-01 item 1, auto-response counted as a DO request: `s-01-free-tier/FINDINGS.md` item 1 (10,000 pings on a fresh room, read the requests next UTC day). Use its own UTC day, no other traffic to the namespace.
7. S-02 baseline and runs A (no ping), B (ping 30 s), C (ping 60 s). The replayed stream is SYNTHETIC (no recorded transcript exists; see "Synthetic stream" in `s-02-capacity/FINDINGS.md`), so the 1.3 replacement stays unmeasured until a real trace is replayed with `replay.ts --trace trace.json` (steps to record it are in that section). Commands: `s-02-capacity/FINDINGS.md` "PENDING-MAC: production run and next day numbers" steps 1 to 6, one run per UTC day, then the formula section to recompute capacity.md section 2. Delete the spike afterwards (`wrangler delete --name monoptah-spike-07`, S-07 step 7).
8. S-01 item 4, usage alerts: `s-01-free-tier/FINDINGS.md` item 4 (Dashboard, Notifications, Add; record the available types).
9. S-01 D1 route CPU: `s-01-free-tier/FINDINGS.md` section "Hono + D1 CPU" steps 1 to 4. The Worker is committed (`s-01-free-tier/worker/index.ts`, `wrangler.jsonc`, `schema.sql`); it is a plain fetch handler with one primary key `SELECT`, not Hono, so record the number as "Hono route minus router cost".
10. S-05 production CPU per verify: `s-05-webauthn/FINDINGS.md` "PENDING-MAC: production CPU per verify" steps 1 to 6.
11. S-06 production CPU per send: `s-06-webpush/FINDINGS.md` "PENDING-MAC: production CPU per send" steps 1 to 5.
12. S-08 Safari 26 on macOS: `s-08-csp/FINDINGS.md` "PENDING-MAC (Safari 26 on macOS)" steps 1 to 4 (also decides removal of `style-src-attr`).

## Consolidated PENDING-DEVICE list

Needs an iPhone on iOS 18 and on iOS 26 unless stated. Several items need an https origin (tunnel or the maintainer's staging deploy).

1. S-03 iPhone crypto and storage (Safari tab and installed PWA; throughput, reload, PWA restart, app switch, `persist()`, 8-day eviction): `s-03-crypto/FINDINGS.md` "PENDING-DEVICE: iPhone" steps 1 to 7.
2. S-08 CSP and Trusted Types on iOS 18 and 26 (expect `typeof window.trustedTypes` undefined on iOS 18): `s-08-csp/FINDINGS.md` "PENDING-DEVICE (iOS 18 and iOS 26)" steps 1 to 3.
3. S-07 installed PWA background socket drop time for D = 10 s, 30 s, 60 s, 3 min, 10 min: `s-07-hibernation/FINDINGS.md` "PENDING-DEVICE: iOS 18 and 26 installed PWA background socket drop" steps 1 to 7 (needs the deployed spike, Mac item 1).
4. S-05 hybrid QR (Chrome desktop, Safari macOS, Firefox on Windows 11 and macOS, optional Android) and passkey inside an installed iOS PWA: `s-05-webauthn/FINDINGS.md` "PENDING-DEVICE: hybrid QR and installed iOS PWA". Use a staging RP ID, never `zptah.app`.
5. S-06 real notification delivery on iOS Home Screen PWA, Android Chrome, desktop Chrome, Firefox, Safari, plus collapse, 404/410, silent push on iOS, permission outside a gesture: `s-06-webpush/FINDINGS.md` "Device runbook" steps 1 to 9 (sender runs on the Mac).

## Proposed doc changes (the doc owners apply; WS-Q does not edit web/docs)

"Decision" marks items that need a maintainer decision before the owner applies them. Each line points to its source section.

### capacity.md

1. Section 2: replace the "x 1.3" duration factor and the "120 frames/minute" with the measured `G / 7.5` and `F` once S-02 has production numbers (formula in `s-02-capacity/FINDINGS.md`). Waits for PENDING-MAC item 7.
2. Section 2, "Watch frames" row: add the per-device fan-out. Each device has its own Noise channel, so the desktop sends one frame per device: frames per streaming minute are `F_per_device x devices`. With 120 per device-minute: typical 3,600 becomes 7,200, heavy 14,400 becomes 43,200, light 1,200 stays (still billed 20:1, so typical +180 requests/day). This correction is independent of production numbers. Source: S-02 FINDINGS, "Formula". The frame size model behind byte totals is an assumption (no recorded transcript in the repo).
3. Section 1, KV note ("unverified: ..." for the Rate Limiting binding): replace by the free plan result after PENDING-MAC item 3, and add "works in `wrangler dev --local` since wrangler 4.36" (S-01 item 2; the docs page does not mention plan availability).
4. Section 1, "Concurrent WS per object": keep "not stated"; add "local workerd handles 1000 in one object; the local process stops at about 1015 sockets overall; the 17 per room of architecture.md 4.4 does not bind" and the production result after PENDING-MAC item 2 (S-01 item 3, S-07).
5. Section 1, Hono route CPU sentence ("10 ms CPU is enough for a Hono route with a D1 query"): replace by the measured P99 after PENDING-MAC item 9.
6. Sections 3 and 5: record the ping decision (30 s vs 60 s) with the S-01 item 1 result. Decision rule from S-02: if auto-responses are not counted, keep 30 s; if counted, 60 s saves about 1,440 x sockets requests/day but the 90 s presence window must become 150 s or the ping 45 s. Decision: maintainer, after the production numbers.
7. Section 2 "Pings" row: becomes 0 if S-01 item 1 shows auto-responses are not counted; stays as is (pessimistic) if they are. Section 5 alert row: add the date and "re-checked in the dashboard notification picker" after PENDING-MAC item 8 (the docs list no Workers or DO usage notification for the free plan; the fallback of a daily dashboard check and the synthetic check stays).
8. Headline: recompute "Users per 13,000 GB-s/day" with the measured `G` (light 10, typical 30, heavy 120 streaming minutes, 70/25/5 mix). Unchanged until then.

### architecture.md

1. Section 3.4: replace the clause "so Safari's 7-day eviction of script-writable storage is avoided for installed PWAs" with the wording of the WebKit storage-policy article (https://webkit.org/blog/14403/updates-to-storage-policy/, read 2026-11-17): LRU eviction by origin, origins with an active page or in persistent mode excluded, `persist()` granted by undisclosed heuristics (opening as a Home Screen Web App is named as one). The article states no fixed 7-day cap; whether it applies in iOS 26 is PENDING-DEVICE. Keep the re-registration fallback. (S-03)
2. Section 3.4: Firefox shows a permission prompt for `persist()` and the call can stay pending, so the app must not await it on a critical path (use a timeout; the spike uses 4 s). (S-03)
3. Section 4.5 (close codes): docs name no close code for a forced code update, so mark 1001 "unconfirmed until S-07 PENDING-MAC step 5" and keep the client rule "any unexpected close: reconnect with a jittered backoff" as the contract. Also in the 4.5 table (the 1001 row, "reconnect immediately once"). (S-07)
4. Section 4.4 "Hibernation": local workerd evicts an idle object with open sockets after about 10 s (observed 8 to 12 s), matching the lifecycle docs; frames up to 65,555 bytes relay intact locally. (S-07)
5. Section 4.4 presence: if the ping goes to 60 s, widen the 90 s presence window (see capacity.md item 6). (S-02)
6. Push section: the sender is verified against the RFC 8291 Appendix A vector in Node and workerd, no flags needed; point to `web/spikes/s-06-webpush/src/webpush.ts` as the reference for P-01. Add the response classification table: 404 and 410 delete the subscription, 429 and 5xx retry honoring Retry-After, 400, 401, 403, 413 drop and log; a 403 after VAPID key rotation means the subscription must be re-created (`vapid_key_id`). Cache one signed VAPID JWT per push service origin for up to 12 h (24 h maximum), since Apple asks not to refresh it more than once per hour. (S-06)
7. WebAuthn: local verify cost about 0.2 to 0.4 ms (Node) vs the 10 ms cap, production pending; bundle contribution about 130 KiB gzip (library and X.509 dependencies), no compressed size limit on the Free plan, 64 MiB uncompressed (https://developers.cloudflare.com/workers/platform/limits/). (S-05)

### security.md

1. Section 8.1: remove `style-src-attr 'unsafe-inline'` and its justification sentence ("needed only for `style` attributes written by motion and vaul"); S-08 showed React style props, motion and vaul use CSSOM and all libraries run clean without it in 3 engines. Condition: Safari 26 confirms (PENDING-MAC item 12). Also note that a dev server needs https or no `upgrade-insecure-requests` in WebKit. Decision: maintainer (remove, or keep as stated defense-in-depth slack).
2. Section 8.2: replace "Library compatibility ... is a spike item" with the result: React 19, TanStack Router and Query, motion, Workbox, shiki tokens need nothing; Sonner and vaul need the two build patches. Finalize "accept the fallback" for iOS 18 (no Trusted Types before Safari 26 per https://caniuse.com/trusted-types, read 2026-11-17; protection is strict CSP, AST renderer, shiki tokens, ESLint bans). The iOS 18 device check is still PENDING-DEVICE.
3. Passkeys: no `nodejs_compat` needed for `@simplewebauthn/server` 14.x (verified with `no_nodejs_compat` and `no_nodejs_compat_v2`); supported algorithms ES256, EdDSA, RS256 with their WebCrypto names; restrict `supportedAlgorithmIDs` explicitly to what the app wants. (S-05)
4. Passkey counter rule: store `newCounter` after every login; a stored counter above 0 with a response counter not greater is rejected (replay or cloned authenticator) and is logged to the audit log; 0 on both sides is normal for synced passkeys and is accepted. (S-05)
5. Browser support list: hybrid client Android 9+, ChromeOS 108+, iOS/iPadOS 16+, macOS 13+, Windows 23H2+; autofill UI Safari 16.1+, Chrome 108+, Firefox 122+; conditional create Safari 18+, Chrome 136+, Firefox 136+; related origin requests Chrome/Edge 128+, Safari 15+ macOS and 18+ iOS, Firefox 152+ (passkeys.dev, read 2026-11-17); "Firefox hybrid: Windows 11 and macOS only (OS API), not Linux"; "installed iOS PWA passkeys: pending device test". (S-05)
6. Push: the service worker `push` handler must always call `showNotification` (Safari and iOS revoke the permission otherwise); permission and `subscribe` only from a user gesture; `userVisibleOnly: true`; minimum iOS/iPadOS 16.4 for Home Screen web apps, macOS Safari 16 on macOS 13+. (S-06)
7. Crypto availability: WebCrypto X25519 and Ed25519 are available in Chromium 159, Firefox 159 and WebKit 27.2 on desktop (iOS unknown); the design still correctly avoids relying on it. (S-03)

### roadmap.md

1. Section 8 (Open questions): currently "None at this time". Proposed: no removals needed. Update the decision row "Trusted Types on iOS 18 and older ... finalize after S-08" to "finalized by S-08: accept the fallback; iOS 18 device check pending". Decision: maintainer, if one of the questions below is wanted in section 8 with options and a recommendation: (a) drop `style-src-attr` (recommended, after Safari 26), (b) ping 30 s vs 60 s (rule in capacity.md item 6).
2. S-03 / S-05: record the browser results in the ticket notes (S-03 table; S-05 local cost and flags).
3. S-06: the RFC 8291 section 5 example prints `Content-Length: 145`, the real size is 144 octets; do not use it as an assertion.
4. S-01 to S-08 status lines: PASS locally with the PENDING-MAC and PENDING-DEVICE lists above.

### features.md

No change proposed by any spike.

## Proposed code and config changes for other workstreams

| Owner | Change | Source | Decision |
|---|---|---|---|
| web/worker (`src/csp.ts` `buildCsp`) | Drop the `style-src-attr` line; keep a test that the policy contains no `unsafe-inline` | S-08 | Maintainer, after Safari 26 |
| WS-S (web/app Vite config) | Add the `staticLibraryCss` plugin and the `react-style-singleton` alias (copy from `s-08-csp/app/vite.config.ts` and `app/src/patches/style-singleton.ts`); add a build test that fails if a built asset still contains `createElement("style")`; pin Sonner 2.0.8 and vaul 1.1.2 and re-run S-08 on upgrades. Patch alternative: `Drawer.Root modal={false}` (loses focus trap and scroll lock). Needs adopted stylesheets (Safari 16.4+) | S-08 | Maintainer if the alternative is wanted |
| web/eslint.config.js | Add bans: `setHTMLUnsafe`, `Document.parseHTMLUnsafe`, `createContextualFragment`, `DOMParser.parseFromString` (HTML), `iframe.srcdoc` and the `srcdoc` JSX attribute, `setAttribute` with literal `"style"`, `"srcdoc"` or an `on*` name, `new Worker`/`importScripts`/`SharedWorker` outside an allowlisted module, `document.createElement("script")` and `script.src/text/textContent`, string-literal `setTimeout`/`setInterval` (covered by `no-implied-eval`) | S-08 | No |
| WS-K (`web/protocol/vectors.json`, `gen-vectors.ts`) | Fold the Noise vectors in as a top-level `noise` key with the schema in `s-04-noise/FINDINGS.md` ("Proposed `noise` section"); remove the `$comment` line saying Noise vectors are not part of the file; teach `frame.rs` and the TS suites to ignore the key until used. Size: about 560 KB growth with two max-size cases | S-04 | Maintainer on size (keep one max-size case or a digest form) |
| WS-R (relay, R-02) | Use the S-07 prototype pattern: `ctx.acceptWebSocket` with tags, `serializeAttachment` (role, device id, joinedAt, bucket), `setWebSocketAutoResponse("ping","pong")`, lazy bucket rebuild, no timer, alarm or outbound socket; overwrite the device id from the attachment; own size check before relaying (close 1009 above 65,555); treat any unexpected close as reconnect with jittered backoff | S-07 | No |
| WS-R (push, P-01) | Port `s-06-webpush/src/webpush.ts`; cache one JWT per audience; `classifyPushResponse` and `retryDelaySeconds` (30 s doubling, cap 1 h, never below Retry-After); store `vapid_key_id`; always show a notification | S-06 | No |
| WS-A (WebAuthn, A-03) | Store `newCounter` every login; audit-log the rejection; restrict `supportedAlgorithmIDs`; request `attestationType: "none"` (the X.509 path is not exercised) | S-05 | No |
| web/wrangler.jsonc | Comment already says `nodejs_compat` is default from 2026-08-04; may add that `no_nodejs_compat` plus `no_nodejs_compat_v2` also works for this code | S-05 | Maintainer, only if opting out |
| Client storage code | Do not await `navigator.storage.persist()` on a critical path (4 s timeout) | S-03 | No |
| Dev and tests | Serve WebKit over https or strip `upgrade-insecure-requests` on http://localhost; Playwright `page.evaluate` does not enforce the CSP eval block (use a served page script); `@cloudflare/vitest-plugin` forces Node compat in the pool, so verify "no flags" claims with `wrangler dev --local` | S-08, S-05 | No |
| Controller (package wiring) | `web/spikes` `test` script already runs S-05, S-06 and S-07 workers configs; keep it that way; keep `s-07-hibernation/device/dist` and `.wrangler` ignored | S-05, S-06, S-07 | No |

## Re-sizing note (later phases)

Only where a spike changed an assumption; no sizes are proposed here.

- Capacity model (Phase 0 / capacity.md): the frame count per streaming minute scales with the number of devices (per-device Noise channels), so the old "120 total" understates typical and heavy profiles. Requests stay far under the daily limit because of the 20:1 billing ratio, so the headline, bound by DO duration, is not changed by this. The duration factor is the one number that could still move the headline; it waits for S-02 production data.
- S-04 risk ("high: unaudited crypto implementation") is retired locally: the interop holds in both roles against `snow` and against the published cacophony vectors (only cacophony is independent of `snow`; the `snow.txt` vectors are generated by snow itself, S-04 FINDINGS). WS-K takes the vector fold-in as new work.
- S-05 and S-06 ("med: CPU cap") look low risk locally, but the cap risk stays open until the production CPU numbers (PENDING-MAC items 10 and 11).
- S-06: P-01 starts from a working, tested sender instead of from scratch (`s-06-webpush/src/webpush.ts`).
- S-08: the web/app build gains two patches and a build test (Sonner and vaul); this is new work for the app owner, not a replacement of any library.
- Nothing found requires moving to a paid plan or dropping a library.
