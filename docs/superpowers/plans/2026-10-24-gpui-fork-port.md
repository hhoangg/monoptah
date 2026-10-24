# Port Monoptah fork features to the native GPUI app - Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bring every fork-only Monoptah feature except Mail into the Rust + GPUI native app on `feat/gpui-migration`, starting with the fork's identity and data paths.

**Architecture:** Backend logic goes into `crates/monocode-core` (no Tauri, no GPUI), UI into `crates/monocode-gpui`. Fork Rust code that is already Tauri-free (`src-tauri/src/clickup.rs`, `provider_session_cleanup.rs`, `contributors/*`, `telegram/*`) moves into core with its tests; TypeScript logic is translated to Rust with its unit tests ported as Rust tests. Existing native systems (inbox `Service`, automations, terminal dock, Mono API, habit scheduler) are extended, never duplicated.

**Tech Stack:** Rust 2021/2024, GPUI (`gpui-pre` 0.3.9), `ureq` 2.12, `rusqlite` 0.40.2, `alacritty_terminal` 0.26.

**Spec:** the user's brief for this branch (2026-10-24) plus the behavior reference in `src/` and `src-tauri/`. The feature inventory below replaces the out-of-date `docs/fork-features.md` in the main checkout.

## Global Constraints

- Do not port Mail: no `mail*.rs`, `aws_*.rs`, `src/features/mail`, Mail settings.
- Do not break the React/Tauri app. Touch `src/` and `src-tauri/` only for a truly shared change, and then also run `npm run check` (read its exit status directly, never through a pipe).
- Never commit or push. The controller stages reviewed work with `git add -A`; implementers do not run `git add`, `git commit`, `git stash`, or `git reset`.
- Never open the real installed database. No `native:build` launch, no `--check-data`, no `--monocode-data` against real data. Runtime checks: `MONOCODE_NATIVE_DATA_DIR=$(mktemp -d) npm run native:dev`.
- Verification gate after each task: the focused cargo tests for the touched crate; after each phase: `npm run native:check > /tmp/claude-monoptah/check.log 2>&1; echo $?` must print `0`.
- Cargo command prefix: `cargo <cmd> --manifest-path crates/monocode-gpui/Cargo.toml --locked -p monocode-core|monocode-gpui --profile native-dev`.
- Identifiers that never change: `monocode.*` localStorage keys, the `monocode.db` file name, crate names `monocode-core`/`monocode-gpui`, binary `monocode-gpui`, `MONOCODE_NATIVE_*` env vars, Info.plist key `MonocodeDataMode`, `<monocode_context>` tag, keyring service `com.monocode.mobile`, `monocode-host` and anything in the remote host protocol, `INTERRUPT_MESSAGE`.
- Native identity: product name `Monoptah Native`, bundle id `com.monoptah.native`, dev data directory `Monoptah Development`. The installed desktop app it reads is `com.monoptah.desktop`.
- User-visible copy says `Monoptah`. Links to upstream go to `https://github.com/hhoangg/monoptah`; the Website menu item goes to `https://zptah.app`.
- Code, comments, test names and log text in English. Match the surrounding code's density and idiom (the native crates use terse comments and free functions).
- Shared data stays compatible with the Tauri app: same file names and JSON shapes (`clickup-config.json`, `telegram-config.json`, Mono files), same DB columns, same automation event keys.

## Review Focus

1. An install that has only the fork's data (`com.monoptah.desktop`) and no upstream data: the release build must find the DB, WebKit localStorage and `clickup-config.json`, and must never fall back to `com.monocode.desktop`. Pinned by Task 1.1 path tests.
2. An isolated dev data dir must still refuse to read desktop preferences after the identity change (`isolated_data_directory_cannot_read_desktop_preferences` keeps passing; Task 1.1).
3. ClickUp tasks without a custom id: two tasks in the same space must not collide on `Item::key` (Task 2.2 test `clickup_items_key_on_task_id`).
4. ClickUp rate limit (429) while the Inbox refreshes in the background: every call returns the rate-limited error until the reset time instead of hammering the API (Task 2.1 ported backoff tests).
5. Automation claim ledger shared with Tauri: a ClickUp task already claimed by the Tauri app must not fire again natively, so the event key must be exactly `clickup:task:<id>` (Task 2.3 test `clickup_event_key_matches_desktop`).

---

## Feature inventory (fork-only, from `git log --no-merges upstream/main..6718e98a^1`, 248 commits)

| # | Feature | Reference code | Native status | Phase |
|---|---|---|---|---|
| A | Branding, app id, data paths | `tauri.conf.json`, `macos.rs`, branding checker | hardcodes `com.monocode.desktop`, `dev.usemono.gpui`, MonoCode copy | 1 |
| B | ClickUp inbox source + "Task appeared" trigger | `src-tauri/src/clickup.rs` (1673 lines, 33 tests), `automations.rs`, `src/features/inbox/model/clickup.ts` (27 tests), `ClickUpSettings.tsx` | none | 2 |
| C | Delete provider session files (sessions + sidebar bulk) | `provider_session_cleanup.rs` (27 tests), `DeleteSessionDialog.tsx` | delete has no checkboxes; sidebar delete has no confirm | 3 |
| D | Cmd+Q hold-to-quit gate | `quit_hotkey.rs`, `quitGate.ts` (10 tests), `QuitGateNotice.tsx` | none | 3 |
| E | Git blame in the editor (inline + gutter column + hover + menu) | `fs.rs` `git_blame` (6 tests), `editorBlame.ts` (53 tests), `host/git-blame.ts` | none | 3 |
| F | Inbox extras: origin-only GitHub repo, Send to agent on PRs + busy spinner, per-source view memory | `fs.rs` repo discovery, `inboxViewMemory.ts`, `inboxRunningSessions.ts` | adds fork parent; no PR send; no memory | 3 |
| G | TUI session surface (Claude/agy in a PTY pane bound to a session row, `sessions.surface`) | `pty.rs`, `tuiLaunch.ts`, `tuiSession.ts`, `docs/tui-sessions.md` | terminal dock exists, no session binding | 4 |
| H | Claude hook busy state for TUI sessions | `tui_hook.rs` (12 tests) | none | 4 |
| I | New-session provider prompt (needs G's surface preference) | `newSessionProviderPrompt.ts` (13 tests) | none | 4 |
| J | Automations run in the terminal when that is the surface | `automationSession.ts` | none | 4 |
| K | Terminal pane lifecycle (close on clean exit, restart bar) | `TerminalView.tsx` (xterm-specific parts not ported) | partial | 4 |
| L | Mono pipeline controls: `completionDelivery`, `parentSessionId`, `folderName`/`folderId`, habit-run reports to the Mono chat, no-commit worktree error, habit Stop sheet | `agentApp.ts`, `monoHabits.ts`, `habitStop.ts`, `startedRail.ts` (~90 tests) | none | 5 |
| M | Habit schedule `interval` (every N hours from an anchor) | `monoHabits.ts` (+8 tests) | 4 of 5 kinds | 5 |
| N | Mono reset deletes provider files (incl. retired rotation ids) | `provider_session_cleanup.rs`, `monoProviderFiles.ts` | none | 5 |
| O | Telegram questions (per-project bot, topics, forward + answer) | `src-tauri/src/telegram/` (7902 lines, 154 tests), `src/features/telegram/` (151 tests) | none | 5 |
| P | Contributors (GitHub stats, leaderboard, identities, sync store) | `src-tauri/src/contributors*`, `src/features/contributors/model/` | none | 6 |
| Q | Ticket Contributors (AI-scored ClickUp tickets, report export) | `contributors/{assessor,clickup_tickets,rubric,ticket_jobs,ticket_store}.rs`, `src/features/ticket-contributors/` | none (needs B) | 6 |
| R | Workflows (engine, Mono files, runner, 16 CLI actions, board, run graph, YAML import/export, guide) | `src/features/workflows/` (4181-line model, 598 TS tests), `workflow_command.rs` (26 tests) | none | 7 |
| - | Mail | - | dropped on purpose | - |
| - | Release/updater/Homebrew infra for native | - | out of scope for now | - |

Phases 4 to 7 get their own detailed plan when they start; their sketches are at the end.

---

## Phase 1 - Branding and data paths

### Task 1.1: Desktop data paths point at Monoptah, and the baseline is green

**Files:**
- Create: `crates/monocode-core/src/brand.rs`
- Modify: `crates/monocode-core/src/lib.rs` (add `pub mod brand;`)
- Modify: `crates/monocode-core/src/store/monocode_import.rs:17-33` (`monocode_database`)
- Modify: `crates/monocode-core/src/desktop_preferences/webview.rs:17,135` and `crates/monocode-core/src/desktop_preferences.rs:731`
- Modify: `crates/monocode-gpui/src/shell.rs:8013` (baseline clippy `let_underscore_future` under Rust 1.99: replace `let _ = window.prompt(...)` with `drop(window.prompt(...))`, same semantics)

**Interfaces:**
- Produces: `monocode_core::brand::{PRODUCT: &str = "Monoptah", NATIVE_PRODUCT: &str = "Monoptah Native", DESKTOP_APP_ID: &str = "com.monoptah.desktop", NATIVE_APP_ID: &str = "com.monoptah.native"}`; `store::monocode_database()` unchanged signature.

- [ ] **Step 1: Write failing tests** in `monocode_import.rs` tests: `desktop_database_uses_the_monoptah_identifier` asserts `monocode_database().ends_with("com.monoptah.desktop/monocode.db")`; in `webview.rs` tests: `webview2_root_uses_the_monoptah_identifier` (set `LOCALAPPDATA` to a temp dir containing `com.monoptah.desktop/EBWebView/Default/Local Storage/leveldb` fixture is not needed: assert the computed root path via a small `fn webview2_root(base: &Path) -> PathBuf` helper).
- [ ] **Step 2:** run `cargo test ... -p monocode-core monoptah_identifier`; expect FAIL.
- [ ] **Step 3:** add `brand.rs`, use `brand::DESKTOP_APP_ID` in all three path builders (macOS WebKit `Library/WebKit/<id>/WebsiteData/Default`, Linux XDG `<id>`, Windows `%APPDATA%/<id>` and `%LOCALAPPDATA%/<id>`). Fix the clippy lint in `shell.rs`.
- [ ] **Step 4:** run the two tests plus `isolated_data_directory_cannot_read_desktop_preferences`; expect PASS. Run `cargo clippy ... -p monocode-core -p monocode-gpui --all-targets -- -D warnings`; expect clean.

### Task 1.2: Native app identity

**Files:**
- Modify: `crates/monocode-gpui/src/main.rs:133-148` (dev dir `Monoptah Development`), `:179` (`--version` prints `{NATIVE_PRODUCT} {version}`), `:247` help text and the other user-facing `eprintln!` copy in `main()` (say Monoptah).
- Modify: `crates/monocode-core/src/app_update/mod.rs:20` (`APP_ID = brand::NATIVE_APP_ID`), `:63-65` error copy, `crates/monocode-core/src/app_update/install.rs` (bundle name `Monoptah Native.app` and its tests).
- Modify: `scripts/bundle-native-macos.sh` (bundle `Monoptah Native.app`, `CFBundleIdentifier` `com.monoptah.native`, names `Monoptah Native`), `scripts/dev-native.mjs:107`, `scripts/package-native.mjs` (app_id, embedded version string, `.app` name, desktop entry Name/StartupWMClass; keep package/binary names `monocode-native`/`monocode-gpui`), `scripts/native-release.mjs:51`, and their tests `scripts/*.test.mjs`.
- Modify: `crates/monocode-gpui/src/shell.rs:8015` About title `Monoptah Native`.
- Modify: `docs/native/README.md` paths and names only (data table, `.app` name, app id).
- Leave: `scripts/native-installer.nsi`, `.github/workflows/native*.yml` (release infra, out of scope; list them in the phase report).

- [ ] **Step 1:** update `scripts/*.test.mjs` expectations to the new names/id first; run `npm run native:test:dev`; expect FAIL.
- [ ] **Step 2:** apply the renames; add a `main.rs` unit test `plist_data_mode_still_reads_monocode_key` if none covers `plist_data_mode` (key stays `MonocodeDataMode`).
- [ ] **Step 3:** run `npm run native:test:dev` and `cargo test ... -p monocode-core app_update`; expect PASS.

### Task 1.3: User-visible copy and the branding checker covers the native crates

**Files:**
- Modify: `scripts/check-fork-branding.sh`: add `crates/monocode-core/src/ crates/monocode-gpui/src/` to the scanned paths; extend `PATTERN` with `Monocode Native|Monocode Development`. Add ALLOW entries only with a reason.
- Modify: every hit the extended checker reports (about 120 lines across ~55 files, listed by running it). Rule per hit: open the Tauri counterpart (`src-tauri/src/<same name>.rs`, e.g. `remote_ssh.rs`, `pty.rs`, `checkpoint/turn.rs`, `mobile_router.rs`, `harness_updates.rs`) and do what the fork did there. Known decisions:
  - `User-Agent` values, git checkpoint author `MonoCode` -> `Monoptah` (Tauri did).
  - `TERM_PROGRAM` -> `Monoptah` (Tauri `pty.rs:350`).
  - "MonoCode Host" in UI/progress copy -> "Monoptah Host" (Tauri `remote.rs:427`); the host binary name, data dir and launchd label stay.
  - `app_menu.rs`: `REPO` and issue links -> `https://github.com/hhoangg/monoptah`; "MonoCode Website" -> "Monoptah Website" at `https://zptah.app`.
  - `mono.rs:320` "MonoCode adds this context ... a persistent Mono in MonoCode" -> Monoptah; tag `<monocode_context>` stays.
  - `remote/ssh.rs:401` host package download URL: keep upstream and ALLOW-list it, reason: "native version 0.16.x has no Monoptah host release; release infra is out of scope". Report it.
  - Tests asserting old copy (`skills.rs:762-782`, `scm/tests.rs`, `remote/ssh.rs:892`): update assertions where the code under test changed; pure fixture data that does not come from product copy (e.g. a `MonoCode ship` skill description fixture) may stay, ALLOW-listed by line content with the reason.
  - Protocol identifiers (`remote/protocol.rs` doc comment, `monocode_native` clientInfo name) stay; `clientInfo.title` "MonoCode Native" -> "Monoptah Native".
- [ ] **Step 1:** extend the checker; run `./scripts/check-fork-branding.sh`; expect exit 1 with the native hits.
- [ ] **Step 2:** fix or ALLOW each hit per the rules.
- [ ] **Step 3:** run `./scripts/check-fork-branding.sh; echo $?` -> `0`, then `npm run native:check` -> `0`. Because the checker also scans `src/` and `src-tauri/`, confirm it still reports nothing there.

**Phase 1 commit message (proposed):** `Native: Monoptah identity, data paths and branding`

---

## Phase 2 - ClickUp inbox source and automation trigger

### Task 2.1: ClickUp client in core

**Files:**
- Create: `crates/monocode-core/src/inbox/clickup.rs` (port of `src-tauri/src/clickup.rs` without the 8 `#[tauri::command]` wrappers)
- Modify: `crates/monocode-core/src/inbox.rs:6-11` (`pub mod clickup;`)

**Interfaces (mirror `inbox/jira.rs`: every fn takes `app: PathBuf`, the data dir, and is blocking):**
- `clickup_status(app) -> Result<ClickUpStatus, String>`
- `clickup_set_config(app, token: String, team_id: Option<String>) -> Result<ClickUpStatus, String>` (empty token deletes `clickup-config.json`)
- `clickup_list_workspaces(app, token: Option<String>) -> Result<Vec<ClickUpWorkspace>, String>`
- `clickup_list_spaces(app) -> Result<Vec<ClickUpSpace>, String>`
- `clickup_list_issues(app, space_ids: Option<Vec<String>>, include_closed: bool, limit: Option<usize>) -> Result<Vec<ClickUpIssue>, String>`
- `clickup_issue_details(app, task_id: String)`, `clickup_issue_thread(app, task_id)`, `clickup_issue_comment(app, task_id, body)`
- `pub(crate)` for Phase 6: `clickup_get`, `require_config`, `fetch_spaces_for`, `epoch_ms_to_iso`, `task_url`, `RATE_LIMITED`; reuse `jira.rs`'s `write_secret_file` (make it `pub(crate)` in a shared spot rather than copying).
- Config JSON stays `{"token","teamId","teamName","username"}` at `<data_dir>/clickup-config.json`, mode 0600.

- [ ] **Step 1:** copy the 33 Tauri tests (`clickup.rs:1111+`) into the new module, adapting only `AppHandle` -> `PathBuf`; run `cargo test ... -p monocode-core inbox::clickup`; expect compile FAIL.
- [ ] **Step 2:** port the logic verbatim (limits 40/100, `order_by=updated&subtasks=true`, digit-only space ids, 5 pages of 100, `newest_first`, `state_type` mapping open/unstarted->`new`, closed/done->`done`, else `indeterminate`, thread pages of 25 up to 5, comment markdown from runs, 429 backoff from `X-RateLimit-Reset` default 60 s max 120 s).
- [ ] **Step 3:** run the tests; expect 33 PASS.

### Task 2.2: ClickUp in the inbox `Service`

**Files:**
- Modify: `crates/monocode-core/src/inbox.rs` (`PROVIDERS` 5->6, `provider_label` "ClickUp", `is_tracker`, `Item::key`, `Status`, `ViewState`, `Snapshot`, `NotificationTracker`, `Service::{status,connect,list,details,thread,comment}`)

**Interfaces:**
- Consumes: Task 2.1 functions.
- Produces: `ViewState.hidden_clickup_spaces: Vec<String>` (serde default; imported from desktop pref `monocode.clickupHiddenSpaces` on first run AND when the saved state lacks the field), `Snapshot.clickup_spaces: Vec<OptionRow>`, `Status.organization` = workspace name, `Status.email` = ClickUp username (used for assigned-to-me), `Service::connect_clickup(token: String, team_id: Option<String>) -> Result<Status, String>` and `Service::clickup_workspaces(token: String) -> Result<Vec<OptionRow>, String>` for the GPUI dialog.
- Item mapping: `repo`/team = space name, `team_id` = space id, `project_name` = list name, `identifier` = custom id or `#id`, key on the raw task id.

Translate from `src/features/inbox/model/clickup.ts` and `githubTasks.ts:1012-1039`: `reconcile_hidden_spaces`, `space_ids_for_fetch` (None = no filter, `Some(vec![])` = everything hidden -> return no items), `assigned_to(username)` (case-insensitive, empty username matches nothing), limit 100 when state is "all" or assigned-to-me, otherwise 40; drop hidden `team_id`s from results.

- [ ] **Step 1:** write tests ported from `clickup.test.ts` (27 cases; skip pure React/localStorage I/O cases and say which), plus `clickup_items_key_on_task_id` (two tasks, no custom id, same space -> different keys) and `clickup_hidden_spaces_import_from_desktop_prefs`.
- [ ] **Step 2:** run; expect FAIL. **Step 3:** implement. **Step 4:** run `cargo test ... -p monocode-core inbox`; expect PASS.

### Task 2.3: ClickUp automation trigger in core

**Files:**
- Modify: `crates/monocode-core/src/automations.rs` (allowlists near `:299`, `:338`, `:913`; `event_label` `:1122` -> "Task appeared"; trigger arm near `:1310` -> `issue_created`; event key `clickup:task:<id>`; repo/space filter compares `team_id`)

- [ ] **Step 1:** tests `accepts_clickup_triggers`, `clickup_event_key_matches_desktop` (`clickup:task:86abc12`), `clickup_space_filter_matches_space_id`; expect FAIL. **Step 2:** implement. **Step 3:** `cargo test ... -p monocode-core automations`; PASS.

### Task 2.4: ClickUp in the GPUI Inbox and Settings

**Files:**
- Create: `crates/monocode-gpui/assets/inbox-clickup.svg` (two chevrons, `#8930FD` / `#49CCF9`, from `InboxProviderMark.tsx:64`)
- Modify: `crates/monocode-gpui/src/assets.rs:70`, `crates/monocode-gpui/src/inbox/ui.rs` (`provider_mark`, `filters_active`, a SPACES section in the filters menu like Jira's PROJECTS, "Assigned to me (newest 100)" label for ClickUp, kind label "Task", "Open in ClickUp", source label `space / list`, connect hint and a workspace step when the token reaches more than one workspace, "Create API token" link `https://app.clickup.com/settings/apps`), `crates/monocode-gpui/src/inbox.rs` (connect flow calls `connect_clickup`; start-work draft prefix "Work on this ClickUp task:"), `crates/monocode-gpui/src/settings/pages/inbox.rs:31-37,104`, `crates/monocode-gpui/src/settings/model.rs:146,487`, `crates/monocode-gpui/src/settings/notification_preferences.rs:180,661` (kind `clickup`, issues category, project id `clickup:{host}:space:{team_id}`).

- [ ] **Step 1:** add GPUI unit tests next to the existing Jira ones in `inbox/ui.rs`/`settings` tests (filters menu lists spaces; hiding a space updates `hidden_clickup_spaces`; settings search finds "clickup"). **Step 2:** implement. **Step 3:** `cargo test ... -p monocode-gpui inbox -- --test-threads=1`; PASS.

### Task 2.5: ClickUp in the GPUI automations editor

**Files:**
- Modify: `crates/monocode-gpui/src/automations/trigger_menu.rs:11,62`, `crates/monocode-gpui/src/automations/editor.rs:48` (`events("clickup")` -> Task appeared; a space picker for the ClickUp trigger filled from `clickup_list_spaces`, stored by space id, like `AutomationsView.tsx:1841-1895`), `crates/monocode-gpui/src/automations.rs:45` (icon).

- [ ] **Step 1:** test that the trigger menu offers ClickUp > Task appeared and the editor saves a space-id filter. **Step 2:** implement. **Step 3:** `cargo test ... -p monocode-gpui automations -- --test-threads=1`; PASS. Then `npm run native:check` -> `0`.

**Phase 2 commit message (proposed):** `Native: ClickUp inbox source and Task appeared trigger`

---

## Phase 3 - Small items

### Task 3.1: Delete provider session files

**Files:**
- Create: `crates/monocode-core/src/provider_cleanup.rs` (port of `src-tauri/src/provider_session_cleanup.rs` minus the Mono-reset planner, which lands in Phase 5; reuse `accounts::Profile::home`, `Book::list`, and `accounts/transfer/codex.rs` `state_db`/`rollout_matches`/`history_base` instead of copies)
- Modify: `crates/monocode-core/src/lib.rs`; `crates/monocode-gpui/src/shell/project_removal.rs:130` (custom modal: "Also delete the Claude Code/Codex session files" and "Also delete the unused worktree", both on by default, provider checkbox only for Claude/Codex); `crates/monocode-gpui/src/shell/session_groups.rs:175,615,750-786` (sidebar delete, Backspace, bulk delete go through the dialog; cleanup runs on the background task after the row is gone, record read before delete).

**Interfaces:** `provider_cleanup::{CleanupRequest, cleanup_request(session: &store::Session, side_threads: &[String]) -> Option<CleanupRequest>, run_cleanup(data_dir: &Path, request: &CleanupRequest) -> Vec<String>}` (problems, never panics).

- [ ] Steps: port the 27 Tauri tests (drop the reset-planner ones and list them for Phase 5) -> FAIL -> implement -> PASS; GPUI test that sidebar delete opens the dialog instead of deleting immediately.

### Task 3.2: Cmd+Q hold-to-quit gate

**Files:** Create `crates/monocode-gpui/src/shell/quit_gate.rs` (pure state machine translated from `quitGate.ts`: `QUIT_HOLD_MS = 1000`, `QUIT_ARM_MS = 2000`, `fn press(&mut self, repeat: bool, now: Instant) -> QuitGateStep { Arm, Quit, Ignore }`); macOS key monitor following `shell/shortcuts/macos.rs:18-75` (Cmd+Q, no Shift/Control/Option, Caps Lock allowed); centered "Hold ⌘Q to Quit" pill beside `notices_overlay` (`shell/notifications.rs:906`); off macOS Ctrl+Q gated except when the terminal dock has focus. Menu Quit and tray stay ungated.

- [ ] Steps: port the 10 `quitGate.test.ts` cases + 5 `quit_hotkey.rs` matcher cases -> FAIL -> implement -> PASS.

### Task 3.3: Git blame - core

**Files:** Modify `crates/monocode-core/src/scm.rs` (`pub fn blame(cwd: &str, relative: &str) -> Result<Vec<BlameHunk>, String>`, porcelain parse ported from Tauri `fs.rs` `git_blame`/`parse_git_blame_porcelain`/`blame_candidate`, remote dispatch like `diff_stats` at `scm.rs:132` via command `git_blame`), `crates/monocode-core/src/files/remote.rs:57-99` (allow `git_blame`).
`BlameHunk { sha, short_sha, author, author_time: i64, summary, uncommitted: bool, start_line: usize, line_count: usize }` (serde camelCase to match the host's JSON).

- [ ] Steps: port the 6 Rust tests -> FAIL -> implement -> PASS.

### Task 3.4: Git blame - editor

**Files:** Create `crates/monocode-gpui/src/file_editor/blame.rs` (pure: `map_blame_lines`, `format_blame_age`, `PAST_END` handling, ported with the 53 `editorBlame.ts` cases that are not CodeMirror-DOM specific); modify `crates/monocode-gpui/src/input/code.rs` (blame column 22ch before line numbers, keeps width while loading; inline text after the cursor line, "Not committed yet" for uncommitted; hover tooltip via `code_tooltip`; click opens `ReviewTab::Commit`), `crates/monocode-gpui/src/file_editor.rs` (fetch beside `refresh_git` `:563`, refetch on disk revision), `crates/monocode-gpui/src/settings/preferences.rs:57` + `crates/monocode-core/src/desktop_preferences/workspace_preferences.rs:12` (`editorInlineBlame` default true, `editorBlameColumn` default false), `crates/monocode-gpui/src/app_menu.rs:93` (View > Toggle Git Blame, `cmd-alt-b` / `ctrl-alt-b`).

- [ ] Steps: tests -> FAIL -> implement -> PASS.

### Task 3.5: Inbox extras

**Files:** `crates/monocode-core/src/inbox/github.rs:125-159` (origin-derived `HOST/OWNER/NAME` target, `gh repo view <target> --json nameWithOwner`, no parent; port the 3 Tauri tests incl. `..._leaves_a_forks_parent_out`); `crates/monocode-gpui/src/inbox/ui.rs:901` (Send to agent on PRs), `:540`, `:875` (spinner via `session_mark::spinner`, label "N related threads, M running"); `crates/monocode-gpui/src/inbox.rs` (`set_busy(HashSet<i64>)`, per-source `HashMap<String, ViewMemory { search, selected_key, scroll, list_limit }>` in memory, saved/restored in `change_source` `:532`); `crates/monocode-gpui/src/shell/app_views.rs:251` (push busy ids).

- [ ] Steps: port `inboxRunningSessions.ts` (4) and `inboxViewMemory.ts` (5) cases -> FAIL -> implement -> PASS. Then `npm run native:check` -> `0`.

**Phase 3 commit message (proposed):** `Native: provider file cleanup, quit gate, git blame and Inbox extras`

The new-session provider prompt (I) moves to Phase 4: it is only shown when the default provider's surface is Terminal, which does not exist natively before Phase 4.

---

## Phase 4 - Terminal (TUI) session surface

Behavior contract: `docs/tui-sessions.md`. Reference survey with file:line for both apps: `.superpowers/sdd/2026-10-24-gpui-fork-port/phase4-survey.md` (git-ignored scratch; the facts below are the binding ones).

**Phase rulings:**
- A TUI session row gets a `TuiSession` entity in its own Shell map and never a `Conversation`; every path that would create, resume, preview or send to a `Conversation` checks `Session.surface` first. Cost if wrong: more guards later instead of fewer.
- Surface storage: shared desktop DB uses the `sessions.surface` column when the table has it (the fork's Tauri app always adds it); if the column is missing, TUI is unavailable and new sessions are chat. `shared_schema.sql` gains `surface TEXT` for new installs. The native (non-shared) store keeps surface in the per-session agent settings JSON. Cost if wrong: TUI unavailable on DBs from upstream builds.
- A TUI row with a non-blank provider id is listed (`has_user_message = 1` when the id is saved); blocks are never stored for TUI rows, so `shared_store_blocks` can never clear the flag.
- Native writes Claude hook events to `<data_dir>/tui-hooks-native` (the path rides in each launch's argv), so the Tauri app and native never drain each other's spool.
- Account env removal (`ANTHROPIC_API_KEY` etc.) uses an `/usr/bin/env -u KEY ... <program> <args>` wrapper on unix, because alacritty's `tty::Options` cannot remove variables. Windows sets env only and injects no hooks (as Tauri).
- `--model` is omitted when the session has no model; access maps through the existing `AccessMode::claude()` (ReadOnly -> `plan`).
- Quit: a confirmation only when terminal turns are busy, copy from `quitWhileBusyMessage(0, n)`: "N terminal turn(s) ... will stop, though its/their conversation(s) is/are saved". Native has no chat-turn quit confirmation today and this phase does not add one.
- Moving a pane between tabs needs no PTY handoff natively (entities live in Shell maps keyed by session id).

### Task 4.1: Core surface model

**Files:** Create `crates/monocode-core/src/surface.rs`; modify `crates/monocode-core/src/lib.rs`, `store.rs` (`Session.surface`), `store/shared.rs` (read via `shared_field("surface", ...)`, `create` with surface only when the column exists, `set_terminal_conversation(id, provider_id)` sets `provider_session_id` and `has_user_message = 1`, guard `shared_store_blocks` for TUI rows), the native store equivalent (agent settings JSON), `store/shared_schema.sql`.
**Interfaces:** `pub enum Surface { Chat, Tui }` (serde lowercase, default Chat, `parse(&str) -> Option<Surface>`); `pub struct TuiCaps { resume: bool, initial_prompt: bool, launch_flags: bool }`, `pub fn tui_caps(kind: provider::Kind) -> TuiCaps` (only Claude all true); `pub fn new_session_surface(kind, cwd: &str, preferred: Surface) -> Surface` (remote `remote://` -> Chat); `pub fn automation_surface(kind, cwd, preferred) -> Surface` (TUI only when preferred TUI, `initial_prompt`, not remote); `Store::create_session(provider, cwd, surface) -> Result<i64, String>` (or extend the existing create; falls back to Chat when TUI is unavailable and reports which it used); `Store::set_terminal_conversation(id, provider_id)`; `Store::surface_supported() -> bool`.
- [ ] Port tests: Tauri `session_store.rs` surface tests (4), `sessionStoreSurface.test.ts` and `sessionSurface.test.ts` cases that apply to the store/model, `automationSession.test.ts` surface cases. RED, implement, GREEN (`cargo test ... -p monocode-core surface store`).

### Task 4.2: Claude hook spool and `tui-hook` subcommand

**Files:** Create `crates/monocode-core/src/tui_hook.rs` (port of `src-tauri/src/tui_hook.rs`: `classify`, `write_event`, `drain`, `prune`, `spool_arg`, `run`, `MAX_EVENT_AGE = 5 s`, `MAX_STDIN = 1 MiB`, file name `{nanos:020}-{pid:010}.json`, temp-then-rename); modify `crates/monocode-gpui/src/main.rs` dispatch (`tui-hook` beside `visual`/`app`, exits with `run`'s code, prints nothing).
**Interfaces:** `pub fn spool_dir(data_dir: &Path) -> PathBuf` (= `<data_dir>/tui-hooks-native`), `pub struct HookEvent { provider_session_id: String, busy: bool }`, `pub fn drain(dir: &Path, now: SystemTime) -> Vec<HookEvent>`, `pub fn prune(dir: &Path, age: Duration)`, `pub fn run(args: impl Iterator<Item = String>) -> i32`.
- [ ] Port the 12 Tauri tests verbatim (adapting only paths). RED, implement, GREEN.

### Task 4.3: Core TUI launch builder

**Files:** Create `crates/monocode-core/src/tui_launch.rs`; modify `crates/monocode-core/src/accounts.rs` (`Profile::env(kind) -> Result<(Vec<(String, OsString)>, Vec<&'static str>), String>` returning the same set/remove pairs `Profile::apply` uses; `apply` reuses it).
**Interfaces:** `pub struct LaunchInput { kind, binary: PathBuf, model: Option<String>, access: AccessMode, provider_id: Option<String>, new_id: Option<String>, initial_prompt: Option<String>, hook: Option<(PathBuf /*cli*/, PathBuf /*spool*/)>, windows: bool }`, `pub struct TuiLaunch { program: OsString, args: Vec<OsString>, minted_id: Option<String> }`, `pub fn build(input: LaunchInput) -> TuiLaunch` (argument order exactly as `tuiLaunch.ts` :101-150: `--model`, `--permission-mode`, `--settings <json>`, `--resume <id>` or `--session-id <id>`, then `--` if the prompt starts with `-`, then the prompt, only when not resuming); `pub fn hook_settings_json(cli: &Path, spool: &Path) -> String` (exact JSON, five events, `async: true`); `pub fn shell_path(path: &Path) -> String` (bare when `^[A-Za-z0-9._/:-]+$` and absolute, else single-quoted with `'\''`); `pub fn tui_binary(kind) -> Result<PathBuf, String>` (`agy` literal for Antigravity, else `provider::resolve`); `pub fn no_conversation(text: &str) -> bool` (regex `no conversation found with session id`, case-insensitive); `pub const EARLY_EXIT: Duration = 5 s`; `pub fn exit_notice(...) -> String` ("Could not start X" / "Process exited (code N)" + hints); `pub fn can_start_fresh(...) -> bool`; `pub fn clean_title(raw: &str) -> Option<String>` (strip control and leading non-alphanumerics, max 80); `pub fn can_retitle(current: &str, last_cli_title: Option<&str>, placeholder: &str) -> bool`.
- [ ] Port `tuiLaunch.test.ts` (25, incl. the exact command-string test), `tuiBinary.test.ts` (3), and the pure `tuiSession.test.ts` cases (notice, fresh-start, title, no-conversation) with exact copy. RED, implement, GREEN.

### Task 4.4: Terminal program launch

**Files:** Modify `crates/monocode-gpui/src/terminal.rs`; `crates/monocode-core/src/process.rs` (a pub way to register a pid so `shutdown_all` reaps it).
**Interfaces:** `Terminal::launch(cwd: &Path, program: OsString, args: Vec<OsString>, env_set: Vec<(String, OsString)>, env_remove: Vec<&'static str>, cx) -> Terminal` (same base env as the dock shell: TERM, COLORTERM, COLORFGBG, `TERM_PROGRAM=Monoptah`, PATH, PWD; cwd falls back to home when missing); `TerminalEvent::{Changed, Started { at: Instant }, Exited { code: Option<i32>, error: Option<String>, tail: String }, Title(String)}` (tail = last 4000 chars of plain grid text); `Terminal::kill(&self)` (SIGHUP + SIGTERM to pid and group, SIGKILL after 1 s); a spawner seam so GPUI tests can fake a launch (the `cfg(test)` detached path stays for the dock).
- [ ] Tests: program exits with code N reported in `Exited`; spawn error reported; title forwarded; env removal wrapper built correctly (pure function test). RED, implement, GREEN. Dock terminal behavior unchanged.

### Task 4.5: `TuiSession` pane in the shell

**Files:** Create `crates/monocode-gpui/src/tui_session.rs`; modify `shell.rs` (`tui_sessions: HashMap<i64, Entity<TuiSession>>`, `ensure_session`/`session_loading.rs` branch on surface, `session_surface` renders the TUI view in place of `chat_with_background`, `replace_blank_pane` and `ensure_project_session` never treat a TUI session as blank, `refresh_session_mark` ORs TUI busy, tab/pane close and `forget_session`/archive/delete kill the CLI, the exit-close plan: close the pane, or the tab, or turn the last project tab into a blank chat), sidebar row (no preview popover for TUI rows).
**Behavior:** launch on first show: pin account, resolve binary, build launch (hooks when `claude_hooks` is on and not Windows; cli = `current_exe`), persist a minted id with `set_terminal_conversation` BEFORE spawning (save failure -> nothing spawns, error bar); exit within 5 s with the no-conversation text on a reused id -> relaunch once with `--session-id <same id>`; close only on code 0, no error, not early, no no-conversation; otherwise the bar with Restart, and "Start new conversation" when `can_start_fresh`; title follows the CLI title through `clean_title`/`can_retitle` and `store.title`; header "<Provider> terminal", cwd, disabled "New worktree".
- [ ] Port the applicable `TuiSessionPane.test.ts` cases (skip xterm/attach/handoff ones, list them), `sessionRemovalTui.test.ts` (5), `projectReturn` TUI case, using the spawner seam. RED, implement, GREEN.

### Task 4.6: Hook busy watcher

**Files:** `crates/monocode-gpui/src/tui_session.rs` or a small `shell/tui_busy.rs`; `shell.rs` init.
**Behavior:** one app-wide watcher: `prune(spool, MAX_EVENT_AGE)` at boot, then every 300 ms on the background executor `drain`; map `provider_session_id` to the open TUI session with that provider id; busy true/false feeds `refresh_session_mark`; process exit clears busy. Busy never persists across restarts.
- [ ] Tests: event for an open session sets busy; unknown id ignored; exit clears; stale busy ignored (via core). RED, implement, GREEN.

### Task 4.7: Surface preference, creation paths and the provider prompt

**Files:** `settings/preferences.rs` (`provider_surfaces: HashMap<String, Surface>`), `desktop_preferences/workspace_preferences.rs` (import `monocode.providerSurface.v1`, keep valid kinds and values), `settings/pages/providers.rs` (segmented "Chat" / "Terminal (TUI)" per provider, settings search entry), `shell.rs` (`NewCodex`/New tab/sidebar + use `new_session_surface`; when `should_prompt_for_provider` open the "Choose a provider" modal; `split_session` follows the preference), every app-driven path in survey B12 pinned to Chat (continuation keeps the source surface), `start_inbox_session` keeps TUI only when the provider takes an initial prompt (prompt becomes the first prompt).
**Interfaces:** `pub fn should_prompt_for_provider(cwd, default_kind, surfaces, available: &[Kind]) -> bool` (false for remote; false unless default surface is TUI; true when at least 2 providers qualify: shown in the picker, available or not yet probed, not hidden for the project); `move_highlight(index, len, delta) -> usize` (wraps). Modal copy: title "Choose a provider", description "For this session only", hint "The default provider opens as a terminal session, which can't switch providers afterwards.", row badge "TUI" or "Chat", default highlighted first, up/down/Enter/click; dropped if the project changes.
- [ ] Port `providerSurface.test.ts` (6), `newSessionProviderPrompt.test.ts` (13), `NewSessionProviderDialog.test.ts` (4) as Rust/GPUI tests; a test per forced-chat path group (Inbox Ask, Monos, handoff, quick composer, mobile, orchestration, app control). RED, implement, GREEN.

### Task 4.8: Automations in the terminal and quit count

**Files:** `crates/monocode-gpui/src/shell/automation_runs.rs` (TUI path: never reuse a session, prompt becomes the initial prompt, "new worktree" created before launch, `running` written first, success after the CLI stays up 5 s, `failed` with the exit notice if it exits or cannot start first, `cancelled` if the session is gone), automations editor (Conversation reuse disabled for a TUI-surface provider), `shell.rs` `request_quit` (terminal-turn confirmation per the phase ruling).
- [ ] Port `automationSession.test.ts` (6) and the `appLifecycleTui` quit-count cases. RED, implement, GREEN. Then `npm run native:check` -> 0 and `./scripts/check-fork-branding.sh` -> 0.

**Phase 4 commit message:** `Native: terminal (TUI) session surface with Claude hook busy state`

## Phase 5 - Mono extensions and Telegram questions

Reference survey with file:line for both apps: `.superpowers/sdd/2026-10-24-gpui-fork-port/phase5-survey.md` (git-ignored scratch; the facts below are the binding ones). Two commits: 5a = Tasks 5.1-5.7 (Mono), 5b = Tasks 5.8-5.10 (Telegram).

**Phase rulings:**
- Data-loss fix first: native `mono/habits.rs` `parse`/`update` currently drop habits they cannot validate and rewrite `habits.json`, deleting interval habits saved by the Tauri app. Unreadable entries must be kept verbatim on every write.
- Native session ids are integers. `parentSessionId` takes a native session id (number or numeric string). The Tauri `app-<parent>-<requestId>` id scheme, its 255-character limit and its "no hyphens" advice do not apply; HELP and the Mono prompt describe the native rule. The parent link is stored with the existing `save_started_link(child, parent)`, so it shows in the sidebar as a started session.
- Parent validation uses the persisted `Mono.launches` (TS keeps an in-memory registry lost on restart): the parent must be a session this Mono (or its habit run) started, listed in the same project, and must not itself have a parent ("a child of a child is not allowed"). Error copy follows TS.
- Habit semantics follow the fork: sessions a habit run starts report to the Mono's chat with `each` delivery; the run does not wait for them; a run timeout or approval wait stops the run but not the sessions it started; the post says what it started ("Started N sessions." etc.).
- Mono reset with "Also delete ... session files" deletes the old conversation row (as Tauri's reset does) after reading its provider ids, then runs the provider-file cleanup in the background; problems show as a non-blocking notice. Without the box ticked, the old row is also deleted (Tauri parity). Native records retired provider ids on rotation in the setting `mono-retired:<session>` (newest last, cap 200) and, on desktop data, also reads Tauri's `monocode:mono-rotations[<persisted id>].retired`.
- Folder colors use the existing palette index (`Groups::set_color`), not TS hex custom colors.
- Telegram uses the same `<data_dir>/telegram-config.json` as the Tauri app (shapes unchanged). If both apps poll one bot, Telegram answers HTTP 409 for one of them; the existing `api.rs` classification surfaces it as the project's `problem` ("Another app is reading this bot" or the Tauri wording). No cross-app lock.
- Question mapping: Telegram `request_id: u64` = stable hash of the native question id; option ids map to native answer labels; `__custom__` + text becomes typed text; session keys in `topics` use `store.persisted_session_id(id)` so Tauri and native share topic bindings.
- Workflow context in Telegram headers and workflow tickets stays empty until Phase 7.

### Task 5.1: Interval schedule in core (and stop losing habits)
**Files:** `crates/monocode-core/src/mono/habits.rs` (Schedule gains `interval_hours: Option<u8>`, `interval_from: Option<String>` serde camelCase defaults; `Schedule::from_cli(&Value) -> Result<Schedule, String>` with the TS error strings; `anchor(now)`; interval `next()` per `nextIntervalRunAt`; label "Every hour from HH:MM" / "Every N hours from HH:MM"; `parse`/`update` keep unreadable entries verbatim), CLI schedule call sites (`monos/host.rs` habits.add/update, `mono/cards.rs`, `monos/cards.rs`) use `from_cli`; `anchor` applied on add and on update when the schedule changes; `mono/bridge.rs` HELP habits line.
**Tests:** port `monoHabits.test.ts` interval cases (8) and the `agentAppMemory` CLI round-trip (1); a test that updating a file containing an interval habit and an unknown-kind habit keeps both bytes-equivalent.

### Task 5.2: Interval editor in GPUI
**Files:** `crates/monocode-gpui/src/monos/scheduler.rs` (`schedule_controls`, `habit_schedule`): kind "Every N hours", default 4, "Hours between runs" picker "1 hour".."24 hours", "from" instead of "at", Next preview via `anchor`.
**Tests:** editor builds an interval schedule with the anchor date; label shown in the list.

### Task 5.3: `sessions.start` fields: no-commit worktree, folders, parent
**Files:** `mono/api.rs` (fields `completionDelivery`, `parentSessionId`, `folderName`, `folderId`; `completionDelivery` must be "group"|"each" with the TS error; `parentSessionId` non-text), `monos/host.rs` sessions.start: unborn-HEAD check ("This project has no commits yet; make an initial commit before using workspaceMode worktree") and every validation BEFORE `db.create` (fixes the orphan row on worktree failure); `folderChoice`/`fileLaunchInFolder` semantics on `Groups` (existing `folderId` wins, never renamed; `folderName` creates; unknown `folderId` alone -> "Folder was not found in this project"; Mono palette color; result `folderId`/`folderName`; hidden launches validated but not filed); `parentSessionId` validation per phase ruling + `save_started_link` + `Shell::mono_link_started`; `crates/monocode-core/src/git.rs` `unborn(cwd) -> bool`.
**Tests:** port the folder, parent and no-commit cases of `agentApp.test.ts` as host tests (`monos/tests.rs` setup/call).

### Task 5.4: Completion delivery and habit reporting
**Files:** `mono.rs` `Launch` gains `#[serde(default)] each: bool` (and parent if needed); `monos/host.rs` `check_completions` groups by `origin_turn` for group and per launch for each; `sessions.send` `completionDelivery`; habit-run launches report to the Mono chat (origin = the chat session) with forced each; `monos/scheduler.rs` `check_habits` no longer stops children on timeout/approval wait, `habit_changed` no longer waits for children, post text per TS ("Started a session." / "Started N sessions."); prompt text in `mono.rs` context, the habit run prompt (`scheduler.rs`), and `bridge.rs` HELP (sessions.start/send lines) describe each/group, parent and folders in native terms.
**Tests:** port `monoCompletionBatches.test.ts` (8) semantics and the habit reporting cases of `useMonoHabits.test.ts`.

### Task 5.5: Habit Stop sheet
**Files:** `monos/mod.rs` `RunningHabit` gains `stop: Option<StopMode>` and `started: Vec<i64>`; `monos/scheduler.rs` `habit_actions` Stop button (only while running), 3-button prompt: text `Stop "<name>"? Sessions it already started can keep running.`, buttons "Stop run only", "Stop run and its sessions", "Keep running"; "Stopping..." disabled after request; `"<name>" already finished, so there was nothing to stop.` if it ended meanwhile; `check_habits` applies the stop (record "failed" with "Stopped by the user."; mode all stops the sessions the run created; post "Stopped. Started N session that keeps / sessions that keep running." and "Could not stop N session(s): ids.").
**Tests:** port `habitStop.test.ts` (7) and the stop cases of `useMonoHabits.test.ts`.

### Task 5.6: Started rail latch
**Files:** create `crates/monocode-gpui/src/shell/started_rail.rs` (pure port of `src/app/shell/startedRail.ts`: `{open, closed}`, `started_rail_open`, `sync_started_rail`, `toggle_started_rail`), `shell/rails.rs` `rail_open` uses it, row hover chevron "Collapse started sessions" / "Expand started sessions".
**Tests:** port `startedRail.test.ts` (18) verbatim and `startedSessions.test.ts` (3).

### Task 5.7: Mono reset deletes provider files
**Files:** `crates/monocode-core/src/provider_cleanup.rs` (`ProviderSessionRef`, `plan_reset_cleanup`, `run_cleanup_report(data_dir, &[CleanupRequest], include_mono_stores)`, `mono_codex_homes`), retired-id store (setting `mono-retired:<session>`, cap 200, + desktop read of `monocode:mono-rotations`), `crates/monocode-gpui/src/runtime.rs` rotation records the retired id, `monos/files.rs` `reset_conversation` custom modal with the checkbox ("Also delete the <label> session files on this computer", note "They can't be recovered." + provider note from `monoProviderFiles`), deletes the old row, background cleanup, problem notice copy from `providerFilesProblemMessage`.
**Tests:** port the 11 reset Rust tests left out in Phase 3, `monoProviderFiles.test.ts` (13), `monoRotation.test.ts` retired cases (8), and reset-flow cases from `monoWorkspace.test.ts`. Then `npm run native:check` -> 0 and commit 5a.

**Phase 5a commit message:** `Native: Mono pipeline controls, interval habits, habit stop and reset cleanup`

### Task 5.8: Telegram core
**Files:** move `src-tauri/src/telegram/{api,config,queue,render,pending,poller}.rs` into `crates/monocode-core/src/telegram/` with a Tauri-free `mod.rs` (functions taking `data_dir`, an `EventSink` that the GPUI side implements); do not modify `src-tauri/` (copy, not move: Tauri stays production).
**Tests:** all 154 Rust tests pass in core.

### Task 5.9: Forwarding model and question API
**Files:** `crates/monocode-core/src/telegram/forward.rs` (port of `telegramForward.ts`: `is_topic_linkable`, `is_forwardable`, `session_ticket_id`, `topic_title`, `question_context`, `forward_delay`, `header_of`, `forward_payload`, `due_forwards`), `telegram/forwarder.rs` (state machine of `useTelegramQuestions.ts`: 15 s tick, 2 attempts, eligibility on first sight, close reporting with "Answered on desktop" / "Skipped" / "Cancelled", answer validation); `Conversation::pending_question`, `answer_question`, and an outcome hook; ticket registry set in `start_inbox_session`.
**Tests:** port `telegramForward.test.ts` (56) and the portable `useTelegramQuestions.test.ts` cases.

### Task 5.10: Telegram in the GPUI app
**Files:** bridge started once per app with the native sink; answers applied through `answer_question`; project menu "Telegram..." dialog (token, connect link, enable toggle, "Send after" presets 0/30/60/120/300/600/900/1800/3600 s with "(custom)", status rows, Change token, Disconnect confirm, Remove webhook, any-group-member warning; hidden for remote projects; polls status every 3 s while open); session menu "Link Telegram topic..." (only when configured with a chat) and its dialog.
**Tests:** port the logic of `TelegramProjectDialog.test.ts` and `LinkTelegramTopicDialog.test.ts` that is not DOM-specific. Then `npm run native:check` -> 0 and commit 5b.

**Phase 5b commit message:** `Native: forward agent questions to Telegram`

## Phases 4 to 7 (sketch; detailed plan written when each starts)

- **Phase 4 - TUI surface:** per-provider surface preference (`monocode.providerSurface.v1`); `sessions.surface` read/write in `store.rs`/`store/shared.rs` (Tauri TUI rows stop showing as chat); `terminal.rs` spawn of program+args+env with exit code; a session-bound terminal pane opened from `open`/`activate_session`; Claude launch args (`--model`, `--permission-mode`, `--resume`/`--session-id`, `--settings` hooks), `agy` exception; `tui-hook` subcommand + spool watcher + `classify` with 5 s busy staleness into `busy_sessions`; app-driven sessions forced to chat; new-session provider prompt; automations follow the surface; close-on-clean-exit.
- **Phase 5 - Mono extensions and Telegram:** `api.rs` fields `completionDelivery`, `parentSessionId`, `folderName`/`folderId`; habit runs report to the Mono chat with `each` delivery; `interval` schedule in `mono/habits.rs`; habit Stop sheet in `monos/scheduler.rs`; no-commit worktree error in `git.rs`; Mono reset provider-file cleanup with retired rotation ids; `bridge.rs` HELP text; Telegram core module moved from `src-tauri/src/telegram/` with its 154 tests, forwarding model translated from `telegramForward.ts`, GPUI project dialog and topic link.
- **Phase 6 - Contributors and Ticket Contributors:** move `src-tauri/src/contributors*` into a new crate or core module (same DB tables), translate scoring/identity/leaderboard/period TS with tests, GPUI page with Ticket/GitHub tabs; ticket assessor and store reuse Phase 2's ClickUp client.
- **Phase 7 - Workflows:** core `mono/workflows/` (types, parse, validate, template, condition, output, engine, YAML, store as Mono files, report, guide) with the 598 TS cases triaged into Rust tests; `workflow_command.rs` command runner; 16 CLI actions in `api.rs`/`bridge.rs`/`host.rs`; `files.rs` allow-list for `workflows.json` and `workflow-runs/`; GPUI board editor and run graph.
