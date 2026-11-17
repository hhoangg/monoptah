# Monoptah web app: architecture

Scope: frontend, Worker, relay, protocol, Web Push, desktop changes, self-hosting. Security reasoning lives in [security.md](security.md); both documents use the same route, table, token and frame names. Paths such as `crates/monocode-core/src/mobile/host.rs:640` refer to the repo as researched on 2026-11-17.

## 1. System overview

```
 Browser (PWA, React CSR)                 Cloudflare (ONE Worker)                    Monoptah Native (desktop)
 +----------------------+      HTTPS     +--------------------------------+          +-------------------------+
 | app shell (SW cache) |--------------->| Assets binding (app/dist, SPA) |          |  MobileHost (unchanged) |
 | TanStack Router/Query|   /api/* JSON  | Hono /api/*  --> D1 (Drizzle)  |          |   ^ handle()/watch()    |
 | Zustand link store   |--------------->|                                |          |   | peer = key fingerprint |
 | Noise IK (noble)     |                |                                |  WSS     |  +-------------------+  |
 |   device static key  |  WSS /relay/*  | /relay/* --> DO "DesktopRoom"  |<---------|  | web_link (snow)   |  |
 |                      |<==============>|  (1 per desktop, hibernation)  |  outbound|  | Noise IK responder|  |
 +----------------------+  opaque frames +--------------------------------+          |  +-------------------+  |
          ^                                     |  Web Push (VAPID, aes128gcm)         |  QUIC listener stays    |
          +-------- push service <--------------+<------ POST /api/push/notify -------+  (phones, unchanged)    |
```

Trust split: the Worker knows accounts, public keys and push endpoints (metadata). Conversation content only exists in the browser and on the desktop; the relay sees ciphertext (see security.md section 5).

## 2. Repository layout

```
web/
  app/        Vite + React 19 SPA, service worker (src/sw.ts)
  worker/     Hono API, Drizzle schema + migrations, DesktopRoom DO, push sender
  protocol/   shared TypeScript types + constants (frames, envelopes, close codes, error codes)
  docs/       these documents
  wrangler.jsonc
  pnpm-workspace.yaml
```

`web/protocol` is the single source for frame constants; the Rust side mirrors them in `crates/monocode-core/src/mobile/web_link/frame.rs` and a shared test-vector file (`web/protocol/vectors.json`) is read by both test suites.

## 3. Frontend

### 3.1 Route tree (TanStack Router, file-based, CSR)

| Route | Auth | Purpose |
|---|---|---|
| `/` | session | Restores the last place (last used desktop, project, session) and redirects there, else `/desktops` |
| `/login` | public | Passkey sign-in (discoverable credential, hybrid QR offered by the browser), link to `/recover` |
| `/register?invite=CODE` | public | Invite code, handle, create passkey, then one-time recovery codes screen |
| `/recover` | public | Handle + recovery code, then forced "add a new passkey" |
| `/pair/:code` | session (login redirect keeps the code) | Approve a desktop: machine name, fingerprint, confirmation code (section 6.4). The `#fp` fragment is saved to `sessionStorage` before any redirect to `/login?redirect=` and restored after sign-in; without a `#fp` the page shows a warning that the desktop key cannot be checked |
| `/desktops` | session | List: online status, rename, color, pending confirmations badge |
| `/desktops/:id` | session | Manage one desktop: rename, color, devices and grants, per-device permission, revoke desktop |
| `/d/:desktopId` | session + link | Home for one desktop: projects and recent sessions (catalog) |
| `/d/:desktopId/p/:projectId` | session + link | Project: sessions list (`sessions.list` paging) |
| `/d/:desktopId/p/:projectId/files/$` | session + link | File browser and viewer (splat path) |
| `/d/:desktopId/p/:projectId/new` | session + link | New session in a project (model, mode, first prompt) |
| `/d/:desktopId/p/:projectId/changes/$` | session + link | Working-tree diff (splat path selects a file) |
| `/d/:desktopId/p/:projectId/git` | session + link | Git status and actions |
| `/d/:desktopId/s/:sessionId` | session + link | Transcript and composer. Project is resolved from the catalog, so push links need only desktop and session |
| `/d/:desktopId/monos/:monoId` | session + link | Mono chat |
| `/d/:desktopId/monos/:monoId/profile` | session + link | Mono profile; children: `/edit`, `/habit/:id`, `/documents`, `/file/:name` |
| `/d/:desktopId/settings` | session + link | Per-desktop: model defaults, project selection view, notification categories |
| `/settings` | session | Account: passkeys, recovery codes, active sessions, notifications, theme, install |
| `/admin`, `/admin/users`, `/admin/invites` | session, role admin, fresh | Minimal admin (security.md section 4) |
| `/install` | public | Add to Home Screen guide, push enable button |

Guards: the root `beforeLoad` calls `ensureQueryData(['me'])`; a 401 redirects to `/login?redirect=<path>`. `/d/*` routes mount a `LinkProvider` for the desktop id that owns the connection lifecycle (section 3.3). Route params are typed; `projectId` is the percent-encoded value from the catalog, used as received.

### 3.2 State split

| Layer | Tool | Holds |
|---|---|---|
| Worker API data | TanStack Query (`['me']`, `['desktops']`, `['passkeys']`, `['admin','users']`) | Server-owned account data. Normal `fetch` with cookie, `X-Monoptah-CSRF: 1` header |
| Desktop link data | TanStack Query with link adapters | Keys `['link', desktopId, method, params]` and `['link', desktopId, 'conversation', conversationId]`. `staleTime: Infinity`, `networkMode: 'always'`, `gcTime` 10 min |
| Link connection state | Zustand store per desktop (`useLink(desktopId)`) | `status`: `idle -> connecting -> handshaking -> pending -> ready -> reconnecting -> closed`; `permission`; `desktopName`; `lastFrameAt`; open watches; `revoked` flag |
| UI state | Zustand (small slices) | Theme, composer drafts, sheet state |

Adapters:

- **Request adapter**: `queryFn` calls `link.request(method, params)` and returns `result`. Mutations call `link.request` with a command id kept per `(method, params)` signature until success, mirroring `mobile/src/features/workspace/WorkspaceProvider.tsx:418-436` and the desktop receipt table (`crates/monocode-gpui/src/shell/mobile_host.rs:1269-1333`).
- **Watch adapter**: `link.watch('conversation.watch', params, onFrame)`; `onFrame` applies frames with a port of `mobile/src/features/workspace/watchUpdates.ts:77-132` and writes the projected turns with `queryClient.setQueryData`. A `null` apply result closes and reopens the watch with `from` set to the first held turn (same rule as `WorkspaceProvider.tsx:319-321`, where apply returns false, and `:370-389`, the reopen with `from`).
- **Catalog**: `workspace.list` refetched on a 5 s interval while visible and `ready` (as on mobile, `ConnectionProvider.tsx:157-160`) and on `visibilitychange` to visible.
- The desktop already sends projected turns, so the web app does not reimplement `crates/mobile-link/src/transcript.rs`. It ports the mobile JS rendering path (`mobile/src/features/sessions/TranscriptTurn.tsx`, `markdownParser.ts`) into `web/app/src/features/transcript/`.

### 3.3 Data layer over the E2E link

`web/app/src/link/` contains: `relay.ts` (WebSocket), `noise.ts` (Noise IK on @noble), `session.ts` (request/response, watches, chunking), `store.ts` (Zustand).

Connect sequence:

1. `GET /api/desktops/:id` returns `{publicKey, fingerprint, name}` (the directory). The device pins `publicKey` in IndexedDB on first success; a later change blocks the link with a "desktop key changed" screen.
2. Open `wss://HOST/relay/device/:desktopId?device=<deviceId>` (cookie authenticates; section 4.4).
3. Handshake: send relay frame `HS1` with Noise message 1; receive `HS2`; both sides now hold transport CipherStates (section 5.3).
4. First encrypted message from the desktop is `{"type":"state","state":"pending"|"confirmed","permission":"read"|"control","proto":1,"desktop":{"name","version"}}`. In `pending` the store shows "Confirm this device on the desktop" with the device fingerprint and the pinned desktop fingerprint (the user compares both with the desktop dialog, section 6.3); requests are not sent. `confirmed` moves to `ready`.
5. Requests: allocate `rid` (u32, per session), send JSON envelope, resolve on matching `rid` response, timeout 35 s (desktop handler 30 s, `crates/mobile-link/src/transport.rs:160`). Watches: send with `"watch":true`, expect `{rid, ok:true, watching:true}`, then frames; `{rid, ok:true, heartbeat:true}` every 25 s; the client treats 75 s of silence as dead.
6. Keepalive: client sends WebSocket text `ping` every 30 s; the DO auto-responds `pong` without waking (section 4.4). It also keeps proxies from idling the socket out.

Resync and reconnect:

- `visibilitychange` to `hidden`: do nothing (iOS may kill the socket; do not fight it).
- To `visible`: if the socket is not open or `lastFrameAt` is older than 75 s, reconnect immediately and reopen every watch that was open (each with `from`); then invalidate the catalog query and refetch the current conversation with `conversation.read` as the fallback the mobile app uses (`WorkspaceProvider.tsx:391-394`).
- Socket close: backoff `min(30 s, 0.5 s * 2^n) * random(0.5..1)`, reset after 10 s of `ready`. While `document.hidden` no reconnect is attempted; it happens on the next `visible`. `navigator.onLine === false` pauses attempts until the `online` event.
- Close codes (section 4.5) decide behaviour: `4003` or `4011` stop reconnecting and show "Access revoked" (sets `revoked`); `4009` replaced (another tab or reconnect) retries once; `4010`/`4008` back off 60 s.
- Handshake failure after reconnect always starts a fresh Noise session; Noise sessions are never resumed.

### 3.4 Key storage

- The device static X25519 keypair is generated with `@noble/curves` (raw 32-byte keys). WebCrypto X25519 is not used: Safari support is unverified and a non-extractable X25519 `CryptoKey` read back as null from IndexedDB on Safari 26.4 (https://bugs.webkit.org/show_bug.cgi?id=312279, read 2026-11).
- Because noble needs raw bytes, the private key cannot be a non-extractable `CryptoKey`. Instead: a random **AES-GCM-256 `CryptoKey` created with `extractable: false`** is stored in IndexedDB (AES keys survive structured clone), and the X25519 private key is stored only as AES-GCM ciphertext (random 96-bit IV, AAD = `deviceId`) in the same database. At runtime the key is unwrapped once into a module-scoped `Uint8Array` and zeroed on `pagehide`.
- Effect: a copy of the profile files or an IndexedDB dump does not yield the private key (the AES key cannot be exported); an XSS that runs inside the origin can still call the unwrap path. That is why XSS is the top risk (security.md section 2) and why CSP and Trusted Types are mandatory.
- `navigator.storage.persist()` is requested after sign-in so Safari's 7-day eviction of script-writable storage is avoided for installed PWAs; if the key is evicted the device simply re-registers and must be re-confirmed on each desktop.
- "Sign out" keeps the device key. "Forget this device" deletes the key, the cache and calls `DELETE /api/devices/:id`.
- The desktop public keys it pinned live in IndexedDB store `pins`.

### 3.5 Service worker

- `vite-plugin-pwa` with `strategies: 'injectManifest'`, `registerType: 'prompt'`, source `web/app/src/sw.ts`. No automatic `skipWaiting`: a toast offers "Update available", and the update is applied on user action so a running link is not cut.
- Workbox `precacheAndRoute(self.__WB_MANIFEST)` for the app shell and static assets; `NavigationRoute` to `/index.html` with `denylist: [/^\/api\//, /^\/relay\//]`.
- No runtime caching of `/api/*`, `/relay/*`, or anything with cookies. API responses are `Cache-Control: no-store`.
- Handlers: `push` (parse JSON, always call `showNotification`; iOS revokes silent pushes), `notificationclick` (close, find a window client, `client.navigate(data.url)` then `focus()`, else `clients.openWindow(data.url)`), `pushsubscriptionchange` (re-subscribe and `POST /api/push/subscriptions`).
- Manifest: `display: standalone`, `start_url: /`, `id: /`, maskable icons, theme colors from the design system.

### 3.6 Offline behaviour

| State | Behaviour |
|---|---|
| Offline, shell cached | App opens; `/api/me` fails, so the last known account name is shown from `localStorage`; desktops list read from persisted Query cache |
| Desktop offline (relay says `desktop_offline`) | Desktop shows "Offline" badge; the last cached catalog and last 30 turns of recently opened sessions are readable; composer disabled with a clear reason |
| Mutations while not `ready` | Not queued (a stale prompt must not run later on a desktop); the composer keeps the draft |
| Cache at rest | Catalog and last turns persisted in IndexedDB, AES-GCM encrypted with the same non-extractable key as the device key blob; wiped on "Forget this device" and on `revoked` |

### 3.7 Install flow and iOS standalone detection

- `isStandalone = navigator.standalone === true || matchMedia('(display-mode: standalone)').matches`.
- Android and desktop Chromium: capture `beforeinstallprompt`, show an "Install" button on `/install` and in `/settings`.
- iOS Safari, not standalone: `/install` shows the Share, "Add to Home Screen" steps. The "Enable notifications" button is hidden until `isStandalone && 'PushManager' in window`; permission is requested only from that button's click handler (user gesture).
- First launch from the Home Screen is a separate storage partition from Safari: the user signs in again (a passkey makes this one tap) and the device key is created fresh and must be re-confirmed on each desktop. The `/install` page states this.

## 4. Worker

### 4.1 Layout and config

One Worker: `worker/src/index.ts` exports `default { fetch }` (Hono app) and the `DesktopRoom` Durable Object class. `wrangler.jsonc` (excerpt):

```jsonc
{
  "name": "monoptah-web",
  "main": "worker/src/index.ts",
  "compatibility_date": "2026-09-01",          // nodejs_compat is default from 2026-08-04
  "assets": {
    "directory": "app/dist",
    "binding": "ASSETS",
    "not_found_handling": "single-page-application",
    "run_worker_first": ["/api/*", "/relay/*"]
  },
  "durable_objects": { "bindings": [{ "name": "ROOM", "class_name": "DesktopRoom" }] },
  "migrations": [{ "tag": "v1", "new_sqlite_classes": ["DesktopRoom"] }],
  "d1_databases": [{ "binding": "DB", "database_name": "monoptah", "database_id": "<id>", "migrations_dir": "worker/migrations" }],
  "vars": { "RP_ID": "app.example.com", "RP_ORIGIN": "https://app.example.com", "VAPID_PUBLIC_KEY": "<b64url>", "VAPID_SUBJECT": "mailto:admin@example.com" }
}
```

Secrets (never in `vars`): `VAPID_PRIVATE_KEY`, `RECOVERY_PEPPER`, `AUDIT_IP_SALT` (salt for the IP hashes in `audit_log` and `pairing_requests`). Dev uses `@cloudflare/vite-plugin` so SPA, Worker, DO and a local D1 run in one `vite dev`.

Middleware order for `/api/*`: security headers, then, **only for cookie-authenticated routes** (routes marked session, fresh or admin, plus the public browser auth routes under `/api/auth/`), the `Origin` check on non-GET (must equal `RP_ORIGIN`) and the `X-Monoptah-CSRF: 1` check on non-GET; then rate limit, auth resolver per route, handler. Token routes (`/api/desktop/*`, `/api/push/notify`) and the desktop pairing routes (`/api/pair/start`, `/api/pair/poll`) are called by the desktop app, carry no `Origin` and no cookie, and ignore cookies entirely (a cookie on these requests is never read). Rate limits: D1 `rate_limits` for the low-volume auth, pairing and notify groups; the general per-session limit (120 per minute) is an in-memory per-isolate counter, best effort, so it costs no D1 writes (security.md 3.5). Errors are `{error, code}` JSON.

### 4.2 Hono route table

Auth legend: **public**; **session** (valid cookie session); **fresh** (session plus passkey re-auth within the last 5 min, `sessions.reauth_at`); **token** (desktop bearer token with the stated scope); **admin** (session, `users.role = 'admin'`, fresh).

| Method and path | Auth | Purpose |
|---|---|---|
| `GET /api/health` | public | Liveness probe for monitoring (`{ok: true}`, no D1 read) |
| `POST /api/auth/register/options` | public, rate-limited | Validate invite, handle; return WebAuthn creation options (`userVerification: required`, `residentKey: required`) |
| `POST /api/auth/register/verify` | public | Verify attestation, create user, passkey, 10 recovery codes (returned once), session |
| `POST /api/auth/login/options` | public, rate-limited | Discoverable-credential request options |
| `POST /api/auth/login/verify` | public | Verify assertion, check counter, create session |
| `POST /api/auth/reauth/options`, `POST /api/auth/reauth/verify` | session | Fresh assertion for sensitive actions |
| `POST /api/auth/recover` | public, rate-limited | `{handle, code}`; creates a `recovery_only` session |
| `POST /api/auth/logout` | session | Delete current session row |
| `GET /api/me` | session | User, role, invites left, whether `recovery_only` |
| `PATCH /api/me` | session | Change display name |
| `DELETE /api/me` | fresh | Delete the account and all its rows (security.md section 10) |
| `GET /api/audit` | session | Own audit events (paged), for the account activity view |
| `GET /api/sessions`, `DELETE /api/sessions/:id` | session | List and revoke own sessions |
| `GET /api/passkeys` | session | List |
| `POST /api/passkeys/options`, `POST /api/passkeys/verify` | fresh (or `recovery_only`) | Add a passkey |
| `PATCH /api/passkeys/:id`, `DELETE /api/passkeys/:id` | fresh | Rename; delete (never the last one) |
| `POST /api/recovery-codes/regenerate` | fresh | Replace all codes, return 10 new ones once |
| `GET /api/invites`, `POST /api/invites`, `DELETE /api/invites/:id` | session | List own; create (quota); revoke unused |
| `POST /api/pair/start` | public, rate-limited per IP | Desktop starts device authorization (section 6.4) |
| `POST /api/pair/poll` | public (secret `device_code` + `verifier`) | Desktop polls; on approval receives its token once |
| `GET /api/pair/:userCode` | session | Show machine name, fingerprint, confirmation code |
| `POST /api/pair/:userCode/approve` | fresh | Bind desktop key to the user, mint token |
| `POST /api/pair/:userCode/deny` | session | Deny |
| `GET /api/desktops` | session | List with `online` (DO presence), name, color, pending count |
| `GET /api/desktops/:id` | session (owner) | Directory entry incl. `publicKey`, `fingerprint` |
| `PATCH /api/desktops/:id` | session (owner) | Rename, color |
| `DELETE /api/desktops/:id` | fresh (owner) | Revoke desktop: tokens revoked, grants revoked, room sockets closed `4003` |
| `POST /api/devices` | session | Register this browser: `{name, publicKey}` returns `deviceId` |
| `GET /api/devices`, `PATCH /api/devices/:id`, `DELETE /api/devices/:id` | session | List, rename, forget (revokes all grants) |
| `POST /api/desktops/:id/grants` | session (owner) | Request access of a device to a desktop (creates `pending` grant, notifies desktop) |
| `GET /api/desktops/:id/grants` | session (owner) | Grants with state, fingerprint, permission |
| `PUT /api/desktops/:id/grants/:deviceId` | session (owner); fresh to revoke | Set `requestedPermission`, or `state: "revoked"` (server blocks the relay join at once) |
| `GET /api/desktop/me` | token `relay` | Desktop identity and account handle |
| `GET /api/desktop/grants` | token `grants` | Grants plus device public keys for this desktop (desktop applies them, section 7.5) |
| `POST /api/desktop/grants/:deviceId/state` | token `grants` | Desktop reports `confirmed`, `pending`, `revoked`, and the permission it applied |
| `GET /api/push/vapid-key` | public | VAPID public key |
| `POST /api/push/subscriptions` | session | Store `{endpoint, keys, deviceId, vapidKeyId}` |
| `DELETE /api/push/subscriptions/:id` | session | Remove |
| `PUT /api/push/prefs/:desktopId` | session (owner) | Enable and categories per desktop |
| `POST /api/push/notify` | token `push` | Desktop asks the Worker to push (section 8) |
| `GET /api/admin/users` | admin | Users, status, invite tree position, desktop counts |
| `POST /api/admin/users/:id/block` | admin | `{cascade: boolean, reason}` |
| `POST /api/admin/users/:id/unblock` | admin | Unblock one user (never cascades) |
| `POST /api/admin/users/:id/invites` | admin | Grant extra invites |
| `GET /api/admin/audit` | admin | Paged audit log |
| `GET /relay/desktop/:desktopId` | token `relay` | WebSocket upgrade for the desktop (section 4.4) |
| `GET /relay/device/:desktopId?device=ID` | session + device grant | WebSocket upgrade for a device |

### 4.3 D1 schema (Drizzle, `worker/src/db/schema.ts`)

All ids are random UUIDv4 text unless noted. Times are unix seconds (integer). Secrets are stored only as hashes.

| Table | Key columns |
|---|---|
| `users` | `id` PK; `handle` UNIQUE (3-32 chars, lowercase); `display_name`; `role` (`user`/`admin`); `status` (`active`/`blocked`); `invited_by` FK users (null for the bootstrap admin); `invite_id` FK invites; `invites_granted` int (default 3); `blocked_at`; `blocked_reason`; `blocked_via` (user id of the branch root that cascaded, or null); `created_at` |
| `passkeys` | `id` PK (credential id, base64url); `user_id` FK; `public_key` BLOB; `counter` int; `transports` JSON; `device_type`; `backed_up` bool; `name`; `created_at`; `last_used_at` |
| `challenges` | `id` PK; `purpose` (`register`/`login`/`reauth`/`addpasskey`); `user_id` nullable; `challenge`; `payload` JSON (invite, handle); `expires_at` (5 min); consumed on use |
| `recovery_codes` | `id` PK; `user_id` FK; `code_hash` (HMAC-SHA-256 with `RECOVERY_PEPPER`, hex); `batch` int; `used_at`; `created_at` |
| `sessions` | `id_hash` PK (SHA-256 of the cookie token); `user_id`; `created_at`; `last_seen_at`; `idle_expires_at`; `expires_at`; `reauth_at`; `amr` (`passkey`/`recovery`); `recovery_only` bool; `user_agent` (truncated); `revoked_at` |
| `invites` | `id` PK; `code_hash` (SHA-256); `created_by` FK users; `kind` (`user`/`bootstrap`); `expires_at`; `used_by` FK users; `used_at`; `revoked_at`; `created_at` |
| `desktops` | `id` PK; `user_id` FK; `name`; `color`; `public_key` (X25519, base64url); `fingerprint` (hex SHA-256 of the public key); `os`; `app_version`; `created_at`; `last_seen_at` (written on desktop disconnect); `revoked_at` |
| `desktop_tokens` | `id` PK; `desktop_id` FK; `token_hash` (SHA-256 of the 256-bit token) UNIQUE; `scope` (`relay push grants`); `created_at`; `last_used_at`; `revoked_at` |
| `devices` | `id` PK; `user_id` FK; `name`; `public_key`; `fingerprint`; `created_at`; `last_seen_at` (written when the device's last socket closes, never per heartbeat); `revoked_at` |
| `device_grants` | PK (`device_id`, `desktop_id`); `state` (`pending`/`confirmed`/`revoked`); `permission` (`read`/`control`, applied by desktop); `requested_permission`; `confirmed_at`; `updated_at` |
| `push_subscriptions` | `id` PK; `user_id`; `device_id`; `endpoint` UNIQUE; `p256dh`; `auth`; `vapid_key_id` (which VAPID public key the subscription was created with, for rotation); `ua`; `created_at`; `last_success_at`; `failures` int |
| `push_prefs` | PK (`user_id`, `desktop_id`); `enabled` bool; `categories` JSON (`agentFinished`, `agentInput`, `monoMessages`) |
| `pairing_requests` | `id` PK; `user_code` UNIQUE (8 chars, ambiguity-free alphabet); `device_code_hash`; `challenge` (SHA-256 of the desktop's verifier); `desktop_public_key`; `machine_name`; `os`; `confirmation_code`; `status` (`pending`/`approved`/`denied`/`expired`/`consumed`); `user_id`; `desktop_id`; `created_at`; `expires_at` (10 min); `last_polled_at`; `ip_hash` |
| `audit_log` | `id` PK; `ts`; `actor_user_id`; `actor_kind` (`user`/`desktop`/`admin`/`system`); `action`; `target_type`; `target_id`; `meta` JSON (no secrets, no content) |
| `rate_limits` | PK (`key`, `window_start`); `count`. Used for auth, pairing and notify only; relay limits live in the DO and the general per-session API limit is in memory per isolate |

Indexes: `passkeys(user_id)`, `sessions(user_id)`, `sessions(idle_expires_at)`, `invites(created_by)`, `users(invited_by)` (branch walks), `desktops(user_id)`, `devices(user_id)`, `device_grants(desktop_id)`, `push_subscriptions(user_id)`. A daily cron trigger deletes expired `challenges`, `sessions`, `pairing_requests`, `rate_limits` rows and audit rows older than the retention (security.md section 10).

### 4.4 Durable Object relay: `DesktopRoom`

One room per desktop: `env.ROOM.get(env.ROOM.idFromName(desktopId))`. SQLite-backed class, WebSocket Hibernation API.

Who may join (checked in the Worker before forwarding the upgrade; the DO has no public route):

| Role | Credentials | Checks |
|---|---|---|
| `desktop` | `Authorization: Bearer <desktop token>` on the upgrade request | `desktop_tokens` hash lookup, not revoked, scope `relay`, desktop not revoked, owner not blocked, token's `desktop_id` equals the path id |
| `device` | Session cookie, `Origin` equals `RP_ORIGIN`, query `device=<deviceId>` | Valid unrevoked session (not `recovery_only`), `devices.user_id` equals session user, device not revoked, `desktops.user_id` equals session user, `device_grants` row exists and `state != 'revoked'` (a `pending` grant may join so it can handshake and be confirmed on the desktop) |

The Worker forwards the upgrade to the DO with internal headers `x-role`, `x-device-id`, `x-user-id` (client-supplied copies are stripped). The DO calls `ctx.acceptWebSocket(ws, [tag])` with tag `desktop` or `device:<deviceId>` and `ws.serializeAttachment({role, deviceId, userId, joinedAt})` (well under the 16 KiB limit). Nothing else is kept in memory; any state needed after hibernation is in the attachment or DO storage.

Routing of opaque frames (frame layout in section 5.1):

- Device to desktop: handler `webSocketMessage` validates length (20 to 65,555 bytes, binary only), type in `{HS1, TRANSPORT, BYE}`, **overwrites the `deviceId` header field with the attached authenticated id**, forwards to the single `desktop` socket. No desktop connected: replies to that socket with an `ERROR` control frame `desktop_offline`.
- Desktop to device: reads `deviceId` from the header and sends the frame to every socket tagged `device:<deviceId>` (a device may have several tabs; each tab filters on `channel`).
- Relay-originated control frames (type `CTRL`, JSON body, metadata only): to devices `{"t":"presence","desktop":"online"|"offline"}`; to the desktop `{"t":"device","id","event":"joined"|"left"}` and `{"t":"grants"}` (re-sync grants, section 7.5).
- The DO never parses ciphertext and never logs frame bytes.

Presence: online means a live `desktop` socket exists. `GET /api/desktops` asks each DO (`room.presence()` RPC, parallel; users hold few desktops) and merges the answer; D1 `desktops.last_seen_at` is written only when the desktop socket closes (not per heartbeat) to spare the 100,000 writes/day free budget. Devices get a `presence` control frame when the state changes.

Per-room quotas (constants in `web/protocol/limits.ts`):

| Quota | Value | Violation |
|---|---|---|
| Desktop sockets | 1 (a new one replaces the old, which closes `4009`) | n/a |
| Device sockets per room | 16; per device id 4 | new socket closes `4010` |
| Max WebSocket message | 65,555 bytes (20 header + 65,535 Noise) | close `1009` |
| Device inbound rate | 40 msg/s sustained, burst 100, 1 MiB/s (in-memory token bucket, rebuilt after hibernation) | drop then close `4008` after 3 breaches in 10 s |
| Desktop outbound rate | 200 msg/s, 4 MiB/s | same |
| Text frames | only exactly `ping` (auto-response) | other text closes `4012` |

Hibernation: no timers or alarms in the DO, so idle rooms are evicted while sockets stay open. `ctx.setWebSocketAutoResponse(new WebSocketRequestResponsePair('ping','pong'))` answers keepalives without waking the object. Because the object may be reconstructed on every message, the constructor does no work beyond reading bindings.

Revocation path: Worker handlers (`DELETE /api/desktops/:id`, `PUT .../grants/:deviceId` with `revoked`, `DELETE /api/devices/:id`, admin block) call an internal DO method `room.revoke({deviceId?}|{all:true}, code)` that closes the matching sockets with `4003` or `4011`.

### 4.5 Close codes

| Code | Name | Client reaction |
|---|---|---|
| 1000 | normal | reconnect with backoff |
| 1001 | going away (deploy) | reconnect immediately once |
| 1008 | policy | no retry, show error |
| 1009 | message too big | bug; no retry |
| 4001 | unauthorized (cookie or token invalid) | device: go to `/login`; desktop: report "token invalid" |
| 4003 | revoked (grant, device, desktop or token) | stop; show "Access revoked" |
| 4008 | rate limited | back off 60 s |
| 4009 | replaced by a newer connection | retry once |
| 4010 | room quota exceeded | back off 60 s |
| 4011 | account blocked | stop; show blocked screen |
| 4012 | protocol error | reconnect, new handshake |

## 5. Protocol (`web/protocol`)

### 5.1 Outer relay frame (binary WebSocket message)

```
offset  size  field
0       1     version      = 1
1       1     type         0x01 HS1  device -> desktop (Noise msg 1)
                           0x02 HS2  desktop -> device (Noise msg 2)
                           0x03 TRANSPORT  either direction (Noise transport message)
                           0x04 BYE  either direction (session end, empty body)
                           0x10 CTRL relay -> peer (JSON body, plaintext, metadata only)
                           0x11 ERROR relay -> peer (JSON body {code})
2       2     channel      u16 big-endian, chosen by the device per tab/session
4       16    deviceId     UUID bytes. The relay overwrites it on device->desktop frames
20      N     body         opaque ciphertext (HS1/HS2/TRANSPORT) or JSON (CTRL/ERROR), N <= 65,535
```

The desktop keys a session by `(deviceId, channel)`. Frames for an unknown pair other than `HS1` are answered with `BYE`.

### 5.2 Max size and chunking

- Noise messages are at most 65,535 bytes, so each TRANSPORT body is at most 65,535 (including the 16-byte tag, so 65,519 bytes of plaintext).
- Plaintext layout: `kind` (1 byte) then payload.
  - `0x01 MSG`: complete UTF-8 JSON envelope (fits one message).
  - `0x02 CHUNK`: `msgId` u32, `index` u16, `flags` u8 (bit0 = last), then bytes. Receiver concatenates by `msgId`.
  - `0x03 REKEY`: empty; the receiver rekeys its receiving CipherState (section 5.4).
- Reassembled limits: request 8 MiB, response 8 MiB (the QUIC limits are 32 MiB and 8 MiB, `crates/mobile-link/src/lib.rs:29-30`). The web transport lowers the request limit to 8 MiB; a `conversation.send` request is capped at 8 MiB in total, base64 attachments included, and the client refuses anything larger (the desktop allows up to 10 attachments, `crates/monocode-gpui/src/shell/mobile_host.rs:37` and `:1811-1812`, so large images are down-scaled client-side). At most 4 messages in reassembly per session; the whole reassembly must finish within 60 s or the session is closed with `BYE`.

### 5.3 Noise handshake

Pattern `Noise_IK_25519_ChaChaPoly_SHA256`; the device is the initiator and already knows the desktop static key from `GET /api/desktops/:id` (pinned after first use).

```
Prologue = "monoptah-web/1" || desktopId(16 bytes) || deviceId(16 bytes) || channel(2)
IK:   <- s
      ...
      -> e, es, s, ss          (HS1 body; payload = {"name":"MacBook Safari","ua":"..."} encrypted)
      <- e, ee, se             (HS2 body; payload = {"proto":1} encrypted)
```

- JS: manual implementation on `@noble/curves` (x25519), `@noble/ciphers` (chacha20poly1305), `@noble/hashes` (sha256, hkdf), cross-tested against `snow` with the Noise test vectors in the spike. Rust: `snow` (`Builder::new("Noise_IK_25519_ChaChaPoly_SHA256")`).
- The desktop learns the device static key `s` from HS1 and computes `peer = lowercase hex SHA-256(s)` (the device fingerprint, same format as the TLS fingerprint used today, `crates/mobile-link/src/lib.rs:91-96`). It looks `peer` up in its own confirmed-device list (the `Gate`-style map, `crates/mobile-link/src/lib.rs:160-177`):
  - **Known and confirmed**: session `ready` with the stored permission.
  - **Unknown**: session `pending`: the Noise session completes (so the confirmation prompt can show the device name from the encrypted payload and the fingerprint), the desktop sends `{"type":"state","state":"pending"}`, and ignores all requests until the user confirms on the desktop (5 min timeout, then `BYE`). The device name is displayed as untrusted text.
  - **Revoked**: `BYE` immediately, no state message.
- The server cannot forge a device: it never holds device private keys, and the desktop checks `s` against its own list, not against the server directory. The directory only provides the desktop key so the device can start the handshake. On first contact a substituted desktop key is **not** caught by IK itself: a malicious directory that returns its own key simply completes the handshake as the responder. Detection is by comparison: the approving browser has `#fp` from the QR (section 6.4), the web pending screen shows the pinned desktop fingerprint, and the desktop confirmation dialog shows both the desktop's and the device's fingerprints (section 6.3) for the user to compare. After the first connection the pin blocks any later change.

### 5.4 Rekey and lifetime

- Sender sends `REKEY` and then calls Noise `Rekey()` on its sending CipherState after 2^16 messages or 1 hour, whichever comes first; the receiver rekeys on receipt. A decryption failure closes the session with `BYE` and the client opens a new handshake.
- Sessions end on `BYE`, socket close, desktop revoke, or an idle limit of 12 h.

### 5.5 Application envelope (unchanged mobile-link JSON plus `rid`)

The plaintext inside `MSG` reuses the existing shapes verbatim (`crates/mobile-link/src/transport.rs:165-168, 202-225`; `crates/monocode-core/src/mobile/host.rs:640-730`) and adds only `rid`, a client-chosen u32 correlation id, because a WebSocket has no per-request stream (QUIC uses one bidirectional stream per request):

```jsonc
// request  (device -> desktop)
{ "version": 1, "rid": 7, "id": "<command id>", "method": "conversation.send", "params": { } }
// watch open (only conversation.watch)
{ "version": 1, "rid": 8, "id": "...", "method": "conversation.watch", "params": { "conversationId": "...", "tail": 3, "incremental": true }, "watch": true }
// watch stop
{ "rid": 8, "cancel": true }
// response / error
{ "rid": 7, "ok": true,  "result": { } }
{ "rid": 7, "ok": false, "error": "Desktop is busy; try again", "code": "busy" }
// watch lifecycle
{ "rid": 8, "ok": true, "watching": true }
{ "rid": 8, "ok": true, "result": { /* frame: usageLimit, busy, older, order, question, turns, patches */ } }
{ "rid": 8, "ok": true, "heartbeat": true }
// session state (desktop -> device, no rid)
{ "type": "state", "state": "pending|confirmed|revoked", "permission": "read|control", "proto": 1, "desktop": { "name": "...", "version": "..." } }
```

The `error` string stays; the optional additive `code` (`busy`, `forbidden`, `read_only`, `not_selected`, `path_denied`, `revoked`, `disabled`, `too_large`) gives the web client a stable key (gap G14, security.md section 7). `watch` frame semantics, `from`/`append` UTF-16 offsets and idempotency receipts are unchanged (`crates/monocode-core/src/mobile/watch.rs:65-158`). Rules: `id` required for app methods and at most 128 chars (`host.rs:698-701`); at most 16 pending requests host-wide and 4 watches per device (`host.rs:57-58`); heartbeat period for web is 25 s instead of 10 s to keep DO message billing low (capacity.md).

### 5.6 Versioning

Three independent counters: relay frame `version` (byte 0), Noise prologue string `monoptah-web/N`, and the JSON `"version": 1` already enforced by the host (`host.rs:641`). The `state` message carries `proto`; the client refuses a desktop whose `proto` is higher than it knows and shows "Update the web app" (the SW update prompt), and a desktop refuses an older `HS1` prologue with `ERROR`/`BYE` and a visible "Update Monoptah" message. Method set discovery is by error (`Unsupported mobile operation`), as today.

## 6. Desktop changes (Monoptah Native)

### 6.1 Transport beside QUIC

- New module `crates/monocode-core/src/mobile/web_link/` (`mod.rs`, `relay.rs` outbound WebSocket client with reconnect, `noise.rs` using `snow`, `session.rs` per-device sessions, `frame.rs`). It runs as a task started in `MobileHost::start` next to the QUIC `server_endpoint` (`host.rs:512-557`); QUIC for phones is unchanged and may be on or off independently.
- **The seam**: `serve` receives two closures bound to `MobileHost::handle` (`host.rs:640`) and `MobileHost::watch` (`host.rs:734`) (`host.rs:543-554`, signature at `transport.rs:91-97`). The web transport calls the same two closures with `peer = fingerprint hex of the device's Noise static key`, so allowlist, dedup (`mobile_command`), backpressure and `WatchSink` work as is.
- **Refactor**: extract the post-handshake logic of `serve` (response wrapping `transport.rs:165-168`, watch accept frame `:202-211`, heartbeat `:217-225`, Gate recheck before each request and frame `:124-139, 228-236`, 30 s handler timeout `:160`) into a generic over a small duplex trait, with one impl for a QUIC bidi stream and one for a Noise channel. Heartbeat period becomes a parameter (10 s QUIC, 25 s web).
- The desktop is an **outbound** client: no inbound port, router mapping or `remote_address` is needed for the web path. `inner.endpoint` (`host.rs:182`) is generalized into a transport-neutral status so Settings shows "Web app: connected/connecting/offline".
- Identity: `identity_store.rs` gains a second sealed row `web_identity` (X25519 static key, desktop id, desktop token, relay URL) using the same `credentials.key` sealing (`crates/monocode-core/src/mobile/identity_store.rs:1-15`, `accounts/sealed.rs`). The token is never written to `mobile-access.json`.
- `Device` (`host.rs:62-69`) gains `kind` (`phone`/`web`), `permission`, `projects`/`monos` selection, `created_at`, `last_seen`; all `#[serde(default)]` so existing `mobile-access.json` files stay valid (`host.rs:60-99`). Existing phones default to `permission: control` and `kind: phone` and are not subject to web-only denials. **Project and Mono selection is enforced in `MobileHost::handle` for every device, phones included** (security.md section 7, G1): a phone's selection is stored on its row under desktop Settings "Your phones" (default: all projects), replacing the phone-only filter in `mobile/src/features/connection/selection.ts`.
- Hardening hooks live where security.md section 7 places them: `MobileHost::handle` (`host.rs:640-730`) for permission, selection and audit; `mobile_session_command` and `mobile_start` (`mobile_host.rs:1400-1405, 1548-1553`) and `models.list` (`mobile_host.rs:1864`) for `runtimeMode`; `project.rs` `inside_root` and the file functions (`crates/monocode-gpui/src/mobile/project.rs:79-170`) for the deny-list.

### 6.2 Settings > Connections > "Web app"

A new card below "Your phones" (section header at `crates/monocode-gpui/src/settings/pages/mobile.rs:408`, inside `mobile_page`, which starts at `:321`), implemented in `pages/web_app.rs` and reusing `SettingsView::row`, `Input::field`, `secondary_button`, `toggle` (`settings/widgets.rs`), the shared modal (`crates/monocode-gpui/src/modal.rs`), the QR canvas (`pages/mobile.rs:1167`) and the popover choice select (`popover.rs`).

```
Web app                                            [Beta]
  Host URL  [ https://app.monoptah.example        ]   (default: official host)
  Status: Connected to account "alice" (relay online)        [ Unpair ]
  [ Pair ]    -> modal: QR (384 px) + "Copy link" + "Open in browser" + user code + confirmation code
  Devices
   MacBook Safari   fp a1b2 c3d4 e5f6 ...   Control v   last seen 2m   [ Revoke ]
   iPhone PWA       fp 99aa 10bc ...        Read-only v last seen 1h   [ Revoke ]
   Pending: "Pixel Chrome"  fp 5c6d ...     Projects: [x] api [x] web   [ Confirm ] [ Deny ]
  Remote push in Git: [off]   Activity log: [ Open ]
```

States: unpaired (host URL + Pair), pairing (modal with countdown 10 min), paired (account, devices), error banner (token invalid, host unreachable). New `SettingsEvent::WebPair`, `WebCancelPair`, `WebUnpair`, `WebConfirmDevice`, `WebDenyDevice`, `WebSetPermission`, `WebRevokeDevice` next to the Mobile events (`settings/mod.rs:145-155`) with a `web::Status` snapshot in core like `mobile::Status` (`host.rs:102-115`). The page keeps the existing 1 s refresh pattern (`pages/mobile.rs:118-148`). A pending device also raises a desktop notification and a badge on the Connections section, because the request can arrive while Settings is closed.

### 6.3 Device confirmation

A confirmation dialog (modal.rs) shows: device name (as sent, untrusted), the device fingerprint **and this desktop's own fingerprint**, each as the first 32 hex characters (128 bits) in 8 groups of 4, account handle, desktop-side project selection checkboxes (default: all rail projects checked, Monos unchecked), permission choice (default Read-only), and "Confirm" / "Deny". The web app shows the device fingerprint under the device (`/desktops/:id`) and, on the pending screen, both the device fingerprint and the pinned desktop fingerprint, so the user compares both pairs with the dialog; a mismatch of the desktop fingerprint means the directory returned a substituted key (security.md T3). Confirm writes the device into `config.devices` (`mobile-access.json`), adds the fingerprint to the Gate, reports `POST /api/desktop/grants/:deviceId/state` and sends the encrypted `state: confirmed` message.

### 6.4 Pairing flow (device authorization style)

```
 Desktop                         Worker (/api)                         Browser (phone or computer)
    | generate X25519 key (once), verifier=32 random bytes, challenge=SHA-256(verifier)
    |--POST /api/pair/start------->|
    |  {publicKey, machineName, os, appVersion, challenge}
    |                              | insert pairing_requests(pending, 10 min)
    |<-{userCode, deviceCode, confirmationCode, expiresIn:600, interval:3}
    | show QR + link: https://HOST/pair/WXYZ-4829#fp=<full hex fingerprint>   and confirmationCode
    |                              |                                     | scan QR or open link
    |                              |                                     | /pair/WXYZ-4829 (login if needed)
    |                              |<--GET /api/pair/WXYZ-4829 (session)-|
    |                              |--{machineName, os, fingerprint, confirmationCode}-->|
    |                              |                                     | SPA compares fingerprint with #fp (not sent to the server)
    |                              |                                     | user compares confirmationCode with the desktop screen
    |                              |<--POST /api/pair/WXYZ-4829/approve (fresh passkey re-auth)--|
    |                              | create desktops row, desktop_tokens(scope relay push grants), status=approved
    |--POST /api/pair/poll (every 3 s) {deviceCode, verifier}-------->|
    |<-{status:"pending"} ...                                          |
    |<-{status:"approved", desktopId, desktopToken, account:{handle}, relayUrl}  (once; row -> consumed)
    | seal token in web_identity; connect wss /relay/desktop/:desktopId with Bearer token
    | UI: "Connected to alice"
```

Rules: `userCode` is 8 chars from an alphabet without 0/O/1/I/L, single use, 10 min; poll rate beyond `interval` returns `{status:"slow_down"}`; `poll` verifies `SHA-256(verifier) == challenge` and `SHA-256(deviceCode) == device_code_hash`, so neither the QR nor the user code alone can collect the token; the token is returned exactly once. The `#fp=` fragment lets the SPA check that the server-delivered public key matches what the desktop displayed (a malicious host cannot swap keys silently; the QR channel is out of band from the server). The fragment never reaches the server; the SPA saves it to `sessionStorage` before any `/login?redirect=` hop and restores it afterwards. If `#fp` is missing (link typed or copied without it) the page shows a warning that the desktop key cannot be checked and the user must compare the fingerprint shown there with the desktop screen. The UI always displays the first 32 hex characters (128 bits) as 8 groups of 4. Rate limits: `pair/start` 5 per IP per 10 min and 3 outstanding per IP; `pair/poll` per `deviceCode`.

Device-to-desktop confirmation (first connection of a browser):

```
 Browser                      Worker / DO                       Desktop
   | POST /api/devices {name, publicKey} -> deviceId
   | POST /api/desktops/:id/grants {deviceId}  (state=pending) -->| CTRL {"t":"grants"}  --> desktop GET /api/desktop/grants
   | WSS /relay/device/:id (cookie) ==================== frames ====>
   | HS1 (e, es, s, ss, payload{name}) ------------------------------>| peer=SHA-256(s); unknown -> dialog name+fingerprint
   |<-------------------------------------------------- HS2 + {"type":"state","state":"pending"}
   |                                                                 | user Confirms (permission, projects)
   |<--------------------------------- {"type":"state","state":"confirmed","permission":"read"}
   |                                                                 | POST /api/desktop/grants/:deviceId/state
   | requests / watches                                              |
```

### 6.5 Push hook

Where `notify_mobile` posts to Expo today (`crates/monocode-core/src/mobile/notify.rs:7`, called from `mobile_host.rs:1635-1711`), the web path adds `POST /api/push/notify` with the desktop token and minimal metadata (section 8). Expo remains for phones. Per-device category preferences move to the account (`push_prefs`), not to the desktop. The desktop sends nothing when no web account is paired or the relay path is disabled.

## 7. Desktop link: gates and permissions (reference)

### 7.1 Permission classification

| Class | Methods |
|---|---|
| read | `conversation.read`, `conversation.older`, `conversation.watch`, `models.list`, `accounts.usage` (cached usage, no `refresh`), `device.selection` (get), `workspace.list`, `sessions.list`, `project.changes`, `project.worktrees`, `project.files`, `project.file`, `project.search`, `project.fileHighlight`, `project.git`, `mono.document`, `mono.profile`, `mono.file`, `device.leave` |
| control | `session.configure`, `session.start`, `conversation.send`, `session.resumeUsage`, `session.resumeUsageAtReset`, `session.answerQuestion`, `session.keepQuestionOpen`, `session.cancel`, `session.rename`, `session.archive`, `session.delete`, `accounts.usage` with `refresh: true`, `project.gitAction` with `stage`, `unstage`, `generate` or a `commit` that does not push, `device.selection` (set; may only narrow, widening needs desktop confirmation), `mono.create`, `mono.update` |
| denied for web in v1 | `mono.saveFile`, `mono.reset`, `mono.delete`; `project.gitAction` `sync`, `pr`, and `commit` with `push: true` or `createPr: true` (the only ways the desktop pushes, `crates/monocode-gpui/src/mobile/project.rs:435-457`), unless the desktop toggle "Remote push in Git" is on (default off); `runtimeMode: "full-access"` in `session.configure`/`session.start` (denied for every remote device, phones included) |

`read` devices get `{ok:false, code:"read_only"}` for control methods. Details and rationale in security.md section 7.

### 7.2 Grants sync

The desktop is the authority for `state` and `permission`. The server mirrors them in `device_grants` for the UI. A server-side revoke blocks the relay join immediately and sends `CTRL {"t":"grants"}`; the desktop re-reads `GET /api/desktop/grants` and removes the device from its Gate. A permission change in the web UI sets `requested_permission`; the desktop applies a lowering automatically and a raise only after local confirmation.

## 8. Web Push flow

```
 Desktop                Worker                           Push service            PWA service worker
   | POST /api/push/notify (Bearer token)  {category, project, status, sessionId}
   |----------------------->| check token scope push, rate limit 30/min
   |                        | owner = desktops.user_id; push_prefs enabled for category
   |                        | for each push_subscriptions row of owner:
   |                        |   encrypt (RFC 8291 aes128gcm) + VAPID JWT (ES256, RFC 8292)
   |                        |-- POST endpoint (TTL 86400, Urgency high|normal, Topic) -->|
   |                        |<-- 201 / 404 / 410 (delete row) / 429 (backoff) ---------|
   |                        |                                                          |-- push event
   |                        |                                                          | showNotification(...)
   |                                                                                  click -> open /d/:desktopId/s/:sessionId
```

- **Subscribe**: after install, on a button click, the app calls `Notification.requestPermission()`, then `registration.pushManager.subscribe({userVisibleOnly: true, applicationServerKey: VAPID_PUBLIC_KEY})` and `POST /api/push/subscriptions`. Never on page load. iOS requires a Home Screen web app and a user gesture (https://firt.dev/notes/pwa-ios/ and https://pushpad.xyz/blog/ios-special-requirements-for-web-push-notifications, read 2026-11); minimum iOS 16.4 (secondary sources).
- **Notify request** (`POST /api/push/notify`): `{category: "agentFinished"|"agentInput"|"monoMessages", project: "<display name>", status: "finished"|"needs_input"|"message", sessionId, desktopName?}`. No title, no assistant text, no transcript: this closes gap G8 (security.md section 7) for the web path.
- **Sent payload** (JSON, under 1 KiB, encrypted end to end to the browser): `{"v":1,"t":"<desktop name>","b":"<project>: Finished" | "<project>: Needs your input" | "<project>: New message","url":"/d/<desktopId>/s/<sessionId>","tag":"<sessionId>"}`. Headers: `TTL: 86400`, `Urgency: high` for `agentInput` and `normal` otherwise, `Topic`: first 32 url-safe base64 chars of SHA-256(sessionId) so repeated notices for one session collapse.
- **When** the desktop sends mirrors the mobile rules (`mobile_host.rs:1643-1655`: only for sessions a remote device started, not while the desktop window shows that session). Account-level preferences are applied by the Worker.
- **Sender**: WebCrypto only (ECDH P-256, HKDF-SHA-256, AES-128-GCM, ES256 JWT), an in-house sender of about 150 lines in `worker/src/push/`; `web-push` (Node) does not run on Workers (https://github.com/web-push-libs/web-push/issues/718, read 2026-11). Cross-check against a test vector and a real Chrome and Safari endpoint in the spike.
- **Cleanup**: `404` and `410` delete the subscription; `failures` increments on other errors and the row is dropped after 5 consecutive failures; `429` honours `Retry-After`.
- **iOS**: every push must display a notification (silent pushes get the subscription revoked); badge and sound rely on the OS. Declarative Web Push (iOS 18.4+) is a later optimization, not used in v1. Because background WebSockets are dropped, push is the only background channel; opening the notification triggers the resync of section 3.3.
- **Click**: the SW opens `/d/:desktopId/s/:sessionId`; the link handshake then loads the transcript. If the session no longer exists the app shows the project list.

## 9. Self-host guide (outline)

1. Prerequisites: Cloudflare account, Node 22, pnpm, `wrangler` (4.36 or later), a domain on Cloudflare.
2. `git clone` the repo, `cd web && pnpm install`.
3. `wrangler login`.
4. `wrangler d1 create monoptah`, copy the `database_id` into `wrangler.jsonc`.
5. `pnpm --filter worker db:migrate:remote` (runs `wrangler d1 migrations apply monoptah --remote`).
6. Generate VAPID keys: `pnpm --filter worker vapid:generate` prints a public key and a private key (P-256, base64url). Put the public key and `VAPID_SUBJECT` in `vars`; `wrangler secret put VAPID_PRIVATE_KEY`.
7. `wrangler secret put RECOVERY_PEPPER` (32 random bytes, base64; losing it invalidates stored recovery codes, so back it up) and `wrangler secret put AUDIT_IP_SALT` (32 random bytes, base64).
8. Set `RP_ID` and `RP_ORIGIN` to the final hostname (passkeys are bound to `RP_ID`; changing it breaks every passkey).
9. Custom domain: add a `routes` entry with `custom_domain: true`, or attach in the dashboard; HTTPS is mandatory.
10. `pnpm build && wrangler deploy`.
11. First admin: `pnpm --filter worker bootstrap -- --remote` creates a `bootstrap` invite (24 h, one use, grants `role = admin`) and prints the code once; open `/register?invite=CODE`, create a passkey, save the recovery codes.
12. In Monoptah Native set the Host URL (Settings > Connections > Web app) and Pair.
13. Update: `git pull`, `pnpm install`, apply new migrations (`db:migrate:remote`), `pnpm build && wrangler deploy`. Every deploy disconnects all relay sockets; clients reconnect with backoff (section 3.3). Migrations are forward-only and backward compatible for one release so a rollback of the Worker works.
14. Backups: D1 Time Travel is on by default; export with `wrangler d1 export` before risky migrations.
15. Docker is out of scope.
