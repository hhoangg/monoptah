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

## Phases 4 to 7 (sketch; detailed plan written when each starts)

- **Phase 4 - TUI surface:** per-provider surface preference (`monocode.providerSurface.v1`); `sessions.surface` read/write in `store.rs`/`store/shared.rs` (Tauri TUI rows stop showing as chat); `terminal.rs` spawn of program+args+env with exit code; a session-bound terminal pane opened from `open`/`activate_session`; Claude launch args (`--model`, `--permission-mode`, `--resume`/`--session-id`, `--settings` hooks), `agy` exception; `tui-hook` subcommand + spool watcher + `classify` with 5 s busy staleness into `busy_sessions`; app-driven sessions forced to chat; new-session provider prompt; automations follow the surface; close-on-clean-exit.
- **Phase 5 - Mono extensions and Telegram:** `api.rs` fields `completionDelivery`, `parentSessionId`, `folderName`/`folderId`; habit runs report to the Mono chat with `each` delivery; `interval` schedule in `mono/habits.rs`; habit Stop sheet in `monos/scheduler.rs`; no-commit worktree error in `git.rs`; Mono reset provider-file cleanup with retired rotation ids; `bridge.rs` HELP text; Telegram core module moved from `src-tauri/src/telegram/` with its 154 tests, forwarding model translated from `telegramForward.ts`, GPUI project dialog and topic link.
- **Phase 6 - Contributors and Ticket Contributors:** move `src-tauri/src/contributors*` into a new crate or core module (same DB tables), translate scoring/identity/leaderboard/period TS with tests, GPUI page with Ticket/GitHub tabs; ticket assessor and store reuse Phase 2's ClickUp client.
- **Phase 7 - Workflows:** core `mono/workflows/` (types, parse, validate, template, condition, output, engine, YAML, store as Mono files, report, guide) with the 598 TS cases triaged into Rust tests; `workflow_command.rs` command runner; 16 CLI actions in `api.rs`/`bridge.rs`/`host.rs`; `files.rs` allow-list for `workflows.json` and `workflow-runs/`; GPUI board editor and run graph.
