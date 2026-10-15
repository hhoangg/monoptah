# Contributors Sync Store Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Contributors stores processed commits/PRs in SQLite, syncs only when asked (with progress, cancel and a footer indicator), and shows 7d/Month/Quarter/Year periods from the store.

**Architecture:** Rust: a store module over `monocode.db` (new migration), a sync engine that reuses the existing collector pieces in batches/month chunks and writes per batch, and a job manager exposing Tauri commands plus `contributors://sync` events. TypeScript: a `Period` model replacing range+season, a hook that loads from the store and drives sync, page states, and a footer indicator.

**Tech Stack:** Rust (Tauri 2, rusqlite), React + TypeScript, vitest.

**Spec:** `docs/superpowers/specs/2026-10-15-contributors-sync-store-design.md` (read it first; it is the authority).

Repo/worktree: `/Users/hhoangg/workspaces/my-projects/monoptah-worktrees/contributors-sync-store`, branch `contributors-sync-store`.

## Global Constraints

- Verify with `npm run check > /tmp/check-sync.log 2>&1; echo $?` from the worktree root; read the exit status directly (never through a pipe); it must be 0. (`npm run check` = vitest, `tsc --noEmit`, `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test`.)
- Clippy must also be clean for newer lints: no `format!("{x}")` where `x.to_string()` works, no needless borrows/clones.
- Never rename identifiers listed in the repo's `CLAUDE.md` ("Never rename these"): `monocode.*` localStorage keys, crate `monocode`, lib `monocode_lib`, etc. Existing localStorage key names stay (`monocode.contributors.range`, `monocode.contributors.mergeIdentities`).
- All git processes go through `git_command(root)` in `contributors.rs`; network git/gh calls keep hard timeouts; never create or move refs in the user's repository.
- SQLite: the app DB is `SessionStore` (`src-tauri/src/session_store.rs`, `monocode.db`), migrations via `schema_migrations` and `if current < N` steps; the next version is 20. Take the write lock (`lock_conn()`-style accessor used by `mail_sync.rs`) only for short bursts; never hold it across git or network work.
- No background work unless started by the user (Sync button) or the auto top-up of an already synced period whose branch head moved.
- Code, comments, identifiers, UI copy in English; no em dash or en dash characters anywhere in new text; use a plain hyphen.
- Remote (connected machine) projects: Contributors stays local-only, as today.
- Commits: clear English messages ending with `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`. Do not push.

## Review Focus

1. A period partly synced (Sync cancelled half way): the page must say it is incomplete and offer Sync again, never show partial numbers as if complete.
2. Two Sync requests for the same repo (double click, or another period while one runs): one job runs, the other queues; no duplicate rows, no DB lock held across git work.
3. A force-pushed or rebased branch: stored commits that are no longer on the branch never appear in a loaded period.
4. Opening the page for a period that was never synced must not start any git or gh work beyond the cheap estimate.
5. App restart while a job runs: the footer indicator does not show a ghost job; the next Sync resumes from stored batches.

---

### Task 1: Period model (TypeScript only)

Replace the range (`7d|30d|custom`) and the monthly season with one `Period`.

**Files:**
- Create: `src/features/contributors/model/period.ts`, `period.test.ts`
- Modify: `model/season.ts` (remove or reduce to re-exports used nowhere else; delete if unused), `model/range.ts` (delete when unused), `model/prefs.ts`, `model/scoring.ts`, `model/pipeline.ts`, `model/leaderboard.ts`, `model/stats.ts`, `model/snapshot.ts`, `model/askAgent.ts`, `hooks/useContributors.ts`, `ui/ContributorsView.tsx`, `ui/LeaderboardTab.tsx`, `ui/StatsTab.tsx`, `ui/FormulaSidebar.tsx`, and their tests.

**Interfaces (Produces):**
```ts
export type PeriodKind = "7d" | "month" | "quarter" | "year";
export type Period = { kind: PeriodKind; id: string; label: string; start: Date; end: Date }; // end exclusive, local midnight
export const PERIOD_KINDS: { value: PeriodKind; label: string }[]; // 7d, Month, Quarter, Year
export const DEFAULT_PERIOD_KIND: PeriodKind = "7d";
export function currentPeriod(kind: PeriodKind, now: Date): Period;
export function previousPeriod(p: Period): Period;
export function nextPeriod(p: Period, now: Date): Period | null; // null when it would start after today
export function periodFromId(kind: PeriodKind, id: string, now: Date): Period | undefined;
export function syncWindow(p: Period, now: Date): { from: Date; to: Date }; // previous.start .. min(p.end, tomorrow 00:00)
export function isInPeriod(p: Period, iso: string): boolean;
export function periodDayKeys(p: Period, now: Date): string[]; // local day keys, not beyond today
```
- ids: 7d -> `"7d"`; month -> `"2026-09"`; quarter -> `"2026-Q3"`; year -> `"2026"`. Labels: `"Last 7 days"`, `"Sep 2026"`, `"Q3 2026"`, `"2026"`.
- 7d: start = today 00:00 minus 6 days, end = tomorrow 00:00; previous = the 7 days before; `nextPeriod` returns null.
- Scoring/pipeline take `Period` where they took `Season` (`scoreSeason` -> keep the name or rename to `scorePeriod`; update all callers). `streakOf` uses `periodDayKeys`.
- Leaderboard ranks the selected period; rank movement vs `previousPeriod`.
- Stats chart buckets: day for 7d and month, week for quarter, month for year (restore month buckets that were removed in commit 01d5325 if needed: `git show 01d5325` shows what was removed).
- prefs: `RANGE_KEY` stores `"<kind>"` or `"<kind>:<id>"`; any other stored value (old `30d`, `custom...`, `90d`, `1y`, `all`) falls back to `currentPeriod("7d")`. Remove any separate season pref if one exists, falling back the same way.
- FormulaSidebar: add one sentence: "Consistency grows with the length of the period; Breadth is capped, so in long periods it weighs less."
- Ask Agent snapshot and file name use the period label/id instead of the season.
- UI: a kind selector (7d / Month / Quarter / Year) plus `‹ label ›` arrows for month/quarter/year (next arrow disabled when `nextPeriod` is null). Remove the custom date inputs and the season picker.
- Until Task 6, `useContributors` keeps calling `contributors_collect` with `since = syncWindow(p).from` (ISO) and `until = syncWindow(p).to` (ISO); remove `sinceFor`.

- [ ] Step 1: write `period.test.ts`: month/quarter/year boundaries (Dec->Jan, Q4->Q1 of next year, leap Feb), previous/next, `nextPeriod` null for the current period, 7d bounds and previous, `periodFromId` round trip and invalid ids, `syncWindow` caps `to` at tomorrow for the current period, DST week (Europe/Berlin last Sunday of March) day keys count = 7. Run `npx vitest run src/features/contributors/model/period.test.ts`, expect FAIL.
- [ ] Step 2: implement `period.ts`; run the test, expect PASS.
- [ ] Step 3: migrate scoring/pipeline/leaderboard/stats/snapshot/askAgent/prefs and the UI to `Period`; update their tests (no test may still assert 30d/custom/season-picker behaviour); add prefs fallback tests for `30d`, `custom:...`, `all`, garbage.
- [ ] Step 4: `npx vitest run src/features/contributors src/app`, then the full check (Global Constraints). Commit "Contributors: periods (7d, month, quarter, year) for stats and leaderboard".

### Task 2: Store schema and read path (Rust)

**Files:**
- Modify: `src-tauri/src/session_store.rs` (migration 20), `src-tauri/src/contributors.rs` (`mod store;`)
- Create: `src-tauri/src/contributors/store.rs` (+ tests in the same file)

**Interfaces (Produces):**
```rust
pub(super) struct StoredCommit { pub commit: RawCommit /* pr_id None */, pub patch_id: Option<String>, pub reverts: Option<String>, pub oversized: bool }
pub(super) struct CoverageRow { pub from_epoch: i64, pub to_epoch: i64, pub head: String, pub synced_at: i64, pub gh_state: String, pub gh_note: Option<String> }
pub(super) fn repo_id(conn: &Connection, git_dir: &str, slug: Option<&str>) -> Result<i64, String>;
pub(super) fn find_repo_id(conn: &Connection, git_dir: &str) -> Result<Option<i64>, String>;
pub(super) fn stored_hashes(conn: &Connection, repo: i64, hashes: &[String]) -> Result<HashSet<String>, String>;
pub(super) fn insert_commits(tx: &Transaction, repo: i64, commits: &[StoredCommit]) -> Result<(), String>; // INSERT OR IGNORE
pub(super) fn put_prs(tx: &Transaction, repo: i64, branch: &str, prs: &[(RawPr, Vec<String>)]) -> Result<(), String>; // replace by pr_id; hashes -> contrib_commit_prs
pub(super) fn put_reviews(tx: &Transaction, repo: i64, branch: &str, reviews: &[RawReview]) -> Result<(), String>; // replace all reviews of the PRs present
pub(super) fn put_survival(tx: &Transaction, repo: i64, branch: &str, head: &str, rows: &[RawSurvival]) -> Result<(), String>;
pub(super) fn add_coverage(tx: &Transaction, repo: i64, branch: &str, row: &CoverageRow) -> Result<(), String>; // merges overlapping/adjacent intervals, newest head/synced_at/gh_state win
pub(super) fn coverage(conn: &Connection, repo: i64, branch: &str) -> Result<Vec<CoverageRow>, String>;
pub(super) fn missing_intervals(rows: &[CoverageRow], from: i64, to: i64) -> Vec<(i64, i64)>;
pub(super) fn clear_repo(conn: &Connection, git_dir: &str) -> Result<(), String>;
pub(super) struct Loaded { pub raw: RawData, pub covered: bool, pub missing_commits: u64, pub synced_at: Option<i64>, pub coverage_head: Option<String>, pub gh_state: Option<String> }
pub(super) fn load(conn: &Connection, root: &Path, repo: i64, branch: &str, head: &str, since: i64, until: i64) -> Result<Loaded, String>;
pub(crate) fn git_common_dir(root: &Path) -> Result<String, String>; // absolute, canonical
```
- Tables exactly as the spec section 1 (`contrib_repos`, `contrib_commits`, `contrib_prs`, `contrib_commit_prs`, `contrib_reviews`, `contrib_survival`, `contrib_coverage`), JSON columns via serde_json, epochs as INTEGER seconds, indexes on `(repo_id, committed_epoch)`, `(repo_id, branch, merged_epoch)`, `(repo_id, branch, closed_epoch)`, `(repo_id, branch, submitted_epoch)`.
- `load`: `git rev-list --no-merges --since=@<since> --until=@<until> <head> --` (via `git_command`) for hashes; join stored commits (missing ones counted in `missing_commits`); drop duplicates with the existing `select_duplicates` over stored patch ids of those commits; `pr_id` per commit from `contrib_commit_prs`; PRs merged or closed in the window; reviews submitted in the window; survival rows of those PRs; `reverted_pr_ids` computed from stored `reverts` and the commit->PR map (reuse/adapt `reverted_pr_ids`); `covered` = `missing_intervals(coverage, since, until)` is empty AND `missing_commits == 0`; `truncated` = false; `gh_available`/`gh_note` from coverage (`gh_state != "unavailable"`).

- [ ] Step 1: tests first (temp DB via `SessionStore::open_in_memory()` or a temp file, temp git repos via the existing test helpers): migration from an existing DB at version 19 adds the tables; repo id is stable and shared by two worktrees of one repo (`git worktree add`); insert/stored_hashes round trip; `add_coverage` merging (overlap, adjacency, disjoint); `missing_intervals`; `load` on a fully synced fixture returns the same commits/PRs/reviews as stored and `covered == true`; a commit not stored -> `covered == false`, `missing_commits == 1`; after `git reset --hard HEAD~1` the dropped commit is not in `load`; `clear_repo` removes everything for that repo only. Run `cd src-tauri && cargo test contributors::store`, expect FAIL.
- [ ] Step 2: implement; tests PASS; full check; commit "Contributors: SQLite store and read path".

### Task 3: Sync engine part 1 - estimate and commit ingestion (Rust)

**Files:** Create `src-tauri/src/contributors/sync.rs` (+ tests); modify `contributors.rs` (`mod sync;`, small `pub(super)` visibility changes of reused helpers only).

**Interfaces:**
- Consumes: Task 2 store functions; existing `parse_log`, `LOG_FORMAT`, `patch_ids`-style streaming, `Limits`, `reverted_commit`.
- Produces:
```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)] #[serde(rename_all = "camelCase")]
pub enum SyncStep { Counting, Commits, PullRequests, Github, Survival }
pub(super) trait Reporter: Sync { fn progress(&self, step: SyncStep, done: u64, total: u64); fn cancelled(&self) -> bool; }
#[derive(Serialize)] #[serde(rename_all = "camelCase")]
pub struct Estimate { pub commits: u64, pub stored_commits: u64, pub first_parent: u64, pub prs: Option<u64> }
pub(super) fn estimate(root: &Path, head: &str, from: i64, to: i64, conn_read: &Connection, repo: Option<i64>, gh: &dyn gh::GhRunner) -> Result<Estimate, String>;
pub(super) fn ingest_commits(store: &SessionStore, repo: i64, root: &Path, hashes: &[String], reporter: &dyn Reporter) -> Result<IngestOutcome, String>;
pub(super) struct IngestOutcome { pub written: u64, pub oversized: u64, pub cancelled: bool }
pub(super) const COMMIT_BATCH: usize = 500;
```
- `estimate`: `rev-list --count` for the window, minus already stored (`stored_hashes` over `rev-list` output), first-parent count, and one gh search count (`gh api search/issues -f q="repo:<slug> is:pr is:merged merged:<from>..<to>"` total_count, or `gh pr list --search ... --json number --limit 1` is NOT enough; use the search API `total_count`) with the existing gh status/slug helpers; `prs: None` when gh is unavailable. It must not read diffs.
- `ingest_commits`: only hashes not stored; batches of `COMMIT_BATCH` fed to `git log --no-walk --stdin --numstat -z -w -M <LOG_FORMAT>` and to a `git log -p --no-walk --stdin` -> `git patch-id --stable` pipe with the existing byte cap; a batch over the cap is split in halves recursively; a single commit over the cap gets `patch_id = None`; a commit over `max_files_per_commit` is stored with empty files and `oversized = true`. One transaction per batch, the DB lock held only while writing. `reporter.cancelled()` is checked between batches; `progress(Commits, done, total)` after each batch.

- [ ] Step 1: tests (temp repos, in-memory store, a recording Reporter): estimate counts match `git rev-list --count` and drop to 0 stored after ingest; ingest of 1,200 commits (generate fast with `git fast-import`) writes 3 batches and reports 500/1000/1200; re-running writes nothing; cancel after the first batch keeps 500 rows and reports `cancelled`; an oversized commit (cap lowered via test Limits) is flagged and the batch still succeeds; a byte cap smaller than one batch splits it and every commit gets a patch id; a cherry-picked duplicate has the same patch id as its original. FAIL first.
- [ ] Step 2: implement; PASS; full check; commit "Contributors: sync estimate and batched commit ingestion".

### Task 4: Sync engine part 2 - PRs, GitHub, survival, coverage, modes (Rust)

**Files:** Modify `sync.rs`, `contributors.rs`, `contributors/gh.rs` (only to expose month-chunk entry points; keep the v0.22 behaviour), + tests.

**Interfaces:**
- Consumes: Tasks 2-3; `derive_prs_from_git`, `gh::enrich`-internals (`fetch_all`, `overlay_fetched`, `local_heads`, `commits_of`), `compute_survival`.
- Produces:
```rust
pub enum SyncMode { Full, TopUp { old_head: String }, GithubRetry }
pub(super) struct SyncRequest<'a> { pub store: &'a SessionStore, pub root: &'a Path, pub branch: &'a str, pub head: &'a str, pub from: i64, pub to: i64, pub mode: SyncMode, pub gh: &'a dyn gh::GhRunner, pub commit_cache: &'a gh::CommitCache }
pub(super) struct SyncOutcome { pub cancelled: bool, pub gh_state: String, pub gh_note: Option<String> }
pub(super) fn run_sync(req: &SyncRequest, reporter: &dyn Reporter) -> Result<SyncOutcome, String>;
pub(super) fn month_chunks(from: i64, to: i64) -> Vec<(i64, i64)>; // local calendar months clipped to [from, to)
```
- `Full`: for each missing interval (`missing_intervals`) -> Counting (exact totals for every later step) -> Commits (`ingest_commits` of `rev-list` hashes in the interval) -> PullRequests (`derive_prs_from_git` per `month_chunks`) -> Github (per month: merged and closed lists, local PR heads, gh commit fallback, reviews; overlay onto that month's git PRs; a month whose gh part fails keeps git PRs and sets `gh_state = "partial"` with the note) -> Survival (existing sampling over the requested window, newest first) -> `add_coverage` for the interval with `head`, now, gh_state. Coverage is written only for an interval whose steps all completed (cancel = no coverage row, stored batches stay).
- `TopUp { old_head }`: commits in `old_head..head` (if `old_head` is not an ancestor of `head`: the missing commits of the covered window instead), PRs/GitHub for the months touched since the coverage `synced_at`, no survival; extends coverage `to` to now and updates head.
- `GithubRetry`: only the Github step for covered intervals whose gh_state is not `complete`.
- Progress: weights Counting 2, Commits 50, PullRequests 13, Github 25, Survival 10; percent computed in the job manager from (step, done, total) using these weights (export `fn percent(step, done, total) -> u8`).

- [ ] Step 1: tests with `FakeGh` and temp repos: a two-month window yields two gh list calls with `merged:` ranges of each month; a month whose gh call fails keeps git PRs and the outcome is `partial`; cancel during Github leaves no coverage row; a second Full run after completion does no git log -p and no gh commit calls (all covered); TopUp after one new merge commit ingests exactly that PR's commits and updates the coverage head; TopUp after a force push syncs the missing commits; `load` after a Full sync returns `covered == true` and PRs/reviews/survival for the window; `percent` is monotonic over a recorded run and ends at 100; `month_chunks` across a DST change and a year end. FAIL first.
- [ ] Step 2: implement; PASS; full check; commit "Contributors: sync PRs, GitHub and survival into the store".

### Task 5: Job manager, commands and events (Rust)

**Files:** Create `src-tauri/src/contributors/jobs.rs` (+ tests); modify `contributors.rs` (commands), `src-tauri/src/lib.rs` (register commands, manage state).

**Interfaces (Produces, Tauri commands, camelCase args):**
- `contributors_load(cwd, branch: Option<String>, since: String, until: String) -> LoadResponse { raw: RawData, covered, missingCommits, syncedAt: Option<i64>, coverageHead: Option<String>, head: String, ghState: Option<String> }`
- `contributors_sync_estimate(cwd, branch, from, to) -> Estimate`
- `contributors_sync_start(cwd, branch, from, to, mode: "full" | "topup" | "github", oldHead: Option<String>) -> ()` (queues if a job for the same repo runs; an identical request already queued or running is a no-op)
- `contributors_sync_cancel(cwd) -> ()` (cancels the running job and drops queued ones of that repo)
- `contributors_sync_status() -> Vec<SyncEvent>` (last event of every running/queued job)
- `contributors_clear(cwd) -> ()` (refuses while a job of that repo runs)
- Event `contributors://sync`, payload `SyncEvent { repoKey: String, project: String, branch: String, step: SyncStep, done: u64, total: u64, percent: u8, state: "queued" | "running" | "done" | "error" | "cancelled", error: Option<String> }`; emitted at most every 150 ms per job while running plus every state change.
- One worker thread per repo with a queue; jobs run outside the DB lock; cancellation via `AtomicBool` checked by the Reporter; the running git child is killed through the existing timeout runner or by checking cancel between batches.
- Keep `contributors_collect` registered (Task 6 removes it).

- [ ] Step 1: tests with an injectable runner (a fake `run_sync` closure) and a recording event sink: two starts for one repo run sequentially; identical start is a no-op; cancel stops the running job (state `cancelled`) and drops the queue; status lists running and queued; clear while running returns an error; events are throttled but state changes are never dropped. FAIL first.
- [ ] Step 2: implement and register; PASS; full check; commit "Contributors: sync jobs, commands and progress events".

### Task 6: Frontend - load from the store, sync flow, page states; retire collect

**Files:** Modify `model/api.ts`, `hooks/useContributors.ts`, `ui/ContributorsView.tsx` (+ small new UI pieces under `ui/` if a component grows past ~300 lines), tests; Rust: remove `contributors_collect`, its `Cache`, `collect_cached*` and dead code they leave (keep anything sync uses), unregister in `lib.rs`.

**Interfaces:**
- Consumes: Task 1 `Period`, `syncWindow`; Task 5 commands and `SyncEvent`.
- Produces (for Task 7): `src/features/contributors/model/syncEvents.ts` exporting `type SyncEvent` (mirrors Rust) and `SYNC_EVENT = "contributors://sync"`.
- Hook behaviour: on period/branch change call `contributors_load` with `syncWindow`. If `covered` is false: state `notSynced` with `contributors_sync_estimate` result (no other work). Sync button -> `contributors_sync_start(mode "full")`; listen to `SYNC_EVENT` for this repo: `syncing` with step/done/total/percent; on `done` reload. If `covered` and `coverageHead !== head`: show data and start `mode "topup"` with `oldHead = coverageHead` once per head; small spinner while it runs; reload on done. `ghState` `partial`/`unavailable` -> notice with "Retry GitHub" (`mode "github"`). Cancel button -> `contributors_sync_cancel`. Menu item "Clear synced data" with a confirm -> `contributors_clear`, then reload.
- Copy: "No data for {label} yet", "About {commits} commits and {prs} pull requests to read." (omit PRs when null), "Sync", "Syncing {step}: {done} of {total}", "Cancel", "Synced {relative time}", "GitHub data is incomplete for this period.", "Retry GitHub", "Clear synced data". Steps shown as: Counting, Commits, Pull requests, GitHub, Survival.
- An incomplete period (cancelled earlier) shows `notSynced` with the estimate of what is left, never partial numbers.

- [ ] Step 1: tests (mock `invoke` and event listen as the existing hook tests do): never-synced period triggers only load + estimate; Sync starts a full job and progress events update the UI; done reloads; topup starts exactly once when heads differ and not when equal; partial gh shows Retry; cancel calls the command; clear asks for confirmation; switching period while syncing keeps the job running and shows the new period's state; old `contributors_collect` is no longer invoked anywhere (grep test like the existing wiring tests). FAIL first.
- [ ] Step 2: implement; remove the Rust collect command and cache; PASS; full check; commit "Contributors: load from the sync store and sync on demand".

### Task 7: Footer sync indicator

**Files:** Modify `src/app/shell/UsageFooter.tsx` (indicator inside the right `ml-auto` group, left of the Terminal item), `src/app/App.tsx` (wire click -> open the project's Contributors page via the existing `setContributorsProject` path), add an analytics icon to `src/shared/ui/icons.tsx` if none exists; create `src/features/contributors/hooks/useSyncJobs.ts` (+ tests).

**Interfaces:**
- Consumes: Task 6 `SyncEvent`, `SYNC_EVENT`; Task 5 `contributors_sync_status`.
- `useSyncJobs()` returns `{ jobs: SyncEvent[]; percent: number | null; failed: SyncEvent | null; dismissFailure(): void }`: seeds from `contributors_sync_status()` on mount, updates from events, drops jobs on `done`/`cancelled`, keeps an `error` job until dismissed.
- Indicator: shown only while `jobs` has running/queued entries or a failure: analytics icon + `{percent}%` (combined: average weighted equally per job); tooltip "Syncing contributors · {project basename} · {step} {done}/{total}" one line per job; click opens Contributors for the first job's project; failure: red icon, tooltip with the error, click opens the page and dismisses. Matches footer styling (`text-[11px]`, `h-7`).

- [ ] Step 1: tests: no jobs -> nothing rendered; one running job -> icon and percent; two jobs -> combined percent and two tooltip lines; done removes it; error stays red until click; status seeding after reload; source-wiring test that the indicator sits before the Terminal item and its click opens Contributors. FAIL first.
- [ ] Step 2: implement; PASS; full check; commit "Contributors: footer indicator for running syncs".
