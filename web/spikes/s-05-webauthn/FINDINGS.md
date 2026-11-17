# S-05 WebAuthn on Workers - findings

Date of research and runs: 2026-11-17. Ticket: S-05 in `web/docs/roadmap.md`. Evidence (logs, bundles) in `/tmp/evidence/ws-q/t4/`, not committed.

## Acceptance table

| Item | Status | Evidence |
|---|---|---|
| `@simplewebauthn/server` 14.1.0 registration and authentication verification run in workerd with compatibility_date 2026-09-01 | PASS | `test/webauthn.workers.test.ts` (workers pool, `wrangler.jsonc`): ES256, RS256 (2048) and Ed25519, register + authenticate. Also run through `wrangler dev --local` (real workerd, no vitest pool) with `/verify` for all three. |
| Flags needed (`nodejs_compat` or not) | PASS: none needed | Three controls under `wrangler dev --local` (the vitest pool turns `nodejs_compat` on by itself, so it cannot prove this; the check is automated by `scripts/check-nocompat.ts`, see Reproduce): (1) `wrangler.jsonc`, date 2026-09-01, no flags: `Buffer`/`process` present, because `nodejs_compat` is default from 2026-08-04; (2) `wrangler.olddate.jsonc`, date 2025-09-01, no flags: `Buffer` and `process` undefined, all three algorithms verify; (3) `wrangler.nonode.jsonc`, date 2026-09-01 plus `no_nodejs_compat` and `no_nodejs_compat_v2`: `Buffer` and `process` undefined, all three verify. |
| Any `node:` module pulled in | PASS: none | `wrangler deploy --dry-run --outdir` bundle of the spike Worker (730.63 KiB, gzip 132.96 KiB, includes the spike authenticator): `grep 'node:'` finds nothing. The library has no `node:` import in its own `esm/` either. |
| WebCrypto algorithms used | recorded | `subtle.digest` (SHA-256), `importKey` (jwk/raw), `verify` with ECDSA P-256/SHA-256 (ES256), RSASSA-PKCS1-v1_5/SHA-256 (RS256), Ed25519 (EdDSA, `crypto.subtle` name `Ed25519`). Bundle counts: digest 7, importKey 2, verify 7 call sites. Attestation formats other than `none` additionally parse X.509 with `@peculiar/x509` (pure JS, in the bundle, not exercised: the app asks for `attestationType: "none"`). |
| Counter handling | PASS, recorded | Stored counter 0 and response counter 0: accepted (authenticators that do not count, e.g. many synced passkeys). After a verify the library returns `newCounter` 1; replaying counter 1 against stored 1 fails with `Response counter value 1 was lower than expected 1`; a lower counter (0 against stored 1) fails with `Response counter value 0 was lower than expected 1`. Rule in `verifyAuthenticationResponse`: `(counter > 0 \|\| stored > 0) && counter <= stored` throws. The app must therefore always store `newCounter`, and must treat the error as a clone/replay signal only when the stored counter is non-zero. |
| CPU ms per verify, local | measured, NOT production | Node 24 `performance.now()`, 200 verifies after warm-up, `verifyAuthenticationResponse` only (assertions pre-built): ES256 mean 0.34-0.36 ms (p95 0.43-0.47), RS256 mean 0.21 ms (p95 0.26), Ed25519 mean 0.21 ms (p95 0.24). In workerd (vitest pool), timer resolution is coarse (readings are whole ms: p95/max read 1, means 0.13-0.23 ms over 30 runs), so those are an upper-bound sanity check, not a measurement. Registration verify with `none` attestation was 0-2 ms in workerd. Raw: `/tmp/evidence/ws-q/t4/node-tests.log`, `s-05-webauthn-workers.log`. |
| CPU ms per verify under the 10 ms free cap, production | PENDING-MAC | Steps below. Free plan limit 10 ms CPU per request, I/O wait not counted (https://developers.cloudflare.com/workers/platform/limits/, read 2026-11-17). Local numbers are two orders of magnitude under the cap, so the "med" risk looks low, but production CPU is not measured. |
| Register and login end to end in a browser | covered by E-01 | Not duplicated here. E-01 drives Chromium with a CDP virtual authenticator against local `wrangler dev`. |
| Hybrid QR: Chrome desktop + iPhone, Safari, Firefox | PENDING-DEVICE | Steps below. |
| Passkey sign-in inside an installed iOS PWA | PENDING-DEVICE | Steps below. Nothing official found (see Sources). |
| Browser versions list for security.md | sourced list below, device verification pending | |

## Reproduce

From `web/spikes`:

- `npx vitest run s-05` (Node)
- `npx vitest run -c s-05-webauthn/vitest.workers.config.ts` (workerd; add `--reporter=verbose` to see the `S05-*` log lines)
- Automated no-flags check (starts `wrangler dev --local` on free ports for the three configs, asserts `/probe` Node globals absent/present as expected and that ES256, RS256 and Ed25519 register and login verify; about 45 s, kept out of `pnpm test`): `node s-05-webauthn/scripts/check-nocompat.ts` from `web/spikes` (exit 0 on 2026-11-17, all three PASS).
- Manual control runs (same thing by hand): `npx wrangler dev --local -c s-05-webauthn/wrangler.nonode.jsonc --port 8795`, then `curl localhost:8795/probe` and `curl 'localhost:8795/verify?alg=RS256&op=auth'`. Use `wrangler.olddate.jsonc` the same way. The first request per algorithm builds the fixture.
- Bundle check: `npx wrangler deploy --dry-run --outdir /tmp/x -c s-05-webauthn/wrangler.jsonc`, then grep `node:` in `/tmp/x/index.js`.

The software authenticator is spike-local (`src/authenticator.ts`, ES256 + RS256 + Ed25519, imports only the CBOR encoder from `web/worker/test/support/cbor.ts`). The product authenticator `web/worker/test/support/authenticator.ts` is ES256 only; RS256 and Ed25519 are only used here.

## PENDING-MAC: production CPU per verify

The spike Worker is `worker/index.ts`, config `wrangler.jsonc` (name `monoptah-spike-s05`, `observability.enabled` true). Nothing is deployed by the agent.

1. On the Mac, in a scratch checkout: `cd web/spikes && npx wrangler login` (free account).
2. `npx wrangler deploy -c s-05-webauthn/wrangler.jsonc`. Note the printed `https://monoptah-spike-s05.<subdomain>.workers.dev` URL.
3. Driver, per algorithm and operation. The first request of each isolate builds the fixture (key generation, RSA is slow) and must be discarded:
   `for alg in ES256 RS256 Ed25519; do for op in auth reg; do for i in $(seq 1 100); do curl -s "https://monoptah-spike-s05.<subdomain>.workers.dev/verify?alg=$alg&op=$op" > /dev/null; done; done; done`
4. Read the CPU time: Cloudflare dashboard, Workers & Pages, `monoptah-spike-s05`, Observability (Workers Logs): each invocation log carries CPU time and wall time (https://developers.cloudflare.com/workers/platform/limits/ says CPU and wall time appear in the invocation log; the Workers Logs page itself does not list the field names, so record the field names you see). Also Metrics, "CPU Time per execution" chart (quantiles via reservoir sampling, https://developers.cloudflare.com/workers/observability/metrics-and-analytics/).
5. Record in the table above: median and p99 CPU ms per (alg, op), the isolate-first request separately, and whether any request reported `exceededCpu` (Metrics, Errors, Invocation Statuses: "Exceeded CPU Time Limits").
6. Clean up: `npx wrangler delete --name monoptah-spike-s05`.

Pass criterion from the ticket: CPU per verify under 10 ms. If the ES256 verify (the algorithm the app will really see) is over, reopen architecture for a free-tier fallback.

## PENDING-DEVICE: hybrid QR and installed iOS PWA

WebAuthn binds a credential to an RP ID that must be a registrable domain suffix of the page origin, over https (localhost is a special case that works only on the same machine). A phone cannot reach `localhost` of a laptop and the passkey would be bound to `localhost`, so these tests need a real https hostname. Use the maintainer's own staging deploy of the full app on its real domain (for example `https://staging.<maintainer domain>` with `RP_ID` set to that domain), or a throwaway Worker on `*.workers.dev` with `RP_ID` set to its hostname. A tunnel hostname (Cloudflare Tunnel on a domain the maintainer owns) also works. Do not use the production `zptah.app` RP ID for experiments.

Hybrid QR (record browser and OS versions for each row, and the result of register and of login):

1. Chrome on desktop (macOS or Windows 11) + iPhone (iOS 17+, passkeys enabled in iCloud Keychain, Bluetooth on both): open the staging app, start register, choose "Use a phone or tablet" (the QR appears), scan with the iPhone camera, approve with Face ID, finish. Sign out, sign in with the same QR route. Expect success.
2. Safari on macOS (13+) + iPhone: same ("Other options", then "iPhone, iPad or Android device" QR). Expect success.
3. Firefox on desktop + iPhone: same. Firefox status is uncertain: Bugzilla 1543674 says hybrid works on Windows and macOS through the OS WebAuthn APIs and is open only for Linux (authenticator-rs); a Mozilla support thread reports it on Windows 11. Record the real result on Windows 11 and on macOS.
4. Chrome desktop + Android phone (optional).

Passkey inside an installed iOS PWA (the app is added to the Home Screen from Safari, iOS 16.4 or later):

1. On an iPhone, open the staging app in Safari, Share, "Add to Home Screen", launch it from the icon (standalone).
2. Register a passkey inside the standalone app (a new account, then "add passkey").
3. Sign out, sign in with the passkey in the standalone app. Then repeat sign-in from Safari with the same account (the passkey is the same iCloud credential).
4. Record: does the passkey sheet appear in standalone mode, does autofill UI work, do iOS version-specific bugs appear (Apple Developer Forums threads describe passkey problems with Home Screen shortcuts on iOS 18.6.2: https://developer.apple.com/forums/thread/815784, user reports, not Apple guidance).

## Browser and OS support (for security.md)

From passkeys.dev device support (https://passkeys.dev/device-support/, read 2026-11-17):

- Hybrid, cross-device client (shows the QR code): Android 9+, ChromeOS 108+, iOS/iPadOS 16+, macOS 13+, Windows 23H2+, Ubuntu with Chrome or Edge (no version given). Cross-device authenticator (the phone side): Android 9+, iOS/iPadOS 16+.
- Synced passkeys: Android 9+, ChromeOS 129+, iOS/iPadOS 16+, macOS 13+. Third-party credential managers: Android 14+, iOS/iPadOS 17+, macOS 14+, Windows 25H2+.
- Autofill UI (conditional get): iOS/iPadOS 16.1+, Safari 16.1+, Chrome 108+, Firefox 122+, Edge 122+ (macOS rows). Conditional create: Safari 18+, Chrome 136+, Firefox 136+.
- Related origin requests: Chrome/Edge 128+, Safari macOS 15+ and iOS 18+, Firefox 152+.
- Google (https://developers.google.com/identity/passkeys/supported-environments, read 2026-11-17): Chrome passkeys on Android 9+, iOS/iPadOS 16+ (Google Password Manager on iOS needs 17+), macOS, Windows, Linux and ChromeOS, with cross-device sign-in on all. The page gives no browser version numbers.
- Firefox hybrid: not confirmed from an official support page. Bugzilla 1543674 (https://bugzilla.mozilla.org/show_bug.cgi?id=1543674, read 2026-11-17) states hybrid works on recent Windows and macOS through OS APIs and is a work in progress for Linux. Treat Firefox hybrid on Linux as unsupported.
- Passkeys in an installed iOS PWA: docs do not state. No Apple or WebKit page found. PENDING-DEVICE.

## Proposed doc changes (for the doc owners; I did not edit docs)

1. `security.md` passkeys section: add "no `nodejs_compat` flag is needed for `@simplewebauthn/server` 14.x; verified with `no_nodejs_compat` and `no_nodejs_compat_v2`" and the supported-algorithm list (ES256, EdDSA, RS256) with the WebCrypto names, and restrict `supportedAlgorithmIDs` explicitly to what the app wants.
2. `security.md` counter rule: store `newCounter` after every login; a stored counter above 0 with a response counter not greater is rejected by the library (replay or cloned authenticator); counter 0 on both sides is normal for synced passkeys and is accepted. Log the rejection in the audit log.
3. `security.md` browser support list: the sourced versions above, with "Firefox hybrid: Windows 11 and macOS only (OS API), not Linux", and "installed iOS PWA passkeys: pending device test".
4. `architecture.md` / `roadmap.md` S-05: local verify cost is about 0.2-0.4 ms (Node) versus the 10 ms cap, production CPU pending S-05 PENDING-MAC. Note the bundle size contribution (about 130 KiB gzip for the library and its X.509 dependencies; the Free plan has no compressed size limit per https://developers.cloudflare.com/workers/platform/limits/, 64 MiB uncompressed).
5. `web/wrangler.jsonc` comment already says `nodejs_compat` is default from 2026-08-04 (confirmed at https://developers.cloudflare.com/workers/configuration/compatibility-flags/); add that `no_nodejs_compat` + `no_nodejs_compat_v2` also work for this code if the maintainer wants to opt out.
6. Test infra note: `@cloudflare/vitest-plugin` forces Node compat in the pool, so a "no flags" claim must be checked with `wrangler dev --local` (`scripts/check-nocompat.ts`), not with vitest.

## Sources (read 2026-11-17)

- https://passkeys.dev/device-support/
- https://developers.google.com/identity/passkeys/supported-environments
- https://bugzilla.mozilla.org/show_bug.cgi?id=1543674
- https://developers.cloudflare.com/workers/platform/limits/
- https://developers.cloudflare.com/workers/observability/logs/workers-logs/
- https://developers.cloudflare.com/workers/observability/metrics-and-analytics/
- https://developers.cloudflare.com/workers/configuration/compatibility-flags/
- https://developer.apple.com/forums/thread/815784 (user reports, via search result)
