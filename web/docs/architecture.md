# Monoptah web app: architecture

Scope: frontend, Worker, relay, protocol, Web Push, desktop changes, self-hosting. Security reasoning lives in [security.md](security.md); both documents use the same route, table, token and frame names. Paths such as `crates/monocode-core/src/mobile/host.rs:640` refer to the repo as researched on 2026-11-17.

## 1. System overview

```
 Browser (PWA, React CSR)                 Cloudflare (ONE Worker)                    Monoptah Native (desktop)
 +----------------------+      HTTPS     +--------------------------------+          +-------------------------+
 | app shell (SW cache) |--------------->| Assets (app/dist/client, SPA)  |          |  MobileHost (unchanged) |
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

Reuse rule: code taken from the old Tauri React app (`src/`) is copied into `web/app` or `web/protocol` (provenance comment, license header kept, branding fixed, Tauri APIs removed) and never imported from `src/` or `src-tauri/`; no `web/` config may reference those directories (CI guard, roadmap.md W-20). Inventory: [tauri-reuse.md](tauri-reuse.md).

`web/protocol` is the single source for frame constants; the Rust side mirrors them in `crates/monocode-core/src/mobile/web_link/frame.rs` and a shared test-vector file (`web/protocol/vectors.json`) is read by both test suites.

## 3. Frontend

### 3.1 Route tree (TanStack Router, file-based, CSR)

| Route | Auth | Purpose |
|---|---|---|
| `/` | session | Restores the last place (last used desktop, project, session) and redirects there, else `/desktops` |
| `/login` | public | Passkey sign-in (discoverable credential, hybrid QR offered by the browser), link to `/recover` |
| `/register#invite=CODE` | public | Invite code (read from the URL fragment, so it stays out of logs and history), immutable handle, display name, create passkey, one-time recovery codes screen with "I saved these", then a prompt to add a second passkey |
| `/recover` | public | Handle + recovery code, then forced "add a new passkey" |
| `/pair/:code` | session (login redirect keeps the code) | Approve a desktop: machine name, fingerprint, confirmation code (section 6.4); if the account already has desktops, an option "This replaces <desktop>" (fresh re-auth). The `#fp` fragment is saved to `sessionStorage` before any redirect to `/login?redirect=` and restored after sign-in; without a `#fp` the page shows a warning that the desktop key cannot be checked |
| `/desktops` | session | List: online status, rename, color, pending confirmations badge |
| `/desktops/:id` | session | Manage one desktop: rename, color, devices and grants, per-device permission (lower now, request raise), revoke device, "add another device" link, revoke desktop |
| `/desktops/:id/verify#fp=<full hex>` | session | Re-pin after "Desktop key changed": compares the directory key of `GET /api/desktops/:id` with `#fp` (the link or QR shown in desktop Settings > Web app > "Verify on another device"); on match it replaces the pin, on mismatch it blocks. There is no "trust new key" button |
| `/d/:desktopId` | session + link | Home for one desktop: projects and recent sessions (catalog) |
| `/d/:desktopId/p/:projectId` | session + link | Project: sessions list (`sessions.list` paging) |
| `/d/:desktopId/p/:projectId/files/$` | session + link | File browser and viewer (splat path) |
| `/d/:desktopId/p/:projectId/new` | session + link | New session in a project (model, mode, first prompt) |
| `/d/:desktopId/p/:projectId/changes/$` | session + link | Working-tree diff (splat path selects a file) |
| `/d/:desktopId/p/:projectId/git` | session + link | Git status and actions |
| `/d/:desktopId/s/:sessionId` | session + link | Transcript and composer. Project is resolved from the catalog, so push links need only desktop and session |
| `/d/:desktopId/monos/:monoId` | session + link | Mono chat |
| `/d/:desktopId/monos/:monoId/profile` | session + link | Mono profile; children: `/edit`, `/habit/:id`, `/documents`, `/file/:name` |
| `/d/:desktopId/cloud` | session + link | Cloud instance list: state, sessions per instance, Stop and Resume (U-17) |
| `/d/:desktopId/settings` | session + link | Per-desktop: model defaults, project selection view, notification categories, Reconnect row |
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

`web/app/src/link/` contains: `relay.ts` (WebSocket), `noise.ts` (Noise IK on @noble, copied from `web/mock-desktop/src`), `session.ts` (request/response, watches, chunking), `connection.ts` (connect sequence, reconnects, one per desktop), `keys.ts` and `pins.ts` (key storage, section 3.4), `store.ts` (Zustand). `web/app/src/data/` holds the TanStack Query layer over it and `web/app/src/version/` the capability and version handling.

Connect sequence:

1. `GET /api/desktops/:id` returns `{publicKey, fingerprint, name}` (the directory). The device pins `publicKey` in IndexedDB on first success; a later change blocks the link with a "desktop key changed" screen.
2. Open `wss://HOST/relay/device/:desktopId?device=<deviceId>` (cookie authenticates; section 4.4).
3. Handshake: send relay frame `HS1` with Noise message 1; receive `HS2`; both sides now hold transport CipherStates (section 5.3).
4. First encrypted message from the desktop is `{"type":"state","state":"pending"|"confirmed"|"revoked","permission":"read"|"control","proto":1,"capabilities":[...],"desktop":{"name","version"}}`. `capabilities` is a string list (for example `approvals`, `cloud`, `cloudStart`, `cloudMove`, `gitPush`, `monoTools`, `monoFiles`): the web hides what is not advertised and shows "Update Monoptah on your desktop to use <feature>"; a `proto` mismatch shows a blocking screen (features.md N-23). A permission or selection change sends a new `state` message to every live channel of that device, and so does a change of a desktop toggle that maps to a capability ("Remote cloud start" to `cloudStart`, "Remote push in Git" to `gitPush`, and `cloudMove`, which needs both): the new `capabilities` go to every live channel of every device. In `pending` the store shows "Confirm this device on the desktop" with the device fingerprint and the pinned desktop fingerprint (the user compares both with the desktop dialog, section 6.3); requests are not sent. `confirmed` moves to `ready`.
5. Requests: allocate `rid` (u32, per session), send JSON envelope, resolve on matching `rid` response, timeout 35 s (desktop handler 30 s, `crates/mobile-link/src/transport.rs:160`). Watches: send with `"watch":true`, expect `{rid, ok:true, watching:true}`, then frames; `{rid, ok:true, heartbeat:true}` every 25 s; the client treats 75 s of silence as dead.
6. Keepalive: client sends WebSocket text `ping` every 30 s; the DO auto-responds `pong` without waking (section 4.4). It also keeps proxies from idling the socket out.

Resync and reconnect:

- `visibilitychange` to `hidden`: do nothing (iOS may kill the socket; do not fight it).
- To `visible`: if the socket is not open or `lastFrameAt` is older than 75 s, reconnect immediately and reopen every watch that was open (each with `from`); then invalidate the catalog query and refetch the current conversation with `conversation.read` as the fallback the mobile app uses (`WorkspaceProvider.tsx:391-394`).
- A browser cannot read the HTTP status of a refused WebSocket upgrade, so a relay join without a grant surfaces as close `1006` and is retried with backoff like a network error, not as a block. A `BYE` before any `state` is not authenticated, so it never means "revoked": it blocks the link as `desktop_refused` ("This desktop did not accept this device"), sets no `revoked` flag and wipes no cache, and a user Reconnect or the next time the tab becomes visible tries once more. A `BYE`, `HS2` or `TRANSPORT` frame that the relay connection buffered before this session sent its HS1 belongs to an earlier session on the same channel and is dropped, so it cannot end the new handshake. A relay `ERROR` before any `state` does not block: the link retries with the normal backoff (an error code of `desktop_offline` also sets presence to offline), with `unsupported_version` as the only exception (below). `revoked` and the cache wipe come only from an encrypted `state: "revoked"` or a relay close `4003`. The relay `unsupported_version` error means the desktop refused an older web prologue (`web/protocol/src/errors.ts`), so the web app is the outdated side and the link blocks with "Reload to update the web app"; "Update Monoptah on your desktop" is shown when the desktop's `proto` (HS2 payload or `state`) is lower than the web app's `PROTOCOL_VERSION`, and a higher desktop `proto` shows "Reload to update the web app".
- Socket close: backoff `min(30 s, 0.5 s * 2^n) * random(0.5..1)`, reset after 10 s of `ready`. While `document.hidden` no reconnect is attempted; it happens on the next `visible`. `navigator.onLine === false` pauses attempts until the `online` event.
- Close codes (section 4.5) decide behaviour: `4003` stops reconnecting and shows "Access revoked" (sets `revoked`, wipes the cache); `4011` stops reconnecting and shows the blocked screen (`account_blocked`; no `revoked`, no cache wipe); `4009` replaced (another tab or reconnect) retries once; `4010`/`4008` back off 60 s.
- Handshake failure after reconnect always starts a fresh Noise session; Noise sessions are never resumed.
- A desktop restart ends every session with `BYE`: devices re-handshake and reopen their watches. The client never auto-resends a mutation after a reconnect: it re-reads state (`conversation.read`), and the optimistic turn reconciles or shows "Not sent, retry".
- The pending screen of a device survives reconnects (the grant is server state, section 7.2).

### 3.3a Reusing the Tauri React code

Frontend code from `src/` is copied, not imported (rule above and tauri-reuse.md section 1). Consequences for the frontend: the markdown renderer is the AST port of `mobile/src/features/sessions/markdownParser.ts` (security.md section 8), not the streamdown stack; transcript projection is not copied because the desktop sends projected turns; copied code blocks render shiki `codeToTokens` output as React elements; `web/app` ESLint and `tsconfig` never list `../src`; new localStorage keys use the `monoptah.` prefix.

### 3.4 Key storage

- The device static X25519 keypair is generated with `@noble/curves` (raw 32-byte keys). WebCrypto X25519 is not used: Safari support is unverified and a non-extractable X25519 `CryptoKey` read back as null from IndexedDB on Safari 26.4 (https://bugs.webkit.org/show_bug.cgi?id=312279, read 2026-11).
- Because noble needs raw bytes, the private key cannot be a non-extractable `CryptoKey`. Instead: a random **AES-GCM-256 `CryptoKey` created with `extractable: false`** is stored in IndexedDB (AES keys survive structured clone), and the X25519 private key is stored only as AES-GCM ciphertext (random 96-bit IV, AAD = a local `keyId` generated with the key, because the key is created before the server assigns the `deviceId`, which is stored beside it once known) in the same database. At runtime the key is unwrapped once into a module-scoped `Uint8Array` and zeroed on `pagehide`.
- Creating the key and registering the device run under the Web Lock `monoptah-device-key`, so two tabs never create two keys or register one key under two ids; without Web Locks the AES key and the key record are written in one IndexedDB transaction with `add`, and a tab that loses the race reads what the winner stored. Binding the server `deviceId` checks that the stored key is the one that was registered.
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
    "directory": "app/dist/client",
    "binding": "ASSETS",
    "not_found_handling": "single-page-application",
    "run_worker_first": ["/api/*", "/relay/*"]
  },
  "durable_objects": { "bindings": [{ "name": "ROOM", "class_name": "DesktopRoom" }] },
  "migrations": [{ "tag": "v1", "new_sqlite_classes": ["DesktopRoom"] }],
  "d1_databases": [{ "binding": "DB", "database_name": "monoptah", "database_id": "<id>", "migrations_dir": "worker/migrations" }],
  "vars": { "RP_ID": "zptah.app", "RP_ORIGIN": "https://zptah.app", "VAPID_PUBLIC_KEY": "<b64url>", "VAPID_SUBJECT": "mailto:admin@zptah.app", "MAX_USERS": "150" }
}
```

Secrets (never in `vars`): `VAPID_PRIVATE_KEY`, `RECOVERY_PEPPER`, `AUDIT_IP_SALT` (salt for the IP hashes in `audit_log` and `pairing_requests`). Dev uses `@cloudflare/vite-plugin` so SPA, Worker, DO and a local D1 run in one `vite dev`.

Middleware order for `/api/*`: security headers, then, **only for cookie-authenticated routes** (routes marked session, fresh or admin, plus the public browser auth routes under `/api/auth/`), the `Origin` check on non-GET (must equal `RP_ORIGIN`) and the `X-Monoptah-CSRF: 1` check on non-GET; then rate limit, auth resolver per route, handler. Token routes (`/api/desktop/*`, `/api/push/notify`) and the desktop pairing routes (`/api/pair/start`, `/api/pair/poll`) are called by the desktop app, carry no `Origin` and no cookie, and ignore cookies entirely (a cookie on these requests is never read). Rate limits: D1 `rate_limits` for the low-volume auth, pairing and notify groups; the general per-session limit (120 per minute) is an in-memory per-isolate counter, best effort, so it costs no D1 writes (security.md 3.5). Errors are `{error, code}` JSON.

### 4.2 Hono route table

Auth legend: **public**; **session** (valid cookie session); **fresh** (session plus passkey re-auth within the last 5 min, `sessions.reauth_at`); **token** (desktop bearer token with the stated scope); **admin** (session, `users.role = 'admin'`, fresh).

| Method and path | Auth | Purpose |
|---|---|---|
| `GET /api/health` | public | Liveness probe for monitoring (`{ok: true}`, no D1 read) |
| `POST /api/auth/register/options` | public, rate-limited | Validate invite, handle (immutable) and displayName; return WebAuthn creation options (`userVerification: required`, `residentKey: required`). Fails with the same generic error as a bad invite while `invites_paused` is set or the user count has reached `MAX_USERS` (section 4.4) |
| `POST /api/auth/register/verify` | public | Verify attestation, create user, passkey, 10 recovery codes (returned once), session. Re-checks the pause flag and `MAX_USERS` in the same batch that consumes the invite (generic error on failure) |
| `POST /api/auth/login/options` | public, rate-limited | Discoverable-credential request options |
| `POST /api/auth/login/verify` | public | Verify assertion, check counter, create session |
| `POST /api/auth/reauth/options`, `POST /api/auth/reauth/verify` | session | Fresh assertion for sensitive actions |
| `POST /api/auth/recover` | public, rate-limited | `{handle, code}`; creates a `recovery_only` session; consumes the code; device keys, desktops, grants and invite count are unchanged |
| `POST /api/auth/logout` | session | Delete current session row and call `room.revoke` for that session's sockets (section 4.4) |
| `GET /api/me` | session | User, role, invites left, whether `recovery_only`, `recoveryAckAt` (null forces a regenerate at sign-in) |
| `PATCH /api/me` | session | Change display name |
| `DELETE /api/me` | fresh | Delete the account and all its rows (security.md section 10) |
| `GET /api/audit` | session | Own audit events (paged), for the account activity view |
| `GET /api/sessions`, `DELETE /api/sessions/:id` | session | List and revoke own sessions |
| `GET /api/passkeys` | session | List |
| `POST /api/passkeys/options`, `POST /api/passkeys/verify` | fresh (or `recovery_only`) | Add a passkey |
| `PATCH /api/passkeys/:id`, `DELETE /api/passkeys/:id` | fresh | Rename; delete (never the last one) |
| `POST /api/recovery-codes/regenerate` | fresh | Replace all codes, return 10 new ones once |
| `GET /api/invites`, `POST /api/invites`, `DELETE /api/invites/:id` | session | List own; create (quota; `code: invites_paused` while paused or at `MAX_USERS`); revoke unused |
| `POST /api/pair/start` | public, rate-limited per IP | Desktop starts device authorization (section 6.4) |
| `POST /api/pair/poll` | public (secret `device_code` + `verifier`) | Desktop polls; on approval receives its token once. A wrong verifier is 403, an unknown device code 404, a poll sooner than the 3 s per-device-code middleware window gets `429 rate_limited` with `Retry-After` (the desktop treats it like `slow_down`); `slow_down` only appears across a window boundary, when two polls land within 2 s |
| `GET /api/pair/:userCode` | session | Show machine name, fingerprint, confirmation code |
| `POST /api/pair/:userCode/approve` | fresh | Bind desktop key to the user (new desktop under `MAX_DESKTOPS_PER_USER`, else `409 quota_exceeded`); the token is minted later by `poll`. Optional `{replaces: desktopId}` re-pairs an existing desktop (section 6.7) |
| `POST /api/pair/:userCode/deny` | session | Deny |
| `GET /api/desktops` | session | List with `online` (DO presence), name, color, pending count |
| `GET /api/desktops/:id` | session (owner) | Directory entry incl. `publicKey`, `fingerprint` |
| `PATCH /api/desktops/:id` | session (owner) | Rename, color |
| `DELETE /api/desktops/:id` | fresh (owner) | Revoke desktop: tokens revoked, grants revoked, room sockets closed `4003` |
| `POST /api/devices` | session | Register this browser: `{name, publicKey}` returns `deviceId` |
| `GET /api/devices`, `PATCH /api/devices/:id`, `DELETE /api/devices/:id` | session | List, rename, forget (revokes all grants) |
| `POST /api/desktops/:id/grants` | session (owner) | Request access of a device to a desktop (creates `pending` grant, notifies desktop; an existing `pending` or `confirmed` grant is returned unchanged, a `revoked` one is `409 conflict`) |
| `GET /api/desktops/:id/grants` | session (owner) | Grants with state, fingerprint, permission |
| `PUT /api/desktops/:id/grants/:deviceId` | session (owner); fresh to revoke | Set `requestedPermission` (the server never writes `permission` or `confirmed`), or `state: "revoked"` (server blocks the relay join at once; `revoked` wins when both are sent) |
| `GET /api/desktop/me` | token `relay` | Desktop identity and account handle |
| `DELETE /api/desktop/me` | token `relay` | Unpair: the desktop revokes its own tokens and the room closes its sockets; the desktop row stays (section 6.7) |
| `GET /api/desktop/grants` | token `grants` | Grants plus device public keys for this desktop (desktop applies them, section 7.5) |
| `POST /api/desktop/grants/:deviceId/state` | token `grants` | Desktop reports `confirmed` (permission required), `pending`, `revoked`, and the permission it applied; `revoked` calls `room.revoke` for that device. A grant the server revoked stays revoked (`409 conflict`; reporting `revoked` for it is accepted) |
| `GET /api/push/vapid-key` | public | VAPID public key |
| `POST /api/push/subscriptions` | session | Store `{endpoint, keys, deviceId, vapidKeyId}` |
| `DELETE /api/push/subscriptions/:id` | session | Remove |
| `PUT /api/push/prefs/:desktopId` | session (owner) | Enable and categories per desktop |
| `POST /api/push/notify` | token `push` | Desktop asks the Worker to push to the listed devices (section 8) |
| `GET /api/admin/users` | admin | Users, status, invite tree position, desktop counts |
| `POST /api/admin/users/:id/block` | admin | `{cascade: boolean, reason}` |
| `POST /api/admin/users/:id/unblock` | admin | Unblock one user (never cascades) |
| `POST /api/admin/users/:id/invites` | admin | Grant extra invites |
| `GET /api/admin/settings`, `PUT /api/admin/settings` | admin | `{invitesPaused, maxUsers, userCount}`; `PUT {invitesPaused: boolean}` is the "Pause invites" switch (audited) |
| `GET /api/admin/audit` | admin | Paged audit log |
| `GET /relay/desktop/:desktopId` | token `relay` | WebSocket upgrade for the desktop (section 4.4) |
| `GET /relay/device/:desktopId?device=ID` | session + device grant | WebSocket upgrade for a device |

### 4.3 D1 schema (Drizzle, `worker/src/db/schema.ts`)

All ids are random UUIDv4 text unless noted. Times are unix seconds (integer). Secrets are stored only as hashes.

| Table | Key columns |
|---|---|
| `users` | `id` PK; `handle` UNIQUE (3-32 chars, lowercase); `display_name`; `role` (`user`/`admin`); `status` (`active`/`blocked`); `invited_by` FK users (null for the bootstrap admin); `invite_id` FK invites; `invites_granted` int (default 3); `blocked_at`; `blocked_reason`; `blocked_via` (user id of the branch root that cascaded, or null); `recovery_ack_at` (set when the user confirms "I saved these"; null at the next sign-in forces a regenerate); `created_at` |
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
| `settings` | `key` PK; `value` (text); `updated_at`; `updated_by` FK users. Row `invites_paused` (`0`/`1`, absent = `0`) is the "Pause invites" switch; written only by `PUT /api/admin/settings` |
| `audit_log` | `id` PK; `ts`; `actor_user_id`; `actor_kind` (`user`/`desktop`/`admin`/`system`); `action`; `target_type`; `target_id`; `meta` JSON (no secrets, no content) |
| `rate_limits` | PK (`key`, `window_start`); `count`. Used for auth, pairing and notify only; relay limits live in the DO and the general per-session API limit is in memory per isolate |

Indexes: `passkeys(user_id)`, `sessions(user_id)`, `sessions(idle_expires_at)`, `invites(created_by)`, `users(invited_by)` (branch walks), `desktops(user_id)`, `devices(user_id)`, `device_grants(desktop_id)`, `push_subscriptions(user_id)`. Added by later migrations: `audit_log(actor_user_id, ts, id)` (0002, replaces `(actor_user_id, ts)`: the cursor of `GET /api/audit`); 0003: `settings(updated_by)`, `pairing_requests(user_id)`, `pairing_requests(desktop_id)`, `invites(used_by)`, `challenges(user_id)`, `push_prefs(desktop_id)`, `users(invite_id)`, `users(created_at, id)` (the admin user list) and `audit_log(ts, id)` (replaces `(ts)`: the admin audit cursor and the retention cleanup); 0004: `pairing_requests(ip_hash, status, expires_at)` (replaces `(ip_hash)`: "3 outstanding per IP"). A daily cron trigger deletes expired `challenges`, `sessions`, `pairing_requests`, `rate_limits` rows, `pending` `device_grants` older than 7 days, and audit rows older than the retention (security.md section 10).

### 4.4 Durable Object relay: `DesktopRoom`

One room per desktop: `env.ROOM.get(env.ROOM.idFromName(desktopId))`. SQLite-backed class, WebSocket Hibernation API.

Who may join (checked in the Worker before forwarding the upgrade; the DO has no public route):

| Role | Credentials | Checks |
|---|---|---|
| `desktop` | `Authorization: Bearer <desktop token>` on the upgrade request | `desktop_tokens` hash lookup, not revoked, scope `relay`, desktop not revoked, owner not blocked, token's `desktop_id` equals the path id |
| `device` | Session cookie, `Origin` equals `RP_ORIGIN`, query `device=<deviceId>` | Valid unrevoked session (not `recovery_only`), `devices.user_id` equals session user, device not revoked, `desktops.user_id` equals session user, `device_grants` row exists and `state != 'revoked'` (a `pending` grant may join so it can handshake and be confirmed on the desktop) |

A refused upgrade is answered with a WebSocket that is accepted and closed at once, so the peer gets a close code it can act on: desktop 4001 (no, unknown or wrong-scope token, or a token of another desktop), 4003 (revoked token or desktop), 4011 (blocked owner); device 4003 for every failed check (the device query is one indexed lookup and does not say which check failed). Missing `Upgrade: websocket` answers 426 `invalid`; a missing or bad session or Origin answers 401 or 403 before the upgrade. The Worker forwards the upgrade to the DO with internal headers `x-role`, `x-desktop-id`, `x-user-id`, and for devices `x-device-id`, `x-session-id-hash` and `x-grant-state` (`pending` or `confirmed`; a missing or invalid value counts as `pending`; client-supplied copies are stripped). The DO calls `ctx.acceptWebSocket(ws, [tag])` with tag `desktop` or `device:<deviceId>` (lowercase UUID) and `ws.serializeAttachment({role, desktopId, deviceId, userId, sessionIdHash, grantState, joinedAt, channels, sid, gone?})` (well under the 16 KiB limit; `channels` is the capped list of channels a device socket used, `sid` keys the in-memory rate limiter, `gone` marks a socket already closed so no effect runs twice, `grantState` feeds the cap on sockets of pending devices and is refreshed by the RPC `setGrantState(deviceId, state)`, which the Worker calls after the desktop reports `confirmed` or `pending`; it touches only attachments, no D1 and no frames). Nothing else is kept in memory; any state needed after hibernation is in the attachment or DO storage.

Routing of opaque frames (frame layout in section 5.1):

- Device to desktop: handler `webSocketMessage` validates length (20 to 65,555 bytes, binary only), type in `{HS1, TRANSPORT, BYE}`, **overwrites the `deviceId` header field with the attached authenticated id**, forwards to the single `desktop` socket. No desktop connected: replies to that socket with an `ERROR` control frame `desktop_offline`.
- Desktop to device: reads `deviceId` from the header and sends the frame to every socket tagged `device:<deviceId>` (a device may have several tabs; each tab filters on `channel`).
- Relay-originated control frames (type `CTRL`, JSON body, metadata only): to devices `{"t":"presence","desktop":"online"|"offline"}`; to the desktop `{"t":"device","id","event":"joined"|"left"}` and `{"t":"grants"}` (re-sync grants, section 7.5).
- The DO never parses ciphertext and never logs frame bytes.

Presence: online means a live `desktop` socket exists and its last keepalive `ping` arrived within 90 s (read through the auto-response timestamp of the socket, `ctx.getWebSocketAutoResponseTimestamp`). `GET /api/desktops` asks each DO (`room.presence()` RPC, parallel; users hold few desktops) and merges the answer; D1 `desktops.last_seen_at` is written only when the desktop socket closes (not per heartbeat) to spare the 100,000 writes/day free budget. `GET /api/desktops` caches each answer 10 s per isolate (bounded to 1000 entries; dropped when this isolate revokes in that room), so a refresh does not wake every room. A device gets a `presence` CTRL frame when it joins and when the desktop socket closes or is replaced. No timers run in the DO, so no `presence: offline` CTRL is sent when pings stop (ping timeout); a device learns that from `GET /api/desktops` or on its next join, and the offline CTRL is sent only when the desktop socket closes. `desktops.last_seen_at` and `devices.last_seen_at` are rewritten at most once an hour per row (conditional UPDATE).

Per-room quotas (constants in `web/protocol/limits.ts`):

| Quota | Value | Violation |
|---|---|---|
| Desktop sockets | 1 (a new one replaces the old, which closes `4009`) | n/a |
| Device sockets per room | 16; per device id 4; at most 4 sockets of devices whose grant is still `pending` (the Worker passes `x-grant-state`) | new socket closes `4010` |
| Max WebSocket message | 65,555 bytes (20 header + 65,535 Noise) | close `1009` |
| Device inbound rate | 40 msg/s sustained, burst 100, 1 MiB/s (in-memory token bucket, rebuilt after hibernation) | drop then close `4008` after 3 breaches in 10 s |
| Desktop outbound rate | 200 msg/s, 4 MiB/s | same |
| Desktop token calls (Worker, in memory per isolate, keyed by the SHA-256 of the token, no D1 write) | 60 per minute on the token API routes, 12 per minute on the `/relay/desktop/:id` upgrade | API: `429 rate_limited` with `Retry-After`; relay: accepted and closed `4008` |
| Text frames | only exactly `ping` (auto-response) | other text closes `4012` |

Per-account quotas (enforced by the Worker at pairing and device registration; violation returns `quota_exceeded`):

| Quota | Default | Config var |
|---|---|---|
| Desktops per user | 5 | `MAX_DESKTOPS_PER_USER=5` |
| Devices per user | 10 | `MAX_DEVICES_PER_USER=10` |
| Invites per user | 3 | `invites_granted` (admin can grant more) |
| Registered users (whole instance) | official host 150; self-host 0 (off, unlimited) | `MAX_USERS` (a count of `users` rows incl. blocked) |

Self-hosters may change the first two and `MAX_USERS` in `vars`. Capacity control (capacity.md section 4, security.md section 4): when `settings.invites_paused` is `1`, or `MAX_USERS` is above 0 and the `users` count has reached it, `POST /api/invites` answers `code: invites_paused` and registration fails with the generic invalid-invite error (the check runs in the registration batch, so two parallel registrations cannot pass the cap together). Existing users, invites already issued and sign-in are unaffected. The official host `zptah.app` uses the defaults. Domain rule: `RP_ID` is `zptah.app` and must never change (every passkey would break); nothing else may be hosted on `zptah.app` or its subdomains.

Hibernation: no timers or alarms in the DO, so idle rooms are evicted while sockets stay open. `ctx.setWebSocketAutoResponse(new WebSocketRequestResponsePair('ping','pong'))` answers keepalives without waking the object. Because the object may be reconstructed on every message, the constructor does no work beyond reading bindings.

`room.revoke` sends the desktop a `BYE` frame (deviceId header = that device) for each channel a revoked device socket used, before closing the socket with the code, so the desktop drops those Noise sessions; it sends CTRL `device left` when no socket of the device remains. `revoke({all: true})` closes the desktop socket too and skips the BYE frames. The rate limits count a breach per dropped frame (3 within 10 s close `4008`); the desktop socket has a 200 msg/s burst. `presence()` also treats a desktop socket with no `ping` yet as online until 90 s after it joined.

Revocation path: Worker handlers (`DELETE /api/desktops/:id`, `PUT .../grants/:deviceId` with `revoked`, `DELETE /api/devices/:id`, admin block) call an internal DO method `room.revoke({deviceId?}|{sessionIdHash?}|{all:true}, code)` that closes the matching sockets with `4003`, `4011` or (for a revoked session) `4001`. The desktop-side revoke (`POST /api/desktop/grants/:deviceId/state` with `revoked`) calls it too. Logout, "sign out everywhere", recovery and a sign-in that replaces a session cookie call `room.revoke({sessionIdHash}, 4001)` once per revoked session and desktop, spent round-robin across the user's desktops (session 0 on every desktop, then session 1, ...) up to a cap of 40 calls, with a truncation logged as counts. That can exceed the "5 DO calls" this section is meant to have: `RevokeTarget` now supports `{ sessionIdHashes: string[] }` in the DO, so each desktop needs one call (at most 5), but the WS-A caller `closeSessionSockets` still makes one call per session and desktop: switching it over is a pending WS-A change. A desktop `revoked` report on a grant the server already revoked still calls `room.revoke({deviceId}, 4003)` (idempotent, no D1 write). The admin block does not use per-session calls: it calls `room.revoke({all: true}, 4011)` on each live desktop of the branch.

### 4.5 Close codes

| Code | Name | Client reaction |
|---|---|---|
| 1000 | normal | reconnect with backoff |
| 1001 | going away (deploy) | reconnect immediately once |
| 1008 | policy | no retry, show error |
| 1009 | message too big | bug; no retry |
| 4001 | unauthorized (cookie or token invalid) | device: go to `/login`; desktop: stop reconnecting and show "Web app: access revoked, pair again" |
| 4003 | revoked (grant, device, desktop or token) | device: stop; show "Access revoked"; desktop: stop reconnecting and show "Web app: access revoked, pair again" |
| 4008 | rate limited | back off 60 s |
| 4009 | replaced by a newer connection | retry once |
| 4010 | room quota exceeded | back off 60 s |
| 4011 | account blocked | device: stop; show blocked screen; desktop: stop reconnecting and show "Web app: access revoked, pair again" |
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
{ "type": "state", "state": "pending|confirmed|revoked", "permission": "read|control", "proto": 1, "capabilities": ["cloud"], "desktop": { "name": "...", "version": "..." } }
```

The `state` message is re-sent on a permission or selection change and on a capability toggle change (3.3 step 4), so a toggle flipped on the desktop shows or hides the web feature without a reconnect.

The `error` string stays; the optional additive `code` (`busy`, `forbidden`, `read_only`, `not_selected`, `path_denied`, `revoked`, `disabled`, `too_large`, `unsupported`, `unsupported_target`, `starting`, `auto_stopping`, `instance_busy`) gives the web client a stable key (gap G14, security.md section 7). `watch` frame semantics, `from`/`append` UTF-16 offsets and idempotency receipts are unchanged (`crates/monocode-core/src/mobile/watch.rs:65-158`). Rules: `id` required for app methods and at most 128 chars (`host.rs:698-701`); at most 16 pending requests host-wide and 4 watches per device (`host.rs:57-58`); heartbeat period for web is 25 s instead of 10 s to keep DO message billing low (capacity.md).

### 5.6 Versioning

Three independent counters: relay frame `version` (byte 0), Noise prologue string `monoptah-web/N`, and the JSON `"version": 1` already enforced by the host (`host.rs:641`). The `state` message carries `proto`; the client refuses a desktop whose `proto` is higher than it knows and shows the blocking screen "Reload to update the web app" (the SW update prompt), and a desktop refuses an older `HS1` prologue with `ERROR`/`BYE` and a visible "Update Monoptah on your desktop" message. Method set discovery is by `capabilities` (N-23), with the error `Unsupported mobile operation` as the fallback, as today.

## 6. Desktop changes (Monoptah Native)

### 6.1 Transport beside QUIC

- New module `crates/monocode-core/src/mobile/web_link/` (`mod.rs`, `relay.rs` outbound WebSocket client with reconnect, `noise.rs` using `snow`, `session.rs` per-device sessions, `frame.rs`). It runs as a task that starts independently of the QUIC bind: today `MobileHost::handle` and `watch` reject everything when the QUIC `config.enabled` is false (`crates/monocode-core/src/mobile/host.rs:651`) and `start` is tied to the QUIC bind (`host.rs:512-537`). The spec adds a separate `web_enabled` flag, gates per transport, and the port-mapping router does not run when only the web path is on. QUIC for phones is unchanged and may be on or off independently.
- **The seam**: `serve` receives two closures bound to `MobileHost::handle` (`host.rs:640`) and `MobileHost::watch` (`host.rs:734`) (`host.rs:543-554`, signature at `transport.rs:91-97`). The web transport calls the same two closures with `peer = fingerprint hex of the device's Noise static key`, so allowlist, dedup (`mobile_command`), backpressure and `WatchSink` work as is. **Device identity in core**: `Request` (`host.rs:125`) and `WatchStart` (`host.rs:141`) carry no device identity today. Both gain `device: DeviceContext {id, kind, permission, projects, monos}`; the permission class and the web-only denials are checked in core (`handle` and `watch`); selection by `projectId` or `monoId` is checked in core; `conversationId` to project resolution happens in the app (`mobile_target`, `crates/monocode-gpui/src/shell/mobile_host.rs:632`) using the `DeviceContext`. **Ordering**: on the web channel mutations are processed in order per channel (QUIC serves one request at a time, `crates/mobile-link/src/transport.rs:158`); reads may run concurrently. **Web-only answers**: `workspace.list` strips `addresses` (`host.rs:677`) for web devices; `device.push` and `project.fileHighlight` answer `code: unsupported`; the desktop "Your phones" list and the Expo `push_tokens` filter by `kind: phone`.
- **Core-answered methods**: `workspace.list`, `sessions.list`, `device.leave` and `device.push` are answered inside `MobileHost::handle` from `inner.catalog` and `inner.config` (`host.rs:640-700`) and never reach the app, so the pre-call `policy::authorize` cannot filter their replies. `handle` therefore also applies a reply filter to the two catalog replies (selection, then the cloud scope: cloud and remote sessions and the new fields are stripped for QUIC phones, which keep today's lists), answers `device.selection` through the policy module (the host persists the validated narrower selection), and closes every open watch of a device when its config changes (selection, permission, revoke); the client reopens with `from` and the app-side check runs again (the core `Watcher` holds only `peer` and `sink`). Seam names: workstreams.md 3.2.
- **Refactor**: extract the post-handshake logic of `serve` (response wrapping `transport.rs:165-168`, watch accept frame `:202-211`, heartbeat `:217-225`, Gate recheck before each request and frame `:124-139, 228-236`, 30 s handler timeout `:160`) into a generic over a small duplex trait, with one impl for a QUIC bidi stream and one for a Noise channel. Heartbeat period becomes a parameter (10 s QUIC, 25 s web).
- The desktop is an **outbound** client: no inbound port, router mapping or `remote_address` is needed for the web path. `inner.endpoint` (`host.rs:182`) is generalized into a transport-neutral status so Settings shows "Web app: connected/connecting/offline".
- Identity: `identity_store.rs` gains a second sealed row `web_identity` (X25519 static key, desktop id, desktop token, relay URL) using the same `credentials.key` sealing (`crates/monocode-core/src/mobile/identity_store.rs:1-15`, `accounts/sealed.rs`). The token is never written to `mobile-access.json`.
- `Device` (`host.rs:62-69`) gains `kind` (`phone`/`web`), `permission`, `projects`/`monos` selection, `created_at`, `last_seen`; all `#[serde(default)]` so existing `mobile-access.json` files stay valid (`host.rs:60-99`). `Device` also stores `server_device_id`: the session is rejected if the relay `deviceId` header does not match the id stored for that fingerprint. Existing phones default to `permission: control` and `kind: phone` and are not subject to web-only denials. **Project and Mono selection is enforced in `MobileHost::handle` for every device, phones included** (security.md section 7, G1): a phone's selection is stored on its row under desktop Settings "Your phones" (default: all projects), replacing the phone-only filter in `mobile/src/features/connection/selection.ts`.
- Hardening hooks live where security.md section 7 places them: `MobileHost::handle` (`host.rs:640-730`) for permission, selection and audit; `mobile_session_command` and `mobile_start` (`mobile_host.rs:1400-1405, 1548-1553`) and `models.list` (`mobile_host.rs:1864`) for `runtimeMode`; `project.rs` `inside_root` and the file functions (`crates/monocode-gpui/src/mobile/project.rs:79-170`) for the deny-list.

### 6.2 Settings > Connections > "Web app"

A new card below "Your phones" (section header at `crates/monocode-gpui/src/settings/pages/mobile.rs:408`, inside `mobile_page`, which starts at `:321`), implemented in `pages/web_app.rs` and reusing `SettingsView::row`, `Input::field`, `secondary_button`, `toggle` (`settings/widgets.rs`), the shared modal (`crates/monocode-gpui/src/modal.rs`), the QR canvas (`pages/mobile.rs:1167`) and the popover choice select (`popover.rs`).

```
Web app                                            [Beta]
  Host URL  [ https://zptah.app                    ]   (default: https://zptah.app)
  Status: Connected to account "alice" (relay online)        [ Unpair ]
  [ Pair ]    (disabled while paired; Re-pair = Unpair, then Pair)
              -> modal: QR (384 px) + "Copy link" + "Open in browser" + user code + confirmation code
  [ Verify on another device ]  -> link and QR https://HOST/desktops/:id/verify#fp=<full hex>
  Devices
   MacBook Safari   fp a1b2 c3d4 e5f6 ...   Control v   last seen 2m   [ Revoke ]
   iPhone PWA       fp 99aa 10bc ...        Read-only v last seen 1h   [ Revoke ]
   Pending: "Pixel Chrome"  fp 5c6d ...     Projects: [x] api [x] web   [ Confirm ] [ Deny ]
   Requested: Control  "iPhone PWA"  (raise from Read-only)          [ Approve ] [ Deny ]
  Remote push in Git: [off]   Activity log: [ Open ]
```

States: unpaired (host URL + Pair), pairing (modal with countdown 10 min), paired (account, devices), error banner (token invalid, host unreachable, "Web app: access revoked, pair again" after close `4001`, `4003` or `4011`). New `SettingsEvent::WebPair`, `WebCancelPair`, `WebUnpair`, `WebConfirmDevice`, `WebDenyDevice`, `WebSetPermission`, `WebRevokeDevice` next to the Mobile events (`settings/mod.rs:145-155`) with a `web::Status` snapshot in core like `mobile::Status` (`host.rs:102-115`). The page keeps the existing 1 s refresh pattern (`pages/mobile.rs:118-148`). A raise or widening requested from the web shows as the "Requested: Control" row. A pending device also raises a desktop notification and a badge on the Connections section, because the request can arrive while Settings is closed.

### 6.3 Device confirmation

A confirmation dialog (modal.rs) shows: device name (as sent, untrusted), the device fingerprint **and this desktop's own fingerprint**, each as the first 32 hex characters (128 bits) in 8 groups of 4, account handle, desktop-side project selection checkboxes (default: all rail projects checked, Monos unchecked), permission choice (default Read-only), and "Confirm" / "Deny". The web app shows the device fingerprint under the device (`/desktops/:id`) and, on the pending screen, both the device fingerprint and the pinned desktop fingerprint, so the user compares both pairs with the dialog; a mismatch of the desktop fingerprint means the directory returned a substituted key (security.md T3). Confirm writes the device into `config.devices` (`mobile-access.json`), adds the fingerprint to the Gate, reports `POST /api/desktop/grants/:deviceId/state` and sends the encrypted `state: confirmed` message. The desktop persists the pending grants it reads from `GET /api/desktop/grants` (device public key and name), so Settings and a desktop notification list them even when the device is offline and the user can confirm offline against the grant's fingerprint; on the first `HS1` the static key must match the grant's key or the session gets `BYE`. The 5 min timeout drops only the pending Noise session, never the request; pending requests expire after 7 days (cron, section 4.3). A revoked device has no tombstone on the desktop: it reappears only as a new pending request, which the server-side revoked grant already blocks at relay join.

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
    |                              | create desktops row, status=approved (no token yet)
    |--POST /api/pair/poll (every 3 s) {deviceCode, verifier}-------->|
    |<-{status:"pending"} ...                                          |
    |<-{status:"approved", desktopId, desktopToken, account:{handle}, relayUrl}  (token minted here, once; row -> consumed)
    | seal token in web_identity; connect wss /relay/desktop/:desktopId with Bearer token
    | UI: "Connected to alice"
```

Rules: `userCode` is 8 chars from an alphabet without 0/O/1/I/L, single use, 10 min; poll rate beyond `interval` is normally answered by the `pair-poll` middleware limit (1 per 3 s per device code) with `429 rate_limited` and `Retry-After`, before the handler runs, and the desktop must treat that 429 like `slow_down` (wait and retry); the handler answers `{status:"slow_down"}` only when two polls fall within 2 s across a window boundary; `poll` verifies `SHA-256(verifier) == challenge` and `SHA-256(deviceCode) == device_code_hash`, so neither the QR nor the user code alone can collect the token; the token is returned exactly once. Implementation notes (R-03): `userCode` is stored without the dash and returned as `XXXX-XXXX`; `GET`, `approve` and `deny` accept it in any case with or without the dash. `publicKey` and `challenge` must be unpadded base64url of exactly 32 bytes (else `400`). A wrong verifier is `403`, a wrong or unknown device code `404`; consumed and expired requests answer `{status:"expired"}` and never the token again. The token is minted by the `poll` that wins one batch (`UPDATE ... SET status='consumed' WHERE status='approved'`, then `INSERT INTO desktop_tokens ... WHERE changes() > 0`, scope `relay push grants`), so only its SHA-256 is stored. `relayUrl` is the full URL `wss://<RP_ORIGIN host>/relay/desktop/<desktopId>` (`ws://` when `RP_ORIGIN` is http). `approve` creates the desktop in one guarded statement that also checks the quota (`MAX_DESKTOPS_PER_USER`, default 5, live desktops only; the 6th is `409 quota_exceeded` and the request stays pending). `pairing_requests` has no `app_version` column, so `desktops.app_version` stays null until the relay (or a later ticket) reports it. Requests are deleted 24 hours after they expire. The `#fp=` fragment lets the SPA check that the server-delivered public key matches what the desktop displayed (a malicious host cannot swap keys silently; the QR channel is out of band from the server). The fragment never reaches the server; the SPA saves it to `sessionStorage` before any `/login?redirect=` hop and restores it afterwards. If `#fp` is missing (link typed or copied without it) the page shows a warning that the desktop key cannot be checked and the user must compare the fingerprint shown there with the desktop screen. The UI always displays the first 32 hex characters (128 bits) as 8 groups of 4. Rate limits: `pair/start` 5 per IP per 10 min and 3 outstanding per IP; `pair/poll` per `deviceCode`.

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

Where `notify_mobile` posts to Expo today (`crates/monocode-core/src/mobile/notify.rs:7`, called from `mobile_host.rs:1635-1711`), the web path adds `POST /api/push/notify` with the desktop token and minimal metadata (section 8). Expo remains for phones. Per-device category preferences move to the account (`push_prefs`), not to the desktop. The desktop sends nothing when no web account is paired or the relay path is disabled, and nothing for reminders. Sender policy: a background thread, 10 s timeout, one retry, dropped (not queued) when offline. The request lists `deviceIds` (section 8).

### 6.6 Cloud sessions (Docker sandbox, GitHub Codespace, remote machines)

Today the mobile host knows nothing about cloud sessions (`crates/monocode-core/src/mobile/host.rs:22-54` has no `cloud.*` method; the catalog in `crates/monocode-gpui/src/shell/mobile_host.rs:414-417` keeps a session only if `path_key(summary.cwd)` equals a rail project path, so a cloud session with cwd `remote://<env>/...` is dropped from `workspace.list` and `sessions.list`; confirmed by H-09. A remote-machine session is listed today only when its `remote://` project was added to the rail, and then phones see it too; H-09 leaves that unchanged). The desktop already has the projection for them in the app API (`crates/monocode-core/src/mono/cloud_api.rs:128-157` `target_json`, `annotate`; `CloudApi::session_targets` `:972`). Moved sessions keep their local row and cwd (`crates/monocode-core/src/cloud_sessions/continue_local.rs:4-7`), so they already reach phones as local sessions: H-09 detects them with `cloud_api::is_cloud_session_of` and gives them their cloud `target` on the web transport; QUIC phones keep today's behaviour (listed as local), recorded with a regression test.

**Catalog.** `mobile_catalog_sessions` groups a session by its local twin project (`Shell::project_of`, `crates/monocode-gpui/src/shell/cloud.rs:1017`) instead of the raw cwd, so codespace and sandbox sessions appear under the project the user sees in the sidebar. Project selection (H-02) applies to that project. A session on a remote machine (`monocode-host`, no cloud binding) is grouped under its own remote project: `Shell::project_of` gives a non-cloud remote session its own cwd as its project, so there is no local twin. It is listed with the new kind `remote` when that remote project is selected; `project.*` calls on any non-local session answer `code: unsupported_target` (`crates/monocode-gpui/src/mobile/project.rs:365` says the native app has no remote projects), and the web hides Files, Changes and Git for them. The check is on the project id (a remote project's id starts with `remote%3A%2F%2F`, `cloud_policy::is_non_local_project`) in `Shell::mobile_project_path`, so `project.*`, `project.gitAction`, `session.start` and the `addProject` of `mono.update` are covered by one hook; it applies to web devices only, a phone keeps today's answer. A remote-machine project the user added to the rail stays in the phone's lists as it is today.

**Transport scope.** Cloud and remote-machine sessions in `workspace.list` and `sessions.list`, the `target`, `cloudState`, `startup` and `cloud` fields, and the asynchronous wake reply are served only to devices on the web transport (Noise relay) only. "Clients that announce a capability" is not implemented: the handshake has no client capability yet, so the scope is the transport (`DeviceKind::Web`). The app builds one catalog for every device and marks the web-only sessions, projects and fields; `cloud_policy::scope_catalog` removes them for phones before the host cuts the lists to the recent few and pages them, so a phone's offsets and `hasMore` are what they always were. QUIC phones keep today's lists and send semantics: they never see cloud or remote sessions and a phone send can never wake billable compute without a cost notice (H-09, H-10 regression test).

**Fields** (the catalog fields, plus the `cloud` and `approvals` fields of `conversation.read` results and watch frames; read, additive, absent for old clients). `target_json` and `annotate` in `cloud_api.rs` emit only `local`, `codespace` and `sandbox` (`crates/monocode-core/src/cloud_sessions/backend.rs:88-93` names the backends); `remote` is a new kind added by H-09:

| Where | Field | Content |
|---|---|---|
| `workspace.list` and `sessions.list` session card | `target` | `{kind: "local" or "remote" or "codespace" or "sandbox", instance?, state?}` (`instance` is the codespace or sandbox name, metadata only) |
| same | `cloudState` | `available`, `starting`, `stopping`, `stopped`, `unavailable`, `unknown` (`state_id`, `cloud_api.rs:123`; enum at `crates/monocode-core/src/cloud_sessions.rs:465`) |
| same | `startup` | `{step, error?}` while the instance starts or its start failed |
| `conversation.read` result and watch frames (sent on change only) | `cloud` | `{kind, instance, state, startup?: {steps: [{id, status, startedAt, finishedAt}], current, error?}, kept?: true, autoStop?: {at, reason}, move?: {step, status}}`. The 9 step ids are `create`, `provision`, `buildContainer`, `blockingCommands`, `configureCodespace`, `connect`, `openProject`, `claudeSetup`, `lifecycleCommands`; status is `pending`, `running`, `done`, `failed`, `skipped` (`crates/monocode-core/src/cloud_sessions/startup.rs:96,202`). `kept: true` means a failed start left a billable instance (Stop is allowed on it). `autoStop` announces an auto-stop. `move.step` is one of 7 ids (`snapshot`, `push`, `workspace`, `upload`, `restoreCode`, `restoreConversation`, `flip`, `crates/monocode-core/src/cloud_sessions/move.rs:164-176`) |
| `conversation.read` result and watch frames (sent on change only; an empty array clears) | `approvals` | `[{requestId, label, detail}]`: the pending tool approvals of the conversation (X-01, H-08). `label` is `Approval.title` and `detail` is `Approval.detail` (`crates/monocode-core/src/agent.rs:220-225`; provider text, untrusted, `detail` capped in length, rendered by the safe renderer). No risk field in v1. Present only when the desktop advertises `approvals` |

Every timestamp on the wire (`startedAt`, `finishedAt`, `autoStop.at`, `lastUsedAt`) is integer milliseconds since the Unix epoch. The `cloud` field of a remote-machine session has `state: "unknown"` and its card has no `cloudState`: the desktop does not track a machine's connection for the catalog. Reads use cached state only (`STATE_TTL` 30 s, `cloud_sessions.rs:95`, in practice the shell's refreshed cache, so no `gh` or `sbx` call) and never wake an instance. `cloudState` `unknown` is shown as "Not connected" like the desktop (`crates/monocode-gpui/src/cloud_mark.rs:31-43`). A conversation whose instance has no live connection may read empty or stale; the web shows the state, not an error.

**Methods** (classification in 7.1):

| Method | Maps to | Notes |
|---|---|---|
| `cloud.instances` (read) | `CloudApi::list` (`cloud_api.rs:571`) | `{instances: [{kind, instance, displayName, state, rawState, lastUsedAt, sessions: [{sessionId, title, busy}], costEstimatePerHour?, stopReport?}], backends, otherCount?}`. Built from the cached `workspaces_overview` (30 s TTL, `crates/monocode-core/src/cloud_sessions/workspaces.rs`) with cost estimates from `WorkspaceRow`, never a fresh `gh` or `sbx` call per request; rate limited; never wakes. Filtered to instances used by sessions of selected projects. Failed and orphan instances that bill (`crates/monocode-core/src/cloud_sessions/footer_status.rs:28-41`) are listed with names only to devices whose selection is all projects; other devices get `otherCount` ("2 other instances on this desktop"). `stopReport` is `stopping`, `takingLong` or `stillRunning` (`crates/monocode-core/src/cloud_sessions/stopping.rs:54-62`) |
| `cloud.stop {sessionId}` or `{kind, instance}` (control) | `CloudApi::stop` (`:681`) | Recoverable. `stopped: true` only means requested; progress is `stopReport`. Returns the desktop's `warning` naming interrupted busy sessions. Refused with `code: instance_busy` while the desktop archives or converts on that instance. Allowed on a `kept` instance. The web shows the sessions sharing the instance before sending (`plan_instance_action`, `cloud_sessions/instance_action.rs`) |
| `cloud.resume {sessionId}` (control) | `CloudApi::resume` (`:728`) | Blocks for minutes, so it runs on a worker thread and answers at once with `{accepted: true}`; progress arrives as `cloud` fields in the watch frame of that session |
| `cloud.startOptions {projectId}` (read, P2) | `CloudApi` readiness, branch and preset lists | `{backends: [{kind, ready}], branches, machines (codespace, allowed list), sizes (sandbox presets)}`. Cached or read-only, never creates anything. Built from local reads only: `ready` is "prerequisites present" (a GitHub origin and `gh` installed; `sbx` installed and its ssh alias configured), because a sign-in cannot be known without running `gh` or `sbx`; `branches` are the project's remote-tracking branches (local git, no network); `machines` appear only when an earlier desktop call cached the list, else they are left out; `sizes` are the fixed sandbox presets (ids like `2cpu-4g`). An empty list is omitted and then offers no choice |
| `session.start` with `target` (control, P2) | `CloudApi::begin_start` (`:948`) | `target` accepts only `kind`, `instance` (`"new"` or a name from `cloud.instances`), `branch`, `machine` (codespace, from `cloud.startOptions`) or `size` (sandbox preset). The Claude kit follows the desktop's saved preference (`cloud_sessions/kit_prefs.rs`); the client can never set `kit`, `template`, `image`, `cpus`, `memory` or a `ttl` (rejected `code: forbidden`). Claude only, no draft, no worktree. Rejected with `code: disabled` unless the desktop toggle "Remote cloud start" is on (default off), like "Remote push in Git". `branch`, `machine` and `size` must be values `cloud.startOptions` lists (`forbidden` otherwise) and `instance` must be `"new"` or an instance the project already uses (`unsupported_target`); a `target` from a QUIC phone is `unsupported`. The reply carries the new session id once `begin_start` has recorded the session (seconds); the instance then comes up in the background and progress arrives in the `cloud` field. It counts in the wake rate limit until the start ends |
| `cloud.retryStart {sessionId}` (control, P1) | `run_start` (`startup.rs`) | Retry of a failed start; cost notice, rate limited. Rebuild, open in browser and delete codespace stay desktop-only (denied) |
| `cloud.keepRunning {sessionId}` (control, P2) | auto-stop module (`crates/monocode-gpui/src/conversation/auto_stop.rs`) | Cancels a pending auto-stop. A send during an auto-stop answers `code: auto_stopping`; a web send counts as user activity, watching does not |
| `cloud.moveToCloud`, `cloud.moveToLocal`, `cloud.cancelMove` (control, P2) | app API `sessions.moveToCloud`, `sessions.moveToLocal`, `sessions.cancelMove` (`crates/monocode-core/src/mono/move_api.rs:1-17`) | Allowed only while both desktop toggles "Remote cloud start" and "Remote push in Git" are on (a move pushes a branch, `move.rs:28-30`); cost notice; progress is the `cloud.move` field (a step id and a status only); a cancel never deletes a workspace. `cloud.moveToLocal` only opens the desktop's Continue locally confirmation (the app API's rule: it changes the working tree), so the person at the desktop confirms it; the reply is `{accepted, awaitingConfirmation}`. The `cloud.move` field covers moves to the cloud, not the return |
| `session.archive` with `cloud: "keep"` or `"stop"` (control) | `crates/monocode-gpui/src/shell/cloud_app_archive.rs:27-35` | Required for cloud sessions, like the app API; the web shows a cost notice because conversion may wake the instance. Today the mobile host calls `toggle_archive_now` (`mobile_host.rs:1433-1437`) and skips the keep/stop question and the local-copy conversion |
| `session.delete` on a cloud session | n/a | Answers `code: unsupported_target` (delete on the desktop); the web hides Delete for cloud sessions |
| `cloud.delete`, `cloud.create`, `cloud.rebuild`, open in browser, delete codespace, `pullEvidence` | n/a | Never exposed. The Operator already cannot delete (`crates/monocode-core/src/app_access.rs:45`) |

**Wake on send.** Today the mobile path does not wake: `mobile_submit` defers when the history is not ready (`crates/monocode-gpui/src/conversation/mobile.rs:113-118`) and background loads never wake (`crates/monocode-gpui/src/cloud_wake.rs:5-7`), while the Operator path calls `wake_for_send` (defined at `crates/monocode-gpui/src/cloud_wake.rs:24`, called at `shell/app_commands.rs:842`) and refuses with "Retry in a minute". The host's reply wait is 25 s (`host.rs:56`) and a wake takes minutes, so on the web transport only (H-10): `conversation.send` to a cloud session whose instance is not connected needs consent: without `wake: true` in the send params it is refused with `code: wake_required` ("Sending wakes the stopped cloud instance; confirm to continue"), because waking bills the user and the web cannot otherwise tell a plain send from a confirmed one. The same applies when the state is `unknown`. With `wake: true` the per-device rate limit is checked (security.md T16), then the desktop calls `wake_for_send`, relies on the existing deferral (`defer_history_action`; the reload after a wake flushes queued sends) and replies `{sessionId, wake: true, queued: true}` at once; a second send while one wake-send is pending gets `code: busy`. A remote-machine session never needs `wake`. H-10 adds only the `wake_for_send` call, the busy rule and the reply, with a test, not a new queue. Progress and the final delivery are visible through the `cloud` field of the watch frame (state `starting`, then `available`, then the user turn appears). A send while the instance is starting is refused with `code: starting` carrying the desktop's `starting_refusal` text (`crates/monocode-core/src/cloud_sessions/startup.rs:740`); the web disables Send while `cloudState` is `starting` and shows the steps. (`queue_start_message`, `startup.rs:759`, is used only for the first prompt at creation, `shell/cloud_create.rs:147`.) Sends are idempotent by command id as today.

**As built (H-10, H-12).** "Not connected" means the cached state is `stopped`, `stopping` or `unknown`; an `available` instance sends as today even when `wake: true` is given, and `unavailable` answers `unsupported_target`. A send also answers `auto_stopping` while the desktop stops the instance on its own and `instance_busy` while it archives or converts. The pending-send flag is per session (one wake-send at a time, any device), cleared when the deferred send runs, when the wake fails (the queued send is dropped with it) or after 15 minutes. `cloud.stop` replies `{stopped, kind, instance, sessions, warning?}` with only the sessions of the device's selected projects, and `cloud.resume`, `cloud.retryStart` answer `{accepted: true}` at once. `cloud.keepRunning` starts a fresh idle window for the instance and exempts it from the unused-stop; the desktop has no standing "never auto-stop", so the next idle window can stop it again. `autoStop {at, reason: "idle"}` appears in the `cloud` field when the idle stop is at most 5 minutes away or already running. `session.archive` of a cloud session answers at once (the archive takes minutes); a failure shows on the desktop error line. Audit lines carry device, method and outcome; the instance kind is not in the audit event today.

**Review hardening (WS-D4 fixes).** A remote send never reconnects an instance by itself: `mobile_submit` runs with a no-wake mode that `remote_send_guard` honours, and only the one send the host checked for consent (`wake: true`, the wake plan) is queued to wake. The check also looks at the live view: an `available` cache with a loaded view that is not connected counts as stopped (`wake_required`, which costs nothing). The wake itself (pending flag, rate-limit slot, `wake_for_send`) starts only after the text is valid and its attachments are staged, right before the message is queued, so a refused message wakes nothing and holds no slot. The queued message carries the token of its pending flag: a failed wake or the 15 minute timeout cancels exactly that message and leaves the other waiting actions alone. A phone (QUIC) cannot reach a session only web devices see (a cloud-born session, or a remote-machine session whose folder is not a rail project): `mobile_target` answers `unsupported_target`, the Mono launch cards follow the same scope and selection, and a phone cannot delete or archive a session that is bound to an instance (it would orphan a billing instance or skip keep/stop); this is a deliberate tightening over byte-for-byte parity. Web-only remote projects have an opaque id (`remote-` and 16 hex digits of a SHA-256 of the path) and no `path`; sessions and launch cards use the same id, and `is_non_local_project` knows both spellings. `cloud.instances` answers from the cached overviews only and says `loading: true` while a background load fills a missing one; `cloud.stop` answers at once and stops on a worker, only for an instance a session of the app uses or one a listing showed, at most 10 a minute per device; a retry that would create the instance needs "Remote cloud start". The `state` message of a web device carries the capabilities (`cloud` always, `cloudStart`, `cloudMove`, `gitPush`, `approvals` by toggle). A toggle whose save fails ends off and says so. `autoStop.at` is a fixed deadline, so the watch does not resend it every second.

**What never goes into frames**: GitHub, Docker or account tokens, codespace or sandbox credentials, environment variables, the Claude kit or pinned account data, `web_url`, SSH targets and host pairing tokens (`Machine.ssh`), `remote_cwd`, `gh_state`, `StartRecord.log_tail`, `StartEvent::Log` lines and step `detail` text (they may hold paths, repositories and commands). `startup.error` is sanitized (no command lines or paths) and capped at a tunable length (default 200 characters, H-10 picks the final value). The core states "Secret values never pass through this module" (`cloud_api.rs:32`); the mobile projection follows that rule and a test asserts that no frame contains these fields. Instance names ride only inside the Noise tunnel: never in push payloads, and the Worker audit table stores at most the kind.

**Permissions, billing and audit.** Read methods need `read`; stop, resume, retry, start, move, keep running and archive need `control`; every call is written to the desktop audit log (H-05) with device id, method, instance kind and outcome (the instance name is allowed there because the log is local). Billing rule: control plus a cost notice for send-to-stopped, resume, retry, start, move, archive-with-conversion and continue locally; Stop is control without a cost notice; delete, create and rebuild stay denied. Wake-causing calls are rate limited per device (security.md T16). Details: security.md section 7.

### 6.7 Desktop key lifecycle

| Event | Behaviour |
|---|---|
| Unpair | The desktop calls `DELETE /api/desktop/me` (token scope `relay`) to revoke all its tokens and close the room (`4003`), then wipes `web_identity`. The desktop row and its grants stay (audited as `desktop.revoke` by the desktop), so the next pairing can choose "This replaces <desktop>" and no orphan is created |
| Pair while paired | Pair is disabled; "Re-pair" is Unpair then Pair |
| Re-pair onto an existing desktop | On `/pair/:code`, if the account already has desktops the user may choose "This replaces <desktop>" (fresh re-auth, `POST /api/pair/:userCode/approve` with `replaces`). The desktop id, name and color are kept, the public key and OS are swapped, old tokens are revoked and every non-revoked grant resets to `pending` with `permission`, `requested_permission` and `confirmed_at` cleared (each device is re-confirmed; revoked grants stay revoked), so no orphan row counts against the quota. One batch does it, then the room closes every old socket with `4003`; the new token comes from the next `poll` |
| Unreadable `credentials.key` (identity_store `UNREADABLE`) | Treated as a reinstall: new key, new pairing, "This replaces <desktop>" |
| A device pinned the old key | It shows "Desktop key changed" and can re-pin only by opening the verify link or QR from desktop Settings > Web app > "Verify on another device" (`https://HOST/desktops/:id/verify#fp=<full hex>`, section 3.1); never a "trust new key" button |

## 7. Desktop link: gates and permissions (reference)

### 7.1 Permission classification

| Class | Methods |
|---|---|
| read | `conversation.read`, `conversation.older`, `conversation.watch`, `models.list`, `accounts.usage` (cached usage, no `refresh`), `device.selection` (get), `workspace.list`, `sessions.list`, `project.changes`, `project.worktrees`, `project.files`, `project.file`, `project.search`, `project.fileHighlight` (phones only, `unsupported` on web), `project.git`, `mono.document`, `mono.profile`, `mono.file`, `device.leave`, `cloud.instances`, `cloud.startOptions` |
| control | `session.configure`, `session.start`, `conversation.send`, `session.answerApproval`, `session.resumeUsage`, `session.resumeUsageAtReset`, `session.answerQuestion`, `session.keepQuestionOpen`, `session.cancel`, `session.rename`, `session.archive` (a cloud session needs `cloud: "keep"` or `"stop"`), `session.delete` (not for cloud sessions: `unsupported_target`), `accounts.usage` with `refresh: true`, `project.gitAction` with `stage`, `unstage`, `generate` or a `commit` that does not push, `device.selection` (set; may only narrow, widening needs desktop confirmation), `mono.create`, `mono.update`, `cloud.stop`, `cloud.resume`, `cloud.retryStart`, `cloud.keepRunning`, `cloud.moveToCloud`, `cloud.moveToLocal`, `cloud.cancelMove` (only while "Remote cloud start" and "Remote push in Git" are on), `session.start` with `target` (only while the desktop toggle "Remote cloud start" is on) |
| denied for web in v1 | `mono.saveFile`, `mono.reset`, `mono.delete`; `project.gitAction` `sync`, `pr`, and `commit` with `push: true` or `createPr: true` (the only ways the desktop pushes, `crates/monocode-gpui/src/mobile/project.rs:435-457`), unless the desktop toggle "Remote push in Git" is on (default off); `runtimeMode: "full-access"` in `session.configure`/`session.start` (denied for every remote device, phones included); `cloud.delete`, `cloud.create`, `cloud.rebuild`, open in browser, delete codespace, `pullEvidence` (never exposed); answered `code: unsupported` on the web transport: `device.push` (Expo tokens) and `project.fileHighlight` |

`read` devices get `{ok:false, code:"read_only"}` for control methods. Details and rationale in security.md section 7.

### 7.2 Grants sync

The desktop is the authority for `state` and `permission`. The server mirrors them in `device_grants` for the UI. A server-side revoke blocks the relay join immediately and sends `CTRL {"t":"grants"}`; the desktop re-reads `GET /api/desktop/grants` and removes the device from its Gate. The desktop also re-syncs grants on every relay connect, not only on CTRL `grants`. A permission change in the web UI sets `requested_permission`; the desktop applies a lowering automatically and a raise or widening only after local confirmation (shown as the "Requested: Control" row, section 6.2). Every device needs one confirmation per desktop; it requests its grant on a desktop when the user first opens `/d/:desktopId` or taps "Request access" in the switcher, never automatically for all desktops. Server states: `pending -> confirmed -> revoked`. Only `POST /api/desktop/grants/:deviceId/state` sets `confirmed` and `permission` (a confirmed report fulfils a matching `requested_permission`; a reported `pending` clears `permission` and `confirmed_at`); the web side creates `pending` grants, writes `requested_permission` and revokes. `revoked` is final: the desktop cannot resurrect it, and a revoked device comes back only as a new device registration. A forgotten device (`DELETE /api/devices/:id`) revokes all its grants, deletes its push subscriptions and has each desktop's room close its sockets (`4003`) and re-read grants; `GET /api/desktop/grants` reports its grants as `revoked`. `GET /api/desktop/grants` carries `deviceName` and `publicKey` so the desktop can list a pending grant while the device is offline. The cron deletes `pending` grants not updated for 7 days. A desktop-side revoke sends `state: revoked` plus `BYE` on the live channels and posts `POST /api/desktop/grants/:deviceId/state` with `revoked`, which calls `room.revoke`; `device.leave` reports `revoked`. A permission or selection change sends a new `state` message to every live channel of that device.

## 8. Web Push flow

```
 Desktop                Worker                           Push service            PWA service worker
   | POST /api/push/notify (Bearer token)  {category, project, status, sessionId, monoId?, habitRun?}
   |----------------------->| check token scope push, rate limit 30/min
   |                        | owner = desktops.user_id; push_prefs enabled for category
   |                        | for each push_subscriptions row of the listed deviceIds (confirmed grant):
   |                        |   encrypt (RFC 8291 aes128gcm) + VAPID JWT (ES256, RFC 8292)
   |                        |-- POST endpoint (TTL 86400, Urgency high|normal, Topic) -->|
   |                        |<-- 201 / 404 / 410 (delete row) / 429 (backoff) ---------|
   |                        |                                                          |-- push event
   |                        |                                                          | showNotification(...)
   |                                                                                  click -> open url (rule below)
```

- **Subscribe**: after install, on a button click, the app calls `Notification.requestPermission()`, then `registration.pushManager.subscribe({userVisibleOnly: true, applicationServerKey: VAPID_PUBLIC_KEY})` and `POST /api/push/subscriptions`. Never on page load. iOS requires a Home Screen web app and a user gesture (https://firt.dev/notes/pwa-ios/ and https://pushpad.xyz/blog/ios-special-requirements-for-web-push-notifications, read 2026-11); minimum iOS 16.4 (secondary sources).
- **Notify request** (`POST /api/push/notify`): `{category: "agentFinished"|"agentInput"|"monoMessages", project: "<display name>", status: "finished"|"needs_input"|"message", sessionId, monoId?, habitRun?: true, deviceIds: [...], desktopName?}`. `habitRun: true` is set when the notice is for a Mono's habit run: the desktop sends `monoId`, `projectId` and `sessionId` together for it (`crates/monocode-gpui/src/shell/mobile_host.rs:1671-1676`) and mobile opens the session when all three are set (`mobile/src/features/notifications/SessionNotifications.tsx:211-217`). `deviceIds` are the server ids of confirmed web devices whose desktop-side selection includes the session's project or Mono; the Worker sends only to subscriptions of those devices that also hold a `confirmed` grant on that desktop. `project` is the Mono name for Mono notices. No title, no assistant text, no transcript: this closes gap G8 (security.md section 7) for the web path.
- **Sent payload** (JSON, under 1 KiB, encrypted end to end to the browser): `{"v":1,"t":"<desktop name>","b":"<project>: Finished" | "<project>: Needs your input" | "<project>: New message","url":<URL rule>,"tag":"<sessionId>"}`. **URL rule** (built by the Worker, the only place it exists): `/d/<desktopId>/s/<sessionId>`, unless `monoId` is set and `habitRun` is not, in which case `/d/<desktopId>/monos/<monoId>`. A habit run therefore opens its session, a Mono message opens the chat. Headers: `TTL: 86400`, `Urgency: high` for `agentInput` and `normal` otherwise, `Topic`: first 32 url-safe base64 chars of SHA-256(sessionId) so repeated notices for one session collapse.
- **When** the desktop sends mirrors the mobile rules (`mobile_host.rs:1643-1655`: only for sessions a remote device started, not while the desktop window shows that session). Account-level preferences are applied by the Worker.
- **Sender**: WebCrypto only (ECDH P-256, HKDF-SHA-256, AES-128-GCM, ES256 JWT), an in-house sender in `worker/src/modules/push/` (`webpush.ts` encryption and request, `vapid.ts` keys and JWT, `payload.ts`, `notify.ts`, `subscriptions.ts`); `web-push` (Node) does not run on Workers (https://github.com/web-push-libs/web-push/issues/718, read 2026-11). The encryption is tested against the RFC 8291 Appendix A vector (exact bytes). Still to do in the spike: a real Chrome and Safari endpoint, and the CPU time per send on the free plan (the maintainer measures it, S-01).
  - **Subscribe checks**: the endpoint must be `https:`, at most 2048 characters, on a known push service host (`fcm.googleapis.com`, `updates.push.services.mozilla.com`, `web.push.apple.com`, `*.push.apple.com`, `*.notify.windows.com`), so the Worker never POSTs to an arbitrary URL; `p256dh` is a 65-byte uncompressed P-256 point and `auth` 16 bytes; `deviceId` must be the caller's non-revoked device (404); `vapidKeyId` must equal the current `VAPID_KEY_ID` (400, the client re-subscribes); at most 4 subscriptions per device (409 `quota_exceeded`); the same endpoint upserts and moves to the latest user and device. No prefs row means push is off for that desktop (opt in).
  - **Send**: one indexed query selects the subscriptions of the listed devices that belong to the owner, are not revoked and hold a `confirmed` grant on this desktop, at most 20 per notify (subrequest budget). Sends run in parallel; every D1 change of one notify is one `db.batch`. A success writes only when `failures > 0` or `last_success_at` is older than 24 h (write budget). `429` sets a per-isolate backoff for that endpoint from `Retry-After` (seconds or date, at most 1 h), not a failure. The same `(desktopId, sessionId, status)` within 10 s is skipped (per isolate, best effort). Names are cut to 64 characters and the payload is asserted under 1 KiB.
  - **Keys**: the JWT is signed with the key whose id equals the subscription's `vapid_key_id`: the current `VAPID_*` pair or, during a rotation, the optional `VAPID_PREVIOUS_*` pair (security.md, "VAPID rotation runbook"). A subscription on neither key id is skipped, not deleted. With the current key missing or invalid (for example the placeholder) notify answers 503 `internal` and sends nothing; one metadata log line is written per isolate.
- **Cleanup**: subscriptions are deleted when a device is forgotten and when an admin blocks the account. A `pending` or `revoked` grant stops delivery at once (a `confirmed` grant is required). Sign-out does not delete subscriptions: the web client must call `DELETE /api/push/subscriptions/:id` before signing out (P-05, WS-S); deleting them server side on logout is a pending WS-A change. `404` and `410` delete the subscription; `failures` increments on other errors and the row is dropped after 5 consecutive failures; `429` honours `Retry-After`.
- **iOS**: every push must display a notification (silent pushes get the subscription revoked); badge and sound rely on the OS. Declarative Web Push (iOS 18.4+) is a later optimization, not used in v1. Because background WebSockets are dropped, push is the only background channel; opening the notification triggers the resync of section 3.3.
- **Click**: the SW opens the `url` of the payload (a session, including a habit run's, or the Mono chat for a plain Mono message); the link handshake then loads the transcript. If the session no longer exists the app shows the project list.

## 9. Self-host guide (outline)

1. Prerequisites: Cloudflare account, Node 22.18 or newer, pnpm, `wrangler` (4.36 or later), a domain on Cloudflare.
2. `git clone` the repo, `cd web && pnpm install --frozen-lockfile`. Every command below runs in `web/` (where `wrangler.jsonc` lives; the `wrangler` commands pass `-c wrangler.jsonc`, the `pnpm --filter @monoptah/worker` scripts pass `-c ../wrangler.jsonc` themselves).
3. `pnpm --filter @monoptah/worker exec wrangler login` (wrangler is a dependency of the worker package; it is not installed globally by the repo). The same prefix applies to every `wrangler` command below.
4. `wrangler d1 create monoptah`, copy the `database_id` into `wrangler.jsonc`.
5. `pnpm --filter @monoptah/worker db:migrate:remote` (runs `wrangler d1 migrations apply monoptah --remote -c ../wrangler.jsonc`).
6. Generate VAPID keys: `pnpm vapid:generate` prints `VAPID_PUBLIC_KEY=` (65-byte uncompressed P-256 point) and `VAPID_PRIVATE_KEY=` (32-byte scalar), both base64url, and writes nothing to disk. Put the public key (replacing the `REPLACE_WITH_VAPID_PUBLIC_KEY_BASE64URL` placeholder) and `VAPID_SUBJECT` in `vars`; `wrangler secret put VAPID_PRIVATE_KEY`.
7. The three secrets, and only these, are Worker secrets (`web/.dev.vars.example` and the comment in `wrangler.jsonc` list the same three; locally they come from the git-ignored `web/.dev.vars`): `VAPID_PRIVATE_KEY` (step 6), `wrangler secret put RECOVERY_PEPPER` (32 random bytes, base64; losing it invalidates stored recovery codes, so back it up) and `wrangler secret put AUDIT_IP_SALT` (32 random bytes, base64). During a VAPID rotation only, the optional `VAPID_PREVIOUS_PRIVATE_KEY` is a fourth secret (security.md 9.1); remove it when the rotation ends.
8. Set `RP_ID` and `RP_ORIGIN` to your own final hostname (the official host uses `zptah.app` and `https://zptah.app`). Passkeys are bound to `RP_ID`: it must never change after the first user registers, because changing it breaks every passkey. Host nothing else on that hostname or its subdomains (cookie and CSP isolation, security.md section 3.3).
9. Custom domain: add a `routes` entry with `custom_domain: true`, or attach in the dashboard; HTTPS is mandatory.
10. `pnpm --filter @monoptah/app build`, then `wrangler deploy -c wrangler.jsonc`. (There is no root `build` script. The official host is not deployed this way: only the maintainer runs `pnpm deploy:official` from their Mac, security.md section 9 and [deploy-runbook.md](deploy-runbook.md).)
11. First admin: `pnpm bootstrap -- --remote` (or `pnpm --filter @monoptah/worker bootstrap -- --remote`; `--local` targets the local dev database and `--persist-to DIR` picks its directory; one of the two flags is required and `--remote` is refused when `CI` is set) creates a `bootstrap` invite (24 h, one use, grants `role = admin`) and prints the link `<RP_ORIGIN>/register#invite=CODE` once (only the SHA-256 hash of the code is stored, and the code is never written to a file or log; `--origin URL` overrides the origin read from `wrangler.jsonc`); open `/register#invite=CODE`, choose a handle, create a passkey, save the recovery codes.
12. In Monoptah Native set the Host URL (Settings > Connections > Web app) and Pair.
13. Update: `git pull`, `pnpm install --frozen-lockfile`, apply new migrations (`db:migrate:remote`) first, then build and `wrangler deploy -c wrangler.jsonc` as in step 10. Every deploy disconnects all relay sockets; clients reconnect with backoff (section 3.3). Migrations are forward-only and backward compatible for one release so a rollback of the Worker works.
14. Backups: D1 Time Travel is on by default; export with `wrangler d1 export` before risky migrations.
15. Docker is out of scope.
