# Deploy runbook (official host)

How the official Monoptah web host (`zptah.app`) is deployed. Self-hosters follow architecture.md section 9 instead.

## Who runs it

Only the maintainer, from their own Mac, with the wrangler OAuth login that exists only on that Mac (security.md
section 9). Agents, sandboxes and CI never run it, never hold Cloudflare credentials and never run
`wrangler deploy`, `wrangler d1 migrations apply --remote`, `wrangler secret put` or `wrangler login`. CI
(`web-ci.yml`) builds and tests only and holds no Cloudflare deploy token. The script refuses when `CI` is set and,
for a real run, when the OS is not macOS.

## Prerequisites (once)

- `wrangler login` done on the Mac, with 2FA on the Cloudflare account.
- The D1 database exists and its real `database_id` is in `web/wrangler.jsonc` (not the all-zero placeholder).
- `VAPID_PUBLIC_KEY` in `wrangler.jsonc` `vars` is the real key (not the `REPLACE_WITH_...` placeholder).
- The three secrets are set with `wrangler secret put`: `VAPID_PRIVATE_KEY`, `RECOVERY_PEPPER`, `AUDIT_IP_SALT`.
  Keep a copy in the sealed operator vault (losing `RECOVERY_PEPPER` invalidates stored recovery codes).

## Deploy

From a clean checkout, in `web/`:

```bash
pnpm deploy:official --dry-run   # preflight for real, prints the plan, runs no remote command
pnpm deploy:official             # the real deploy
```

Order, enforced by the script (`web/scripts/deploy-official.mjs`):

1. Preflight, stopping at the first failure: `CI` not set; macOS (real run only); tree clean (untracked files
   count); branch is `main`; `main` equals `origin/main` after `git fetch origin main`; `wrangler.jsonc` has no
   placeholder `database_id` or VAPID key; `pnpm install --frozen-lockfile` passes (the installed dependencies
   match the lockfile); `pnpm check` passes; `scripts/check-fork-branding.sh` passes.
2. Build the app: `pnpm --filter @monoptah/app build`.
3. Migrations FIRST: `wrangler d1 migrations apply monoptah --remote -c wrangler.jsonc`. A failure stops the run
   before the deploy.
4. Deploy: `wrangler deploy -c wrangler.jsonc`. Every deploy disconnects all relay sockets; clients reconnect.
5. Audit: one `audit_log` row (`actor_kind` `system`, action `system.deploy`, meta with only the 40-character commit sha
   and the app version, both validated before the SQL is built). If this step fails the deploy has already happened: insert the row by hand.
6. The script prints one line. Append it to the deploy log below in the same commit that records the deploy
   (a PR titled `docs(web): record deploy <sha>` or the next change).

## Rollback

Migrations are forward-only and backward compatible for one release (architecture.md section 9), so the previous
Worker version still runs on the new schema. To roll back a bad Worker:

```bash
wrangler rollback -c wrangler.jsonc    # from web/, on the maintainer Mac
```

Never roll back a migration, and never edit an applied one: fix forward with a new migration. If a release shipped a
migration that is not backward compatible, a Worker rollback is not safe: fix forward instead. Record the rollback
in the log below with `(rollback)` after the operator name. Point-in-time restore of D1 (Time Travel) is the
last resort for data damage (security.md section 9).

## Deploy log

Append the printed line (date, commit, app version, operator) at the end.

| Date | Commit | App version | Operator |
|---|---|---|---|
