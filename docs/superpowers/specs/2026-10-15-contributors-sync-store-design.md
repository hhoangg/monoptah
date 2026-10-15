# Contributors: persistent sync store, on-demand sync and periods

Date: 2026-10-15. Status: approved in conversation.

## Problem

Contributors collects everything on every visit: one big `git` + `gh` run per
(repo, branch, head, window), cached in memory only (`contributors.rs` `Cache`, 8
entries, key includes the head sha). On large repositories every collection hits a
safety cap (`max_patch_bytes` 64MB, `max_files_per_commit` 2000, `max_commits` 20k,
gh `MERGED_LIMIT` 1000, ...) and the page says "The history was too large to read in
full", even for 7 days, because the window always reaches back to the previous
season start. A new commit or a restart throws the work away.

## Goals

- Process each commit and each merged PR once, store the result, and serve any
  period from the store.
- No background work unless the user asked for it. A period with no data shows
  nothing until the user presses Sync.
- Periods: 7d (default), Month, Quarter, Year, each pickable back to any time.
  Stats and Leaderboard use the same selected period.
- Progress with real counts, cancel, and a small indicator in the footer while a
  sync runs and the user is elsewhere in the app.
- No "too large" for the period as a whole: caps apply per batch; an oversized single
  commit is skipped on its own.

## Non-goals

- No background scheduler, no syncing of projects the user did not ask for.
- No remote (connected machine) projects, same as today.
- No change to the scoring formula.

## Decisions (from the conversation)

| Topic | Decision |
|---|---|
| When work runs | Only when the user presses Sync (option C), plus the auto top-up below |
| Previous period | Syncing a period also syncs the period right before it (for deltas and rank movement) |
| Stale data | Show stored data at once; if the period is synced and the branch head moved, top up only the new commits/PRs in the background |
| Leaving the page | A running sync keeps running; footer shows an analytics icon + percent left of the Terminal button |
| Range picker | 7d, Month, Quarter, Year; the leaderboard ranks the selected period; Custom is removed |

## 1. Storage

New migration in `session_store.rs` (`monocode.db`, next version after 19), same
`schema_migrations` pattern. All rows are keyed by `repo_id`.

- **Repo identity**: the absolute `git rev-parse --git-common-dir`, so worktrees of one
  repository share data. `contrib_repos(id INTEGER PK, git_dir TEXT UNIQUE, slug TEXT NULL)`.
- `contrib_commits(repo_id, hash, subject, author_json, committer_json, authored_at,
  committed_at, committed_epoch INTEGER, co_authors_json, files_json, patch_id NULL,
  reverts NULL, oversized INTEGER, PRIMARY KEY(repo_id, hash))`. Branch independent;
  a commit is immutable so it is written once. `reverts` is the hash a revert commit
  names; `oversized` marks a commit whose file list exceeded the per-commit cap (stored
  without files, never marks the period truncated).
- `contrib_prs(repo_id, branch, pr_id, number NULL, state, source, merged_epoch NULL,
  closed_epoch NULL, merge_commit NULL, pr_json, PRIMARY KEY(repo_id, branch, pr_id))`.
  `pr_json` is today's `RawPr`.
- `contrib_commit_prs(repo_id, branch, hash, pr_id, PRIMARY KEY(repo_id, branch, hash))`.
- `contrib_reviews(repo_id, branch, pr_id, ordinal, submitted_epoch, review_json,
  PRIMARY KEY(repo_id, branch, pr_id, ordinal))`.
- `contrib_survival(repo_id, branch, pr_id, written, alive, head, computed_at,
  PRIMARY KEY(repo_id, branch, pr_id))`.
- `contrib_coverage(repo_id, branch, from_epoch, to_epoch, head, synced_at, gh_state,
  gh_note NULL)`: intervals synced per branch; adjacent/overlapping intervals are
  merged on write. `gh_state` is `complete | partial | unavailable`.

**Read path** (`contributors_load`): `git rev-list --no-merges --since --until <branch>`
(hashes only, cheap) joined with `contrib_commits`; PRs, reviews and survival from their
tables by branch and epoch; assembled into today's `RawData` so the TypeScript pipeline
keeps working. The response also carries `coverage` (covered / missing / stale for the
requested window), `syncedAt`, `ghState`. Commits in the window that are not in the
store mean the window is not fully covered.

A "Clear synced data" action deletes every row of a repo.

`contributors_collect` and its in-memory `Cache` are retired: the frontend only uses
`contributors_load` and the sync commands. The collection code (`collect_raw` pieces,
`gh.rs`, `pr_heads.rs`) is reused by the sync steps, not duplicated.

## 2. Sync

`contributors_sync_start(cwd, branch, from, to)` starts (or queues) a job; one job runs
per repository at a time. The window synced is the requested period plus the period
before it, minus what `contrib_coverage` already covers for the branch.

Steps, each counted before it starts so progress is exact:

1. **Count**: `rev-list --count` of the commits not yet stored and of first-parent
   entries in the missing window; one gh search for the number of merged and closed PRs
   in the window (when gh is available).
2. **Commits** in batches of 500 hashes: numstat (`-w -M`) and patch-id per batch, one
   DB transaction per batch. A batch whose patch text exceeds the byte cap is split; a
   single commit above the caps is stored `oversized`.
3. **PRs from git**: the existing merge-graph derivation, run per calendar month of the
   missing window.
4. **GitHub**: PR lists per calendar month (`merged:YYYY-MM-01..YYYY-MM-last`) so no
   query hits the 1000-row limit; reviews come with the list; PR commits from local PR
   heads (`pr_heads.rs`), gh only as fallback (the v0.22 logic, applied per chunk).
5. **Survival**: the same sampling as today, over the synced period, last.

Progress is one weighted percentage over the steps. Events on `contributors://sync`:
`{ repoKey, project, branch, step, done, total, percent, state: running|done|error|cancelled, error? }`.
`contributors_sync_cancel(cwd)` stops between batches and kills the running git child.
Batches already written stay; the next Sync continues with what is missing.
`contributors_sync_status()` lists running and queued jobs (for the footer after a
reload).

**Auto top-up**: when a loaded period is covered and the branch head differs from the
coverage head, the frontend starts a top-up job: commits in `old_head..new_head`, PRs
merged since `synced_at`. No survival recompute. If `old_head` is not an ancestor of the
new head (force push), the job syncs whatever commits of the window are missing.

**Errors**: a gh failure or rate limit keeps everything from git and marks coverage
`gh_state = partial` with the note; the UI offers "Retry GitHub" (a job that runs only
step 4 for covered windows whose gh state is not complete).

## 3. UI

Contributors page, for the selected period:

1. **Not synced**: "No data for Q3 2026 yet", an estimate from the counting step
   (`contributors_sync_estimate`, cheap: commits and PRs), and a **Sync** button. Nothing
   runs until the button is pressed.
2. **Syncing**: progress bar with the step, done/total and **Cancel**.
3. **Synced**: the data, "Synced 5 min ago", a re-sync button, and a small spinner while
   an auto top-up runs.
4. **GitHub incomplete**: data from git plus a notice and **Retry GitHub**.

Footer (`UsageFooter.tsx`, inside the right `ml-auto` group, left of the Terminal item):
while any contributors job runs, an analytics icon and `42%`; tooltip
"Syncing contributors · <project> · Commits 3,200/12,000"; several jobs show the combined
percent and list each in the tooltip; click opens that project's Contributors page. On
error the icon turns red and stays until clicked. When all jobs end the indicator goes
away.

Quitting the app stops a job; stored batches remain.

## 4. Periods

`Season { id, label, start, end }` becomes `Period` with a `kind`:
`7d | month | quarter | year`.

| Kind | Picker | Previous |
|---|---|---|
| 7d (default) | last 7 days including today | the 7 days before |
| month | `‹ Sep 2026 ›` | previous month |
| quarter | `‹ Q3 2026 ›` | previous quarter |
| year | `‹ 2026 ›` | previous year |

- No lower bound; future periods are not selectable; the current period runs to today.
- One picker drives Stats and Leaderboard; the leaderboard ranks the selected period and
  rank movement compares with the previous period of the same kind.
- Scoring takes a `Period` where it took a `Season`; the formula is unchanged. The
  formula sidebar notes that Consistency grows with the period length while Breadth is
  capped.
- Stats chart buckets: day for 7d and month, week for quarter, month for year (month
  buckets come back).
- Stored prefs (`monocode.contributors.range` and the season key) with old values fall
  back to 7d; the `monocode.*` key names stay.
- The Ask Agent snapshot uses the period label.

## Testing

- Rust: migration on an existing DB; store round trip; read path against real temp repos
  (covered, partially covered, force-pushed); batch ingestion splitting and `oversized`;
  per-month gh chunking with `FakeGh`; cancel between batches; top-up range; progress
  counts equal the work done; `RawData` from the store equals what the old collector
  produced for a small repo.
- TypeScript: period math (boundaries, previous period, DST, year/quarter edges),
  pickers, page states, footer indicator from events, prefs fallback.
- `npm run check` exits 0.
