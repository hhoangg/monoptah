# Monoptah web app: security

Companion to [architecture.md](architecture.md). Route, table, frame and token names are the ones defined there. Code citations are to the repo as researched on 2026-11-17.

## 1. Assets and actors

Assets, most valuable first:

| Asset | Where it lives |
|---|---|
| Ability to run commands and write files on a desktop (through the agent) | Desktop; reachable remotely via `conversation.send`, `session.start`, `session.configure` (`crates/monocode-core/src/mobile/host.rs:22-54`) |
| Project source and secrets inside projects (`.env`, keys) | Desktop disk; readable through `project.file` (`crates/monocode-gpui/src/mobile/project.rs:127`) |
| Conversation content (prompts, assistant output, diffs) | Desktop and browser memory/IndexedDB only; relay sees ciphertext |
| Device static private key (Noise) | Browser IndexedDB, AES-GCM wrapped (architecture.md 3.4) |
| Desktop static private key and desktop token | Desktop sealed store (`identity_store.rs:1-15`) |
| Session cookie | Browser |
| Passkeys, recovery codes | Authenticator; hashes in D1 |
| Account directory and push endpoints (metadata) | D1 |
| Signing/VAPID/pepper secrets, deploy credentials | Cloudflare secrets; deploy login only on the maintainer's Mac (no deploy token in CI) |

Actors:

| Actor | Capability assumed |
|---|---|
| Account owner | Legitimate user |
| Invited user | Registered via an invite; sees only their own desktops |
| Malicious agent content | Arbitrary text, markdown, file paths, diffs reaching the renderer (prompt injection, hostile repo files) |
| Network attacker | Controls the path between browser and Cloudflare; no TLS break assumed |
| Relay/operator | Runs the official host: sees metadata, serves the JavaScript |
| Compromised host | The Worker, D1 or the Cloudflare account is under attacker control |
| Phisher | Sends links, QR codes or fake pages |
| Malicious or stolen paired device | Holds a confirmed device key and a valid session |
| Other self-hosters | Own operator trust for their instance |

Out of scope: a compromised desktop OS, a compromised user browser or OS, Cloudflare platform compromise, side channels in noble or `snow`.

## 2. Threat model

| # | Threat | Actor | Impact | Mitigation | Residual risk |
|---|---|---|---|---|---|
| T1 | **XSS from agent content** (markdown, code, diffs, file names, tool output rendered in the SPA); top risk because the page holds the unwrapped Noise key and a live control channel | Malicious agent content | Full remote control of every desktop paired to the browser: send prompts, read files, enable tools | No raw HTML anywhere; sanitizer-free renderer that builds React elements from the markdown AST; link URL allowlist; shiki with its JS engine, tokens rendered as text nodes; no `dangerouslySetInnerHTML`; strict CSP; Trusted Types with a throwing default policy; no remote images; `read-only` default permission for new devices; desktop never trusts the web client (section 7); dependency pinning and lockfile review (section 8) | A logic bug in a custom renderer or a compromised dependency; Trusted Types absent on iOS 18 and older, where CSP is the only backstop |
| T2 | Compromised host or malicious operator of an instance | Operator, host compromise | Can serve malicious JS (steal the device key and session), substitute the desktop key in the directory, delete accounts, read all metadata (who, when, which desktop, sizes) | E2E means the server alone cannot read content; desktop confirms every device by fingerprint; pairing shows the fingerprint out of band in `#fp=` (architecture.md 6.4); key pins and a "desktop key changed" block; operator 2FA and deploys only from the maintainer's Mac (section 9); self-hosting for users who do not accept the trust | **Accepted**: users of a host trust its operator not to ship malicious JS. No client-side integrity proof exists for a CSR web app |
| T3 | Relay MITM or tampering | Network attacker, relay | Read or alter traffic between browser and desktop | TLS to Cloudflare; Noise IK authenticates both static keys, AEAD per message, relay sees opaque bytes; frames carry the authenticated `deviceId` rewritten by the relay but binding is in the Noise prologue. First contact is the exception: a directory that returns a substituted desktop key completes the handshake as the responder, so IK alone does not catch it. Mitigation is out-of-band comparison: only the approving browser has `#fp` from the QR; the web pending screen shows the pinned desktop fingerprint, and the desktop confirmation dialog shows the desktop's and the device's fingerprints for the user to compare; after first contact the pin blocks a changed key | Metadata and traffic analysis (sizes, timing); drop or delay of frames (no integrity of delivery, only of content); a user who skips the fingerprint comparison on first contact can be served a substituted desktop key by a malicious host (T2) |
| T4 | Stolen session cookie | Network or local attacker, XSS | Act as the account in `/api/*`; open relay sockets with an existing device id only if the device private key is also present (the handshake requires it) | `__Host-` HttpOnly Secure cookie; server-side session row revocable; Origin and CSRF header checks; fresh passkey re-auth for sensitive actions; idle and absolute expiry; sessions list with revoke | Cookie alone cannot read desktop content (needs device key); it can list desktops, register a new device and request a grant, which still needs desktop confirmation |
| T5 | Phishing of the pairing or login flows | Phisher | Victim approves attacker's desktop onto their account, or signs in on a fake site | Passkeys are bound to `RP_ID` (no password to phish); pairing page shows machine name, OS, fingerprint and a confirmation code that must match the desktop screen; approve needs fresh passkey re-auth; user codes expire in 10 min, single use | A user who ignores the comparison can approve an attacker desktop, which then only receives what that user's own devices send to it (a hostile desktop is a hostile server for prompts it receives) |
| T6 | Invite abuse | Invited user, spammer | Account farming, mass registration | One-time codes, expiry, quota 3 invites per user, 5 desktops and 10 devices per account (configurable on self-host), `users.invited_by` tree, admin cascade block, rate limits on register endpoints | A trusted inviter can invite a malicious person (mitigated by branch blocking) |
| T7 | Lost or stolen device | Thief | Access to the web app as the signed-in user | Device biometrics/passcode and passkey UV; session revoke from another device; `DELETE /api/devices/:id` and desktop-side revoke close the link at once; read-only default permission | Window between loss and revoke; an unlocked device is fully trusted |
| T8 | Malicious or compromised paired device | Paired device | Drive the agent, read project files | Desktop-side `read` vs `control` permission; per-device project selection enforced on the desktop; deny-list for sensitive files; no remote full-access; rate and quota limits; desktop audit log; revocation | A `control` device can still make the agent do arbitrary work within the selected projects and the session's access mode; this is inherent to remote control |
| T9 | Desktop token theft | Local malware on the desktop, token leak | Impersonate the desktop to the relay, send fake pushes | Token is 256-bit random, stored hashed server-side and sealed on the desktop (not in `mobile-access.json`); scopes `relay push grants` only; one live desktop socket (new connection replaces old, `4009`); revoke via `DELETE /api/desktops/:id` or Unpair; notify rate limit 30/min | An attacker with the token can accept device connections but cannot decrypt IK sessions without the desktop static key (kept in the same sealed store, so malware with disk access gets both) |
| T10 | Push payload leakage | Push service, network, Worker | Leak of assistant text or titles (today's Expo path does leak, gap G8, `notify.rs:58-72`) | Web payload carries only desktop name, project display name, status text, session id; encrypted per RFC 8291 to the browser; sent only to the listed confirmed devices (architecture.md section 8); TTL 24 h | The Worker and the push service see metadata (project names, timing); project display names may be sensitive for some users |
| T11 | DoS on relay | Network attacker, abusive device | Exhaust DO budget, block a desktop's room | Auth required before a socket reaches the DO; per-room socket and rate quotas; `ping` auto-response; message size cap; per-IP rate limits on `/api/*`; Cloudflare platform DDoS | Authenticated abuse by an invited user can burn the free-tier request budget (capacity.md); admin can block a branch |
| T12 | Replay or reorder of relay frames | Relay | Re-run a command | Noise transport nonces are strictly increasing; a replayed or reordered frame fails decryption and closes the session; command ids are deduplicated by the desktop receipt table (`mobile_host.rs:1269-1333`) | None beyond DoS by frame drops |
| T13 | Account takeover by recovery code theft | Local attacker who finds the codes | Sign in as the user | Codes shown once, one-time, stored hashed, `recovery_only` session must add a passkey, all other sessions revoked on recovery, email-free so no reset channel to hijack, rate limits | A user storing codes insecurely; recovery does not grant device trust (new device still needs desktop confirmation) |
| T14 | Cross-site request forgery | Malicious site | Unwanted API actions as the user | `SameSite=Strict`, Origin check, `X-Monoptah-CSRF` header, JSON content type only, no state change on GET | None expected |
| T15 | Malicious desktop | Phisher with own desktop | Hostile responses to a web client (crafted frames to trigger XSS or crashes) | Same renderer rules as T1 apply to all desktop-originated content; frame and size validation in the client | Parser bugs |
| T16 | **Cost abuse through cloud lifecycle** (codespaces and Docker sandboxes bill while running) | Malicious or compromised paired device, XSS | Wake, resume or start billable instances, repeatedly; stop a shared instance and interrupt other sessions; delete data | Reads never wake (cached state). `cloud.stop`, `cloud.resume`, `cloud.retryStart`, `cloud.moveToCloud`, `cloud.moveToLocal`, `cloud.cancelMove`, `cloud.keepRunning`, archive with conversion and `session.start` with `target` are `control` only; starting new cloud sessions also needs the desktop toggle "Remote cloud start" (default off, `code: disabled`); `cloud.delete`, `cloud.create`, rebuild, open in browser, delete codespace and `pullEvidence` are never exposed; per-device rate limit on wake-causing calls (`conversation.send` to a stopped session, `cloud.resume`, retry, start, move): default 6 per hour and 2 concurrent, a tunable constant that H-10 finalizes (`code: busy` beyond); each call audited; web shows a cost notice and the sessions sharing an instance; frames carry no tokens or logs (architecture.md 6.6) | A control device can still send to a stopped session and cause compute cost up to the rate limit; auto-stop bounds it: a sandbox stops at its TTL (default 2 h, `crates/monocode-core/src/sandboxes.rs:33`), a codespace at its idle timeout (default 30 min, asserted in `crates/monocode-core/src/cloud_sessions/project_cloud_tests.rs:25`) |

## 3. Authentication

### 3.1 Passkeys only

- `@simplewebauthn/server` on Workers, `@simplewebauthn/browser` in the SPA. No passwords and no email; identity is the `handle`.
- Registration and login set `userVerification: "required"`, `residentKey: "required"` (discoverable credentials, so `/login` needs no username), `attestation: "none"`, `authenticatorSelection.authenticatorAttachment` unset so both platform and roaming authenticators work.
- Multiple passkeys per account (`passkeys` table); at least one must remain (`DELETE /api/passkeys/:id` refuses the last). Signature counters are checked and stored; a decreasing counter on an authenticator that uses counters is logged to `audit_log` and the login is refused.
- Cross-device sign-in is the browser's hybrid (QR plus Bluetooth) flow offered by `navigator.credentials.get`; the app adds no custom QR login. Support varies by browser version (exact versions unverified; https://www.corbado.com/blog/webauthn-passkey-qr-code.md, read 2026-11); the passkey UI text must not promise it.
- Challenges: stored in `challenges` (5 min, single use), bound to purpose and, for re-auth, to the session user. Expected origin is `RP_ORIGIN`, expected RP ID is `RP_ID`.
- `nodejs_compat` is on by default from the compatibility date 2026-08-04 (https://developers.cloudflare.com/workers/runtime-apis/nodejs/, read 2026-11); the spike must verify the library runs on Workers (the JSR listing states Workers support, https://jsr.io/@simplewebauthn/server@13.3.2, read 2026-11).

### 3.2 Recovery codes

- 10 codes, each 16 characters of Crockford base32 (80 bits of entropy) shown as `xxxx-xxxx-xxxx-xxxx`. Shown once at registration (and on regenerate), with a "I saved these" gate and a copy/download action.
- **Hash: HMAC-SHA-256 with a server-side pepper (`RECOVERY_PEPPER` secret), stored as hex in `recovery_codes.code_hash`.** Justification: the codes are high entropy (80 bits), so offline guessing after a database leak is infeasible even with a fast hash; a slow KDF buys nothing. Argon2 is not available in Workers without shipping WASM that would exceed the 10 ms free-plan CPU limit; PBKDF2 is available but would also strain that limit for no benefit. The pepper means a D1 leak alone cannot even be used to test guesses. Comparison is constant time.
- One-time: a used code gets `used_at`; `POST /api/auth/recover {handle, code}` consumes it and creates a `sessions` row with `recovery_only = 1`. That session can only add a passkey (`POST /api/passkeys/options|verify`) and view recovery status; all other sessions of the user are revoked, an `audit_log` entry is written, and the user is told how many codes remain.
- Regenerate: `POST /api/recovery-codes/regenerate` requires a fresh passkey assertion; it invalidates all old codes in one transaction (new `batch`) and returns 10 new ones once.
- Rate limits: 5 recovery attempts per hour per handle and 20 per hour per IP; failures are uniform (same response time and message for unknown handle and wrong code).
- If every passkey and every code is lost the account is unrecoverable (a decided product rule); journeys in section 3.6.

### 3.3 Sessions and cookies

- Domain: the official host is `https://zptah.app` (RP ID `zptah.app`, expected origin `https://zptah.app`). The RP ID must never change, because it would invalidate every passkey. Nothing else may be hosted on `zptah.app` or its subdomains, since another site there could weaken cookie and CSP isolation (the cookie stays `__Host-`). Self-hosters set their own `RP_ID` and origin.
- Cookie `__Host-mc_session`: opaque 256-bit random token; attributes `HttpOnly; Secure; Path=/; SameSite=Strict`, no `Domain` (required by `__Host-`). The server stores only `SHA-256(token)` as `sessions.id_hash`.
- `SameSite=Strict` is safe for this SPA because HTML and assets are served without auth and all API calls are same-site `fetch` from the already loaded app, including when a pairing link is opened from a QR scan (the page loads, then the API call carries the cookie).
- Lifetimes: idle timeout 7 days (`idle_expires_at`, extended on use at most once per hour to limit D1 writes), absolute 30 days (`expires_at`). `recovery_only` sessions last 30 minutes.
- Re-auth for sensitive actions: a fresh passkey assertion sets `reauth_at`; "fresh" means within 5 minutes. Required for: approve pairing, delete a desktop, delete a device, delete a passkey, regenerate recovery codes, revoke a grant, and every admin action.
- Logout deletes the row. Blocking a user deletes all their sessions.

### 3.4 CSRF

Layered: `SameSite=Strict`; an `Origin` header check on every non-GET **cookie-authenticated** request (session, fresh and admin routes, and the public browser auth routes; must equal `RP_ORIGIN`; requests without `Origin` are rejected); a required custom header `X-Monoptah-CSRF: 1` on the same routes (forces a CORS preflight that the Worker never grants); `Content-Type: application/json` only; no CORS headers. Token routes (`/api/desktop/*`, `/api/push/notify`) and the desktop pairing routes (`/api/pair/start`, `/api/pair/poll`) are called by the desktop app, which sends no `Origin`; they are exempt from these checks and must ignore cookies, so a browser request can never ride a session cookie into them. WebSocket upgrades from devices check `Origin` as well; the desktop's upgrade carries a bearer token instead.

### 3.5 Rate limits

The groups below the last row use the D1 `rate_limits` table (fixed windows; low-volume endpoints only) because the Workers Rate Limiting binding's availability on the free plan is unverified (https://developers.cloudflare.com/workers/runtime-apis/bindings/rate-limit/, read 2026-11). The general authenticated limit is an in-memory per-isolate counter, best effort (isolates are not shared, so the effective limit can be higher), so it writes nothing to D1 and stays inside the 100,000 writes/day free budget. Relay limits are in the DO (architecture.md 4.4).

| Endpoint group | Limit |
|---|---|
| `register/options`, `register/verify` | 10 per IP per hour; per invite code 5 attempts |
| `login/options`, `login/verify` | 30 per IP per 10 min |
| `auth/recover` | 5 per handle per hour, 20 per IP per hour |
| `pair/start` | 5 per IP per 10 min, 3 outstanding per IP |
| `pair/poll` | 1 per `interval` (3 s) per `deviceCode` |
| `push/notify` | 30 per desktop token per minute |
| Everything else authenticated | 120 per session per minute (in memory, per isolate, best effort) |

Exceeded limits return `429` with `Retry-After`; repeated breaches are written to `audit_log`.

### 3.6 Account journeys

| Journey | Behaviour |
|---|---|
| Registration | The user chooses an immutable `handle` (login and recovery identifier) and a `displayName`. The invite link uses a fragment, `/register#invite=CODE`, which keeps the code out of logs and history. After the passkey the recovery codes screen shows 10 codes with the handle in the download; `users.recovery_ack_at` is set when the user confirms "I saved these", and if it is null at the next sign-in the app forces a regenerate. Afterwards the app prompts to add a second passkey |
| Recovery | `POST /api/auth/recover` consumes the used code; device keys, desktops, grants and the invite count are unchanged; the app forces adding a passkey and prompts a regenerate when 2 or fewer codes remain |
| Admin block (single or branch) | `POST /api/admin/users/:id/block` sets `status = 'blocked'` (`blocked_via` for a cascade) and, in one batch: deletes the user's sessions, revokes desktop tokens and device grants, deletes push subscriptions, revokes unused invites, writes `audit_log`; the relay closes the user's sockets with `4011` on every desktop. The browser shows the blocked screen; the desktop stops reconnecting and shows "Web app: access revoked, pair again". Sign-in and recovery are refused. Passkeys, recovery codes, desktop rows and device keys are kept |
| Admin unblock (after a block) | `POST /api/admin/users/:id/unblock` restores sign-in only (`status = 'active'`, `blocked_via` cleared); descendants of a branch stay blocked until unblocked one by one ("unblock branch"). It never restores revoked tokens, grants, push subscriptions or invites: the user signs in again (sessions were deleted), pairs each desktop again (new token; "This replaces <desktop>" keeps the row, architecture.md 6.7), every device requests a grant again and is re-confirmed on the desktop, and notifications are enabled again. Invites revoked by the block stay revoked; the admin may grant new ones |
| All passkeys and all codes lost | The account is lost. The desktop owner clicks Unpair locally (revokes the token via `DELETE /api/desktop/me`, architecture.md 6.7); a new account needs a new invite |

## 4. Invites and admin

- Registration requires an invite code: 128-bit random, shown once to its creator, stored as SHA-256 in `invites.code_hash`, single use, expiry 7 days (configurable), revocable by the creator before use. Registration link: `/register#invite=CODE` (fragment, never sent to the server in a request line).
- Capacity control: two real switches (architecture.md 4.4). `MAX_USERS` (official host 150, self-host 0 = off) stops invites and registration when the `users` count reaches it; the admin "Pause invites" switch (`PUT /api/admin/settings {invitesPaused}`, D1 `settings`) does the same at once. While either holds, `POST /api/invites` answers `code: invites_paused` and registration fails with the generic invalid-invite error. The admin pauses at 70% of any free daily limit (capacity.md section 4); sign-in, existing users and issued invites are unaffected, and an issued invite cannot register while paused.
- Quota: each user has `invites_granted` (default 3) minus invites created; revoked-unused invites free a slot, used ones do not. Admin can grant more (`POST /api/admin/users/:id/invites`).
- Invite tree: `users.invited_by` and `users.invite_id` record who invited whom. The first admin comes from a `bootstrap` invite created by the self-host or ops script (architecture.md section 9).
- **Block semantics**: `POST /api/admin/users/:id/block` with `cascade: true` blocks the user and every descendant in the invite tree (recursive CTE over `invited_by`), setting `status = 'blocked'`, `blocked_via` = the root user id. Effects, applied in one batch: delete all their sessions, revoke all desktop tokens and device grants, delete push subscriptions, close their relay sockets (`4011`), revoke their unused invites, write `audit_log`. Data is kept for the audit trail. `cascade: false` blocks one user and leaves descendants untouched. Unblock is per user; unblocking the root does not unblock descendants (`blocked_via` lets the admin UI offer "unblock branch" as an explicit second action). Blocked users cannot log in, recover, or hold relay sockets.
- Admin page (`/admin`): list users with status and tree position, block (single or branch), grant invites, audit view. Admin routes need `role = 'admin'` and a fresh session. There is no admin access to content, because the server has none.

## 5. End-to-end encryption

- Noise `Noise_IK_25519_ChaChaPoly_SHA256`, device initiator, desktop responder (architecture.md 5.3). Prologue binds protocol version, desktop id, device id and channel.
- Keys: desktop static X25519 (sealed on the desktop, public half registered in `desktops.public_key` at pairing); device static X25519 per browser profile (wrapped in IndexedDB, public half in `devices.public_key`). Fingerprint = lowercase hex SHA-256 of the public key (stored in full); displayed as the first 32 hex characters (128 bits) in 8 groups of 4.
- Both libraries (`snow`, hand-written noble code) are unaudited; the JS handshake is verified against `snow` and the Noise test vectors, and the protocol is a security-review item in the roadmap before any release.
- **What the server and relay can see**: account handle, passkey metadata, desktop names, colors, OS, app version, device names and fingerprints, grant states and permissions, push endpoints, and per connection: who connected to which desktop, when, for how long, frame counts and sizes, IP addresses (as seen by Cloudflare), project display name and status in push payloads, `sessionId` in push payloads. 
- **What it cannot see**: prompts, assistant output, file contents, diffs, project paths and ids carried in requests, which methods are called, the git state.
- **Metadata leakage that remains**: activity timing and volume per desktop (a burst after a notification), message sizes correlated with content, and the fact that a specific account controls a specific desktop. No padding is applied in v1.
- The relay never terminates Noise, so a relay compromise yields metadata plus the ability to drop traffic, not content. Content confidentiality does not survive a malicious JS bundle (T2).

## 6. Device trust

- The account is a **directory, not the trust root**. Server records (`desktops`, `devices`, `device_grants`) let browsers find desktops and let the UI list devices, but the desktop decides who may talk to it.
- Every new device is `pending` on the desktop until a human confirms its name and fingerprint in Settings > Connections > Web app (architecture.md 6.3). This closes gap G6 (section 7; today pairing has no desktop confirmation, `host.rs:594-638`). Until confirmed the Noise session can only show the pending state; no method is served.
- The desktop checks the device static key from `HS1` against its own confirmed list (`Gate`-style map, `crates/mobile-link/src/lib.rs:160-177`), not against the server directory, so a hostile server cannot add a device by editing D1.
- Revocation exists on both sides: (a) server: `PUT /api/desktops/:id/grants/:deviceId state=revoked`, `DELETE /api/devices/:id`, `DELETE /api/desktops/:id`, admin block: relay join blocked at once and sockets closed `4003`/`4011`; (b) desktop: "Revoke" removes the fingerprint from the Gate, drops its watches and rewrites `mobile-access.json` (existing `revoke`, `host.rs:407-419`), then reports state to the server. Either side alone is sufficient; revocation on the desktop does not depend on the server being reachable. In-flight requests finish (gap G12, accepted; the desktop rechecks admission before replying, `host.rs:726-728`).
- Per-device `permission` (`read`/`control`) is authoritative on the desktop. The server's `requested_permission` can only lower privilege without desktop action.
- New devices default to `read`. Re-registration (new browser profile, reinstall) creates a new device and requires a new confirmation. Every device needs one confirmation per desktop.
- `Device` stores `server_device_id`; the desktop rejects a session whose relay `deviceId` header does not match the id stored for that fingerprint.
- Revoking sessions (logout, "sign out everywhere", recovery, block) also closes their relay sockets: the relay socket attachment carries `sessionIdHash` and the Worker calls `room.revoke` on each of the user's desktops (architecture.md 4.4).
- Desktop-side revoke sends `state: revoked` plus `BYE` on live channels and reports the state to the server, which closes the sockets too. A revoked device has no tombstone on the desktop: it reappears only as a new pending request, which the server-side revoked grant already blocks at relay join.
- A pinned desktop key that changes is never re-pinned by a button: only the verify link or QR from the desktop (architecture.md 6.7).

## 7. Desktop hardening (same program)

### Gap codes

Gaps found in the existing mobile host (QUIC path), defined once here and referenced by code from every other document. Paths are in the repo as of 2026-11-17.

| Code | Gap |
|---|---|
| G1 | Project and Mono selection is only applied client-side in the phone app (`mobile/src/features/connection/selection.ts:21-40`); the host serves every project and any session id |
| G2 | No deny-list for sensitive files: `.env`, keys and `.git/` contents are listable and readable through `project.file` (`crates/monocode-gpui/src/mobile/project.rs:79-150`) |
| G3 | No per-device permission: every method is open to any paired device (`crates/monocode-core/src/mobile/host.rs:62-69, 657-663`) |
| G4 | `runtimeMode: "full-access"` can be enabled remotely through `session.configure` and `session.start`, which means remote arbitrary command execution |
| G5 | No audit log of enrolls, revokes or method calls on the desktop |
| G6 | Pairing has no on-desktop confirmation of the joining device, and the phone QR embeds a private key (`host.rs:594-638`) |
| G7 | No throttling of a paired but compromised device beyond 16 pending requests and 4 watches (`host.rs:57-58`) |
| G8 | Push payloads carry assistant text and session titles to Expo, Apple and Google in clear (`crates/monocode-core/src/mobile/notify.rs:58-72`) |
| G9 | `mono.saveFile` can overwrite `SOUL.md` and `MEMORY.md`, which feed the agent (persistent prompt injection) |
| G10 | `project.gitAction` can push and open PRs with the desktop's git and `gh` credentials (`project.rs:435-457`: `commit` with `push`/`createPr`, `sync`, `pr`) |
| G11 | `session.delete`, `mono.reset` and `mono.delete` are irreversible and need no desktop confirmation |
| G12 | Revocation is eventually consistent: an in-flight request that passed the gate still completes (`host.rs:726-728`) |
| G13 | `workspace.list` discloses the absolute filesystem path of every project |
| G14 | Errors are unstructured strings, so clients branch on text (`crates/mobile-link/src/transport.rs:165-168`) |
| G15 | The mobile host has no cloud methods and no cloud policy. Adding them exposes billable lifecycle (wake, resume, start) and irreversible delete to remote devices; a send to a stopped cloud session already wakes it (`crates/monocode-gpui/src/conversation/remote.rs:432`), and `mobile_target` resolves any stored session id without a project check (confirmed, `crates/monocode-gpui/src/shell/mobile_host.rs:632-650`) |

### Decisions

Decisions for each item, with the gap it closes:

| Item | Gap | Decision |
|---|---|---|
| Enforce project and Mono selection on the desktop | G1 | `Device` stores `projects` and `monos`. `MobileHost::handle` filters `workspace.list` and `sessions.list` and rejects any `projectId`, `conversationId` (resolved to its project in `mobile_target`, `mobile_host.rs:632`) or `monoId` outside the selection with `code: not_selected`. **Applies to every device, phones included**: phones get a project selection on their row under desktop Settings "Your phones" (default: all projects), replacing the phone-only filter in `selection.ts`. The new `device.selection` method: get is `read`; set may only narrow the selection, and widening needs confirmation on the desktop |
| Per-device permission | G3 | `read` vs `control` per the classification table in architecture.md 7.1, checked in `handle` and `watch` before `Event::Request`. `Request` and `WatchStart` carry `device: DeviceContext {id, kind, permission, projects, monos}` so core can enforce permission class, web-only denials and selection; the web link has its own `web_enabled` flag, independent of the QUIC `enabled` flag (architecture.md 6.1) |
| Forbid full-access from remote | G4 | `runtimeMode: "full-access"` is rejected in `mobile_session_command`/`mobile_start` and removed from `models.list` `runtimeModes` for every remote device (web and phone). A session already in full-access can still receive prompts, and the web UI shows a persistent warning banner |
| Deny-list for files | G2 | Pattern list below, applied after canonicalization (symlink targets included), case-insensitively, in `project::file`, `files` (entries hidden), `search` (results removed), `fileHighlight`, and `changes` (hunks of denied files replaced by a "hidden: sensitive file" marker). Applies to all remote devices |
| Audit log | G5 | Append-only JSONL `mobile-audit.jsonl` in the data dir, rotated at 5 MiB (5 files kept). Fields: `ts`, `deviceId`, fingerprint prefix, transport (`quic`/`web`), `method`, `projectId`, `outcome` (`ok`/`denied:<code>`/`error`), command id. Parameters and content are never logged except the requested path for file reads. Events: enroll, pending, confirm, deny, revoke, connect, disconnect, permission change, every denied call. Viewable under Settings > Connections > Activity log |
| Pairing QR embeds a private key (phone path) | G6 | Not carried over: the web pairing carries only a public user code and a fragment fingerprint. The QUIC phone flow is out of scope for this program |
| Push bodies carry assistant text | G8 | Web path sends minimal metadata only (architecture.md section 8). The Expo path is unchanged and tracked separately |
| Remote cannot approve permission requests | none (design limit, `crates/monocode-gpui/src/conversation/mobile.rs:214`) | Remote approval is in scope (features.md X-01, P1; roadmap.md H-08, U-08, U-13) under these rules: only devices with `control`; one decision per request (`allow` or `deny` once); never "always allow"; never enables full-access remotely (H-03 stays); the desktop re-checks permission, project selection and that the request is still pending; every answer is audited; it ships only after its own security review gate (U-13), because it turns an XSS into direct execution. Until it ships, the web app shows "Waiting for approval on desktop" notices like mobile (`crates/monocode-gpui/src/mobile/transcript.rs:176-187`) |
| `mono.saveFile` writes `SOUL.md`/`MEMORY.md` | G9 | Denied for web devices in v1 (`code: forbidden`); reading stays. Revisit with desktop-side confirmation |
| `project.gitAction` push and PR | G10 | There is no `push` or `createPr` action: the desktop pushes through `sync`, `pr`, and `commit` with `push: true` or `createPr: true` (`crates/monocode-gpui/src/mobile/project.rs:436-455`). Web devices are denied `sync`, `pr` and a pushing `commit` (`code: forbidden`) unless the desktop toggle "Remote push in Git" is on (default off). `stage`, `unstage`, `generate` and a `commit` without push follow `control` |
| `session.delete`, `mono.reset`, `mono.delete` | G11 | `session.delete` allowed under `control` (logged); `mono.reset` and `mono.delete` denied for web devices in v1 |
| In-flight requests after revoke | G12 | Accepted (low): the desktop rechecks admission before replying (`host.rs:726-728`) |
| Absolute paths in `workspace.list` | G13 | Accepted (low). Web devices receive the same catalog; `projectId` embeds the path, so removing it needs a separate change |
| No rate limit for a paired device | G7 | Web sessions inherit the host limits (16 pending, 4 watches per device, `host.rs:57-58`) plus the relay quotas |
| Unstructured error strings | G14 | Additive `code` field (architecture.md 5.5) |
| Cloud session methods (`cloud.instances`, `cloud.startOptions`, `cloud.stop`, `cloud.resume`, `cloud.retryStart`, `cloud.keepRunning`, moves, start with `target`, archive) | G15, T16 | `cloud.instances` and `cloud.startOptions` are `read` (cached, rate limited, never wake); stop, resume, retry, keep running, move, archive and start are `control`. Billing rule: control plus a cost notice for send-to-stopped, resume, retry, start, move, archive-with-conversion and continue locally; Stop is control without a cost notice; delete, create and rebuild stay denied (`session.delete` on a cloud session answers `unsupported_target`). Start of billable sessions additionally needs the desktop toggle "Remote cloud start" (default off), moves need it and "Remote push in Git". The client can never set `kit`, `template`, `image`, `cpus`, `memory` or a `ttl` in a start `target` (`code: forbidden`). Failed and orphan billing instances are named only to devices whose selection is all projects. Selection (G1) applies through the local twin project of a cloud session and `cloud.instances` is filtered to instances of selected projects; `mobile_target` rejects a session outside the selection. Rate limit and audit as T16. Frames never carry tokens, credentials, env, `web_url`, SSH targets, `remote_cwd`, raw logs or step detail; `startup.error` is sanitized and capped (architecture.md 6.6). Instance names are not put in push payloads or the Worker audit |

Default deny-list patterns (all matched against the path relative to the project root and against any path component):

```
.env  .env.*  (allow: .env.example .env.sample .env.template)
*.pem  *.key  *.p12  *.pfx  *.keystore  *.jks  id_rsa*  id_ed25519*  id_ecdsa*
.git/**  .ssh/**  .aws/**  .gnupg/**  .kube/config  .docker/config.json
.npmrc  .pypirc  .netrc  .htpasswd  credentials*.json  secrets.*  *.tfstate  *.kdbx
```

The user can add patterns per project; the defaults cannot be removed by a remote device. The matcher runs in core so it is unit-tested once and reaches every caller (project rule "fix in the shared component").

## 8. XSS defenses

Rules for the renderer (enforced by lint and tests):

1. No raw HTML: the markdown renderer is a port of `mobile/src/features/sessions/markdownParser.ts` that produces an AST, which is rendered as React elements (any HTML in the input stays literal text). No third-party markdown-to-HTML pipeline is used. There is no sanitizer because there is no HTML sink. `dangerouslySetInnerHTML`, `innerHTML`, `outerHTML`, `insertAdjacentHTML`, `document.write` and `eval`/`new Function` are banned by ESLint (`no-restricted-syntax`) and by Trusted Types at runtime.
2. Link URL allowlist: only `https:`, `http:` (shown with a warning), `mailto:` and in-app `/d/...` relative links; everything else (including `javascript:`, `data:`, `vbscript:`, `blob:`) renders as plain text. Links get `rel="noopener noreferrer nofollow"` and `target="_blank"`. Link text and destination are both shown for external links via a confirm sheet.
3. Code: `shiki` with the **JavaScript regex engine** (no WASM, so no `wasm-unsafe-eval` in CSP); output is consumed as tokens (`codeToTokens`) and rendered as `<span>` elements with colors from CSS variables, never as an HTML string.
4. Images: remote images in markdown are not loaded (no `img-src https:`) because an image URL is a data-exfiltration channel; shown as a link. Attachments shown from `blob:`/`data:` URLs created by the app.
5. Untrusted strings (device names, desktop names, file names, session titles) are always React text children, never attribute-built strings; `style` values from content are never accepted.
6. Frames from the desktop are validated against the `web/protocol` types before reaching components.
7. Copied code: files copied from the old Tauri React app (`src/`) get the same rules (no HTML sink, no Tauri APIs, no `monocode.*` keys) and a CI guard fails the build if anything under `web/` references `src/` or `src-tauri/` (roadmap.md W-20, tauri-reuse.md). Known sinks in the originals (`AgentMarkdown.tsx:996` mermaid, `FileTypeIcon.tsx:78`) are replaced, not wrapped.
8. Dependencies: pnpm lockfile, `--frozen-lockfile` in CI, no post-install scripts (`pnpm` `onlyBuiltDependencies` allowlist), Dependabot review, no third-party scripts, fonts or analytics.

### 8.1 Content-Security-Policy (proposed verbatim, sent by the Worker on every HTML response and on `sw.js`)

```
Content-Security-Policy: default-src 'none'; script-src 'self'; style-src 'self'; style-src-attr 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self'; connect-src 'self' wss://HOST; worker-src 'self'; manifest-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'; object-src 'none'; require-trusted-types-for 'script'; trusted-types default; upgrade-insecure-requests
```

`HOST` is substituted with the request host by the Worker (some browsers do not match `'self'` to `wss:` on the same host). `style-src-attr 'unsafe-inline'` is needed only for `style` attributes written by motion and vaul; scripts stay strict. `worker-src 'self'` also covers the same-origin module workers the app starts from `/assets/*.js` (shiki tokenizing, the dither effect), not only `/sw.js`. Nonces are not used: the Vite build emits no inline scripts, and the theme is applied before first paint by a blocking same-origin `/theme-init.js` loaded in `<head>` (no inline theme script). Companion headers:

For static assets the same policy is written to `app/dist/client/_headers` at build time (`web/app/vite-plugins/headers.ts`, using the Worker's `buildCsp`), with `HOST` set to the host of `RP_ORIGIN` from `wrangler.jsonc` `vars` (`zptah.app` by default; self-hosters can override it with `MONOPTAH_RP_ORIGIN`).

```
Strict-Transport-Security: max-age=63072000; includeSubDomains; preload
X-Content-Type-Options: nosniff
Referrer-Policy: no-referrer
Permissions-Policy: camera=(), microphone=(), geolocation=(), payment=(), usb=()
Cross-Origin-Opener-Policy: same-origin
Cross-Origin-Resource-Policy: same-origin
Cache-Control: no-store            (on /api/* and /relay/*)
```

Headers for static assets are set from `app/dist/client/_headers`, which the app build generates (`web/app/vite-plugins/headers.ts`) with the CSP of 8.1 and the host of `RP_ORIGIN` for `connect-src` (override for self-hosters: `MONOPTAH_RP_ORIGIN`); API headers by Hono middleware.

### 8.2 Trusted Types

`require-trusted-types-for 'script'` with `trusted-types default` means any string reaching a DOM or script sink (`innerHTML`, `eval`, `script.src`, `new Worker`, `serviceWorker.register`) goes through the policy named `default`. The app creates it first thing in `main.tsx`:

```ts
window.trustedTypes?.createPolicy("default", {
  createHTML: () => { throw new TypeError("HTML sinks are disabled"); },
  createScript: () => { throw new TypeError("Script sinks are disabled"); },
  createScriptURL: (url) => {
    const u = new URL(url, location.href);
    const ok = u.pathname === "/sw.js" || (u.pathname.startsWith("/assets/") && u.pathname.endsWith(".js"));
    if (u.origin === location.origin && ok) return u.href;
    throw new TypeError("Blocked script URL");
  },
});
```

Browser support: Chromium browsers and Firefox per current releases; Safari and iOS Safari from 26.0 (caniuse; 18.6 and below do not support it). On unsupported browsers the header is ignored, so the strict CSP and the no-HTML-sink rules are the protection there; this is stated in the install page for iOS 18 and older and is a known residual of T1. Decision: accept this fallback (strict CSP, the AST renderer and the no-HTML-sink rules remain enforced); to be finalized after spike S-08. Library compatibility with Trusted Types (React, TanStack, motion, vaul, Workbox) is a spike item; any library that needs an HTML sink is rejected or patched.

## 9. Deploy security

Official host:

- Cloudflare account protected by hardware-key 2FA on every member; no shared logins; account audit logs monitored. Wrangler OAuth login exists only on the maintainer's Mac.
- Deploys only from the maintainer's Mac, via a scripted `pnpm deploy:official` that refuses unless the tree is clean, the branch is `main` and equals `origin/main`, the tests pass and `scripts/check-fork-branding.sh` passes. CI builds and tests but never deploys and holds no Cloudflare deploy token. Each deploy is recorded in the audit and the runbook. Agents and sandboxes never hold Cloudflare credentials and never run `wrangler deploy`, `wrangler d1 migrations apply --remote`, `wrangler secret put` or `wrangler login`; they use only local `wrangler dev` / Miniflare.
- Secrets (`VAPID_PRIVATE_KEY`, `RECOVERY_PEPPER`, `AUDIT_IP_SALT`) exist only as Worker secrets and in a sealed operator vault; never in git, `vars`, logs, or CI output. No secret is accessible to the SPA.
- VAPID rotation: generate a new keypair, deploy with both keys supported for sending (each `push_subscriptions` row records the key it was created with in `vapid_key_id`, so the sender signs with the matching key), ask clients to re-subscribe (a `pushsubscriptionchange`/re-subscribe prompt, because subscriptions are bound to the application server key), and remove the old key once no row references its `vapid_key_id`; rotation is also the response to a suspected private key leak. `RECOVERY_PEPPER` rotation is not supported without regenerating codes (users are prompted).
- D1: Time Travel for point-in-time restore, scheduled `wrangler d1 export` to an access-controlled bucket, restore drill documented.
- Subresource and supply chain: lockfile, CI provenance, release artifacts reproducible from the tag.
- Incident procedure: block a branch, revoke tokens and sessions, rotate secrets, publish a notice (the log has no content to leak, section 10).

Self-host: the owner is the operator and carries all of the above responsibilities. The guide (architecture.md section 9) sets safe defaults (secrets, custom domain, bootstrap invite expiry), but backups, 2FA on the Cloudflare account and update cadence are the self-hoster's.

## 10. Logging and privacy

- The Worker and DO log metadata only: request id, route, status, duration, user id (not handle), desktop id, close codes, rate-limit hits. Never request bodies, never frame bytes, never tokens, cookies, codes or push payloads. Logging of `Authorization` and `Cookie` headers is disabled; tail logs are not persisted.
- `audit_log` events (register, login, passkey changes, recovery use and regenerate, invite create/use/revoke, pairing approve, desktop/device create and revoke, grant changes, block/unblock, admin actions, rate-limit breaches) store ids and coarse metadata only (`meta` forbids secrets and content; IP stored as a salted hash, truncated country optional).
- Retention: `audit_log` 180 days, `sessions` until expiry plus 7 days, `pairing_requests` 24 h after resolution, `challenges` 1 h, `rate_limits` 1 day, push subscriptions until removal or failure, accounts until deleted; user-initiated account deletion removes all rows for the user (desktops, devices, grants, subscriptions, passkeys, codes) and leaves anonymized audit rows.
- The desktop audit log (section 7) stays on the desktop and is never uploaded.
- No analytics, no third-party scripts, no telemetry from the SPA.
