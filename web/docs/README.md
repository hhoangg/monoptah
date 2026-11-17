# Monoptah web app: overview

Status: specification, researched 2026-11-17. No application code exists yet. This folder is the contract for building it. Paths such as `crates/monocode-core/src/mobile/host.rs:640` refer to the repo at that date.

## 1. What it is

A multi-user, installable web app (PWA) that remotely controls Monoptah Native desktops: list sessions, read live transcripts, send prompts, answer questions, stop agents and receive push notifications. It replaces the iOS (Expo) app as the way to reach a desktop from a phone, and also works on tablets and computers.

| Question | Answer |
|---|---|
| Who is it for | Monoptah Native users who want to drive their desktop agents from a phone, tablet or another computer, without a VPN or an open port |
| Official host | A Worker run by the Monoptah maintainers. Registration is invite-only (each user can invite 3). Users of this host trust its operator not to ship malicious JavaScript (security.md T2) |
| Self-host | `wrangler deploy` on the user's own Cloudflare account. The owner is the operator (architecture.md section 9). Docker is out of scope for now |
| What the server stores | Metadata only: users, passkeys, desktop and device public keys, push subscriptions, invites, audit. Never conversation content |
| Accounts | Passkeys only. 10 one-time recovery codes. No email, no password |
| Trust model | The account is a directory, not the trust root. Every new device must be confirmed on the desktop by name and fingerprint |
| What replaces the iOS app | The web app, installed to the Home Screen. Differences: no haptics, no native transcript view, no in-app QR scanner, push needs Add to Home Screen (features.md section 1.3). When to retire the iOS app and its QUIC path is an open question (roadmap.md) |
| Look and feel | Identical in style to the mobile app, derived into a design system (design-system.md) |

## 2. Architecture

```
 Browser (PWA, installed or tab)            Cloudflare: ONE Worker                      Desktop: Monoptah Native
 +-------------------------------+        +----------------------------------+        +---------------------------------+
 | React 19 SPA (CSR)            | HTTPS  | Static assets binding (app/dist, |        | MobileHost (existing)           |
 |  TanStack Router + Query      |------->|   SPA fallback)                  |        |   handle() / watch()            |
 |  Zustand link store           | /api/* | Hono  /api/*  ---------> D1      |        |        ^                        |
 | Service worker (Workbox +     |        |   (Drizzle: users, passkeys,     |        |        | same two closures      |
 |   push handlers)              |        |    invites, desktops, devices,   |        |   +----+--------------------+   |
 | Noise IK initiator            |  WSS   |    grants, push, audit)          |  WSS   |   | web_link (new)            |   |
 |  (@noble: x25519, chacha20)   |<======>| /relay/*  --> Durable Object     |<=======|   |  snow: Noise IK responder |   |
 | Device static key             | opaque |   "DesktopRoom", 1 per desktop,  | out-   |   |  outbound WebSocket client|   |
 |  (AES-GCM wrapped, IndexedDB) | frames |   hibernation, quotas            | bound  |   +---------------------------+   |
 +---------------+---------------+        |                                  |        |   QUIC listener (phones, iOS    |
                 ^                        | Web Push sender (VAPID,          |        |     app) stays, unchanged       |
                 |                        |   aes128gcm, WebCrypto)          |        +----------------+----------------+
                 | push event             +--------+-------------------------+                         |
        +--------+---------+                       |  ^                                                |
        | Push service     |<----------------------+  +---- POST /api/push/notify (desktop token) -----+
        | (Apple, Google,  |  encrypted payload (RFC 8291)
        |  Mozilla)        |
        +------------------+

 E2E tunnel:  browser  ==== Noise_IK_25519_ChaChaPoly_SHA256 (device = initiator, desktop = responder) ====  desktop
              The relay forwards frames it cannot read. It rewrites only the authenticated deviceId header field.
```

What each party can see:

| Party | Sees | Cannot see |
|---|---|---|
| Browser | Its own device key, session cookie, decrypted content | Other users' data, the desktop private key |
| Desktop | Its own keys and token, device public keys, decrypted requests, project files | Passwords (none exist), other accounts |
| Worker, D1, DO (operator) | Handles, passkey metadata, desktop and device names and fingerprints, grants, push endpoints, who connected to which desktop and when, frame counts and sizes, IPs, project display name and status inside push payloads | Prompts, assistant output, file contents, diffs, project paths, methods called |
| Push service | Opaque encrypted payload, endpoint, timing | Payload text |
| Network attacker | TLS-protected traffic to Cloudflare | Anything inside TLS |

Full details: architecture.md sections 1, 5 and 6; security.md sections 2 and 5.

## 3. Repository layout

```
web/
  app/            Vite + React 19 SPA, service worker (src/sw.ts), tests
  worker/         Hono API, Drizzle schema and migrations, DesktopRoom DO, push sender
  protocol/       Shared TypeScript types and constants: frames, envelopes, close codes, error codes,
                  limits.ts, vectors.json (Noise and frame test vectors shared with Rust)
  docs/           This specification
  wrangler.jsonc  Worker, assets, DO, D1 bindings
  pnpm-workspace.yaml
```

Desktop-side changes (same program, Rust):

| Path | Change |
|---|---|
| `crates/monocode-core/src/mobile/web_link/` | New: outbound WebSocket client, Noise responder (`snow`), per-device sessions, frame codec |
| `crates/monocode-core/src/mobile/host.rs` | `Device` gains `kind`, `permission`, selection; permission, selection and audit hooks in `handle` |
| `crates/monocode-core/src/mobile/identity_store.rs` | Second sealed row `web_identity` (X25519 key, desktop id, desktop token, relay URL) |
| `crates/mobile-link/src/transport.rs` | Post-handshake logic of `serve` generalized over a duplex trait (QUIC stream or Noise channel) |
| `crates/monocode-core/src/mobile/notify.rs` | Web push hook next to the Expo path |
| `crates/monocode-gpui/src/settings/pages/web_app.rs` | New: Settings > Connections > "Web app" section |
| `crates/monocode-gpui/src/shell/mobile_host.rs` | Full-access denial, selection checks, notify hook |
| `crates/monocode-gpui/src/mobile/project.rs` | File deny-list applied to file, files, search, changes |

## 4. Tech stack (decided)

| Area | Choice |
|---|---|
| Frontend | Vite, React 19, CSR only (no SSR, no Next), TanStack Router and Query, Zustand |
| UI | shadcn/ui on Tailwind v4 restyled to the mobile app, motion, `@hugeicons/react`, vaul, shiki (JS regex engine), no raw HTML rendering |
| PWA | `vite-plugin-pwa` (Workbox, injectManifest, prompt updates) plus custom push handlers in the service worker |
| Server | ONE Cloudflare Worker: static assets, Hono `/api/*`, Durable Object relay `/relay/*` |
| Data | D1 with Drizzle; no KV |
| Auth | `@simplewebauthn` (passkeys only), recovery codes (HMAC-SHA-256 with pepper), cookie sessions `__Host-mc_session` |
| E2E crypto | Noise IK: `@noble/curves`, `@noble/ciphers`, `@noble/hashes` on web, `snow` on desktop |
| Push | Web Push with VAPID, WebCrypto sender in the Worker (no Node `web-push`) |
| Dev and deploy | `@cloudflare/vite-plugin`, pnpm workspace, `wrangler deploy` from CI only on the official host |
| Desktop | Existing `MobileHost` reused behind a new WebSocket transport; QUIC kept for phones |

## 5. Glossary

| Term | Meaning |
|---|---|
| Desktop | One Monoptah Native installation that has been paired to an account. Has a static X25519 key and its own room |
| Device | One browser profile or installed PWA with its own static X25519 key. A new browser profile is a new device |
| Account | A user on a host, identified by a `handle`. A directory of desktops and devices, not the trust root |
| Host | A deployment of the Worker (official or self-hosted) |
| Room | The Durable Object `DesktopRoom` of one desktop; all its devices and the desktop socket meet there |
| Relay | The DO plus Worker routes that forward opaque frames between a desktop and its devices |
| Frame | One binary WebSocket message of the outer relay protocol (architecture.md 5.1) |
| Pairing | Linking a desktop to an account (OAuth device authorization style). Not the same as device confirmation |
| User code | Short single-use code (8 characters, e.g. `WXYZ-4829`) in the pairing link and QR |
| Device code | Secret polled by the desktop during pairing, hashed on the server; useless without the verifier |
| Confirmation code | Code shown on both the desktop and the approval page; the user compares them |
| Desktop token | Revocable 256-bit bearer token with scopes `relay push grants`, issued once at pairing, sealed on the desktop |
| Grant | The right of one device to join one desktop's room (`pending`, `confirmed`, `revoked`) |
| Device confirmation | A human approves a new device on the desktop by name and fingerprint |
| Fingerprint | Lowercase hex SHA-256 of a public key; shown as the first 32 hex chars (128 bits) in 8 groups of 4 |
| Read-only / control | Per-device permission on the desktop: read methods only, or also mutating methods |
| Invite code | One-time code from an existing user that allows registration (expiry, 3 per user by default) |
| Invite branch | A user and every descendant they invited (`users.invited_by`); admins can block a whole branch |
| Recovery code | One of 10 one-time codes shown once at registration, stored as HMAC hashes; used to regain access and add a passkey |
| Passkey | WebAuthn discoverable credential; the only login method |
| Noise IK | Handshake pattern where the initiator (device) already knows the responder's (desktop) static key |
| Prologue | Data bound into the Noise handshake: protocol version, desktop id, device id, channel |
| VAPID | Application server identification for Web Push (RFC 8292) |
| Hibernation | Durable Object WebSocket mode in which an idle object is evicted while sockets stay open and cost no duration |
| Watch | Streaming `conversation.watch` request that delivers transcript frames |
| `rid` | Client-chosen request id that correlates responses on one Noise channel |

## 6. Index of documents

| File | Content |
|---|---|
| [README.md](README.md) | This overview |
| [features.md](features.md) | Inventory of every mobile feature (F-xx), requested but missing features (X-xx), new web features (N-xx), desktop method matrix, priorities |
| [design-system.md](design-system.md) | Tokens, themes, accents, glass, motion, Tailwind v4 theme, shadcn mapping, responsive layouts, accessibility, new-feature checklist |
| [architecture.md](architecture.md) | Routes, state and data layer, key storage, service worker, Worker routes, D1 schema, DO relay, frames, Noise, desktop changes, pairing, Web Push, self-host guide |
| [security.md](security.md) | Threat model (T1 to T15), auth, invites and admin, E2E, device trust, desktop hardening, XSS rules, CSP, Trusted Types, deploy security, logging |
| [capacity.md](capacity.md) | Cloudflare free tier limits, usage model, how many users fit, mitigations, paid tier threshold, monitoring |
| [roadmap.md](roadmap.md) | Phased tickets with acceptance criteria, dependencies, risk, release gates, open questions |

## 7. Documentation conventions

- **Decided vs open**: everything outside the "Open questions" section of roadmap.md is decided and written as fact. Anything undecided or unverified is marked "open" or "unverified" and must appear in roadmap.md "Open questions" or in a spike ticket. Do not hide an option inside another document.
- **Single source**: route names, table names, frame names, close codes and token scopes are defined in architecture.md. Other documents reference them, never redefine them. Feature IDs (F-, X-, N-) are defined in features.md; ticket IDs in roadmap.md.
- **Evidence**: cite code as `path:line` and external facts with a URL and the date read. Mark each external number "verified" with the date read, or "unverified" (capacity.md convention).
- **How to update**: change the owning document first, then fix every reference in the other documents in the same PR. When an open question is answered, move the answer into the owning document, delete the question, and note the decision in the PR description. When a spike result changes a number, update capacity.md or architecture.md and tick the ticket.
- **Style**: English, concise, tables over prose. No em dash or en dash characters; use "-". Code, identifiers and commit messages are English.
- **Git**: follow `docs/git-conventions.md` (`docs(web): ...` for specification changes).
