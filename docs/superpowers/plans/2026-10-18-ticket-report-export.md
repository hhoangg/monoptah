# Ticket Report Export Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Export the Ticket Contributors view as a self-contained HTML report for managers: summary tiles, four charts, member table, hardest tickets, method notes.

**Architecture:** A pure module builds report data from loaded tickets and renders one HTML string (inline CSS, inline SVG, no scripts, no external resources). The view gets an "Export report" button that asks for a save path with the Tauri dialog plugin, writes the file with the existing `write_text_file` command, and reveals it.

**Spec:** the approved mockup `.superpowers/sdd/2026-10-18-ticket-report-export/mockup.html` (copy of the version the user approved), ADRs 12-14, `CONTEXT.md`.

## Global Constraints

- Branch `contributors-tickets`. No commits, no staging. Report text in English. No `—`/`–`. Never run prettier on files under `src/app`. Prefix git-to-file commands with `rtk proxy`.
- The report is a single HTML file: inline `<style>`, inline `<svg>`, **no `<script>`**, no external URLs except ticket links (`https://` only). Hover text uses SVG `<title>` elements.
- Every string from ClickUp or the AI (names, titles, statuses, reasons, notes, workspace name) is HTML-escaped (`& < > " '`). Ticket links are emitted only when the URL starts with `https://`.
- Palette and layout follow the mockup exactly: series `#2a78d6` (delivered/points), `#eb6834` (active); level ramp XS `#86b6ef`, S `#5598e7`, M `#2a78d6`, L `#1c5cab`, XL `#104281`; surfaces/ink as in the mockup CSS. 4px rounded data ends, 2px gaps between stacked segments, recessive grid.
- Numbers: points are the board's (shares already split); tiles must equal the table's sums. "Faded" members: coverage below 50%.
- Trend buckets: month -> ISO weeks (Monday start) labelled by Monday date ("6 Oct"); quarter and year -> months ("Oct"); all -> months when the data spans at most 24 months, else years.
- Comparison tile: delivered tickets vs the previous Period ("+18% vs September"); omitted for `all` or when the previous Period has 0 delivered tickets.
- Hardest tickets: top 10 delivered scored tickets by points desc, then delivered date desc; level from `effectiveLevel`; "edited" marker for overrides; main reason = reason of the highest-scoring criterion (ties broken by criteria order), or the override note when the level was set by hand ("Set by hand" when no note).
- Notes at the top: coverage note when coverage < 100%; approximate-dates note when `board.datesApproximate`.
- Default file name: `ticket-contributors-<workspace-slug>-<period-id>.html` (slug: lowercase, non-alphanumerics to `-`).

---

### Task 1: Report data and HTML renderer

**Files:** create `src/features/ticket-contributors/model/report.ts`, `report.test.ts`.

**Interfaces (produces):**
```ts
export type ReportInput = { workspaceName: string; period: Period; previousPeriod: Period | null; scope: WorkspaceScope;
  points: Record<Level, number>; tickets: TicketRecord[]; previousTickets: TicketRecord[]; ai: AiConfigStatus | null; generatedAt: Date };
export type ReportData = { /* everything the renderer needs: header, notes, tiles, members (from buildBoard), trend buckets, load (delivered/active per member), hardest, statuses */ };
export function buildReport(input: ReportInput): ReportData;
export function renderReportHtml(data: ReportData): string;
export function reportFileName(workspaceName: string, period: Period): string;
```
Use `periodTickets`, `activeTickets`, `buildBoard`, `deliveredAt`, `effectiveLevel`, `statusesOf` from the existing model; do not duplicate their rules.

- [ ] **Step 1: Tests:** a title containing `<script>alert(1)</script>` and a quote is escaped everywhere and the output contains no `<script`; no `http://` and no external host other than `https://` ticket links; tiles equal table sums; coverage note only below 100%; approximate note only when flagged; comparison tile percentage and its omission for `all`/zero previous; trend buckets for month (weeks) and year (months); hardest ordering, edited marker, reason selection; faded class for coverage < 50%; file name slug.
- [ ] **Step 2-4:** `npx vitest run src/features/ticket-contributors/model/report.test.ts` FAIL -> implement -> PASS; `npx tsc --noEmit` 0.

### Task 2: Export button

**Files:** `src/features/ticket-contributors/hooks/useTicketBoard.ts`, `ui/TicketContributorsView.tsx`, their tests.

- Hook: `exportReport(): Promise<string | null>` loads the previous Period's tickets (`loadTickets([workspaceId], since = previous period start)`, skipped for `all`), builds the report, asks `save` from `@tauri-apps/plugin-dialog` (default name from `reportFileName`, filter HTML), writes with `writeTextFile` from `src/platform/tauri/fs.ts`, then reveals the file (use the existing reveal helper in `src/platform/tauri/fs.ts` or the opener plugin, whichever the codebase already uses). Returns the saved path, or null when cancelled. Errors surface in the view.
- View: "Export report" button in the header next to Refresh; disabled when no workspace, while loading, or when there are no delivered tickets; shows "Report saved." after success.
- [ ] **Step 1: Tests:** button disabled states; calls the hook; success message; cancel path shows nothing; error shown.
- [ ] **Step 2-4:** `npx vitest run src/features/ticket-contributors` FAIL -> implement -> PASS; `npm run check > /tmp/monoptah-check.log 2>&1; echo $?` -> 0; `./scripts/check-fork-branding.sh; echo $?` -> 0.
