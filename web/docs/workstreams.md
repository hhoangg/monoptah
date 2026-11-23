# Monoptah web app: workstreams

How the tickets of [roadmap.md](roadmap.md) are split into parallel workstreams, which contracts must exist first, who owns which paths, and in which order work merges. Names (routes, tables, frames) come from [architecture.md](architecture.md); rules come from [security.md](security.md).

## 1. Purpose and rules

The program runs as several workstreams at the same time, each in its own sandbox and git worktree. This document removes the reasons they would block or break each other: contracts first, disjoint ownership, one owner per shared file.

### 1.1 Hard constraints

| Rule | Detail |
|---|---|
| No deploys from sandboxes | Sandboxes never deploy and hold no Cloudflare credentials. Allowed tools: local `wrangler dev` / Miniflare (D1 and DO), vitest (Workers pool), Playwright, cargo |
| Maintainer deploys | Only the maintainer runs `wrangler deploy`, remote D1 migrations and `wrangler secret put`, from their Mac, through `pnpm deploy:official` with its preflight checks (A-11). CI builds and tests but never deploys (A-10) |
| Free tier only | No paid Cloudflare feature, no new paid service. A change that needs one is a roadmap question (roadmap.md section 8), not a workstream decision |
| Rust work | Follow CLAUDE.md "Developing the native app": `native-dev` aliases (`cargo ntest`, `cargo nclippy`, `cargo ncheck`) from `crates/monocode-gpui`; edit first, then one fmt + clippy + filtered test pass per task; seed `crates/monocode-gpui/target` with `cp -c -R` from the main checkout; Linux-portable tests (`secondary-` keystrokes); fix in shared components (`modal.rs`, `popover.rs`); no realistic secrets in tests |
| Git | One branch per workstream task off `feat/web-app-impl`, merged back into it. Conventions in `docs/git-conventions.md` |
| Review | One review per request. Sonnet implements, Opus reviews the diff against the ticket acceptance list. High-risk tickets get their own review gate: R-13, H-07, W-13, U-13, plus their own reviews for billable cloud lifecycle: H-10, H-11, H-12, H-14, U-17, U-18, U-19 |
| Dev builds on the Mac | E-02 runs Native Dev from a clean worktree matching origin: `npm run native:dev -- --detach` |

### 1.2 Claiming a ticket

1. Check the workstream table (section 3): a ticket belongs to exactly one workstream. Take only tickets of your workstream, in the order of its section 5.
2. Check the ticket's deps in roadmap.md are merged into `feat/web-app-impl` (or the contract it needs is frozen, section 2).
3. Create `feat/web-<ws>-<slug>` and open a draft PR titled `feat(web): <ticket id> <title>` (type per ticket). The PR description lists the ticket id, the acceptance criteria as a checklist mapped to `file:line` and test, and the owned paths touched.
4. A ticket needing a path owned by another workstream uses the hook rules of section 3.2 or the contract-change procedure of section 2.3. Never edit the other path directly.

### 1.3 Definition of done (every ticket)

| Item | Check |
|---|---|
| Acceptance | Every acceptance criterion of the ticket in roadmap.md is met and has a test (or a recorded manual check where the criterion is visual or device-only) |
| Quality | Lint, typecheck and tests green for the touched packages (`pnpm check` for `web/`; fmt, clippy and filtered `cargo ntest` for Rust) |
| Docs | The owning doc updated (architecture.md, security.md, features.md or capacity.md); behaviour that differs from the doc is a doc change in the same PR |
| Ownership | No file outside the workstream's owned paths touched, except through section 2.3 or the hook rules of section 3.2 |
| Safety | No secret in repo or logs; no realistic fake tokens; copy tickets meet the roadmap copy checklist and W-20 |
| Review | Opus review of the diff done; high-risk tickets also pass their gate |

## 2. Contracts that land first (Wave 0)

Contracts are the shared surface. Workstreams code against them and against the mock desktop (g), so none waits for another's implementation.

### 2.1 Contract list

| # | Contract | Owner (ticket) | Files | Freeze rule |
|---|---|---|---|---|
| a | Frames, close codes, error codes, limits, `vectors.json`, `state` message with `capabilities` | WS-K (R-01, after S-04) | `web/protocol/src/{frame,chunk,close,errors,limits,state}.ts`, `web/protocol/vectors.json`, `crates/monocode-core/src/mobile/web_link/frame.rs` | Frozen when the TS and Rust round-trip tests pass on the same vectors |
| b | `/api/*` request and response types for every route of architecture.md 4.2, including `DELETE /api/desktop/me` and the verify route | WS-K (R-01) | `web/protocol/src/api.ts` (zod schemas plus inferred types) | Frozen when every 4.2 row has a type and the Worker stubs and web client compile against it |
| c | Desktop method params and results (including the wire types question reply, `RuntimeMode`, `Attachment`), `cloud` and `approvals` watch fields, Rust mirror | WS-K (R-01) | `web/protocol/src/methods.ts`, `web/protocol/src/watch.ts`, stubs `question.ts`, `models.ts`, `session.ts` (Wave 0), Rust mirror for the frame codec in `crates/monocode-core/src/mobile/web_link/frame.rs` | Frozen when every method of the policy table (h) has a typed entry |
| d | Full D1 schema and first migration | WS-A (A-02) | `web/worker/src/db/schema.ts`, `web/worker/migrations/0001_init.sql` | Frozen when all tables of architecture.md 4.3 plus `users.recovery_ack_at` and `users.display_name` exist and the `EXPLAIN` test passes |
| e | Worker skeleton: one Hono sub-app per module (stubs), DO export, `wrangler.jsonc` | WS-A (A-01) | `web/worker/src/index.ts`, `web/worker/src/cron.ts`, `web/worker/src/modules/*/{index,cleanup}.ts` (stubs), `web/worker/src/relay/DesktopRoom.ts` (stub class with `revoke` and `presence` no-ops; WS-R owns it after the freeze), `web/wrangler.jsonc` (declares every var and secret name the program needs: `MAX_USERS`, `MAX_DESKTOPS_PER_USER`, `MAX_DEVICES_PER_USER`, the VAPID names) | Frozen when every module mounts and returns `501 {error, code:"unsupported"}` and `wrangler dev` starts |
| f | Web app skeleton: route stubs for every route of architecture.md 3.1, design tokens | WS-S (W-01) | `web/app/src/routes/**`, `web/app/src/styles/**`, `web/app/src/ui/**`, `web/app/vite.config.ts` (with `@cloudflare/vite-plugin` configured, Wave 0; the A-01 `vite dev` check depends on it) | Frozen when every route renders a stub and tokens pass the W-01 visual check. After the freeze each route file belongs to the workstream of the route table in section 3.1 |
| g | Mock desktop (TypeScript): implements the protocol over the relay with canned data, including cloud and approval fixtures (it emits the `cloud` and `approvals` fields in reads and frames) | WS-L (supports R-10) | `web/mock-desktop/**` | Frozen when it completes Noise IK against the TS client in a vitest run and serves every method of (c) |
| h | Method permission table as data in core: `DeviceContext`, classification test that fails on an unclassified method, stubs for all new methods answering `unsupported` | WS-D2 (H-01, table part first) | `crates/monocode-core/src/mobile/policy.rs`; the `host.rs` seam (the R-07 seam, merged in Wave 0 as part of (h) by WS-D1: `policy::authorize` and `audit::record` calls, `DeviceContext` field on `Request` and `WatchStart`, `policy::filter_reply`, the `device.selection` arm, closing a device's open watches on a config change, section 3.2); all new `APP_METHODS` entries and the new method list (the cloud methods, `session.answerApproval` and `device.selection`, which `handle` answers itself) are added to `host.rs` here, in Wave 0, by WS-D1 as the host.rs owner, from the list WS-K defines in (c); the WS-D2 hooks in `mobile_host.rs` (cloud hooks including `cloud::read_fields` and `cloud::frame_fields`, `approvals` hooks, the `extra` parameter of `watch::update`); the module declaration files of 3.2 | Frozen when every entry of `APP_METHODS` and every new method has a class, the test is green, and every module of workstreams.md is declared with a stub file |

New error codes in (a), all additive `code` values on the existing error shape: `read_only`, `forbidden`, `not_selected`, `unsupported`, `unsupported_target`, `disabled`, `busy`, `starting`, `auto_stopping`, `instance_busy`. The `state` message carries `capabilities` (for example cloud support, remote cloud start, Git push toggle) so the web hides what the desktop does not offer (W-22).

### 2.2 Wave 0 order

| Step | Work | Notes |
|---|---|---|
| 1 | S-01..S-08 spikes run in parallel (WS-Q), plus the skeleton work below that needs no spike | Spikes produce findings, not product code (except vectors) |
| 2 | A-01 (e) and W-01 (f) start in parallel once S-05 and S-08 are in; A-02 (d) follows A-01 | Sequential: A-01 then A-02 |
| 3 | R-01 (a, b, c) after S-04 and a draft of (d), so `api.ts` matches the schema | Sequential: S-04 then R-01 |
| 4 | Mock desktop (g) after (a) and (c) are drafted; contract (h) after (c): first the R-07 host.rs seam (WS-D1: policy and audit hooks, reply filter, `device.selection` arm, closing open watches on config change, `DeviceContext` field on `Request` and `WatchStart`, `APP_METHODS` additions), then the policy table, method stubs, cloud and approvals hooks (WS-D2); the module declaration PRs of 3.2 (stub files, one per file owner) merge before any feature PR | Parallel with step 2; inside (h) the seam merges first. The rest of R-07 (`web_identity`, `Device` model) continues in Wave 1 |
| 5 | S-09 exit review, then contracts are tagged `contracts-v1` on `feat/web-app-impl` | Integration checkpoint of Wave 0 |

### 2.3 Contract change procedure (after freeze)

1. Open a small PR to the contract owner's paths only: the change, the version bump (`PROTOCOL_VERSION` in `limits.ts`, or the migration number for (d)), regenerated `vectors.json` and Rust mirror where relevant, and the doc edit.
2. The owner reviews it within the same day; contract PRs merge before any PR that uses them.
3. After merge, the owner posts the change (what, why, version) in the program thread and tags every dependent workstream. Dependents rebase before their next push.
4. Breaking changes (a removed or retyped field) need a second reviewer and a note in the PR for every consumer. Additive changes (new optional field, new code) do not.
5. Schema changes are new forward-only migrations numbered by WS-A, never edits of `0001_init.sql`.

## 3. Workstreams

### 3.1 Table

Tickets of roadmap.md plus the new ones (A-11, R-14, R-15, H-12..H-14, W-22, U-19, U-20, E-01, E-02). Each ticket has one owning workstream. R-14 and R-15 span three components, so they are split into slices at claim time (a server, b desktop core, c desktop UI, d web); the lead workstream closes the ticket. H-02 is led by WS-D2 and has one WS-D3 slice (the phone selection UI in `settings/pages/mobile.rs`).

| WS | Name | Tickets | Depends on | Can start after | Owned paths |
|---|---|---|---|---|---|
| WS-K | Contracts | R-01 | S-04 | S-04 done | `web/protocol/src/**` (except the three WS-T files), `web/protocol/{package.json,tsconfig*.json}`, `web/protocol/vectors.json` (S-04 in WS-Q creates it first, then WS-K owns it), `crates/monocode-core/src/mobile/web_link/frame.rs` |
| WS-A | Worker accounts and admin API | A-01, A-02, A-03, A-04, A-05, A-06, A-07, A-08, A-09, A-10, A-11 | S-05, S-09 (A-01); A-01 (A-02); A-02 (A-03, A-07); A-03 (A-04, A-06); A-03, A-04 (A-08); A-03, A-04, A-07 (A-05); A-04, A-06, A-07 (A-09); A-01..A-06 (A-10); A-02, A-10 (A-11); contract (d) for A-03+ | Wave 0 | `web/package.json`, `web/pnpm-workspace.yaml`, `web/tsconfig*.json`, `web/pnpm-lock.yaml` (regenerated), `web/worker/{package.json,tsconfig*.json,vitest.config.*}`, `web/worker/test/**` (except `relay/`), `web/worker/src/index.ts`, `web/worker/src/cron.ts`, `web/worker/src/db/**`, `web/worker/migrations/**`, `web/worker/src/middleware/**`, `web/worker/src/modules/{auth,sessions,recovery,invites,passkeys,admin,ratelimit,audit}/**`, `web/wrangler.jsonc`, `web/scripts/**`, `.github/workflows/web-ci.yml` |
| WS-R | Worker relay, pairing, devices, push server | R-02, R-03, R-04, P-01, P-02, P-06, slices R-14a, R-15a | A-04, R-01, S-07 (R-02); A-04, A-07 (R-03); R-02, R-03 (R-04); A-04, S-06 (P-01); P-01, R-03 (P-02); P-02 (P-06); as R-14 and R-15 (slices a) | Wave 1 (after A-04) | `web/worker/src/relay/**` (including `DesktopRoom.ts`, a stub from A-01), `web/worker/src/modules/{relay,pairing,desktops,devices,grants,push}/**`, `web/worker/test/relay/**` |
| WS-D1 | Desktop transport | R-05, R-06, R-07, R-11, R-14 (lead), R-15 (lead), slices R-14b, R-15b | R-01, S-04 (R-05); R-05 (R-06); none (R-07); R-05, R-06, S-02 (R-11); R-03, R-07, R-08 (R-14); R-04, R-05, R-09 (R-15) | R-07 can start at once; R-05 after R-01 | `crates/monocode-core/src/mobile/web_link/{mod,relay,noise,session,grants}.rs`, `crates/mobile-link/src/{transport,duplex,lib}.rs` (R-06 extracts the post-handshake logic of `serve`, `transport.rs` around line 91, into a generic over the duplex trait, which lives in the new `duplex.rs` of mobile-link because `serve` is generic over it; the QUIC impl stays in `transport.rs`, the Noise impl is `web_link/session.rs`; there is no `monocode-core/src/mobile/transport*.rs`), `crates/monocode-core/src/mobile.rs` (owner: module declarations), `crates/monocode-core/src/mobile/router.rs` (R-05 gating), `crates/monocode-core/src/mobile/host.rs` (owner), `crates/monocode-core/src/mobile/identity_store.rs` and `identity_store/**` |
| WS-D2 | Desktop hardening | H-01, H-02, H-03, H-04, H-05, H-06, H-08 | R-07 (H-01, H-04); R-07, H-01 (H-02); H-01 (H-03, H-06); R-07, R-09 (H-05); H-01, H-02, H-05 (H-08) | After R-07 | `crates/monocode-core/src/mobile/{policy,audit,denylist}.rs`, `crates/monocode-core/src/mobile/watch.rs` (owner), `crates/monocode-gpui/src/shell/mobile_host.rs` (owner), `crates/monocode-gpui/src/mobile/{project,approvals,transcript}.rs`, `crates/monocode-gpui/src/mobile/mod.rs` (owner: module declarations), the module declaration lines of `crates/monocode-gpui/src/shell.rs` and `crates/monocode-gpui/src/conversation.rs`, `crates/monocode-gpui/src/conversation/mobile.rs` (owner), `crates/monocode-gpui/src/settings/pages/web_activity.rs` |
| WS-D3 | Desktop Settings UI and confirmation | R-08, R-09, P-03, slice R-14c, R-15c, slice H-02 (phone selection UI; WS-D2 leads H-02), slices H-06 and H-11 (the Web app section toggle rows; WS-D2 and WS-D4 close on the core flag plus test) | R-03, R-07 (R-08); R-05, R-07, R-08 (R-09); R-07, P-02 (P-03); R-03, R-07, R-08 (R-14c); R-04, R-05, R-09 (R-15c); H-02 core part (H-02 slice, phone selection UI) | After R-07 (R-08 may use a fake `web::Status`) | `crates/monocode-gpui/src/settings/pages/mod.rs` (owner: module declarations), `crates/monocode-gpui/src/settings/pages/web_app.rs`, `.../pages/web_app/**`, `crates/monocode-gpui/src/shell/web_link.rs`, `crates/monocode-core/src/mobile/notify.rs`, `crates/monocode-gpui/src/settings/pages/mobile.rs` (owner), `crates/monocode-gpui/src/settings/mod.rs` (owner of the one registration line) |
| WS-D4 | Desktop cloud | H-09, H-10, H-11, H-12, H-13, H-14 | R-07, H-02 (H-09); H-01, H-05, H-09 (H-10); H-10, H-13 (H-11); H-10 (H-13); H-09, H-10 (H-12); H-06, H-10, H-11 (H-14) | After H-02 and H-09 contract fixtures | `crates/monocode-gpui/src/mobile/cloud.rs`, `crates/monocode-gpui/src/mobile/catalog.rs` (after H-02), `crates/monocode-core/src/mobile/cloud_policy.rs`, `crates/monocode-gpui/src/shell/cloud_app_archive.rs`, `crates/monocode-gpui/src/conversation/auto_stop.rs`, `crates/monocode-gpui/src/conversation/mobile_pending_send.rs` (new, H-10) |
| WS-L | Web link and data layer | R-10, W-05, W-22, slice R-14d, R-15d (client side) | R-01, R-04, S-03, S-04 (R-10); R-10, W-01 (W-05); R-10, W-05 (W-22); contracts (a), (c) drafted (mock desktop); as R-14 and R-15 (slices d) | Wave 0 (against the mock desktop) | `web/app/src/link/**`, `web/app/src/data/**`, `web/app/src/version/**`, `web/mock-desktop/**`, `web/app/src/routes/index.tsx` (`/`, the index redirect), `web/app/src/routes/d/$desktopId/route.tsx` (the `d/$desktopId` layout route with the `LinkProvider`) |
| WS-S | Web shell, accounts UI, settings, SW, admin | W-01, W-02, W-03, W-11, W-12, W-20, U-04, P-04, P-05 | A-01, S-08 (W-01); W-01, A-03, A-05, A-06 (W-02); W-02, R-03 (W-03); W-02, A-08, W-05, H-02 (W-11; buildable earlier against the mock desktop); W-01, R-10 (W-12); W-01 (W-20); A-09, W-02 (U-04); W-12, P-02 (P-04); W-11, P-01, P-04 (P-05) | W-01 in Wave 0; rest after W-01 | `web/app/src/{styles,ui}/**` (except `styles/markdown.css`, `ui/icons.tsx`, `ui/HarnessIcon.tsx` of WS-T and `ui/MatchText.tsx` of WS-P), `web/app/{package.json,index.html,public/**,tsconfig*.json,eslint.config.*,vitest.config.*}`, `web/app/src/sw.ts`, `web/app/src/routes/__root.tsx`, `web/app/src/routes/{register,login,recover,pair,settings,install,admin}{.tsx,/**}`, `web/app/src/routes/d/$desktopId/settings/**` (per-desktop settings), `web/app/src/main.tsx` (owner of the one-line slot registration import, section 3.2), `web/app/src/features/{accounts,settings,admin,push,offline}/**`, `web/app/vite.config.ts`, `web/app/src/routeTree.gen.ts` (generated), `scripts/check-web-isolation.*` (W-20), `.github/workflows/web-guard.yml` (W-20, a separate workflow from WS-A's `web-ci.yml`), `scripts/check-fork-branding.sh` (W-20, only the additive extension that scans `web/`) |
| WS-M | Web multi-desktop and devices | W-04, U-01 | W-01, R-04, R-10 (W-04); W-04, R-04, R-15 (U-01) | After W-01 and R-10 on the mock | `web/app/src/routes/desktops/**`, `web/app/src/features/desktops/**` |
| WS-T | Web sessions, transcript, composer | W-06, W-07, W-08, W-09, W-10, W-15, W-16, W-17, W-18, W-19, W-21, U-07, U-10, U-14, slices of U-09 | W-01, W-20 (W-15, W-17, W-18, W-19, W-21); W-01, W-20, S-08 (W-16); W-05 (W-06); W-05, W-16, W-21, S-08 (W-07); W-05, H-01 (W-08); W-08, H-03 (W-09); W-08 (W-10); W-09, W-11 (U-07); W-06..W-08, W-21 (U-10); W-07, W-16 (U-14); as roadmap for the U-09 slices | Copy tickets after W-20; rest after W-05 | `web/app/src/routes/d/$desktopId/index.tsx`, `web/app/src/routes/d/$desktopId/p/$projectId/{index,new}.tsx`, `web/app/src/routes/d/$desktopId/s/**` (explicitly not `p/$projectId/{files,changes,git}/**`, which WS-P owns), `web/app/src/features/{sessions,transcript,composer,markdown,models}/**`, `web/protocol/src/{question,models,session}.ts`, `web/app/src/lib/**` and `web/app/src/assets/**` (the W-15, W-16, W-21 copies; a file belongs to the workstream that creates it), `web/app/src/ui/{icons,HarnessIcon}.tsx`, `web/app/src/styles/markdown.css` |
| WS-C | Web cloud and approvals UI | U-08, U-16, U-17, U-18, U-19 | H-08, W-08, W-10, W-07 (U-08); H-09, H-10, H-12, W-07, W-08 (U-16); H-10, U-16 (U-17); H-11, H-13, U-16 (U-18); H-14, U-16 (U-19) | After W-07 and W-08 on the mock | `web/app/src/routes/d/$desktopId/cloud/**`, `web/app/src/features/{cloud,approvals}/**` |
| WS-P | Web parity extras | U-02, U-03, U-05, U-06, U-09 (lead; its transcript and drawer slices are WS-T), U-12, U-15, backlog U-11, U-20 | W-06, W-07 (U-02); W-01 (U-03); H-04, W-07, U-15 (U-05); H-06, U-05, U-15 (U-06); W-07, H-06 (U-09); W-15, W-20 (U-15); A-04 (U-11); various (U-12, U-20) | After the MVP (Wave 4) | `web/app/src/features/{files,changes,monos,layout,themes,account-extras}/**` (and the folders U-12 creates, each in its own `features/<name>/`), `web/app/src/lib/{fuzzy,fileKind}.ts`, `web/app/src/ui/MatchText.tsx` (U-15 copies), `web/app/src/routes/d/$desktopId/p/$projectId/{files,changes,git}/**`, `web/app/src/routes/d/$desktopId/monos/**` |
| WS-Q | QA and gates | S-01..S-09, E-01, E-02, R-12, R-13, W-13, W-14, H-07, U-13 | none (S-01, S-03..S-06, S-08); S-01 (S-07); S-07 (S-02); S-01..S-08 (S-09); A-03, R-02, R-05 (E-01); E-01, R-08, R-09 (E-02); R-02..R-10, E-01 (R-12); R-12, R-14, R-15 (R-13); W-07, W-12 (W-13); W-02..W-12, E-01 (W-14); H-01..H-06 (H-07); H-08, U-08, W-13 (U-13) | Wave 0 (spikes, E-01) | `web/e2e/**`, `web/spikes/**` (S-04 also creates `web/protocol/vectors.json`, then WS-K owns it), `crates/monocode-core/src/bin/web_link_headless.rs`, `web/app/tests/corpus/**`, `scripts/e2e-*`, `.github/workflows/web-e2e.yml` |

Tickets placed in WS-Q: the nine spikes S-01..S-09, E-01, E-02, R-12, R-13, W-13, W-14, H-07, U-13 (the harness, the gates and their reviews).

Route ownership (every route of architecture.md 3.1 has exactly one owner; the route stubs of contract (f) are created by WS-S and handed over by this table):

| Route (architecture.md 3.1) | Route file under `web/app/src/routes/` | Owner |
|---|---|---|
| `/` (index redirect) | `index.tsx` | WS-L (calls the last-place resolver of W-06) |
| `/login`, `/register`, `/recover`, `/install` | `login`, `register`, `recover`, `install` | WS-S |
| `/pair/:code` | `pair/**` | WS-S |
| `/settings/desktops`, `/settings/desktops/:id` (old `/desktops`, `/desktops/:id` redirect), `/desktops/:id/verify` | `desktops/**` | WS-M (the verify screen is a slice shared with WS-L) |
| `/d/:desktopId` layout (`LinkProvider`) | `d/$desktopId/route.tsx` | WS-L |
| `/d/:desktopId` (catalog home) | `d/$desktopId/index.tsx` | WS-T |
| `/d/:desktopId/p/:projectId`, `.../new` | `d/$desktopId/p/$projectId/{index,new}.tsx`, plus `route.tsx` (shared project layout, only if needed) | WS-T |
| `/d/:desktopId/p/:projectId/files/$`, `.../changes/$`, `.../git` | `d/$desktopId/p/$projectId/{files,changes,git}/**` | WS-P |
| `/d/:desktopId/s/:sessionId` | `d/$desktopId/s/**` | WS-T |
| `/d/:desktopId/monos/:monoId` and `.../profile` with its children | `d/$desktopId/monos/**` | WS-P |
| `/d/:desktopId/cloud` | `d/$desktopId/cloud/**` | WS-C |
| `/d/:desktopId/settings` (redirects to `/settings/desktops/:id`) | `d/$desktopId/settings/**` | WS-S |
| `/settings/*`, `/admin`, `/admin/users`, `/admin/invites` | `settings/**`, `admin/**` | WS-S |

### 3.2 Shared files: one owner, others hook in

| File | Owner | Who else needs it | How they hook in |
|---|---|---|---|
| `web/worker/src/db/schema.ts` and `migrations/**` | WS-A | WS-R (pairing, grants, push), WS-D1 | Whole schema lands in Wave 0 (d). A later change is a section 2.3 PR to WS-A, which allocates the next migration number. Nobody else adds a migration |
| `web/worker/src/index.ts` | WS-A | WS-R (DO export, relay routes) | WS-A mounts one sub-app per module at skeleton time and runs `cron.ts`, which calls `cleanup(env)` of every `modules/*/cleanup.ts` (stubs from A-01; WS-R fills pairing, grants and push). WS-R fills `modules/*/index.ts`; a new module is a one-line registration PR by WS-A. A-07 implements every row of the rate limit table of security.md 3.5 (pairing and push included) and the audit events of security.md 10; WS-R only uses their keys. `wrangler.jsonc` declares all vars at A-01 (contract (e)); a later var is a one-line PR to WS-A. A-04 and A-09 call `room.revoke` on the stub `DesktopRoom`; R-02 replaces its body |
| `crates/monocode-core/src/mobile/host.rs` | WS-D1 | WS-D2 (permission, selection, audit), WS-D3 (confirmation), WS-D4 (catalog scope) | R-07 adds, in Wave 0 as part of (h), the only host.rs edits others need, all with stubs that keep today's behaviour: `policy::authorize(&DeviceContext, &Request)` in `handle` after the device lookup and before the in-core `match method`, so the methods `handle` answers itself are checked too; `audit::record`; `policy::filter_reply(&DeviceContext, method, &mut Value)` applied to the `workspace.list` and `sessions.list` replies, which `handle` builds from `inner.catalog` and `inner.config` (`host.rs` about lines 650-700) and which never reach the app, so `authorize` cannot filter them (`device.leave` and `device.push` take the same pre-call check); `filter_reply` applies the selection filter and then calls `cloud_policy::scope_catalog(&DeviceContext, &mut Value)` (core, WS-D4 owned; the stub is a no-op because today's catalog holds no cloud or remote sessions and no new fields; H-09 makes it strip both for QUIC phones); a `device.selection` arm in `handle` that delegates to `policy::device_selection` (WS-D2) and persists the validated, narrower selection with the host's own `save`; on a device config change (selection, permission, revoke) host.rs closes every open watch of that device in `inner.watches` (the core `Watcher` stores only `peer` and `sink`, so core has no per-watch project check); the client reopens with `from` and the app-side check (H-02, `mobile_target`) runs again. WS-D2 fills `policy.rs` and `audit.rs`, WS-D4 fills `cloud_policy.rs`, never host.rs. Grant changes WS-D3 needs go through `web_link/grants.rs` events |
| `crates/monocode-gpui/src/shell/mobile_host.rs` | WS-D2 | WS-D4 (cloud methods), WS-D1 (web bridge) | Wave 0 (h) registers every new method as a one-line dispatch arm to `mobile::cloud::dispatch` or `mobile::approvals::dispatch` (stubs answering `unsupported`). WS-D4 and WS-D2 then fill their own modules. WS-D4 makes no direct edit of mobile_host.rs, including the `models.list` capability fields (WS-D2 adds those through the same hooks): its behaviour changes go through the cloud hooks of the next row. The web bridge of WS-D1 is one call `web_link::start(...)` where the host is opened (`mobile_host.rs` about lines 221-228), placed by WS-D2 in the same seam PR with a stub body in `web_link/mod.rs` |
| `crates/monocode-gpui/src/shell/mobile_host.rs` cloud hooks | WS-D2 | WS-D4 (H-09 to H-12) | In Wave 0 contract (h), WS-D2 adds one hook call at each point where cloud behaviour must differ, into functions of `crates/monocode-gpui/src/mobile/cloud.rs` (WS-D4 owned; WS-D2 creates the stubs in the same seam PR): `cloud::catalog_project_of` in `mobile_catalog_sessions` at the `path_key` project filter (H-09), `cloud::before_archive` in `session.archive` before `toggle_archive_now` and `cloud::before_delete` in `session.delete` (H-12), `cloud::before_send` at the head of `mobile_send` for the `starting` and `auto_stopping` refusals and wake on send (H-10, H-12), `cloud::start_target` in `mobile_start` (H-11), `cloud::read_fields(session, cx)` merged into the `mobile_read` result (`mobile_host.rs` about line 855, H-09), and `cloud::frame_fields(session, cx)` computed in `push_mobile_watches` (about line 1145) and passed into `watch::update` through the `extra` parameter (H-09). Stubs return none or keep today's behaviour exactly; a test asserts unchanged phone behaviour. WS-D4 then edits only `cloud.rs` |
| `crates/monocode-gpui/src/mobile/catalog.rs` | WS-D4 (after H-02) | WS-D2 (selection) | H-02 filters in core at `handle` level through `policy::filter_reply` over the catalog replies, so catalog.rs is not touched by H-02. Sequence: H-02 merges, then H-09 edits catalog.rs to add the cloud and remote sessions and their fields to the catalog; `cloud_policy::scope_catalog` (core, WS-D4) strips them again for QUIC phones |
| `crates/monocode-core/src/mobile/cloud_policy.rs` | WS-D4 | WS-D1 (declares the module and creates the stub in Wave 0), WS-D2 (calls) | Path and owner fixed here. Holds `scope_catalog` (called by `policy::filter_reply`, host.rs row) and `is_non_local_project` (called from `project.rs`). Stubs are no-ops until H-09 |
| `crates/monocode-core/src/mobile/web_link/frame.rs` | WS-K | WS-D1 | Contract (a); changes follow section 2.3 |
| `crates/monocode-gpui/src/settings/mod.rs` and Web app section toggles | WS-D3 | WS-D2 (Remote push in Git, H-06), WS-D4 (Remote cloud start, H-11), WS-D2 (activity log, H-05) | Policy flags live in core (`policy.rs` `PolicyFlags`, `cloud_policy.rs`). WS-D3 renders every toggle row and registers the pages in a follow-up task per flag; D2 and D4 deliver the flag and its test |
| `web/app/src/routeTree.gen.ts` | WS-S (generated) | everyone adding a route file | Never hand-merge: on conflict run `pnpm routes:gen` and commit the result. Route files are owned by the workstream of their prefix (owned-paths column) |
| `web/protocol/src/{question,models,session}.ts` | WS-T | WS-K | WS-K defines the wire types (question reply, `RuntimeMode`, `Attachment`) in `methods.ts` and creates these three files as stubs in Wave 0; after that WS-T edits them under copy tickets W-17 and W-18 and they re-export or extend the `methods.ts` types; no other workstream imports their internals beyond the exported types |
| `web/protocol/vectors.json` | WS-K | WS-D1, WS-L (consumers), WS-Q (S-04) | S-04 (WS-Q) creates the first file in Wave 0, before R-01; from R-01 on WS-K owns it, generated by a script in WS-K; regenerate in the contract PR, never edit by hand |
| Web slots in the session screens | WS-T | WS-C (target badge, cloud state chip, cost notices, approval card, archive keep or stop) | The registry (`registerSlot`, `<Slot name>`) is `web/app/src/ui/slots.ts` (WS-S, contract (f)), so W-11 does not wait for WS-T. WS-T creates five named slots as empty components in `web/app/src/features/sessions/slots/`: `composer-top` (W-08, holds the approval card and the cloud banners), `session-card-badge` (W-06), `session-header-chip` (W-08), `session-menu-items` (W-08), `archive-dialog` (W-08, replaced by the keep or stop dialog for cloud sessions). WS-C fills them with `registerSlot(name, Component)` from `features/cloud/register.ts`; the one import line of that file sits in `main.tsx` (WS-S, a one-line PR). WS-C never edits WS-T files. WS-T creates these further seams as no-op defaults in W-06 and W-08, filled by WS-C unless noted: `send-guard` (W-08; async hook that may confirm with a cost notice, refuse with a reason, or allow; used for send-to-stopped and `starting`, filled by U-16); `new-session-target` (W-08; a controlled slot on `p/$projectId/new` for the U-18 target picker with props `value` and `onChange(target | null)`: W-08 holds the state, reads `value` and passes it as `target` to `session.start`; the default no-op never calls `onChange`, so the value stays null, which means local); `desktop-nav-items` (W-06; drawer and sidebar nav entries, created by WS-T and also rendered by WS-P's U-02 sidebar; WS-C fills it with the "Cloud" link to `/d/:desktopId/cloud`); `session-capabilities` (W-06; predicate driven by `target.kind` and `capabilities` that hides Delete, Files, Changes and Git for non-local sessions, filled by U-16, consumed by WS-T menus and by WS-P's file, changes and git entry points); `desktop-settings-sections` (declared by WS-S in W-11 on `/d/:desktopId/settings`, filled by WS-T, which renders the U-07 default-model row there). Further slots, declared in Wave 0 with no-op defaults and filled through `register.ts` files of their workstream: in the `d/$desktopId` layout route (WS-L) `desktop-switcher` and `link-state-screens` (pending, revoked, key-changed; WS-M, `features/desktops/register.ts`), `first-run-picker` (the F-10 picker; WS-S) and `app-shell` (default passthrough, replaced by the U-02 layouts; WS-P, `features/layout/register.ts`); on `/settings` `settings-sections` (WS-S declares in W-11; WS-P fills for themes and account extras). The one import line of each `register.ts` (`features/{cloud,desktops,layout}/register.ts`, stub files created by WS-S) sits in `main.tsx`, placed in W-01. A slot needed later is a one-line PR to the slot owner. The transcript and drawer parts of U-09 (F-43 Mono "Thinking..." header, F-72 cards, Mono drawer rows) are WS-T slices of U-09; the lead stays WS-P |
| `crates/monocode-core/src/mobile/watch.rs` and the watch and read fields | WS-D2 | WS-D4 (`cloud` field, H-09) | core `watch.rs` never calls `cloud_policy`. WS-D2 adds, in Wave 0, one optional `extra: &Value` parameter to `watch::update` (`watch.rs` line 65): a map of field name to value that the baseline remembers, and the frame includes a key only when its value changed (an empty `approvals` array clears; an absent key means unchanged). The app computes the values: `push_mobile_watches` merges `cloud::frame_fields(session, cx)` (WS-D4, `mobile/cloud.rs`, placed by WS-D2) and `approvals::frame_fields(session, cx)` (WS-D2, `mobile/approvals.rs`) into `extra`; `mobile_read` merges `cloud::read_fields` and `approvals::read_fields` into its result. WS-D2 owns both approvals functions and `mobile_host.rs`, so H-08 needs no seam. Field shapes: architecture.md 6.6 |
| `crates/monocode-gpui/src/conversation/mobile.rs` | WS-D2 (H-08 `mobile_pending`) | WS-D4 (wake on send, H-10) | `mobile_submit` keeps its existing `defer_history_action` deferral (`mobile.rs:113-118`). WS-D4 adds only the `wake_for_send` call, the one-pending-send `busy` rule and the immediate `{ok: true, wake: true, queued: true}` reply, in the new file `conversation/mobile_pending_send.rs`, kept only for the busy flag (not a separate queue); WS-D2 adds the single call from `mobile_submit` to it as a small PR |
| `crates/monocode-gpui/src/mobile/project.rs` | WS-D2 | WS-D4 (`unsupported_target`, H-09) | WS-D2 adds one check at the top of each `project.*` entry that calls a WS-D4 predicate `cloud_policy::is_non_local_project(projectId)`; WS-D4 implements the predicate |
| `crates/monocode-gpui/src/shell/mobile_host.rs` beside `notify_mobile` | WS-D2 | WS-D3 (P-03 web push hook) | WS-D2 adds one call to `notify::notify_web(...)`; WS-D3 implements it in `crates/monocode-core/src/mobile/notify.rs` (its own module) |
| `APP_METHODS` in `crates/monocode-core/src/mobile/host.rs` | WS-D1 (host.rs owner) | WS-D2, WS-D4 | Every new method entry (cloud methods, `session.answerApproval`) is added once in Wave 0, contract (h), by WS-D1, from the list WS-K defines in contract (c). Later tickets fill the handler modules, never the array |
| `crates/monocode-gpui/src/settings/pages/mobile.rs` | WS-D3 | WS-D2 (H-02) | H-02 stays a WS-D2 ticket; its phone project-selection UI under "Your phones" is a WS-D3 slice (core types come from WS-D2) |
| Module declaration files | the owner named here | every later workstream | The Wave 0 seam PR of each owner declares every new module of this document, with a stub file, so no later ticket edits a mod file. Core `crates/monocode-core/src/mobile.rs` (WS-D1): `web_link`, `policy`, `audit`, `denylist`, `cloud_policy`; `web_link/mod.rs` declares its own children, `frame` included. `crates/monocode-gpui/src/mobile/mod.rs` (WS-D2): `cloud`, `approvals`. `crates/monocode-gpui/src/shell.rs` (WS-D2, declaration lines only): `web_link`. `crates/monocode-gpui/src/conversation.rs` (WS-D2, declaration lines only): `mobile_pending_send`. `crates/monocode-gpui/src/settings/pages/mod.rs` (WS-D3): `web_app`, `web_activity`, with the state re-exports. `web/app/src/main.tsx` (WS-S): the `register.ts` imports of the slots row. `crates/mobile-link/src/lib.rs` (WS-D1): `duplex`. A module added later is a one-line PR to the file's owner |
| Capability re-send | WS-D1 (`web_link::resend_state`, R-15b) | WS-D2 (`policy::capabilities`), WS-D3 (toggle rows) | WS-D2 derives the `capabilities` list from `PolicyFlags` and the cloud support in `policy::capabilities` (stub: today's list). Every toggle row of WS-D3 that maps to a capability ("Remote cloud start" to `cloudStart`, "Remote push in Git" to `gitPush`, `cloudMove`) calls `web_link::resend_state()` after saving its flag; the stub exists from Wave 0 and R-15 fills it so every live channel gets the new `capabilities` |
| Persisted web cache | WS-L (`data/persist.ts`, W-05) | WS-S (W-12) | W-05 defines the `PersistAdapter {get, set, clear}` interface with an in-memory default; W-12 implements the encrypted IndexedDB adapter in `features/offline/**` and registers it once at startup; forget and revoke call `clear` |
| Web package manifests | WS-A for `web/package.json`, `web/pnpm-workspace.yaml`, `web/tsconfig*.json` | each package owner for its own `package.json`, `tsconfig*.json` and test config: `web/protocol` WS-K, `web/worker` WS-A, `web/app` WS-S, `web/mock-desktop` WS-L, `web/e2e` and `web/spikes` WS-Q | A dependency addition is an additive-line PR to the owner of that manifest, naming the ticket and the reason. The lockfile `web/pnpm-lock.yaml` is regenerated per section 6, never hand-merged |
| Cargo manifests | WS-D1 for `crates/monocode-core/Cargo.toml` and `crates/mobile-link/Cargo.toml` | WS-D3, WS-Q (headless binary) | A dependency addition is an additive-line PR to the manifest owner. `crates/monocode-core/src/bin/web_link_headless.rs` (WS-Q) is picked up by Cargo's default `src/bin` discovery, so no manifest edit is needed. The root `Cargo.lock` is regenerated per section 6 |
| Existing desktop files with no workstream | the existing code owners | any ticket that reads them (`mono/cloud_api.rs`, `cloud_sessions/**`, `shell/cloud.rs`, `shell/app_commands.rs`, `conversation/notifications.rs`, `agent.rs`) | The program reads them. A ticket that needs more (a `pub(crate)` visibility change or a small accessor) makes only that additive change, in its own PR, and lists the file under "owned paths touched". No behaviour change there. New kinds or fields, such as the `remote` target kind, are computed in the ticket's own module (`mobile/cloud.rs`), not by editing `cloud_api.rs` |
| `docs/` files | owning doc per ticket | all | The ticket's PR edits the doc it is about (architecture.md, security.md, features.md). `roadmap.md` and this file are edited only by the program lead through small PRs |

## 4. Waves and parallelization

### 4.1 Dependency diagram

```
Wave 0                 Wave 1 (parallel)                  Wave 2                Wave 3                    Wave 4
S-01..S-08 (WS-Q)
  |
  +-> S-05 -> A-01 -> A-02 (d) --> WS-A: A-03..A-09 -------+--> A-10, A-11
  +-> S-08 -> W-01 (f) ---------> WS-S: W-20, W-02, W-11 ---+--> W-03, W-12 --> P-04, P-05
  +-> S-04 -> R-01 (a,b,c) --+--> WS-R: R-02, R-03, R-04 --+--> R-12 --> R-13 (GATE 2)
  |           |              +--> WS-D1: R-07, R-05, R-06, R-11 --+          |
  |           +--> mock (g) ---> WS-L: R-10, W-05, W-22 ----------+---> WS-M: W-04, U-01
  +-> (h) policy table ------> WS-D2: H-01..H-04 (after R-07) -----> H-05, H-06, H-08 -> H-07 (GATE 3)
                               WS-D3: R-08, R-09 (after R-07) ------> P-03
                                                          WS-T: W-15..W-21, W-06..W-10 --> W-13 (GATE 4), W-14
                                                                     WS-D4: H-09..H-14 --+--> WS-C: U-16..U-19, U-08
                                                                     WS-R/S: P-01, P-02, P-06 --> P-04, P-05 (GATE 5)
                                                                                                     WS-P: U-02..U-20
                                                                                                     U-13 (GATE 6), E-02 per phase
```

### 4.2 Waves

| Wave | Concurrent work | Strictly sequential | Integration checkpoint |
|---|---|---|---|
| 0 | Spikes S-01..S-08 (WS-Q); A-01 and W-01 skeletons; mock desktop (g) and policy table (h) once (a) and (c) are drafted; E-01 harness skeleton | S-04 then R-01 (a, b, c); A-01 then A-02 (d); S-09 last | Contracts tagged `contracts-v1`; `wrangler dev` runs the stub Worker; web stubs render; mock desktop completes a Noise handshake with the TS client; gate 0 findings in docs |
| 1 | WS-A (A-03..A-11), WS-R (R-02, R-03, R-04), WS-D1 (rest of R-07, then R-05, R-06, R-11), WS-D2 (H-01..H-04 after R-07), WS-L (R-10, W-05, W-22 against the mock), WS-S (W-20, W-02, W-11), WS-T copy tickets W-15..W-21 | R-03 then R-04; R-05 then R-06 then R-11; H-01 then H-02 then H-03; H-06 after H-01 (Wave 2); A-03 then A-04 then A-05..A-08; A-09 after A-06 and A-07; A-10 after A-06; A-11 after A-10 | A-01..A-09 API suite green on local `wrangler dev` (Miniflare), with no deploy; A-10 and A-11 merged; R-02..R-04 pass against the mock desktop; the real desktop `web_link` completes a handshake against local `wrangler dev` (first E-01 smoke). The first internal deploy is run by the maintainer through A-11 at the phase 1 gate, after this checkpoint |
| 2 | WS-D3 (R-08, R-09), WS-M (W-04, U-01), WS-T (W-06..W-10), WS-S (W-03, W-12, U-04), WS-D2 (H-05, H-06, H-08), R-14 and R-15 slices on all sides, R-12 integration | R-09 after R-05, R-07, R-08; R-12 after R-02..R-10; R-13 after R-12 | R-12 green in CI with the headless desktop; R-13 security gate passed (gate 2); no public host before it. E-02 run on the Mac |
| 3 | WS-D4 (H-09..H-14) with WS-C (U-16..U-19, U-08 on the mock), push (P-01, P-02, P-03, P-04, P-05, P-06), WS-Q (W-13, W-14, H-07) | H-09 then H-10 then H-13 then H-11 then H-14; H-12 after H-09 and H-10; P-01 then P-02 then P-03/P-04 then P-05; W-13 after W-07 and W-12 | Gates 3, 4 and 5 (H-07, W-13, push on devices, VAPID drill); WS-C against the real desktop in E-01; E-02 run before each gate |
| 4 | WS-P parity (U-02, U-03, U-05, U-06, U-09, U-12, U-15), WS-T (U-07, U-09 slices), WS-Q (U-13), E-02 runs | U-15 before U-05 and U-06; U-08 not released before U-13 | Gate 6 per ticket; U-13 passed before H-08 and U-08 are enabled; billable cloud tickets pass their own reviews |

Phase 7 backlog (U-11, U-20) is claimed after Wave 4 by WS-P, one ticket at a time.

## 5. Workstream sections

Common test pyramid: unit (vitest Workers pool, or `cargo ntest` filtered to touched modules), integration (`wrangler dev` with Miniflare D1 and DO), E2E (E-01 Playwright with the CDP virtual WebAuthn authenticator against local worker plus the headless desktop core or the mock desktop), Native Dev E2E (E-02, on the Mac), and visual checks at 390, 768 and 1280 px for every UI ticket.

### WS-K Contracts

| Item | Content |
|---|---|
| Goal | One source of truth for frames, API types, method types and vectors |
| Tickets | R-01 (a, b, c in that order) |
| Inputs | S-04 vectors, architecture.md 4.2, 5, 7.1 |
| Outputs | `web/protocol` package, Rust frame mirror, `vectors.json`, `contracts-v1` tag |
| Done | Section 2.1 freeze rules (a), (b), (c); every method of 7.1 and the new methods typed |
| Tests | TS and Rust round trip on shared vectors; unknown version or type rejected; size bounds 20 to 65,555 bytes; a type test fails when `api.ts` and the architecture.md route table diverge |
| Risks | A contract that changes weekly stalls everyone: freeze early, extend additively |

### WS-A Worker accounts and admin API

| Item | Content |
|---|---|
| Goal | Accounts, sessions, invites, recovery, admin, rate limits and audit, with deploy tooling |
| Tickets | A-01, A-02 (Wave 0); A-03, A-04, A-07 then A-05, A-06, A-08, A-09; A-10; A-11 (last) |
| Inputs | Contracts (b), (d), (e); S-05 findings |
| Outputs | `/api/auth`, `/api/me`, `/api/sessions`, `/api/passkeys`, `/api/invites`, `/api/admin`, middleware, cron, `pnpm bootstrap`, `pnpm deploy:official` |
| Done | Gate 1 list in roadmap.md; A-11 preflight checks documented; CI never deploys |
| Tests | Workers-pool vitest with a virtual authenticator for A-03; `EXPLAIN QUERY PLAN` test; log-grep test for recovery codes; Origin and CSRF tests; E-01 register and login flows |
| Risks | CPU cap of free plan on WebAuthn verify (S-05); cascade block correctness (A-09); account takeover surface in recovery (A-05) |

### WS-R Worker relay, pairing, devices, push server

| Item | Content |
|---|---|
| Goal | Authenticated, quota-limited relay and the pairing and grants authority on the server |
| Tickets | R-02, R-03, R-04, R-14a, R-15a, P-01, P-02, P-06 |
| Inputs | Contracts (a), (b), (d), (e); A-04, A-07 |
| Outputs | `DesktopRoom` DO, `/relay/*`, `/api/pair/*`, `/api/desktops`, `/api/devices`, desktop token routes, `DELETE /api/desktop/me`, persisted pending grants, push routes and sender |
| Done | R-02..R-04 acceptance; R-14a revoke route; R-15a persisted grants and `sessionIdHash` revoke; P-02 no-transcript test |
| Tests | DO tests in Workers pool (hibernation, quotas, revoke close within 1 s, no timers lint); token issuance tests for every wrong input; RFC 8291 vector; E-01 pair and revoke scenarios |
| Risks | Central trust component (R-02, R-03); server mirror diverging from desktop authority (R-04) |

### WS-D1 Desktop transport

| Item | Content |
|---|---|
| Goal | The desktop links outbound to the relay with Noise IK and reuses `MobileHost` |
| Tickets | R-07, R-05, R-06, R-11, then R-14, R-15 (lead, slices b) |
| Inputs | Contract (a), `vectors.json`; the hook seams for policy and audit (host.rs) |
| Outputs | `web_link/` module, duplex trait with QUIC and Noise impls, `web_identity` row, extended `Device`, key lifecycle (re-pair, close codes), grants and live `state` updates, `server_device_id` |
| Done | R-05..R-07, R-11 acceptance; old `mobile-access.json` loads and phones keep `control`; QUIC golden test unchanged |
| Tests | `cargo ntest` filtered (`mobile::`, `web_link::`); interop with TS vectors; golden QUIC test; fixture for old `mobile-access.json`; E-01 with `web_link_headless` |
| Risks | Regression of the shipping phone path (R-06); sealed storage migration (R-07). One test pass per task, APFS-cloned target dir |

### WS-D2 Desktop hardening

| Item | Content |
|---|---|
| Goal | The desktop enforces permission, selection, deny-list and audit for every device |
| Tickets | H-01 (table first), H-02, H-04, H-03, H-06, H-05, H-08 |
| Inputs | Contract (c) and (h); R-07 `Device`; security.md section 7 |
| Outputs | `policy.rs` (including `filter_reply`, `device_selection`, `capabilities`), `audit.rs`, `denylist.rs`, `approvals.rs` (`read_fields`, `frame_fields`), the `extra` parameter of `watch::update`, hooks in `mobile_host.rs` and `project.rs` |
| Done | Per-method permission tests; selection tests per method; deny-list tests incl. symlink and case; one audit line per event; H-08 stale and double answer refused; phones unchanged by default |
| Tests | Filtered `cargo ntest` (core for policy and deny-list, gpui for hooks); the unclassified-method test; QUIC phone regression pass |
| Risks | Easy-to-miss call path for selection (H-02); path bugs are exploit paths (H-04); new remote execution path (H-08, gated by U-13) |

### WS-D3 Desktop Settings UI and confirmation

| Item | Content |
|---|---|
| Goal | Pair, list and revoke from desktop Settings; confirm devices locally |
| Tickets | R-08, R-09, P-03, slices R-14c, R-15c, H-02 (phone selection UI), H-06 and H-11 (toggle rows) |
| Inputs | R-03 routes (or a fake client), R-07 model, contract (a) states |
| Outputs | `web_app.rs` page and modal (via `modal.rs`), confirmation dialog, pending badge and notification, toggle rows, notify hook |
| Done | All states reachable and GPUI-tested; unknown device gets nothing before confirmation; QR carries only user code and fingerprint |
| Tests | GPUI tests, Linux-portable, one filtered pass; screenshots via Native Dev on the Mac (E-02) |
| Risks | The trust root of the design (R-09); toggles depend on flags from D2 and D4 |

### WS-D4 Desktop cloud

| Item | Content |
|---|---|
| Goal | Cloud sessions visible and safely controllable from the web, never deletable |
| Tickets | H-09, H-10, H-12, H-11, H-13, H-14 |
| Inputs | H-02 selection, H-05 audit, `cloud_api` (`mono/cloud_api.rs`), contract (c) `cloud` watch field |
| Outputs | `mobile/cloud.rs` (including `read_fields` and `frame_fields`), `cloud_policy.rs` (`scope_catalog`), catalog annotation, lifecycle rules of H-12, `cloud.startOptions` and restricted `target` |
| Done | H-09..H-14 acceptance; no secrets in any frame (test); QUIC phone gets today's lists; reads never wake an instance |
| Tests | Filtered `cargo ntest` with fixtures for codespace, sandbox, remote, local and stopped instances; rate-limit tests; own Opus review for H-10, H-11, H-12, H-14 (billable) |
| Risks | Billable side effects; shared instances; async wake past the 25 s `REPLY_WAIT` |

### WS-L Web link and data layer

| Item | Content |
|---|---|
| Goal | Browser link client and data layer all web UI sits on |
| Tickets | Mock desktop (g), R-10, W-05, W-22; R-14d and R-15d client slices |
| Inputs | Contracts (a), (b), (c); S-03, S-04 findings |
| Outputs | `link/`, `data/` (including the typed hooks `useCloud(sessionId)` and `useApprovals(sessionId)`), `version/`, mock desktop, capability gating helpers |
| Done | R-10, W-05, W-22 acceptance; no private key in clear in IndexedDB; capability and version handling hides unsupported features |
| Tests | Vitest in Node and browser against the mock; close-code table test; port of mobile watch tests; E-01 with the mock |
| Risks | Crypto glue (R-10); state-logic edge cases (W-05) |

### WS-S Web shell, accounts UI, settings, SW, admin

| Item | Content |
|---|---|
| Goal | App frame, auth screens, settings, offline and push UI, admin page |
| Tickets | W-01 (Wave 0), W-20, W-02, W-11, W-03, W-12, U-04, P-04, P-05 |
| Inputs | Contracts (b), (f); A-03..A-09 routes or Worker stubs |
| Outputs | Tokens and `ui/`, auth routes, settings, `sw.ts`, push handlers, `/admin`, isolation guard |
| Done | W-01 checklist items; W-20 red on seeded violations; no `/api` or `/relay` response cached |
| Tests | Vitest and Testing Library; Playwright with virtual authenticator; axe; visual at 390, 768, 1280 and 200 percent zoom; push on devices by the maintainer |
| Risks | Service worker cache bugs (W-12); iOS push quirks (P-04) |

### WS-M Web multi-desktop and devices

| Item | Content |
|---|---|
| Goal | List, switch, register devices and manage desktops |
| Tickets | W-04, U-01, slice R-14d `/desktops/:id/verify#fp=` re-pin screen with WS-L |
| Inputs | R-04 types, `web/app/src/link`, mock desktop |
| Outputs | `/desktops`, `/desktops/:id`, pending, revoked and key-changed screens |
| Done | W-04 and U-01 acceptance; pending screen shows both fingerprints |
| Tests | Mock-desktop E2E for pending, confirm, revoke, key change; visual checks |
| Risks | Phishing defence depends on correct UX |

### WS-T Web sessions, transcript, composer

| Item | Content |
|---|---|
| Goal | The core reading and sending experience |
| Tickets | W-15, W-16, W-17, W-18, W-19, W-21 (copy, first), then W-06, W-07, W-08, W-09, W-10, U-14, U-10, U-07; the transcript and drawer slices of U-09 |
| Inputs | Contract (c), W-05 data layer, W-20 guard |
| Outputs | Session list, transcript, safe markdown, composer, pickers, question card |
| Done | Copy checklist for every copy ticket; renderer passes the W-13 corpus; no path sends `full-access` |
| Tests | Copied tests adapted; corpus tests; jsdom scroll-anchor test; iOS PWA manual resume check (N-16); E2E on the mock |
| Risks | XSS surface (W-07, W-16); iOS scroll and keyboard (W-07, W-08) |

### WS-C Web cloud and approvals UI

| Item | Content |
|---|---|
| Goal | Cloud state, instance management and remote approvals in the web |
| Tickets | U-16, U-17, U-08, U-18, U-19 |
| Inputs | H-09..H-14 fields (contract (c)), W-07, W-08, W-10 |
| Outputs | Target badge, startup list, cost notices, `/d/:id/cloud`, approval card, start and move UI |
| Done | Per-ticket acceptance; no delete or create control exists (test); approvals off until U-13 passes |
| Tests | Visual test per state and step status; mock desktop cloud and approval fixtures; a11y announcements |
| Risks | Billable actions in the UI; approval card turns XSS into execution (U-08) |

### WS-P Web parity extras

| Item | Content |
|---|---|
| Goal | Parity features after the MVP |
| Tickets | U-15, U-05, U-06, U-02, U-03, U-09 (lead), U-12; backlog U-11, U-20 |
| Inputs | H-04, H-06, W-07, W-09, the `session-capabilities` predicate (3.2) |
| Outputs | Files, changes, git, layouts, themes, Monos, extras |
| Done | Per-ticket acceptance; features respect denied-file and Git toggle states |
| Tests | As WS-T; layout tests at 768 and 1280 |
| Risks | Layout work touches every screen (U-02); repo writes visible outside the machine (U-06) |

### WS-Q QA and gates

| Item | Content |
|---|---|
| Goal | Measurements, harness, integration test and independent gates |
| Tickets | S-01..S-09, E-01, R-12, R-13, W-13, W-14, H-07, U-13, E-02 |
| Inputs | Everything merged on `feat/web-app-impl` |
| Outputs | Findings notes, `web/e2e`, headless desktop binary, gate reviews recorded in PRs |
| Done | Gate criteria of roadmap.md 0.2 |
| Tests | E-01: Playwright plus CDP virtual WebAuthn authenticator against local `wrangler dev` and the headless core running `web_link`, in CI and in sandboxes. E-02: Native Dev on the Mac (`npm run native:dev -- --detach`, dev builds accept an `http://localhost` host URL) before each phase gate |
| Risks | Harness effort (R-12); reviewers must read code, not rerun suites except to prove a finding |

## 6. Merge order and conflict avoidance

| Topic | Rule |
|---|---|
| Branch names | `feat/web-<ws>-<slug>`, for example `feat/web-d2-permission-table`; `<type>` follows git-conventions (`fix/`, `docs/` allowed). Integration branch: `feat/web-app-impl` |
| Rebase | Rebase on `feat/web-app-impl` at least daily and before opening or updating a PR; never merge it into your branch |
| Merge order | Contracts first (a, b, c, d, e, f, g, h); then shared-file owners' seam PRs (R-07 hooks, route registration); then feature PRs in dependency order of the roadmap; docs-only PRs any time |
| Merge method | Squash into `feat/web-app-impl`; the final PR of `feat/web-app-impl` to `main` is a normal reviewed PR per phase gate |
| Contract merges | The contract owner (WS-K, WS-A for the schema, WS-S for the skeleton, WS-D2 for the policy table) merges contract PRs; nobody merges their own contract change without the owner review |
| Generated files | `migrations/**` and the migration journal: WS-A only. `vectors.json` and the Rust frame mirror: WS-K only, regenerated by script. `routeTree.gen.ts`: regenerate on conflict. `Cargo.lock` and `pnpm-lock.yaml`: regenerate, never hand-merge; dependency additions are listed in the PR |
| New dependencies | Added by the workstream that needs them, free-tier compatible and license-checked; the PR names why |
| Reviews | One Opus review per request, after the Sonnet implementer's checklist; reviewers are read-only. Gate reviews (R-13, H-07, W-13, U-13) are separate requests by WS-Q |
| Conflicts | If two PRs touch one file, the file owner of section 3.2 decides the order; the later PR rebases |
| Cleanup | After a PR merges, clean the worktree and branches with the `cleanup-worktree` skill |

## 7. Release gates mapping

| Roadmap gate | Phase | Reached at | Evidence |
|---|---|---|---|
| 0 | Spike | End of Wave 0 | Findings per spike, S-09 exit review, `contracts-v1` tag |
| 1 | Accounts | Wave 1 checkpoint, then the maintainer's first internal deploy | API suite green on local `wrangler dev`, no secret in logs, CSRF and Origin tests; the maintainer's internal deploy through A-11 closes the gate |
| 2 | Relay and pairing | End of Wave 2 | R-12 green, R-13 findings fixed, QUIC phone regression green; E-02 run. No public host before this |
| 3 | Desktop hardening | Wave 3 (H-07) | H-07 review, deny-list and permission tests, phones not broken. Required before any non-team user gets `control` |
| 4 | Web MVP (includes U-01, U-04) | Wave 3 (W-13, W-14) | XSS gate, accessibility pass, E-01 flows; alpha to invited testers |
| 5 | Push | Wave 3 | Push on iOS PWA, Android Chrome and one desktop browser; no-transcript test; VAPID drill (P-06); then public beta after security.md section 9 items |
| 6 | Parity, cloud sessions and approvals (committed) | Wave 4, per ticket | U-13 before H-08 and U-08 are enabled; own reviews for H-10, H-11, H-12, H-14, U-17, U-18, U-19; E-02 before each phase gate |
| 7 | Backlog (U-11, U-20) | After Wave 4 | Per ticket |
