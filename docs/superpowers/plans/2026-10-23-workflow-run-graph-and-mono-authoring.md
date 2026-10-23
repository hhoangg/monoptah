# Workflow Run Graph and Mono-Assisted Authoring Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Show a workflow run as a live, read-only graph of its definition snapshot (For each fanned out into lanes or tabs, rounds and decisions in a details panel), and let the Mono build and change workflows from chat with an on-demand guide, dry-run validation and paused creation.

**Architecture:** A pure `runGraphModel.ts` turns a `WorkflowRun` into React Flow nodes/edges with run state; a read-only `RunGraph.tsx` renders it with the board's node components (extended with an optional run-state field) and replaces the lane list in `WorkflowRunPage.tsx`. The engine gains a data-only per-round history on `NodeRun`. The app CLI gains `workflows.validate` and `workflows.guide`; `workflows.add` always saves paused; a `workflowGuide.ts` module produces the guide from the same constants and validated examples.

**Tech Stack:** React 19, TypeScript, `@xyflow/react` 12 (already a dependency), Vitest + happy-dom, Rust (only `src-tauri/src/control_cli.rs`).

**Spec:** `docs/superpowers/specs/2026-10-23-workflow-run-graph-and-mono-authoring-design.md` (read it first; it builds on `docs/superpowers/specs/2026-10-22-mono-workflows-design.md`).

## Global Constraints

- Follow `CLAUDE.md`: fork branding, never-rename identifiers, verify with `npm run check > /tmp/wf-check.log 2>&1; echo $?` (read the status directly) and `./scripts/check-fork-branding.sh; echo $?`. Never run plain `npm run tauri dev`; the user may be running `npm run tauri:dev` from this worktree, so do not start or kill it.
- No new dependencies.
- English for code, comments and UI copy; no em dash or en dash characters anywhere.
- Look: reuse the board's node components and `workflowBoard.css`, `monoPanelParts`, `IconButton` from `src/app/shell/TitleBar`, icons from `src/shared/ui/icons.tsx` (new glyphs via deep import + `wrap`), Tailwind tokens; no new palette or fonts. Status colors are the app's existing ones (emerald for done, `var(--mono-color)` for running, amber for waiting, red for failed).
- Engine semantics do not change: the per-round history is data only.
- Workflows created through the CLI (`workflows.add`, JSON or YAML) are always saved paused (decided by the user).
- New localStorage keys use the `monocode.` prefix.
- `MONO_PROMPT_VERSION` stays 21.
- Commits on `feature/mono-workflows`, one per task, each ending with a blank line and `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Never push.

## Review Focus

1. Runs saved before this change (no `history`, no `ui` positions, older branch keys) must render on the run page without crashing (Task 2 test `renders a run saved before history existed`).
2. A definition whose nodes have no `ui` positions (Mono-created) must be auto-placed without overlapping (Task 2 test `places nodes without positions like the board`).
3. A For each that failed (non-list `in`) or expanded to zero items shows the group with "No items" or the failure and no lanes (Task 2 test `empty and failed For each have no lanes`).
4. A For each with many items (30) defaults to the single-lane view and its tabs stay usable in the narrow panel (Task 2 test `defaults to single view for many items`, Task 3 test `tabs scroll horizontally`).
5. `workflows.validate` never writes anything, and every worked example in the guide passes `workflowIssues` (Task 4 test `validate never writes`, Task 5 test `every guide example validates`).

---

## File Structure

Create:
- `src/features/workflows/ui/run/runGraphModel.ts` - pure run -> React Flow nodes/edges with run state, view choice, rounds and items per group.
- `src/features/workflows/ui/run/RunGraph.tsx` - read-only React Flow canvas, view switch, group tabs and round selector.
- `src/features/workflows/ui/run/RunNodeDetails.tsx` - details panel for the selected node.
- `src/features/workflows/model/workflowGuide.ts` - the Mono's authoring guide text and its worked examples.
- Tests next to each (`*.test.ts`, UI with `// @vitest-environment happy-dom`).

Modify:
- `src/features/workflows/model/workflow.ts` (`RoundRecord`, `NodeRun.history`), `workflowEngine.ts` (`resetLoop`, `decideLoop`), the run parser in `workflowStore.ts`/`workflowParse.ts` (keep `history`).
- `src/features/workflows/ui/board/boardModel.ts` (`BoardNodeData.run`), `boardNodes.tsx` (render run state), `workflowBoard.css`.
- `src/features/workflows/ui/WorkflowRunPage.tsx` (graph + details replace the lane list).
- `src/features/agent-app/model/agentAppWorkflows.ts`, `src-tauri/src/control_cli.rs` (`APP_ACTIONS`, `APP_USAGE`), `src/features/monos/model/monoFiles.ts` (`<workflows_rules>`), `docs/agent-access.md`.
- `src/features/monos/ui/MonoOverview.tsx` (Ask Mono action) and whatever carries a prefilled draft to the Mono composer.

---

### Task 1: Per-round history on NodeRun

**Files:**
- Modify: `src/features/workflows/model/workflow.ts` (NodeRun), `src/features/workflows/model/workflowEngine.ts` (`resetLoop` ~389, `decideLoop` ~437), the runs parser (find where `NodeRun` JSON is parsed: `workflowStore.ts` / `workflowParse.ts`)
- Test: `src/features/workflows/model/workflowEngine.test.ts`, the runs parser test

**Interfaces:**
- Produces:
```ts
export type RoundRecord = {
  round: number;
  status: NodeStatus;
  sessionId?: string;
  output?: Record<string, unknown>;
  error?: string;
  decision?: { value: string; reason?: string };
  startedAt?: number;
  endedAt?: number;
};
// NodeRun gains: history?: RoundRecord[]   (oldest first, never includes the current round)
```

- [ ] **Step 1: Write failing tests** in `workflowEngine.test.ts`:
  - `records the finished round before a loop resets a node`: dev/review loop (decide mono); dev done (sessionId "s-dev-1", output {pr_url:"u1"}), review done (sessionId "s-rev-1"), decided `loop` reason "needs tests" -> after the reset, `dev.history` is `[{round:1,status:"done",sessionId:"s-dev-1",output:{pr_url:"u1"},...}]` and `review.history[0].decision` is `{value:"loop", reason:"needs tests"}`; `review.round === 2`.
  - `exit decision stays on the node, not in history`: after round 2 decided `exit`, `review.decision.value === "exit"` and `review.history.length === 1`.
  - `history survives parse`: a serialized run with `history` round-trips through the runs parser unchanged; a run without `history` parses with `history` undefined.
- [ ] **Step 2:** Run `npx vitest run src/features/workflows/model` - expect FAIL.
- [ ] **Step 3: Implement.** In `resetLoop`, before a node's fields are reset, push a `RoundRecord` of its current round (status, sessionId, output, error, decision, startedAt, endedAt) to `history`; in `decideLoop` for value `loop`, set the source node's `decision` to `{value:"loop", reason}` before `resetLoop` runs so the record carries it. Do not change readiness, resets or any other behaviour. Parse `history` defensively (drop malformed records).
- [ ] **Step 4:** Run the same command - expect PASS.
- [ ] **Step 5: Commit** `"Workflows: keep each loop round's record on the node"`.

### Task 2: Run graph model

**Files:**
- Create: `src/features/workflows/ui/run/runGraphModel.ts`
- Modify: `src/features/workflows/ui/board/boardModel.ts` (add `run?: RunNodeState` to `BoardNodeData`)
- Test: `src/features/workflows/ui/run/runGraphModel.test.ts`

**Interfaces:**
- Consumes: `WorkflowRun`, `NodeRun`, `RoundRecord` (Task 1), `parseBranch`/branch helpers in `workflow.ts`, `isAsking` from `workflowEngine.ts`, `toFlow`, `autoPlace`, `NODE_W`, `NODE_H`, `GAP_Y`, `GROUP_PAD`, `GROUP_HEADER` from `boardModel.ts`.
- Produces:
```ts
export type RunNodeStatus = "done" | "running" | "waiting" | "failed" | "skipped" | "pending" | "stopped";
export type RunNodeState = {
  status: RunNodeStatus;
  nodeRunKey: string;            // `${nodeId}|${branch}`, used to select a node
  round: number;
  label: string;                 // second line, e.g. "Done · 12s", "Running 3m", "Done · true", "Failed: <error>"
};
export type RunEdgeState = "taken" | "active" | "idle";
export type RunView = "lanes" | "single";
export type GroupView = {
  groupId: NodeId;
  rounds: number[];              // rounds this For each expanded in, ascending; [] when it never expanded
  round: number;                 // selected round (default the latest)
  items: { index: number; title: string; status: RunNodeStatus }[];
  item: number;                  // selected item for the single view (default first running, else first)
  summary: string;               // "2 done · 1 running · 1 failed" or "No items" or the failure text
};
export function defaultRunView(run: WorkflowRun): RunView;   // "lanes" when every For each has <= 4 items and <= 3 child nodes, else "single"
export function groupViews(run: WorkflowRun, pick?: Record<NodeId, { round?: number; item?: number }>): GroupView[];
export function runGraph(run: WorkflowRun, view: RunView, groups: GroupView[]):
  { nodes: FlowNode[]; edges: (FlowEdge & { data: { loop?: LoopSpec; run: RunEdgeState } })[] };
export function nodeRunsByKey(run: WorkflowRun): Map<string, NodeRun>;
```
Item titles: `title` or `name` of the item from `run.items[groupId]` for the latest round and `run.items["<groupId>@r<n>"]` for archived round n, else `#<index+1>`. Status mapping: `pending`->"pending", `running`+`isAsking`->"waiting", `waitingApproval`->"waiting", others same name. Lanes view: one column per item inside the group, child nodes laid out by their snapshot positions scaled to the lane width, group width grows to fit. Single view: only the selected item's children at full group width. Node ids in the flow are unique per lane (`${nodeId}@${branch}`) and carry `data.run.nodeRunKey`.

- [ ] **Step 1: Write failing tests:**
  - `linear run maps statuses and labels`: trigger done, session done (endedAt-startedAt 12000), report pending -> states `done`, `done` with label starting "Done", `pending`; edges trigger->session `taken`, session->report `idle`.
  - `if marks only the taken side`: check decided `true` -> edge with handle "true" `taken`, "false" edge `idle`, false-side node `skipped`.
  - `running node and active edge`: a running session -> state `running`, its incoming edge `active`.
  - `asking a loop question shows waiting`: node `running` with `endedAt` set -> `waiting`.
  - `lanes view fans a For each into one lane per item`: 4 items, children dev/review -> 8 child flow nodes with distinct ids, each with the right item's state.
  - `single view shows only the selected item`: same run, pick item 2 -> 2 child flow nodes from branch `each#2`.
  - `rounds of a For each inside a loop`: archived `each#0@r1` and items `each@r1` -> `rounds` `[1,2]`; picking round 1 titles items from `items["each@r1"]`.
  - `defaults to single view for many items`: 30 items -> `"single"`; 3 items with 2 children -> `"lanes"`.
  - `empty and failed For each have no lanes`: zero items -> summary "No items", no child nodes; failed For each -> summary is its error, no child nodes.
  - `places nodes without positions like the board`: definition with no `ui` -> every node has a position and no two top-level nodes overlap.
  - `renders a run saved before history existed`: a run object without `history` fields and without `items` for an unexpanded group -> no throw.
- [ ] **Step 2:** Run `npx vitest run src/features/workflows/ui/run` - expect FAIL.
- [ ] **Step 3: Implement** `runGraphModel.ts` with the signatures above, reusing `toFlow` for the snapshot's base layout.
- [ ] **Step 4:** Run the same command - expect PASS.
- [ ] **Step 5: Commit** `"Workflows: model a run as a graph"`.

### Task 3: Run graph on the run page

**Files:**
- Create: `src/features/workflows/ui/run/RunGraph.tsx`, `src/features/workflows/ui/run/RunNodeDetails.tsx`
- Modify: `src/features/workflows/ui/board/boardNodes.tsx` (render `data.run` state: status dot or ring, second line = `run.label`, "round n" badge when n > 1; hide handles' interactivity when `data.run` is set), `src/features/workflows/ui/board/workflowBoard.css` (run state classes), `src/features/workflows/ui/WorkflowRunPage.tsx` (replace the lane list), `src/shared/ui/icons.tsx` if a view-switch glyph is missing
- Test: `src/features/workflows/ui/run/RunGraph.test.ts`, update `src/features/workflows/ui/WorkflowRunPage.test.ts`

**Interfaces:**
- Consumes: Task 2 exports; `nodeTypes` from `boardNodes.tsx`; existing `WorkflowRunPage` props (`onOpenSession`, stop/resume handlers) unchanged.
- Produces: `RunGraph({ run, onSelect, selectedKey }: { run: WorkflowRun; selectedKey?: string; onSelect(key: string): void })`, `RunNodeDetails({ run, nodeRunKey, onOpenSession })`.

Behaviour: read-only React Flow (`nodesDraggable={false}`, `nodesConnectable={false}`, `elementsSelectable` only for picking, no delete key), zoom IconButtons like the board, `fitView` on first render; header icon button toggles Lanes/Single (only when the run has a For each), choice stored in localStorage `monocode.workflowRunView.<workflowId>`, default `defaultRunView(run)`; group tabs (single view) and round selector (when `rounds.length > 1`) render inside or right above the group box using existing pill/tab styles; default selection is the running node, else the last finished node. Details panel: title, type, status, started, duration, round history (from `history`: round, status, decision and reason, error, session link), error text, output fields (values over 200 chars truncated with "Show more"), command exitCode/stdout/stderr tails, "Open session" using the page's existing `onOpenSession` with its existing error notice. Keep the page header, meta, Stop run / Resume and the "The workflow changed since this run started." note; the page keeps refreshing as today.

- [ ] **Step 1: Write failing tests** (happy-dom; stub ResizeObserver/DOMMatrix like `WorkflowBoard.test.ts` does):
  - `run page draws one card per run node`: linear run -> 3 `[data-board-node]` elements with `data-run-status` `done`,`done`,`pending`.
  - `switch toggles lanes and single and is remembered`: 4-item run -> clicking the view switch changes the number of child cards from 8 to 2; remount reads the stored choice.
  - `no switch without a For each`.
  - `tabs scroll horizontally`: 30-item run in single view -> the tab strip has `overflow-x: auto` and the selected tab is visible (`scrollIntoView` called).
  - `clicking a node shows its details and round history`: node with one `history` record -> details list "Round 1" with its decision reason and an "Open session" button that calls `onOpenSession("s-dev-1")`.
  - `keeps stop and the changed note`: running run still shows "Stop run"; changed definition still shows the note.
- [ ] **Step 2:** Run `npx vitest run src/features/workflows/ui` - expect FAIL.
- [ ] **Step 3: Implement** the components and the page change.
- [ ] **Step 4:** Run the same command - expect PASS; then `npx tsc --noEmit` - exit 0.
- [ ] **Step 5: Visual check.** If `/tmp/wf-harness` (throwaway vite harness mounting the panel with Tauri stubs) still works, screenshot the run page at 420x800 in dark and light for a 4-item running run in both views into `/tmp/wf-screens/run-graph-*.png` and compare with the board; otherwise say so in the report.
- [ ] **Step 6: Commit** `"Workflows: show a run as a live graph"`.

### Task 4: CLI creates paused, dry-run validation

**Files:**
- Modify: `src/features/agent-app/model/agentAppWorkflows.ts` (`workflows.add`, new `workflows.validate`, action table + fields), `src-tauri/src/control_cli.rs` (`APP_ACTIONS` + its count: add `workflows.validate` AND `workflows.guide`; `APP_USAGE` lines; the existing `app_mode_exposes_workflow_actions` test covers them), `docs/agent-access.md`
- Test: `src/features/agent-app/model/agentAppWorkflows.test.ts`, Rust test in `control_cli.rs`

**Interfaces:**
- Produces: `workflows.validate {"workflow": {...}}` or `{"yaml": "..."}` plus optional `"values"` -> `{ issues: WorkflowIssue[]; commands: {nodeId,command}[]; prompts: {nodeId,text}[] }`, never writing. `workflows.add` returns as today but the saved workflow always has `enabled: false`.

- [ ] **Step 1: Write failing tests:**
  - `add saves a clean JSON workflow paused`: valid definition with a schedule trigger -> stored `enabled === false`, no `nextRunAt`; response says it is paused.
  - `add ignores enabled true in the input` (or rejects the field: keep whichever the current DEFINITION_FIELDS allows; the stored value must be `false`).
  - `validate reports the same issues as add`: a definition with an unknown `{{var}}` -> same issue list as `add` returns.
  - `validate never writes`: spy on the store write (`writeAgentFile` / `updateWorkflows`) -> zero calls for valid and invalid input.
  - `validate lists commands and prompts`: a command node and a session node -> both listed verbatim.
  - Rust: `app_mode_exposes_workflow_actions` includes `workflows.validate` and `workflows.guide`.
- [ ] **Step 2:** Run `npx vitest run src/features/agent-app` and `cargo test app_mode_exposes_workflow_actions` (in `src-tauri`, log to /tmp) - expect FAIL.
- [ ] **Step 3: Implement.** `workflows.validate` reuses the exact parse + `problems()` path of `workflows.add` (extract a shared function rather than copying). `workflows.guide` is registered in Rust here (the handler arrives in Task 5; until then the TS side returns its usual unknown-action error). Update `APP_USAGE` and `docs/agent-access.md` for both actions and the paused rule. This is the only Rust change in the plan.
- [ ] **Step 4:** Run the same commands plus `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings` - expect PASS / exit 0.
- [ ] **Step 5: Commit** `"Workflows: CLI creates paused and validates without saving"`.

### Task 5: The Mono's authoring guide

**Files:**
- Create: `src/features/workflows/model/workflowGuide.ts`
- Modify: `src/features/agent-app/model/agentAppWorkflows.ts` (`workflows.guide` handler), `src/features/monos/model/monoFiles.ts` (`<workflows_rules>`), `docs/agent-access.md`
- Test: `src/features/workflows/model/workflowGuide.test.ts`, `src/features/monos/model/monoFiles.test.ts`

**Interfaces:**
- Consumes: constants from `workflow.ts` (`WORKFLOWS_MAX`, `WORKFLOW_RUNS_KEPT`, limits for maxParallel, maxRounds, timeouts), `workflowIssues`.
- Produces:
```ts
export const GUIDE_EXAMPLES: { name: string; why: string; workflow: Omit<Workflow, "id" | "createdAt" | "enabled"> }[];
export function workflowGuide(): string;   // Markdown
```

Content (spec 6.1 and 6.3, in this order): what a workflow is; the definition shape (top-level fields; every node type with fields, types, defaults, limits; edges; If handles; loop edges `loop: {maxRounds, decide}`; For each `parent`, `in`, `as`, `maxParallel`; inputs and projects-as-inputs); template variables and where each is readable (prompts, commands, conditions, report templates with their narrower scope: top-level node outputs, `inputs.*`, `run.*`, `results`); `{{#if}}` rules; the condition grammar with examples; session output contract and `inReport`; command safety rules (bare variables only, the unquotable syntax list, the dash rule, no absolute paths, Windows unsupported); models from `app models.list`; limits (interpolated from the constants); the conversational flow (clarify only what cannot be inferred, propose the steps in plain words, `workflows.validate` until clean, `workflows.add` saves paused, tell the user to review and enable on the board; changes through `workflows.update`); the worked examples as JSON (at least: linear session -> report; ticket fan-out with dev -> review loop exiting on a `{{review.verdict}} == "approve"` condition; an If on a session output). Model ids in examples must be catalog ids (`claude:sonnet-5`, `claude:opus-5`).

- [ ] **Step 1: Write failing tests:**
  - `every guide example validates`: for each `GUIDE_EXAMPLES` entry, `workflowIssues({...workflow, id:"wf-aaaaaaaa", createdAt:0, enabled:false, inputValues:{project:"/x"}})` is `[]` (adjust inputValues to each example's inputs).
  - `guide embeds every example and the limits`: `workflowGuide()` contains each example's JSON and the numbers `WORKFLOWS_MAX`, maxParallel max 10, maxRounds max 10, timeout max 1440.
  - `guide mentions every node type`: contains each of `schedule`, `manual`, `session`, `command`, `askMono`, `if`, `forEach`, `report`.
  - `guide has no dashes`: no U+2014 or U+2013.
  - CLI: `workflows.guide returns the guide` equals `workflowGuide()`.
  - `monoContext tells the Mono to read the guide`: `<workflows_rules>` contains "app workflows.guide" and "workflows.validate".
- [ ] **Step 2:** Run `npx vitest run src/features/workflows/model src/features/agent-app src/features/monos/model` - expect FAIL.
- [ ] **Step 3: Implement** `workflowGuide.ts`, the CLI handler, one added sentence in `<workflows_rules>` ("Before adding or changing a workflow, read `app workflows.guide`, check it with `workflows.validate`, and save with `workflows.add`; new workflows are saved paused for the user to review and enable.") and `docs/agent-access.md`.
- [ ] **Step 4:** Run the same command - expect PASS.
- [ ] **Step 5: Commit** `"Workflows: give the Mono an authoring guide"`.

### Task 6: Ask Mono to build a workflow

**Files:**
- Modify: `src/features/monos/ui/MonoOverview.tsx` (Workflows tab header action), the code path that puts text into the Mono composer (find how the composer receives text today: `MonoComposer.tsx` `quoteRequest` / `consumeQuoteRequest`, `setComposerDraft` in `src/features/sessions/model/draftCache.ts`; reuse one, do not add a parallel mechanism)
- Test: `src/features/monos/ui/MonoOverview.test.ts` (and the composer test if the mechanism needs a change)

**Interfaces:**
- Produces: a header IconButton on the Workflows tab, label "Ask Mono to build a workflow" (shared icon), that puts `Help me build a workflow that ` into the Mono chat composer, focuses it, and does not send.

- [ ] **Step 1: Write failing tests:** `ask mono button prefills the composer`: on the Workflows tab, clicking the action results in the Mono composer draft being exactly `Help me build a workflow that ` and no message sent; the action is absent on other tabs.
- [ ] **Step 2:** Run `npx vitest run src/features/monos/ui` - expect FAIL.
- [ ] **Step 3: Implement** using the existing composer mechanism.
- [ ] **Step 4:** Run the same command - expect PASS.
- [ ] **Step 5: Final verification:** `npm run check > /tmp/wf-check.log 2>&1; echo $?` -> `0`; `./scripts/check-fork-branding.sh; echo $?` -> `0`.
- [ ] **Step 6: Commit** `"Workflows: ask the Mono to build a workflow"`.
