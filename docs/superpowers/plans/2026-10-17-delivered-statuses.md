# Delivered Statuses, Subtasks and Active Work Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Count every ticket a member delivered: subtasks included, custom "delivered" statuses such as "ready for casium testing" included, and show active (in-progress) work as an unscored count.

**Architecture:** Rust fetches every ticket updated since the window start (any status) into a new `contrib_ticket_rows` table with all candidate dates, and loads by last-update date. TypeScript decides per workspace which statuses are delivered, computes each ticket's delivered date, applies the parent/subtask rule, and splits delivered from active.

**Spec:** `docs/adr/0014-delivered-statuses-subtasks-and-active-work.md` (amends ADR 7), plus ADRs 12, 13 and `CONTEXT.md`.

## Global Constraints

- Branch `contributors-tickets`. No commits, no staging. English code/copy, no `—`/`–`. Never run prettier on files under `src/app`. Prefix git-to-file commands with `rtk proxy`.
- Delivered date order: (1) done date when status type is `done` or `closed`; (2) `statusSince` (entered current status, from ClickUp status history) when present; (3) `updatedAt`.
- Default delivered statuses (when `deliveredStatuses` is `null`): every status whose type is `done` or `closed`. Status names compare case-insensitively and trimmed. Excluded statuses win over delivered statuses.
- Active ticket: status type `custom`, not delivered, not excluded; anchored by `updatedAt`. Never assessed, never scored.
- A ticket with `hasSubtasks === true` never counts as delivered work (its subtasks do).
- Fetch window: `date_updated_gt = from_ms - 1`, no upper bound, `include_closed=true`, `subtasks=true`.
- Assessments (`contrib_ticket_assessments`) are untouched; the old `contrib_tickets` table is left in place, unused.

## Review Focus

1. A ticket delivered in the Period but edited after the Period ends must still count in the Period when it has a done date or `statusSince`. Test in Task 2.
2. Status history unavailable (empty `current_status`): fetch still succeeds, `statusSince` is null, and the view says dates are approximate. Tests in Tasks 1 and 3.
3. A parent with a subtask in the store: parent excluded, subtask counted; the AI input format is unchanged. Test in Task 2.
4. Changing delivered statuses re-classifies without a refetch. Test in Task 3.
5. Previously stored assessments and overrides still attach to the same ticket ids. Test in Task 1.

---

### Task 1: Rust fetch and store for all statuses

**Files:** `src-tauri/src/contributors/ticket_store.rs`, `clickup_tickets.rs`, `ticket_jobs.rs`, `src-tauri/src/contributors.rs` (load command signature).

**Interfaces (produces):**
```rust
// ticket_store.rs: new table, appended to SCHEMA
// contrib_ticket_rows(source TEXT, ticket_id TEXT, workspace_id TEXT, updated_epoch INTEGER NOT NULL,
//   parent_id TEXT, ticket_json TEXT, PRIMARY KEY(source, ticket_id)); index (workspace_id, updated_epoch); index (parent_id)
pub(crate) struct StoredTicket { source, id, custom_id: Option<String>, title, url, workspace_id, space_id, status, status_type,
  updated_at: String, updated_epoch: i64, done_at: Option<String>, status_since: Option<String>,
  parent_id: Option<String>, assignees: Vec<TicketAssignee>, tags: Vec<String>, description: String }
pub(crate) struct TicketRecord { source, id, custom_id (omitted when None), title, url, workspace_id, space_id, status, status_type,
  updated_at, done_at: Option<String>, status_since: Option<String>, parent_id: Option<String>, has_subtasks: bool,
  assignees, tags, assessment: Option<StoredAssessment>, override_level: Option<String>, override_note: Option<String> } // camelCase
pub(super) fn load_records(conn: &Connection, workspace_ids: &[String], since: i64) -> Result<Vec<TicketRecord>, String>; // updated_epoch >= since, subtasks included, has_subtasks via EXISTS on parent_id
```
`put_tickets`, `unreturned_ids` (now by `updated_epoch >= from`), `delete_tickets`, `tickets_for_assessment` move to the new table with unchanged behaviour. `clickup_tickets.rs`: `tasks_path(link, from_ms, page)` with the Global Constraints query; `parse_task(node, workspace_id) -> Option<StoredTicket>` accepts every status (needs `id` and `date_updated`); `done_at` from `date_done` else `date_closed` only for done/closed types; `fetch_status_since(http, ids) -> Result<HashMap<String, String>, String>` via `GET /task/bulk_time_in_status/task_ids` (100 ids per call), reading `current_status.total_time.since` (epoch ms) as ISO; missing or null means absent. In the fetch job, call it for tickets whose status type is not done/closed; on a non-rate-limit error or an all-empty first response, stop calling it for the rest of the run (rate limits: wait and retry as elsewhere). Reopen cleanup keeps its GET re-check, now deleting only on 404 (all statuses are kept). The load command drops `until` (TS sends only `since`).

- [ ] **Step 1: Tests:** `parse_task_keeps_custom_and_open_statuses`; `parse_task_sets_done_at_only_for_done_or_closed`; `tasks_path_filters_by_date_updated_without_upper_bound`; `fetch_status_since_reads_current_status_since`; `fetch_status_since_returns_empty_when_history_is_off` (Review Focus 2); `load_records_includes_subtasks_and_flags_parents`; `load_records_filters_by_updated_epoch`; `assessments_and_overrides_attach_to_rows_in_the_new_table` (Review Focus 5); update the job tests for the new path and table.
- [ ] **Step 2:** `cd src-tauri && cargo test contributors` -> FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** `cargo test contributors`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` -> 0.

### Task 2: TS model: delivered, active, subtasks

**Files:** `src/features/ticket-contributors/model/tickets.ts`, `board.ts`, `prefs.ts`, `ticketsApi.ts` (loadTickets drops `until`) and their tests.

**Interfaces (produces):**
```ts
export type TicketRecord = { source: "clickup"; id: string; customId?: string; title: string; url: string; workspaceId: string; spaceId: string;
  status: string; statusType: string; updatedAt: string; doneAt: string | null; statusSince: string | null; parentId: string | null;
  hasSubtasks: boolean; assignees: TicketAssignee[]; tags: string[]; assessment: TicketAssessment | null; overrideLevel: Level | null; overrideNote: string | null };
export type WorkspaceScope = { workspaceId: string; name: string; spaceIds: string[] | null; excludedStatuses: string[]; deliveredStatuses: string[] | null };
export function isDelivered(t: TicketRecord, scope: WorkspaceScope): boolean;   // status rule + not excluded + !hasSubtasks + inScope
export function deliveredAt(t: TicketRecord): { at: string; approximate: boolean }; // Global Constraints order
export function isActive(t: TicketRecord, scope: WorkspaceScope): boolean;
// board.ts
export function periodTickets(tickets, scope, period): TicketRecord[]; // delivered and deliveredAt(t).at in period
export function activeByMember(tickets, scope, period): Map<string, number>; // active and updatedAt in period
// MemberRow gains active: number; Board gains datesApproximate: boolean (any counted ticket used updatedAt), and members appear when they only have active work
export function statusesOf(tickets: TicketRecord[]): { name: string; type: string }[]; // distinct, sorted by type then name
```
`prefs.ts` reads old scopes without `deliveredStatuses` as `null`.

- [ ] **Step 1: Tests:** delivered date order incl. a ticket done in the Period and updated after it (Review Focus 1); custom delivered status counts with `statusSince`, else `updatedAt` flagged approximate; excluded wins over delivered; parent with subtasks excluded, subtask counted (Review Focus 3); active counted, never in `periodTickets`; default delivered set is done/closed types; prefs migration.
- [ ] **Step 2-4:** `npx vitest run src/features/ticket-contributors/model` FAIL -> implement -> PASS; `npx tsc --noEmit` 0 (fix callers minimally; Task 3 owns UI changes).

### Task 3: Hook and view

**Files:** `src/features/ticket-contributors/hooks/useTicketBoard.ts`, `ui/TicketContributorsView.tsx` and tests.

- Hook: load with `since` only; expose `statuses` from `statusesOf` over all loaded tickets of the workspace; `setScope` persists `deliveredStatuses`; classification changes apply instantly (no refetch) (Review Focus 4).
- View, in the Scope disclosure: "Delivered statuses" checklist of `statuses` (name plus a small type label); unchecked state of the default shows done/closed checked; help text "Tickets in these statuses count as delivered work." Leaderboard: an "Active" column (count) with header tooltip "In a working status during this period. Not scored." Notice when `board.datesApproximate`: "Some delivery dates are approximate (last update). Turn on the Total time in Status ClickApp in ClickUp for exact dates."
- [ ] **Step 1: Tests:** delivered checklist toggles call `setScope` with the right list; Active column renders counts; approximate notice shows only when flagged (Review Focus 2); reclassification without refetch (Review Focus 4).
- [ ] **Step 2-4:** `npx vitest run src/features/ticket-contributors` FAIL -> implement -> PASS; then `npm run check > /tmp/monoptah-check.log 2>&1; echo $?` -> 0 and `./scripts/check-fork-branding.sh; echo $?` -> 0.
