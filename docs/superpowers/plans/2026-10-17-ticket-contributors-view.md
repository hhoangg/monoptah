# Ticket Contributors View Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move ticket scoring out of repo Contributors into its own app-level view, Ticket Contributors, where the user picks a ClickUp workspace and a Period, sees how many closed tickets need scoring, and scores a chosen coverage percentage.

**Architecture:** Repo Contributors is restored to its pre-ticket state. The Rust ticket backend (fetch, store, rubric, runners, job manager) stays as is, plus one estimate command. A new feature folder `src/features/ticket-contributors/` holds the moved ticket model and api wrappers, a pure board model (members, coverage, run selection), app-level prefs, a hook, and the view, wired into `App.tsx` and the sidebar like Mail.

**Tech Stack:** React + TypeScript, vitest, Rust (Tauri commands, rusqlite).

**Spec:** `docs/adr/0012-ticket-contributors-is-its-own-view.md`, `docs/adr/0013-score-by-coverage-percentage.md`, plus still-valid ADRs 1, 2, 4 (levels/points), 5, 7, 8, 9, 10 (batched job), 11, and `CONTEXT.md`. Previous plan for history: `docs/superpowers/plans/2026-10-16-contributors-tickets.md`.

## Global Constraints

- Branch `contributors-tickets`. **No commits, no staging.** Code, comments, UI copy in English; never `—` or `–`. UI says "Monoptah", never "MonoCode".
- Never rename `monocode.*` localStorage keys or other identifiers listed in root `CLAUDE.md`. New keys use the `monocode.` prefix: `monocode.ticketContributors.settings` and `monocode.ticketContributors.range`.
- Points table defaults `XS 3, S 8, M 16, L 32, XL 48`. Level mapping `XS 0-1, S 2-3, M 4-5, L 6-7, XL 8-10` lives only in `levelOf`.
- Coverage target: `target = ceil(p * N / 100)`, `toAssess = max(0, target - A)`, where N = closed in-scope top-level tickets in the Period and A = those with an assessment or an override.
- Run selection: round-robin across members by ClickUp user id, each member's oldest-closed unassessed ticket first; a ticket with several assignees is taken once, at its first member's turn; tickets without assignees come last, oldest first.
- Token estimate: `ceil(chars / 4)` over title + description + tags + subtask titles of each ticket, plus `ceil(len(system_prompt) / 4)` once per group of 20; comments not counted. Always labelled "estimate".
- Ticket members are ClickUp users keyed by user id, shown by username. No git identity merging.
- Period kinds offered: `month`, `quarter`, `year`, `all` (reuse `src/features/contributors/model/period.ts`; no `7d`). For `all`, the window starts `2015-01-01T00:00:00Z`.
- Repo Contributors after Task 1 must be byte-identical to `HEAD` for every file under `src/features/contributors/` that existed at `HEAD`.
- Full verification: `npm run check > /tmp/monoptah-check.log 2>&1; echo $?` prints `0`; `./scripts/check-fork-branding.sh; echo $?` prints `0`. Never pipe `npm run check`.

## Model Assignment

| Task | Implementer | Reviewer |
|---|---|---|
| 1 revert + relocate | sonnet | opus |
| 2 Rust estimate | sonnet | opus, with 3 |
| 3 board model + prefs | sonnet | opus, with 2 |
| 4 hook | sonnet | opus |
| 5 view | sonnet | opus, with 6 |
| 6 App wiring | sonnet | opus, with 5 |
| Whole branch | - | opus |

## Review Focus

1. **A workspace the token can no longer access, or no ClickUp token at all**: the view says so and points to Settings; it never throws. Test in Task 5.
2. **"All" on a large board**: fetching runs as the existing background job with progress and cancel; switching Period while a fetch runs does not start a second concurrent fetch for the same window. Test in Task 4.
3. **p = 0, p = 100, A > target, N = 0**: the button shows the right count or "nothing to score" and never queues an empty job. Test in Task 3 and Task 5.
4. **Overrides and assessments made under repo Contributors earlier** still show and count (same SQLite rows keyed by ticket id). Test in Task 4 via the load path.
5. **Opening Ticket Contributors closes every other main view and vice versa**, like Mail. Test in Task 6.

---

### Task 1: Restore repo Contributors and relocate the ticket model

**Files:**
- Backup first: `.superpowers/sdd/2026-10-17-ticket-contributors-view/backup/` (git-ignored): `git diff HEAD -- src/features/contributors > backup/contributors.patch` and copy every untracked file under `src/features/contributors/` into `backup/untracked/` keeping paths.
- Restore to `HEAD`: every modified tracked file under `src/features/contributors/` (`git checkout HEAD -- <file>` for each, only after the backup exists).
- Move: `src/features/contributors/model/tickets.ts` -> `src/features/ticket-contributors/model/tickets.ts`; `tickets.test.ts` alongside; `src/features/contributors/model/ticketsApi.ts` -> `src/features/ticket-contributors/model/ticketsApi.ts`.
- Delete (after backup): `src/features/contributors/hooks/useTickets.ts`, `useTickets.test.ts`, `model/scoring.tickets.test.ts`, `ui/TicketsTab.tsx`, `ui/TicketsTab.test.ts`.
- Modify: `src/features/settings/ui/TicketScoringSettings.tsx` import path to the moved `ticketsApi`.

**Interfaces:**
- Produces (moved, unchanged except as noted): `LEVELS`, `Level`, `CRITERIA`, `Criterion`, `Criteria`, `TicketAssessment`, `TicketAssignee`, `TicketRecord`, `DEFAULT_TICKET_POINTS`, `levelOf`, `effectiveLevel`, and all `ticketsApi` wrappers and types (`fetchTickets`, `assessTickets`, `cancelTicketJob`, `ticketJobStatus`, `loadTickets`, `setTicketOverride`, `getAiConfig`, `setAiConfig`, `listWorkspaceSpaces`, `TicketJobEvent`, `AiConfigStatus`, `TICKETS_EVENT`).
- Replace `ClickUpLink`/`TicketsConfig`/`inScope` in `tickets.ts` with:
  ```ts
  export type WorkspaceScope = { workspaceId: string; name: string; spaceIds: string[] | null; excludedStatuses: string[] };
  export function inScope(t: TicketRecord, scope: WorkspaceScope): boolean; // same rules as before, single workspace
  ```

- [ ] **Step 1:** Create the backup and verify it (`git apply --check` of the patch against `HEAD` in a scratch worktree is not required; confirm the patch file is non-empty and the untracked copies exist).
- [ ] **Step 2:** Restore, move, delete, and adjust `tickets.ts` + `tickets.test.ts` (rewrite the `inScope` tests for `WorkspaceScope`).
- [ ] **Step 3:** Run `git diff --stat HEAD -- src/features/contributors`. Expected: no output.
- [ ] **Step 4:** Run `npx vitest run src/features/ticket-contributors src/features/contributors src/features/settings` and `npx tsc --noEmit`. Expected: both exit 0.

---

### Task 2: Rust token estimate command

**Files:**
- Modify: `src-tauri/src/contributors/ticket_jobs.rs` (or a small new `ticket_estimate.rs` in the same module), `src-tauri/src/contributors.rs`, `src-tauri/src/lib.rs`.

**Interfaces:**
- Consumes: `ticket_store::tickets_for_assessment(conn, ids, force)`, `rubric::{system_prompt, TicketInput, truncate_input}`, the existing assignee redaction used when building inputs.
- Produces:
  ```rust
  #[derive(Serialize)] #[serde(rename_all = "camelCase")]
  pub(crate) struct TicketEstimate { tickets: u32, input_tokens: u64 }
  pub(crate) fn estimate(conn: &Connection, ids: &[String]) -> Result<TicketEstimate, String>;
  // command: contributors_tickets_estimate(app, ids: Vec<String>) -> TicketEstimate
  ```
  Counts only tickets that would really be assessed (`force = false`: skips assessed and overridden). Formula per Global Constraints, computed on the same redacted, truncated `TicketInput` the job would build (comments empty).

- [ ] **Step 1: Tests:** `estimate_counts_only_unassessed_tickets`, `estimate_adds_the_system_prompt_once_per_group_of_20` (21 tickets -> 2 prompt shares), `estimate_uses_truncated_redacted_text` (a 10 000-char description counts as 2000 chars).
- [ ] **Step 2:** `cd src-tauri && cargo test contributors::ticket` -> FAIL.
- [ ] **Step 3:** Implement; register the command.
- [ ] **Step 4:** `cargo test contributors`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` -> all exit 0.

---

### Task 3: Board model and prefs

**Files:**
- Create: `src/features/ticket-contributors/model/board.ts`, `board.test.ts`, `model/prefs.ts`, `prefs.test.ts`.

**Interfaces:**
- Consumes: Task 1 `tickets.ts`; `Period`, `isInPeriod`, `PeriodKind`, `PeriodPick` from `src/features/contributors/model/period.ts`.
- Produces:
  ```ts
  // board.ts
  export type MemberRow = { id: string; name: string; points: number; tickets: number; assessed: number;
    levels: Record<Level, number>; ticketList: { ticket: TicketRecord; level: Level | null; points: number; share: number }[] };
  export type Board = { total: number; assessed: number; members: MemberRow[]; unassigned: { tickets: number; assessed: number } };
  export function periodTickets(tickets: TicketRecord[], scope: WorkspaceScope, period: Period): TicketRecord[]; // inScope + isInPeriod, top-level only
  export function buildBoard(tickets: TicketRecord[], points: Record<Level, number>): Board; // tickets already period-filtered; points split equally across distinct assignee ids; members sorted by points desc, then name
  export function toAssessCount(total: number, assessed: number, percent: number): number; // Global Constraints formula; percent clamped 0..100
  export function pickForRun(tickets: TicketRecord[], count: number): string[]; // round-robin per Global Constraints; only tickets with no assessment and no override
  export const ALL_START = "2015-01-01T00:00:00Z";
  // prefs.ts
  export type TicketBoardSettings = { scope: WorkspaceScope | null; points: Record<Level, number>; percent: number };
  export function readSettings(): TicketBoardSettings; // key monocode.ticketContributors.settings, defaults { scope: null, points: DEFAULT_TICKET_POINTS, percent: 100 }, never throws
  export function writeSettings(s: TicketBoardSettings): void;
  export function readRange(now?: Date): PeriodPick; // key monocode.ticketContributors.range, default { kind: "month", id: null }; "7d" maps to month
  export function writeRange(pick: PeriodPick): void;
  ```
  `spaceIds` `[]` or all blank normalizes to `null` when read.

- [ ] **Step 1: Tests** (`board.test.ts`): `splits points equally between distinct assignees`; `a member's assessed count includes overrides`; `unassigned tickets are counted apart`; `toAssessCount covers p=0, p=100, A above target, N=0` (Review Focus 3); `pickForRun interleaves members oldest first`; `pickForRun takes a multi-assignee ticket once`; `pickForRun puts unassigned tickets last`; `periodTickets drops subtasks, other workspaces, excluded statuses, other periods`. (`prefs.test.ts`): defaults, round trip, malformed JSON falls back, `[]` spaceIds -> null, `7d` range -> month.
- [ ] **Step 2:** `npx vitest run src/features/ticket-contributors/model` -> FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Same command -> PASS; `npx tsc --noEmit` -> 0.

---

### Task 4: `useTicketBoard` hook

**Files:**
- Create: `src/features/ticket-contributors/hooks/useTicketBoard.ts`, `useTicketBoard.test.ts`.
- Modify: `src/features/ticket-contributors/model/ticketsApi.ts` (add `estimateTickets(ids: string[]): Promise<{ tickets: number; inputTokens: number }>` for Task 2's command).

**Interfaces:**
- Consumes: Tasks 1-3; `listClickUpWorkspaces`, `clickupConnected` from `src/features/inbox/model/clickup.ts`; `currentPeriod`, `previousPeriod`, `nextPeriod`, `resolvePick`, `pickOf` from contributors `period.ts`.
- Produces:
  ```ts
  export function useTicketBoard(): {
    connected: boolean | null; workspaces: { id: string; name: string }[]; settings: TicketBoardSettings;
    setScope(scope: WorkspaceScope | null): void; setPercent(p: number): void;
    period: Period; setKind(kind: "month" | "quarter" | "year" | "all"): void; goPrevious(): void; goNext(): void; hasNext: boolean;
    tickets: TicketRecord[]; board: Board; statuses: string[];          // statuses = distinct done/closed statuses seen in the period
    toAssess: number; runIds: string[]; estimate: { tickets: number; inputTokens: number } | null;
    ai: AiConfigStatus | null; job: TicketJobEvent | null; error: string | null;
    refresh(): Promise<void>; score(): Promise<void>; cancel(): Promise<void>;
    override(ticketId: string, level: Level | null, note: string | null): Promise<void>; reassess(ids: string[]): Promise<void>;
  };
  ```
- Behaviour: on workspace or Period change, load stored tickets for the window at once, then start one fetch (`fetchTickets([{ workspaceId, spaceIds, ... }], from, to)`) unless the same workspace+window was already fetched in this app session or a fetch for it is queued/running (Review Focus 2). Window = the selected Period only (`period.start`..`period.end`; `all` from `ALL_START`). Reload on job `done`, `cancelled`, `error`. `runIds = pickForRun(periodTickets, toAssess)`; `estimate` refreshes (debounced 300 ms) when `runIds` changes. `score()` returns early when `runIds` is empty. Job error stays visible after a successful reload (same rule as the old `useTickets`).

- [ ] **Step 1: Tests** for the pure helpers the hook uses, exported from the hook module: `fetchKey(workspaceId, from, to)`, `shouldFetch(key, fetchedKeys, job)` (skips fetched keys and a running fetch), `windowOf(period)` (`all` -> `ALL_START`), and `jobErrorAfter` semantics. Include a load-path test that an override-only ticket counts as assessed (Review Focus 4).
- [ ] **Step 2:** `npx vitest run src/features/ticket-contributors/hooks` -> FAIL.
- [ ] **Step 3:** Implement, mirroring the event handling of the deleted `useTickets` (backup copy in Task 1's backup dir).
- [ ] **Step 4:** Same command -> PASS; `npx tsc --noEmit` -> 0.

---

### Task 5: Ticket Contributors view

**Files:**
- Create: `src/features/ticket-contributors/ui/TicketContributorsView.tsx`, `TicketContributorsView.test.ts`.

**Interfaces:**
- Consumes: `useTicketBoard` (Task 4); `Segmented` from `src/features/settings/ui/primitives`; reuse markup and styling ideas from the backed-up `TicketsTab.tsx` and the period picker in `src/features/contributors/ui/ContributorsView.tsx` (~375-411).
- Props: `{ besideRail: boolean; compactRail: boolean; onClose(): void; onToggleSidebar(): void; onOpenSettings(): void }` (same shape as `MailView` plus `onOpenSettings`).
- Layout, top to bottom:
  1. Header: title "Ticket Contributors", workspace select (workspaces from the token), Period `Segmented` (Month / Quarter / Year / All) with previous/next arrows and label, a "Refresh from ClickUp" icon button, and a "Scope" disclosure with spaces (All spaces or a subset; "This token cannot list spaces. Tickets from every space are included." when the list is empty) and excluded statuses (from `statuses`).
  2. Not connected: "Connect ClickUp in Settings to score tickets." with a button calling `onOpenSettings`. No workspace picked: "Pick a ClickUp workspace." (Review Focus 1.)
  3. Score card: "{N} closed tickets in {period label}. {A} scored, {N - A} not scored yet." A percentage input (0-100, slider + number) labelled "Score coverage"; then "Scoring {K} more tickets with {model} ({runner}). About {T} input tokens (estimate)." and a "Score" button; when K = 0: "Coverage already reached. Nothing to score." with the button disabled. While a job runs: progress `done/total`, failed count, "Cancel". Fetch jobs show "Fetching tickets from ClickUp..." with the same progress.
  4. Leaderboard table: rank, member, points, tickets, coverage "{assessed}/{tickets}", level distribution XS..XL; an "Unassigned" row when present. Clicking a member expands their tickets: title (link), level badge ("edited" when overridden), points share, five criterion scores as buttons that expand the reasons, a level select with "AI level" to override, a note field, checkboxes and "Re-assess selected" (disabled while a job runs).
- Copy rules: plural/singular correct ("1 ticket"); numbers formatted with thousands separators.

- [ ] **Step 1: Tests:** not connected shows the Settings button; picking a workspace calls `setScope`; score card shows N, A, K and the estimate label; K = 0 disables Score with the coverage message (Review Focus 3); Score calls `score`; running job shows progress and Cancel; member row expands to tickets; override select calls `override`; re-assess selected disabled while a job runs; empty spaces message.
- [ ] **Step 2:** `npx vitest run src/features/ticket-contributors/ui` -> FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Same command -> PASS; `npx tsc --noEmit` -> 0.

---

### Task 6: App wiring and sidebar entry

**Files:**
- Modify: `src/app/App.tsx` (state `ticketContributorsOpen`, ref, `onOpenTicketContributors`, `onLeaveTicketContributors`, render), `src/app/shell/Sidebar.tsx`, `src/app/shell/ProjectRail.tsx`.
- Create: `src/app/ticketContributorsWiring.test.ts` modelled on `src/app/mailWiring.test.ts`.
- Modify as needed: `src/app/contributorsWiring.test.ts`, `src/app/mailWiring.test.ts`, `src/app/shell/SidebarRename.test.ts` only where they enumerate views.

**Interfaces:**
- Wire exactly like Mail (see `App.tsx` ~1298 state, ~12308 `onOpenMail`, ~14074 render, the ~15 `setMailViewOpen(false); setContributorsProject(null);` sites, `foregroundSurfaceRef` ~1565-1595 and ~2299-2340, `settingsReturnViewRef` ~12376-12457, `onRailBack` ~12471, `UsageFooter` hide condition ~14125). Every site that closes Mail also closes Ticket Contributors, and its opener closes all other views (Review Focus 5). Settings opened from the view returns to it on close.
- Sidebar entry: label "Ticket Contributors", an existing chart/people icon from `src/shared/ui/icons`, in the same three places Mail has an entry (icon row, compact rail, project rail), not gated by a setting.
- `onOpenSettings` from the view opens Settings at the ClickUp section if the settings router supports a section id; otherwise opens Settings.

- [ ] **Step 1: Tests** in `ticketContributorsWiring.test.ts` (source-regex style like `mailWiring.test.ts`): every `setMailViewOpen(false);` site also has `setTicketContributorsOpen(false);`; the opener closes Mail, Inbox, Search, Notes, Automations, Contributors, file picker, settings; the view renders with `onClose={onLeaveTicketContributors}`; the settings return snapshot includes it.
- [ ] **Step 2:** `npx vitest run src/app` -> FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** `npm run check > /tmp/monoptah-check.log 2>&1; echo $?` -> `0`; `./scripts/check-fork-branding.sh; echo $?` -> `0`.

---

## Out of Scope

- Editing the points table in the UI (stored with defaults; editable later).
- Combined code + ticket leaderboard.
- Jira and other sources.
- Batch API, assessment export/import.
