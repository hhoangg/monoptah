# Mono Workflows Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add Workflows to Monos: a graph of nodes (trigger, session, command, Ask Mono, If, For each, Report) edited on a vertical React Flow board in the Mono right panel, run by the app on a schedule, reporting into the Mono chat, manageable from the Mono's app CLI, exportable/importable as YAML.

**Architecture:** Pure TypeScript model modules under `src/features/workflows/model/` (types, parse/validate, templates, output contract, conditions, a pure run state machine, report, YAML). A runner hook `src/app/hooks/useMonoWorkflows.ts` mirrors `useMonoHabits.ts`: it ticks, claims due workflows, drives the state machine and talks to the app through a `MonoWorkflowHost` wired in `App.tsx`. Definitions and runs are Mono files stored through the existing `mono_read`/`mono_save` path. UI lives under `src/features/workflows/ui/` and plugs into `MonoOverview`.

**Tech Stack:** React 19, TypeScript, Tauri 2 (Rust), Vitest, `@xyflow/react` 12 (new), `yaml` (eemeli/yaml, new).

**Spec:** `docs/superpowers/specs/2026-10-22-mono-workflows-design.md` (read it first; section numbers below refer to it). A layout-only HTML prototype of the board is at `/tmp/monoptah-workflow/layout-prototype.html` (layout only, NOT style).

## Global Constraints

- Follow `CLAUDE.md` (fork branding, never-rename identifiers, `npm run tauri:dev` only, never plain `npm run tauri dev`).
- Verify with `npm run check > /tmp/wf-check.log 2>&1; echo $?` and read the exit status directly; never pipe it.
- All code, identifiers, comments, UI copy in English. No em dash or en dash characters in new copy.
- UI must use the app's own design: `monoPanelParts` (`PageHeader`, `Section`, `Property`, `SwitchRow`, `Hint`, `AutoTextarea`, `Empty`, `EmptyState`), `IconButton` from `src/app/shell/TitleBar`, icons from `src/shared/ui/icons.tsx` (add new glyphs there via deep import + `wrap`, `icons.test.ts` enforces it), Tailwind tokens `text-content/40..85`, `bg-content/5`, `border-stroke`, `bg-accent`, `var(--mono-color)`, 11-13 px text. React Flow's default look must be fully overridden. No new fonts or palette.
- New deps allowed: `@xyflow/react@^12`, `yaml@^2`. Nothing else.
- Share schedule code with habits (`HabitSchedule`, `nextHabitRunAt`, `habitScheduleLabel`, `habitSchedule` validator in `src/features/monos/model/monoHabits.ts`); do not copy it.
- Do not regress habits; existing habit tests must keep passing.
- Limits: runs kept per workflow `20`; workflows per Mono `20`; For each `maxParallel` default `3` (1-10); loop `maxRounds` default `3` (1-10); session node timeout default `2h`; command node timeout default `10 min`; command stdout/stderr tails capped at `8000` chars each; approval wait = `HABIT_APPROVAL_WAIT_MS`.
- Commit on branch `feature/mono-workflows` after each task. Never push, never touch `main`.

## Review Focus

1. A session finishes without a valid ` ```workflow-output ` block, or with JSON missing a declared field: exactly one re-ask in the same session, then the node fails with a readable error, the branch fails, other branches continue. (Task 4, Task 7.)
2. For each over an empty list or a non-list value: empty list -> group completes immediately with zero branches and the report says "no items"; non-list -> the For each node fails with "Expected a list for <expr>". (Task 6.)
3. App restarts mid-run: a run left `running` on disk becomes `interrupted` on load; Resume re-runs unfinished nodes from the stored definition snapshot and never re-runs finished ones. (Task 6, Task 7.)
4. Imported YAML from another person: unknown `monoptahWorkflow` version, absolute paths, or missing inputs are rejected or asked for; every command and prompt is shown before saving; it is saved paused. (Task 9, Task 13.)
5. Template value containing shell metacharacters (`; rm -rf ~`, quotes, `$()`) interpolated into a command node: it must reach the shell as one quoted literal argument. (Task 3.)

---

## File Structure

Create:
- `src/features/workflows/model/workflow.ts` - types, constants, `newWorkflow`, ids.
- `src/features/workflows/model/workflowParse.ts` - `parseWorkflows`, `validateWorkflow`.
- `src/features/workflows/model/workflowStore.ts` - load/update definitions and runs (Mono files), running registry, stop requests, check-now event.
- `src/features/workflows/model/workflowTemplate.ts` - `{{...}}` rendering, var listing, shell quoting.
- `src/features/workflows/model/workflowOutput.ts` - output-contract instruction and parser.
- `src/features/workflows/model/workflowCondition.ts` - If / loop condition evaluation.
- `src/features/workflows/model/workflowEngine.ts` - pure run state machine.
- `src/features/workflows/model/workflowReport.ts` - final report Markdown.
- `src/features/workflows/model/workflowYaml.ts` - export/import.
- `src/app/hooks/useMonoWorkflows.ts` - runner hook + `MonoWorkflowHost`.
- `src-tauri/src/workflow_command.rs` - `workflow_run_command` Tauri command.
- `src/features/agent-app/model/agentAppWorkflows.ts` - `workflows.*` CLI handler.
- `src/features/workflows/ui/MonoWorkflows.tsx` - `useWorkflows`, `WorkflowRow`, `WorkflowsList`.
- `src/features/workflows/ui/WorkflowPage.tsx`, `WorkflowRunPage.tsx`, `ImportWorkflowPage.tsx`.
- `src/features/workflows/ui/board/WorkflowBoard.tsx`, `boardNodes.tsx`, `LoopEdge.tsx`, `NodePicker.tsx`, `NodeInspector.tsx`, `boardModel.ts`, `workflowBoard.css`.
- Tests next to each file as `*.test.ts` (UI tests with `// @vitest-environment happy-dom`).

Modify:
- `src-tauri/src/mono.rs` (`resolve_file`), `src-tauri/src/lib.rs` (register command), `src-tauri/src/control_cli.rs` (`APP_ACTIONS`, `APP_USAGE`).
- `src/features/monos/model/monoFiles.ts` (`AgentFilePath`, `monoContext`, `MONO_PROMPT_VERSION`).
- `src/features/sessions/model/session.ts` (`Block.monoWorkflow`).
- `src/features/agent-app/model/agentApp.ts` (`AgentAppHost.workflows`, `FIELDS`, prefix route).
- `src/features/monos/ui/MonoOverview.tsx` (tab, routes, header action).
- `src/app/App.tsx` (runner host wiring, CLI host wiring, `post` for workflow reports).
- `package.json` / `package-lock.json` (deps).

---

### Task 1: Types, parsing, validation

**Files:**
- Create: `src/features/workflows/model/workflow.ts`, `src/features/workflows/model/workflowParse.ts`
- Test: `src/features/workflows/model/workflowParse.test.ts`

**Interfaces:**
- Produces (in `workflow.ts`):

```ts
export type WorkflowId = string;            // "wf-" + 8 lowercase base36 chars
export type NodeId = string;                // /^[a-z][a-z0-9_]{0,39}$/
export type OutputFieldType = "string" | "number" | "boolean" | "list" | "object";
export type OutputField = { type: OutputFieldType; inReport?: boolean; description?: string };
export type Checkout = "root" | "newWorktree" | { worktreeOf: NodeId };
export type WorkflowInput = { type: "project" | "string" | "number" | "boolean"; description?: string };
type Base = { id: NodeId; title?: string; parent?: NodeId; ui?: { x: number; y: number } };
export type ScheduleNode = Base & { type: "schedule"; schedule: HabitSchedule };
export type ManualNode   = Base & { type: "manual" };
export type SessionNode  = Base & { type: "session"; project: string; prompt: string;
  harness?: string; model?: string; effort?: string; checkout: Checkout;
  reuseSession?: boolean /* default true */; nestUnder?: NodeId;
  output: Record<string, OutputField>; timeoutMinutes?: number /* default 120 */ };
export type CommandNode  = Base & { type: "command"; project: string; command: string;
  cwd?: "root" | { worktreeOf: NodeId }; timeoutMinutes?: number /* default 10 */ };
export type AskMonoNode  = Base & { type: "askMono"; question: string; answers: string[] };
export type IfNode       = Base & { type: "if"; condition?: string; decide?: "mono"; question?: string };
export type ForEachNode  = Base & { type: "forEach"; in: string; as?: string /* "item" */; maxParallel?: number /* 3 */ };
export type ReportNode   = Base & { type: "report"; template?: string };
export type WorkflowNode = ScheduleNode | ManualNode | SessionNode | CommandNode | AskMonoNode | IfNode | ForEachNode | ReportNode;
export type LoopSpec = { maxRounds: number; decide: "mono" | string /* condition */ };
export type WorkflowEdge = { from: NodeId; to: NodeId; handle?: "true" | "false"; loop?: LoopSpec };
export type Workflow = { id: WorkflowId; name: string; description?: string;
  inputs: Record<string, WorkflowInput>; inputValues: Record<string, string | number | boolean>;
  nodes: WorkflowNode[]; edges: WorkflowEdge[]; enabled: boolean; createdAt: number;
  nextRunAt?: number; runRequested?: boolean; lastRunId?: string; importedFrom?: string };
export const WORKFLOWS_MAX = 20, WORKFLOW_RUNS_KEPT = 20;
export function newWorkflowId(): WorkflowId;
export function triggerOf(w: Workflow): ScheduleNode | ManualNode | undefined;
```

- Produces (in `workflowParse.ts`):

```ts
export type WorkflowIssue = { nodeId?: NodeId; message: string };
export function parseWorkflows(text: string): Workflow[];          // drops malformed entries, never throws
export function validateWorkflow(w: Workflow): WorkflowIssue[];    // [] = valid
```

`validateWorkflow` rules (spec 5.3, 6, 12.2): exactly one trigger; exactly one `report` node reachable from the trigger; node ids unique and matching the NodeId regex; every edge endpoint exists; `parent` refers to a `forEach`; no cycles when loop edges are ignored; a loop edge's `to` is an ancestor of `from` inside the same parent; `maxRounds` 1-10; `maxParallel` 1-10; `if` has `condition` xor `decide:"mono"`, and its out edges use `handle` `"true"`/`"false"`; every `{{var}}` in prompts/commands/questions/conditions/`in` resolves to an input, an upstream node's declared output, `item` (inside For each, or the `as` name), or `run.*` (uses `templateVars` from Task 3; until Task 3 lands, write the test now and implement after Task 3, or order Task 3 first); every `project` field is exactly `{{inputs.<name>}}` naming a `project` input; no absolute paths anywhere in node fields.

- [ ] **Step 1: Write failing tests** in `workflowParse.test.ts`:
  - `parses a valid ClickUp workflow` - the spec 9.1 example (as an object) round-trips through `JSON.stringify` + `parseWorkflows` unchanged.
  - `drops entries without id or nodes` - `parseWorkflows('[{"name":"x"}]')` -> `[]`; invalid JSON -> `[]`.
  - `flags two triggers`, `flags missing report`, `flags a plain cycle without loop`, `flags loop edge to a non-ancestor`, `flags maxRounds 11`, `flags unknown {{dev.nope}}`, `flags absolute project path "/Users/x/repo"`, `flags if without true/false handles` - each asserts `validateWorkflow(w).some(i => i.message.includes(<key phrase>))`.
  - `accepts loop review -> dev inside forEach` - `validateWorkflow(example)` -> `[]`.
- [ ] **Step 2: Run** `npx vitest run src/features/workflows/model/workflowParse.test.ts` - expect FAIL (module missing).
- [ ] **Step 3: Implement** both files with the signatures above.
- [ ] **Step 4: Run** the same command - expect PASS.
- [ ] **Step 5: Commit** `git commit -m "Workflows: definition types, parsing and validation"`.

### Task 2: Storage (Rust whitelist + TS store)

**Files:**
- Modify: `src-tauri/src/mono.rs` (`resolve_file` ~88, header doc 1-18), `src/features/monos/model/monoFiles.ts:32` (`AgentFilePath`)
- Create: `src/features/workflows/model/workflowStore.ts`
- Test: `src-tauri/src/mono.rs` `mod tests`, `src/features/workflows/model/workflowStore.test.ts`

**Interfaces:**
- Files: `<mono dir>/workflows.json` (JSON array of `Workflow`) and `<mono dir>/workflow-runs/<workflowId>.json` (JSON array of `WorkflowRun`, newest first, max 20). `resolve_file` accepts `workflows.json` and `workflow-runs/<id>.json` where id matches `^wf-[a-z0-9]{8}$`.
- `AgentFilePath` gains `"workflows.json" | \`workflow-runs/${string}.json\``.
- Produces (in `workflowStore.ts`), mirroring `loadHabits`/`updateHabits` in `monoHabits.ts:557/588` (same conflict retry, same `writeAgentFile` event):

```ts
export function loadWorkflows(monoId: string): Promise<Workflow[]>;
export function updateWorkflows<T>(monoId: string, edit: (list: Workflow[]) => { workflows: Workflow[]; result: T }): Promise<T>;
export function loadRuns(monoId: string, workflowId: WorkflowId): Promise<WorkflowRun[]>;
export function updateRuns<T>(monoId: string, workflowId: WorkflowId, edit: (runs: WorkflowRun[]) => { runs: WorkflowRun[]; result: T }): Promise<T>;   // trims to 20
export function setWorkflowRunning(workflowId: WorkflowId, runId: string | null): void;
export function workflowRunningId(workflowId: WorkflowId): string | undefined;
export function subscribeWorkflowsRunning(listener: () => void): () => void;   // event "monocode:mono-workflows-active"
export function requestWorkflowStop(workflowId: WorkflowId): void;
export function workflowStopRequested(workflowId: WorkflowId): boolean;
export function clearWorkflowStop(workflowId: WorkflowId): void;
export function checkWorkflowsNow(): void;
export function subscribeCheckWorkflowsNow(listener: () => void): () => void;
```

(`WorkflowRun` type is defined in Task 6; declare it in `workflow.ts` now as the shape in Task 6's Interfaces so this task compiles.)

- [ ] **Step 1: Rust test** `resolves_workflow_files`: `resolve_file(dir,"workflows.json")` Ok; `"workflow-runs/wf-abc12345.json"` Ok and equals `dir/workflow-runs/wf-abc12345.json`; `"workflow-runs/../x.json"`, `"workflow-runs/wf-ABC.json"`, `"workflow-runs/a/b.json"` Err. `save` creates the `workflow-runs` dir when missing.
- [ ] **Step 2: Run** `cargo test -p monocode mono::tests::resolves_workflow_files` (crate name per `src-tauri/Cargo.toml`) - FAIL.
- [ ] **Step 3: Implement** in `mono.rs`; make `save` create parent dirs.
- [ ] **Step 4: TS tests** (mock `invoke` as `monoHabits.test.ts` does): `updateWorkflows retries once on conflict`, `updateRuns keeps newest 20`, `loadRuns returns [] for a missing file`.
- [ ] **Step 5: Implement** `workflowStore.ts`; run both test suites - PASS.
- [ ] **Step 6: Commit** `"Workflows: store definitions and runs as Mono files"`.

### Task 3: Templates and shell quoting

**Files:** Create `src/features/workflows/model/workflowTemplate.ts`; Test `workflowTemplate.test.ts`

**Interfaces:**

```ts
export type TemplateScope = Record<string, unknown>;   // { inputs, run, item?, [nodeId]: output }
export function templateVars(text: string): string[];  // ["dev.pr_url", "item.title", "inputs.project", ...], includes vars inside {{#if x}}
export function renderTemplate(text: string, scope: TemplateScope, opts?: { shell?: boolean }): string;
export function shellQuote(value: string): string;     // POSIX single-quote quoting
```

Rules: `{{a.b.c}}` path lookup; missing -> empty string; string as-is; number/boolean via `String`; list/object as `JSON.stringify(v, null, 2)`; `{{#if path}}...{{/if}}` renders the body when the value is truthy and not an empty list (no nesting needed, no else); with `shell: true` every interpolated value is passed through `shellQuote` (the template's literal text is not quoted).

- [ ] **Step 1: Tests**: `renders nested paths`, `renders lists as pretty JSON`, `missing var renders empty`, `if block shows on truthy and hides on [] and undefined`, `templateVars lists all vars once in order`, `shell mode quotes "; rm -rf ~" as one literal: renderTemplate("echo {{x}}", {x:"a'b; rm -rf ~"}, {shell:true}) === "echo 'a'\\''b; rm -rf ~'"`, `shell mode quotes $() and backticks`.
- [ ] **Step 2: Run** - FAIL. **Step 3: Implement.** **Step 4: Run** - PASS. **Step 5: Commit** `"Workflows: prompt and command templates"`.

### Task 4: Output contract

**Files:** Create `src/features/workflows/model/workflowOutput.ts`; Test `workflowOutput.test.ts`

**Interfaces:**

```ts
export const OUTPUT_FENCE = "workflow-output";
export function outputInstruction(fields: Record<string, OutputField>): string;  // appended to session prompts
export function outputReask(fields: Record<string, OutputField>, problem: string): string;  // the single follow-up message
export type ParsedOutput = { ok: true; value: Record<string, unknown> } | { ok: false; error: string };
export function parseWorkflowOutput(reply: string, fields: Record<string, OutputField>): ParsedOutput;
```

Rules: use the LAST fenced block whose info string is exactly `workflow-output`; JSON object required; every declared field present with the declared type (`list` = array, `object` = plain object); extra fields kept; with zero declared fields, a missing block is OK (`{}`).

- [ ] **Step 1: Tests**: `parses last block`, `fails when block missing: error contains "No workflow-output block"`, `fails on wrong type: error names field and expected type`, `fails on invalid JSON`, `zero fields accepts missing block`, `instruction lists each field name and type`.
- [ ] **Step 2-4:** FAIL, implement, PASS. **Step 5: Commit** `"Workflows: session output contract"`.

### Task 5: Conditions

**Files:** Create `src/features/workflows/model/workflowCondition.ts`; Test `workflowCondition.test.ts`

**Interfaces:**

```ts
export type ConditionResult = { ok: true; value: boolean } | { ok: false; error: string };
export function evalCondition(condition: string, scope: TemplateScope): ConditionResult;
```

Grammar (whole string, one comparison, no `and`/`or` in v1): `<operand> == <operand>`, `!=`, `contains`, or `<operand> empty` / `<operand> not empty`. Operand: `{{path}}`, a double-quoted string, a number, `true`/`false`. `contains` works on strings (substring) and lists (element equality).

- [ ] Tests: `== on strings`, `!= on numbers`, `contains on list`, `empty on [] and ""`, `not empty`, `parse error for "{{a}} >> 3"`. FAIL -> implement -> PASS -> commit `"Workflows: conditions for If and loops"`.

### Task 6: Pure run state machine

**Files:** Create `src/features/workflows/model/workflowEngine.ts`; Test `workflowEngine.test.ts`

**Interfaces:**

```ts
export type NodeStatus = "pending" | "running" | "waitingApproval" | "done" | "failed" | "skipped" | "stopped";
export type BranchKey = string;                         // "" for top level, "<forEachId>#<index>" inside a group
export type NodeRun = { nodeId: NodeId; branch: BranchKey; status: NodeStatus; round: number;
  sessionId?: string; output?: Record<string, unknown>; error?: string;
  decision?: { value: string; reason?: string }; startedAt?: number; endedAt?: number; reasked?: boolean };
export type RunStatus = "running" | "done" | "doneWithFailures" | "failed" | "stopped" | "interrupted";
export type WorkflowRun = { id: string; workflowId: WorkflowId; definition: Workflow /* snapshot */;
  status: RunStatus; startedAt: number; endedAt?: number; trigger: "schedule" | "manual" | "cli";
  folderId?: string; items: Record<NodeId, unknown[]>; nodes: NodeRun[]; reportPostedAt?: number; error?: string };

export type EngineAction =
  | { kind: "startSession"; nodeId: NodeId; branch: BranchKey; round: number; prompt: string; reuseSessionId?: string }
  | { kind: "runCommand"; nodeId: NodeId; branch: BranchKey; command: string }
  | { kind: "askMono"; nodeId: NodeId; branch: BranchKey; purpose: "askMono" | "if" | "loop"; question: string; answers: string[]; context: Record<string, unknown> }
  | { kind: "postReport" };

export type EngineEvent =
  | { kind: "nodeStarted"; nodeId: NodeId; branch: BranchKey; sessionId?: string; at: number }
  | { kind: "nodeFinished"; nodeId: NodeId; branch: BranchKey; output: Record<string, unknown>; at: number }
  | { kind: "nodeFailed"; nodeId: NodeId; branch: BranchKey; error: string; at: number }
  | { kind: "decided"; nodeId: NodeId; branch: BranchKey; value: string; reason?: string; at: number }
  | { kind: "waitingApproval"; nodeId: NodeId; branch: BranchKey; waiting: boolean }
  | { kind: "reportPosted"; at: number }
  | { kind: "stopped"; at: number };

export function startRun(def: Workflow, trigger: WorkflowRun["trigger"], now: number, id: string): WorkflowRun;
export function nextActions(run: WorkflowRun): EngineAction[];        // actions to start now; never repeats a running node
export function applyEvent(run: WorkflowRun, event: EngineEvent): WorkflowRun;   // immutable update
export function markInterrupted(run: WorkflowRun): WorkflowRun;       // running -> interrupted, running nodes -> pending
export function resumeRun(run: WorkflowRun): WorkflowRun;             // interrupted -> running
export function runScope(run: WorkflowRun, branch: BranchKey): TemplateScope;
```

Semantics (spec 6-8, 12):
- Prompts/commands are rendered in `nextActions` with `runScope` (session prompt = `renderTemplate(prompt) + "\n\n" + outputInstruction(output)`; command uses `shell:true`).
- A node is ready when all its non-loop predecessors in the same branch are `done` (an `if` predecessor counts only along the matching `handle`; nodes on the untaken side become `skipped`, transitively).
- `forEach`: when ready, evaluate `in` against the scope; non-list -> node `failed` with `Expected a list for <expr>`; store `items[forEachId]`; create branches `<id>#0..n-1`; start at most `maxParallel` branches whose children are not finished; group is `done` when every branch is finished (done or failed); empty list -> `done` immediately. Group output = `{ results: <each branch's last node output> }`.
- Loop edge `from -> to` with `maxRounds`: when `from` finishes and its round < maxRounds, emit `askMono` (`purpose:"loop"`, answers `["loop","exit"]`) or evaluate the condition (true = loop); on `loop` reset `to` and every node between `to` and `from` in that branch to `pending` with `round+1`; `startSession` for a looped session node carries `reuseSessionId` when `reuseSession !== false`. At maxRounds always exit and set `decision = { value: "exit", reason: "max rounds reached" }`.
- `if` with `decide:"mono"` -> `askMono` purpose `"if"`, answers `["true","false"]`.
- A failed node fails its branch (remaining branch nodes `skipped`); other branches continue.
- When no node is pending/running: `postReport` once (if a report node is reachable), then status = `done` (no failures), `doneWithFailures` (some), `failed` (no session/command node succeeded), `stopped` (after `stopped`).

- [ ] **Step 1: Tests** (build defs with a small helper): `linear trigger->fetch->report emits startSession then postReport then done`, `forEach fans out 3 items with maxParallel 2 (2 then 1)`, `forEach empty list completes with no branches`, `forEach on non-list fails with "Expected a list"`, `loop asks Mono and resets dev+review to round 2 with reuseSessionId`, `loop stops at maxRounds with reason "max rounds reached"`, `if true/false skips the other side`, `failure in one branch does not stop another; status doneWithFailures`, `markInterrupted keeps done nodes and resets running to pending`, `resumeRun never re-emits a done node`, `stopped event ends run as stopped and emits nothing`.
- [ ] **Step 2-4:** FAIL, implement, PASS. **Step 5: Commit** `"Workflows: run state machine"`.

### Task 7: Report, Block field, Ask Mono prompt

**Files:** Create `src/features/workflows/model/workflowReport.ts`; Modify `src/features/sessions/model/session.ts:~401`; Test `workflowReport.test.ts`

**Interfaces:**

```ts
export function composeReport(run: WorkflowRun): { title: string; text: string };   // Markdown
export function askMonoPrompt(action: Extract<EngineAction, { kind: "askMono" }>, workflowName: string): string;
export function parseMonoDecision(reply: string, answers: string[]): { ok: true; value: string; reason?: string } | { ok: false; error: string };
```

- `Block` gains `monoWorkflow?: { id: WorkflowId; name: string; runId: string; at: number }` next to `monoHabit`.
- Report (spec 8, 12.3): title `"<name> · <status label>"`; per branch (item title if the item has `title`/`name`, else `#n`): status, loop rounds used, "max rounds reached" flag, error if failed, and `inReport` fields of the branch's last session node (default `inReport` = true for string, false otherwise); a `report.template` overrides the body via `renderTemplate` with scope `{ run, results }`. Empty For each -> line "No items".
- Ask Mono prompt ends with: reply with a fenced `workflow-output` block `{"answer": <one of answers>, "reason": "..."}`; parse with `parseWorkflowOutput` then check `answer` is in `answers`.
- [ ] Tests: `report lists each branch with PR link`, `report flags max rounds`, `report for empty forEach says No items`, `template override renders`, `parseMonoDecision rejects answers outside the list`. FAIL -> implement -> PASS -> commit `"Workflows: report and Mono decisions"`.

### Task 8: Command execution (Rust)

**Files:** Create `src-tauri/src/workflow_command.rs`; Modify `src-tauri/src/lib.rs` (module + `invoke_handler` registration near `mono_load` 658)

**Interfaces:**

```rust
#[derive(serde::Serialize)] #[serde(rename_all = "camelCase")]
pub struct CommandResult { pub exit_code: Option<i32>, pub stdout: String, pub stderr: String, pub timed_out: bool }
#[tauri::command]
pub async fn workflow_run_command(command: String, cwd: String, timeout_ms: u64) -> Result<CommandResult, String>;
```

Run via the user's login shell (`$SHELL -lc <command>`, fallback `/bin/sh -c`) in `cwd` (must be an existing absolute directory), kill the process group on timeout, keep the last 8000 chars of stdout and stderr each. Use `spawn_blocking`.

- [ ] Tests (`#[cfg(test)]`, unix): `captures stdout and exit code` (`echo hi; exit 3` -> 3, "hi\n"), `times out` (`sleep 5`, 200 ms -> `timed_out`), `tails long output to 8000 chars`, `rejects relative cwd`. Run `cargo test -p <crate> workflow_command` FAIL -> implement -> PASS. Commit `"Workflows: command node execution"`.

### Task 9: YAML export / import

**Files:** `npm install yaml@^2`; Create `src/features/workflows/model/workflowYaml.ts`; Test `workflowYaml.test.ts`

**Interfaces:**

```ts
export const WORKFLOW_FILE_VERSION = 1;
export function exportWorkflow(w: Workflow): string;    // YAML; top key monoptahWorkflow: 1; name, description, inputs (no values), nodes (with ui positions), edges
export type ImportPreview = { workflow: Omit<Workflow, "id" | "createdAt" | "inputValues" | "enabled">;
  commands: { nodeId: NodeId; command: string }[]; prompts: { nodeId: NodeId; text: string }[]; issues: WorkflowIssue[] };
export function previewImport(text: string): { ok: true; preview: ImportPreview } | { ok: false; error: string };   // YAML or JSON
export function finishImport(preview: ImportPreview, inputValues: Workflow["inputValues"], now: number): Workflow;  // enabled:false, new id
```

Rules: export never includes `inputValues`, `nextRunAt`, `runRequested`, `lastRunId`, `id`, `importedFrom`; reject `monoptahWorkflow` missing or `!== 1` with `"Unsupported workflow file version"`; reject any string field that looks like an absolute path (`/Users/`, `/home/`, `C:\\`) with the field named; `finishImport` throws if any declared input has no value.

- [ ] Tests: `export -> preview round-trips nodes and edges`, `export omits input values`, `preview rejects version 2`, `preview rejects absolute path in a prompt`, `preview lists every command and prompt`, `finishImport saves paused with a new id`, `finishImport throws on missing input value`, `preview accepts JSON`. FAIL -> implement -> PASS -> commit `"Workflows: YAML export and import"`.

### Task 10: Runner hook and App wiring

**Files:** Create `src/app/hooks/useMonoWorkflows.ts`; Modify `src/app/App.tsx` (next to `useMonoHabits` host 9185-9280, `appendToMono` 9138); Test `src/app/hooks/useMonoWorkflows.test.ts` (mock like `useMonoHabits.test.ts`)

**Interfaces:**

```ts
export type MonoWorkflowHost = {
  sessions(): Session[];
  /** Starts a visible project session owned by the Mono, like sessions.start, without a completion report to the Mono. */
  startSession(monoId: string, req: { project: string; prompt: string; harness?: string; model?: string; effort?: string;
    workspaceMode: "current" | "worktree"; worktreeCwd?: string; parentSessionId?: string; folderId?: string; folderName?: string }):
    Promise<{ sessionId: string; folderId?: string; settled: Promise<ControlOutcome> }>;
  /** Sends a follow-up turn to an existing session and resolves when it settles. */
  send(sessionId: string, prompt: string): Promise<ControlOutcome>;
  stop(sessionId: string): Promise<void>;
  runCommand(cwd: string, command: string, timeoutMs: number): Promise<{ exitCode: number | null; stdout: string; stderr: string; timedOut: boolean }>;
  /** Hidden Mono-context turn (habit-style ephemeral session with /operator), returns the final reply. */
  askMono(monoId: string, prompt: string): Promise<string>;
  post(monoSessionId: string, workflow: Workflow, runId: string, title: string, text: string, launches?: MonoSpawnedSession[]): void;
  askApproval(monoSessionId: string, workflow: Workflow, runId: string, block: Block): void;
  endApprovals(runId: string): void;
  projectPath(monoId: string, projectInput: string): string | undefined;  // resolves {{inputs.x}} value to a Mono project path
  worktreeOf(sessionId: string): string | undefined;                       // cwd of a node's session, for checkout worktreeOf
};
export function useMonoWorkflows(host: MonoWorkflowHost, enabled?: boolean): void;
```

Behaviour:
- Tick every 60 s and on `checkWorkflowsNow`; window-owner guard like `OWNER` in `useMonoHabits.ts:44`. Due = `runRequested` or enabled with `nextRunAt <= now` and a schedule trigger; skip (and advance `nextRunAt`) when a run of that workflow is still running; advance with `nextHabitRunAt`.
- On first tick after launch, `markInterrupted` every stored run whose status is `running`.
- Drive: `startRun` -> persist -> loop `nextActions` -> execute each action concurrently -> `applyEvent` -> persist after every event (`updateRuns`). Execution:
  - `startSession`: resolve project via `projectPath`; checkout `root` -> `current`; `newWorktree` -> `worktree`; `{worktreeOf}` -> `current` + `worktreeCwd = worktreeOf(sessionId of that node in this branch)`; `nestUnder` -> `parentSessionId`; first session of the run creates folder `"<name> · <date time>"`, later ones reuse `folderId`. If `reuseSessionId` -> `send` instead. Watch for approvals and timeouts exactly like `watchRun` (`useMonoHabits.ts:102`), using `HABIT_APPROVAL_WAIT_MS` and the node timeout. On settle, read the final reply (export `finalReply` from `useMonoHabits.ts`), `parseWorkflowOutput`; on failure `send(outputReask(...))` once, parse again, else `nodeFailed`.
  - `runCommand`: cwd from project/worktree; output `{exitCode, stdout, stderr}`; nonzero exit or timeout -> `nodeFailed` with stderr tail.
  - `askMono`: `host.askMono(monoId, askMonoPrompt(...))` -> `parseMonoDecision` -> `decided` (invalid -> `nodeFailed`).
  - `postReport`: `composeReport` -> `host.post(...)` -> `reportPosted`.
- Stop: `requestWorkflowStop` -> stop every running session of the run, `stopped` event, `endApprovals`.
- App.tsx: build the host from the same pieces `sessions.start` uses (`launchQuickSessionRef`, `acceptQuickLaunch`, folder helpers `fileLaunchInFolder`/`folderChoice`, `launches` registry so `parentSessionId` nesting is allowed), with no `monitorCompletion`. `post` uses `appendToMono` with `monoWorkflow` block. `askMono` reuses the habit ephemeral-session path (`newSession(... "full-access" ...)`, `"/operator " + monoContext + prompt`, delete after).

- [ ] **Step 1: Tests** (fake host, fake timers): `due schedule workflow starts a run and posts a report`, `skips a tick while previous run is running`, `marks stored running runs interrupted on start`, `re-asks once for missing output then fails the node`, `loop reuses the dev session via send`, `stop stops running sessions and ends as stopped`, `command node failure fails only its branch`.
- [ ] **Step 2-4:** FAIL, implement, PASS. **Step 5:** `npm run check > /tmp/wf-check.log 2>&1; echo $?` -> `0`. **Step 6: Commit** `"Workflows: runner and app wiring"`.

### Task 11: App CLI `workflows.*` and Mono context

**Files:** Modify `src-tauri/src/control_cli.rs` (`APP_ACTIONS` 85-118 + count, `APP_USAGE` 119-298), `src/features/agent-app/model/agentApp.ts` (`AgentAppHost` 98-181, `FIELDS` 183-242, `handleAgentApp` 1074 prefix route), `src/app/App.tsx` (~12380 host entry), `src/features/monos/model/monoFiles.ts` (`monoContext` 333, `MONO_PROMPT_VERSION` 306 -> `21`); Create `src/features/agent-app/model/agentAppWorkflows.ts`; Tests `agentAppWorkflows.test.ts`, `control_cli.rs` tests.

**Interfaces:**
- Actions and inputs (all accept optional `project` like other actions):
  - `workflows.list {}` -> `[{id,name,enabled,trigger,nextRunAt,running,lastRun:{id,status,startedAt}}]`
  - `workflows.get {"id"}` -> full definition + last 5 runs summary
  - `workflows.add {"workflow":{...definition without id}}` or `{"yaml":"..."}` -> `{id, issues}`; saved enabled only when `issues` is empty, else paused
  - `workflows.update {"id","workflow":{...}}` (full replace, keeps id/createdAt/inputValues unless given) -> `{issues}`
  - `workflows.setInputs {"id","values":{...}}`
  - `workflows.run {"id"}`, `workflows.stop {"id"}`, `workflows.pause {"id"}`, `workflows.resume {"id"}`, `workflows.remove {"id"}`
  - `workflows.export {"id"}` -> `{yaml}`; `workflows.import {"yaml","values":{...}}` -> `{id}` (saved paused)
  - `workflows.runs {"id","limit":5}` -> run summaries with per-branch status
- `AgentAppHost.workflows?: { load(monoId): Promise<Workflow[]>; update: typeof updateWorkflows; runs: typeof loadRuns; stop(id): void; checkNow(): void }`.
- `<workflows_rules>` block (next to `<habits_rules>`), copy:

```
<workflows_rules>
You can also keep workflows: graphs of steps the app runs for you, each step usually a session, with results passed to later steps. Use them for multi-step work the user wants repeated, like pulling tickets, implementing each one and reviewing it. Node types: schedule and manual triggers, session, command, askMono, if, forEach (a group whose child nodes run once per list item), and report (one, at the end). A session node declares output fields; later prompts read them as {{nodeId.field}}, {{item.field}} inside a forEach, {{inputs.name}} for workflow inputs and {{run.round}} in a loop. A loop is an edge back to an earlier node in the same branch with maxRounds and decide "mono" (you are asked loop or exit) or a condition. Projects are always inputs, never paths. Manage them with `app workflows.list`, `workflows.get`, `workflows.add`, `workflows.update`, `workflows.setInputs`, `workflows.run`, `workflows.stop`, `workflows.pause`, `workflows.resume`, `workflows.remove`, `workflows.export` and `workflows.import`; add or change one only when the user asks, and show them the steps first. When a run finishes, its report arrives in this chat.
</workflows_rules>
```

- [ ] **Step 1: Tests**: Rust `app_mode_exposes_workflow_actions` (every `workflows.*` in `APP_ACTIONS` and in `app_help()`); TS `add rejects unknown fields`, `add with issues saves paused and returns issues`, `export then import round-trips and imports paused`, `run sets runRequested and calls checkNow`, `list reports running state`, `monoContext includes workflows_rules`.
- [ ] **Step 2-4:** FAIL, implement, PASS. **Step 5:** full check -> `0`. **Step 6: Commit** `"Workflows: app CLI actions and Mono rules"`.

### Task 12: Workflows tab, workflow page, run page

**Files:** Create `src/features/workflows/ui/MonoWorkflows.tsx`, `WorkflowPage.tsx`, `WorkflowRunPage.tsx`; Modify `src/features/monos/ui/MonoOverview.tsx` (`MonoOverviewTab` 42, `TABS` 46-55, `Route` 69, route mapping 158-188, header "+" 250-262, tab body 347-358, counts 225), `src/shared/ui/icons.tsx` (add `Workflow` from a `@hugeicons/core-free-icons/Workflow*Icon` deep import); Tests `MonoWorkflows.test.ts`, `WorkflowRunPage.test.ts`, update `MonoOverview.test.ts`.

**Interfaces:**
- `MonoOverviewTab` adds `"workflows"`; TABS order: Sessions, Documents, Habits, Workflows, Changes.
- Routes: `{kind:"workflow", id}`, `{kind:"workflow-run", id, runId}`, `{kind:"workflow-board", id}` (Task 13), `{kind:"new-workflow"}` (opens the board with an empty workflow holding a manual trigger and a report node), `{kind:"import-workflow"}` (Task 14).
- `useWorkflows(monoId, status): Workflow[] | undefined` and `workflowActions(monoId)` mirror `useHabits`/`habitActions` (`MonoHabits.tsx:32/72`).
- `WorkflowRow` mirrors `HabitRow` (`MonoHabits.tsx:105`): second line = `habitScheduleLabel` of the schedule trigger, "Manual", "Running · <n> branches · <RunningFor>", or "Paused · ..."; hover actions Run now / Stop / Pause-Resume / Remove using the same `HabitButton` pattern.
- `WorkflowPage` mirrors `HabitPage`: `PageHeader` actions Edit board, Run now, Pause/Resume, Export (saves via the existing save-file dialog helper used elsewhere in the app; find it with `grep -rn "save(" src --include=*.ts*` for `@tauri-apps/plugin-dialog`), Remove; sections Inputs (editable values, project inputs as a project picker), Recent runs (dot + status label + duration).
- `WorkflowRunPage`: one lane per branch (top-level nodes first, then each For each branch titled by item `title`/`name` or `#n`); each node chip shows status dot, title, "round n" when > 1, decision reason when present; clicking a chip with `sessionId` opens that session (`onOpenSession`); "Stop run" when running, "Resume" when interrupted; note "The workflow changed since this run started." when the snapshot differs from the current definition.
- Empty state copy: `Ask {agent.name} to chain sessions into a workflow, or create one with +.`
- [ ] Tests: `tab lists workflows with schedule label`, `paused row is dimmed and says Paused`, `run page shows one lane per item and round 2`, `interrupted run shows Resume`. FAIL -> implement -> PASS -> commit `"Workflows: panel tab, workflow and run pages"`.

### Task 13: Board editor

**Files:** `npm install @xyflow/react@^12`; Create `src/features/workflows/ui/board/{WorkflowBoard.tsx, boardNodes.tsx, LoopEdge.tsx, NodePicker.tsx, NodeInspector.tsx, boardModel.ts, workflowBoard.css}`; Tests `boardModel.test.ts`, `WorkflowBoard.test.ts`.

**Interfaces:**

```ts
// boardModel.ts (pure, tested)
export function toFlow(w: Workflow): { nodes: FlowNode[]; edges: FlowEdge[] };   // React Flow shapes; forEach -> group node, children get parentId + extent "parent"
export function fromFlow(base: Workflow, nodes: FlowNode[], edges: FlowEdge[]): Workflow;   // positions saved to node.ui
export function connectionKind(w: Workflow, from: NodeId, to: NodeId): "edge" | "loop" | "invalid";  // loop when `to` is an ancestor of `from` in the same branch
export function autoPlace(w: Workflow, parent?: NodeId): { x: number; y: number };  // below the lowest node in that scope, same x
export function newNode(type: WorkflowNode["type"], w: Workflow): WorkflowNode;    // unique id like "session_2" with sensible defaults
```

Board behaviour (spec 5.3): vertical, top-to-bottom; target handle top, source handle bottom; `if` has two bottom handles `true`/`false`; loop handles on the right side; `LoopEdge` dashed in `var(--mono-color)` with label `"Mono decides · max N"` or `"If <condition> · max N"`; toolbar: back, name (editable), `+ Node`, overflow (Export, Run now, Pause), Save; `NodePicker` grouped Trigger / Action / Flow / End with the v1 types (trigger options disabled when one exists, Report disabled when one exists); clicking a node slides `NodeInspector` over the board (Back to board) with per-type fields (session: prompt `AutoTextarea`, project input picker that creates/reuses a `project` input, model, checkout, nest under, output fields with type and "In report" toggle, timeout; command: command, cwd, timeout; askMono: question, answers; if: condition or Mono decides; forEach: in, as, maxParallel; schedule: the habit schedule editor from `NewHabitPage.tsx`; report: template); dragging a node into/out of a group sets/clears `parent`; validation issues from `validateWorkflow` shown as a red dot + message on the node and a summary above Save; Save of an enabled workflow with issues is blocked (saving as paused is allowed); unsaved changes kept in component state, leaving with changes asks Save / Discard.
Styling: import `@xyflow/react/dist/base.css` (not `style.css`) and style everything in `workflowBoard.css` / Tailwind with the app tokens; no minimap; `Controls` restyled or replaced by `IconButton`s; `proOptions={{ hideAttribution: true }}`.

- [ ] **Step 1: Tests** (`boardModel.test.ts`): `toFlow/fromFlow round-trip keeps nodes, edges, parents and positions`, `connectionKind returns loop for review->dev`, `connectionKind invalid for edge into a trigger`, `newNode ids are unique`, `autoPlace puts a node below the lowest one`. (`WorkflowBoard.test.ts`, happy-dom): `renders one node per workflow node`, `Save disabled for enabled workflow with issues`.
- [ ] **Step 2-4:** FAIL, implement, PASS.
- [ ] **Step 5: Manual check** with `npm run tauri:dev`: create the spec 9.1 workflow on the board from scratch (trigger, fetch, For each with dev/review, loop, report), save, reopen, layout preserved; compare look against the Habits tab for consistency.
- [ ] **Step 6: Commit** `"Workflows: board editor"`.

### Task 14: Import page

**Files:** Create `src/features/workflows/ui/ImportWorkflowPage.tsx`; Modify `MonoOverview.tsx` (header Import action on the Workflows tab); Test `ImportWorkflowPage.test.ts`

**Interfaces:** Uses `previewImport` / `finishImport` (Task 9). Flow: pick a file (open-file dialog helper already used in the app) or paste text -> show name, description, issues, a "Commands this workflow runs" section listing every command verbatim (monospace), a collapsible "Prompts" section, and an Inputs form (project inputs as a project picker from the Mono's projects) -> "Import paused" button -> saves via `updateWorkflows` and opens the workflow page.

- [ ] Tests: `lists commands verbatim`, `Import disabled until every input has a value`, `imported workflow is saved paused`. FAIL -> implement -> PASS.
- [ ] **Final verification:** `npm run check > /tmp/wf-check.log 2>&1; echo $?` -> `0`; `./scripts/check-fork-branding.sh; echo $?` -> `0`; manual run in `npm run tauri:dev` with a Manual-trigger workflow: one session node (`echo` style prompt returning a field) -> report; confirm the report appears in the Mono chat and the run page shows the lane.
- [ ] **Commit** `"Workflows: import page"`.
