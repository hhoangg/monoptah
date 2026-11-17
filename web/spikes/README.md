# Monoptah web spikes

Package `@monoptah/spikes`: throwaway prototypes and measurements for roadmap tickets S-01 to S-09
(`web/docs/roadmap.md`). Nothing here ships. Each spike directory has a `FINDINGS.md` with the acceptance
table, evidence, the manual steps still open and the doc changes proposed for the doc owners.
The exit review (go / no-go, consolidated manual steps) is `s-09-exit-review/FINDINGS.md`.

## Status legend

- PASS: proven by an automated test or run, locally (local workerd, desktop browsers; not production, not a phone).
- FAIL: tried and did not hold.
- PENDING-MAC: needs the maintainer's own Cloudflare account (deploy, dashboard metrics) or a Mac (Safari). Steps are in the FINDINGS.
- PENDING-DEVICE: needs a real phone or a device. Steps are in the FINDINGS.

## Spikes

| Id | Dir | Status | Rerun |
|---|---|---|---|
| S-01 | `s-01-free-tier` | Docs read; rate limit binding PASS locally; auto-response billing, free plan rate limit, usage alerts, Hono + D1 CPU PENDING-MAC | `node s-01-free-tier/scripts/check-ratelimit.ts` (from `web/spikes`) |
| S-02 | `s-02-capacity` | Replay tool and local dry run PASS; production GB-s and request numbers PENDING-MAC | `pnpm --filter @monoptah/spikes exec vitest run s-02`; `node s-02-capacity/replay.ts [--url ...] [--ping-s 30]` |
| S-03 | `s-03-crypto` | PASS on Chromium, Firefox, WebKit (desktop); iPhone PENDING-DEVICE | `pnpm --filter @monoptah/spikes run browser` |
| S-04 | `s-04-noise` | PASS (TS vs `snow`, both roles, published vectors) | `pnpm --filter @monoptah/spikes exec vitest run s-04`; `cargo test --manifest-path web/spikes/s-04-noise/rust/Cargo.toml` |
| S-05 | `s-05-webauthn` | PASS in workerd, no flags; production CPU PENDING-MAC; hybrid QR and iOS PWA PENDING-DEVICE | `pnpm --filter @monoptah/spikes run test` (includes `vitest run -c s-05-webauthn/vitest.workers.config.ts`) |
| S-06 | `s-06-webpush` | RFC 8291 vector PASS in Node and workerd; production CPU PENDING-MAC; real delivery PENDING-DEVICE | `pnpm --filter @monoptah/spikes run test` (includes the S-06 workers config) |
| S-07 | `s-07-hibernation` | PASS locally (eviction, auto-response, attachments, 65,555 byte frames); deploy close code and production metrics PENDING-MAC; iOS drop time PENDING-DEVICE | `pnpm --filter @monoptah/spikes run test` (includes the S-07 workers config); `node s-07-hibernation/scripts/local-driver.ts sockets\|idle\|pings` |
| S-08 | `s-08-csp` | PASS on Chromium, Firefox, WebKit with two build patches; Safari 26 PENDING-MAC; iOS 18 and 26 PENDING-DEVICE | `pnpm --filter @monoptah/spikes run browser` |
| S-09 | `s-09-exit-review` | Exit review written; docs not updated here (handed over as proposals) | read `s-09-exit-review/FINDINGS.md` |

All commands run from `web/` unless a path says otherwise. Other checks: `pnpm --filter @monoptah/spikes run typecheck`,
`... run lint`. The E-01 harness is a separate package (`web/e2e`, `scripts/e2e-web.sh`).
