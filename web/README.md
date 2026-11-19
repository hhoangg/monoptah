# Monoptah web

The browser client for Monoptah: a React SPA served by a Cloudflare Worker, with a Durable Object relay
and D1 storage, talking to the user's desktop app over an end-to-end encrypted channel.
The spec is in [`docs/README.md`](docs/README.md) (architecture, security, design system, workstreams, roadmap).

## Layout

| Path | Package | What it is |
|---|---|---|
| `protocol/` | `@monoptah/protocol` | Wire contracts, zod schemas, test vectors (`vectors.json`, `methods.json`) |
| `worker/` | `@monoptah/worker` | Worker, `DesktopRoom` Durable Object, D1 schema and migrations |
| `app/` | `@monoptah/app` | Vite + React SPA, design tokens, TanStack Router routes |
| `mock-desktop/` | `@monoptah/mock-desktop` | Scripted fake desktop for tests and manual runs (never talks to a real host) |
| `wrangler.jsonc` | - | Worker config shared by `worker` and `app` (bindings, assets, cron, vars) |
| `.dev.vars.example` | - | Fake local secrets; copy to `.dev.vars` |

## Develop locally

Prerequisites: Node 22.18 or newer, pnpm 12.21.0 (`packageManager` in `package.json`, e.g. `corepack enable`).

```bash
cd web
pnpm install
cp .dev.vars.example .dev.vars   # fake values only, git-ignored
pnpm check
```

`.dev.vars.example` also sets `RP_ID=localhost` and `RP_ORIGIN=http://localhost:5173` (the `vite dev` port),
overriding the official `zptah.app` values of `wrangler.jsonc`, so passkeys and the Origin checks work locally.
With `wrangler dev` (port 8787) use `RP_ORIGIN=http://localhost:8787` instead.

`wrangler.jsonc` has a `custom_domain` route, so a bare `wrangler dev` treats the zone host as upstream and rewrites
`Origin` to `https://zptah.app`: every browser POST then answers `403 bad_origin`. Always pass
`--local-upstream localhost:<port>` with the port you serve on (the worker `dev` script, the API smoke and the E2E
harness do). `vite dev` with the Cloudflare plugin does not need it.

`pnpm check` runs, in order and stopping at the first failure: `typecheck`, `lint`, `test` over every package
(`pnpm -r`, one at a time). Read its exit status directly, not through a pipe.

pnpm 12 needs `run` for script names. Replace `<pkg>` with `protocol`, `worker`, `app` or `mock-desktop`.

| Command (from `web/`) | Does |
|---|---|
| `pnpm --filter @monoptah/<pkg> run typecheck` | `tsc --noEmit` (worker also checks its config and scripts) |
| `pnpm --filter @monoptah/<pkg> run lint` | ESLint |
| `pnpm --filter @monoptah/<pkg> run test` | Vitest; the worker tests run in the Workers pool (Miniflare) |
| `pnpm --filter @monoptah/protocol run vectors:gen` | Regenerate `vectors.json` and `methods.json` |
| `pnpm --filter @monoptah/app run routes:gen` | Regenerate the TanStack route tree |
| `pnpm --filter @monoptah/worker run db:migrate:local` | Apply D1 migrations to the local database |
| `pnpm --filter @monoptah/worker run db:generate` | Generate SQL from `schema.ts` (copy it to the next `migrations/NNNN_name.sql`) |
| `pnpm --filter @monoptah/worker run dev` | `wrangler dev` on `http://localhost:8787` (`/api/health` returns `{"ok":true}`) |
| `pnpm --filter @monoptah/app run dev` | `vite dev`: SPA, Worker, Durable Object and local D1 together |
| `pnpm --filter @monoptah/app run build` | Typecheck, `vite build`, then `scripts/check-dist.mjs` (see below) |
| `pnpm --filter @monoptah/app run preview` | Serve the built app |
| `pnpm --filter @monoptah/mock-desktop run start` | CLI: needs `--relay <url> --token <t>`; see `mock-desktop/src/cli.ts` |

The app and worker tests need a placeholder `app/dist/client/index.html`; `pretest` and `predev` create it when absent.

## API calls from the browser

Every non-GET call to a cookie-authenticated route (session, fresh and admin routes, and the public
`/api/auth/*` routes) must send `Origin` equal to `RP_ORIGIN`, `X-Monoptah-CSRF: 1` and
`Content-Type: application/json`, bodiless POST and DELETE included. A missing or different Content-Type is
answered `415` with code `unsupported_media_type`. WebSocket upgrades (`/relay/device/...`) check `Origin` only.
Token and pairing routes are exempt (security.md 3.4).

## Build output and security headers

`wrangler.jsonc` serves `app/dist/client` as the assets directory. The Vite build also writes the Worker bundle
and its sourcemap to `app/dist/monoptah_web`; that directory must never be served, which is why the assets
directory is `client` and not `dist`. `app/scripts/check-dist.mjs` runs at the end of `build` and fails when
`_headers` is missing or when a Worker bundle or sourcemap sits in the assets directory.

`app/dist/client/_headers` is generated at build time (`app/vite-plugins/headers.ts`) with the CSP from the Worker's
`buildCsp` (`worker/src/csp.ts`), so the policy exists once. `connect-src` gets `wss://<host>`, where the host
is that of `RP_ORIGIN` in `wrangler.jsonc` `vars` (`https://zptah.app` by default). A self-hoster who serves the
app from another origin builds with `MONOPTAH_RP_ORIGIN=https://web.example.org pnpm --filter @monoptah/app run build`
(and changes `RP_ORIGIN` and `RP_ID` in `wrangler.jsonc`).

## Never deploy from here

Sandboxes and CI never deploy and never hold Cloudflare credentials. Only the maintainer deploys, by hand
(`db:migrate:remote` included). See [`docs/architecture.md`](docs/architecture.md) section 9 and
[`docs/security.md`](docs/security.md) section 9.

## Desktop seams

The desktop side of the web link lives in `crates/monocode-core/src/mobile/`. Its tests run from
`crates/monocode-gpui` (the `native-dev` profile alias, see the repo `CLAUDE.md`):

```bash
cd crates/monocode-gpui
cargo ntest -p monocode-core mobile::
```
