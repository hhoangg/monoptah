# Monoptah Workflows - Design

Date: 2026-10-22. Status: approved in chat by the user; this document records it.

## 1. Intent

Habits let a Mono run one prompt on a schedule. Real work is often a chain: pull tickets,
implement each one, review, fix, report. Today the user wires that by hand through Mono
prompts (the ClickUp -> Dev -> Review pipeline built on v0.30.0). Workflows make such a chain
a first-class, visual, shareable object:

- the user (or the Mono, through chat) defines a graph of nodes on a board,
- a trigger starts a run, each node runs (most often as an agent session),
- node results flow into later nodes,
- the run ends with a report in the Mono chat,
- a workflow can be exported to a file and imported by someone else.

Workflows belong to a Mono, exactly like habits.

## 2. Goals

1. A **Workflows** tab in the Mono right panel, next to Habits.
2. A **board editor** inside that panel, built on `@xyflow/react` (React Flow v12, MIT),
   laid out **vertically, top to bottom**, sized for the narrow right panel.
3. Node types (v1): Schedule trigger, Manual trigger, Session, Command, Ask Mono, If,
   For each (group), Report to Mono. Loops are **edges**, not nodes.
4. Data passing between nodes through declared outputs and `{{...}}` template variables.
5. Fan-out: For each runs one branch per list item, with a parallel limit.
6. Loops: a back edge with `maxRounds`; at each pass the Mono (or a condition) decides loop
   or exit; the app hard-stops at `maxRounds`.
7. Runs are driven by the **app**, not by the Mono's chat. The Mono is consulted only for
   Ask Mono nodes, Mono-decided loop edges and If nodes set to "Mono decides", and receives
   the final report.
8. Run state persisted to disk after every node so a restart does not lose it.
9. Mono can list, create, edit, run, pause and remove workflows from chat through the app
   CLI (`workflows.*`), the same way it manages habits.
10. Export / import as a YAML template file.

## 3. Non-goals (v1)

- Event triggers (PR opened, ClickUp webhook, session finished). The trigger model must
  leave room for them, but none ship in v1.
- Wait-for-approval (human gate) node.
- Sticky notes, node comments, workflow versioning, sharing through a server or gallery.
- Auto-resuming interrupted runs after a restart (they are marked interrupted, the user
  resumes manually).
- Running workflows on a remote host.

## 4. Anti-goals (would be a failure even if it works)

- **Off-brand UI.** The board, nodes, inspector, list rows and dialogs must look like the
  rest of Monoptah: reuse the Habits panel patterns (`HabitRow`-style rows, `PageHeader`,
  `EmptyState`, `Section`), the shared icon set (`src/shared/ui/icons`), Tailwind tokens
  such as `text-content/60`, `bg-content/5`, `--mono-color`. React Flow's default theme
  (white boxes, its own handles/controls/minimap look) must be fully restyled. No new
  fonts, no new color palette. The HTML prototype shared in chat shows **layout only**,
  not style.
- A shared file that silently runs shell commands on the importer's machine.
- A run that spends the Mono's chat turns for routine steps (the app walks the graph).
- Losing a half-finished run's state because Monoptah restarted.

## 5. User-facing behaviour

### 5.1 Workflows tab (list)

- Lives in `MonoOverview` tabs after Habits. Icon from the shared icon set.
- Rows mirror habit rows: name, second line = trigger label (e.g. "Weekdays 09:00") or
  running state ("Running · 3 branches · 12m"), paused rows dimmed.
- Row hover actions: Run now, Stop (when running), Pause/Resume, Remove.
- Header actions: New, Import.
- Empty state text in the style of habits, e.g. "Ask <Mono> to chain sessions into a
  workflow, or create one here."

### 5.2 Workflow page

Pushed on the panel stack like the habit page. Shows name, trigger label, description,
buttons Edit board, Run now, Pause/Resume, Export, Remove, and **Recent runs** (newest
first). Clicking a run opens the **Run page**: one lane per branch, each lane showing the
nodes it passed, the loop round ("Dev · round 2"), status dots (running, done, failed,
waiting), and links to open each node's session.

### 5.3 Board editor

- Vertical layout, top to bottom. Main edges enter a node at the top and leave at the
  bottom. Loop edges attach to dedicated side handles and render as a dashed edge in the
  Mono color with a small label ("Mono decides · max 3").
- Toolbar: back, workflow name, **+ Node**, overflow menu (Export, Run now, Pause), Save.
- **+ Node** opens a picker grouped as Trigger / Action / Flow / End (section 6).
- Clicking a node slides its **inspector** over the board (same panel, "Back to board").
- Drag a node into a For each group to make it a child; drag out to detach.
- Connecting an edge from a lower node to a node above it in the same branch creates a
  loop edge (asks for maxRounds and decider).
- Pan, zoom, fit view; no minimap (panel is narrow).
- Validation shown inline on nodes and blocking Save of an enabled workflow: exactly one
  trigger, a Report to Mono node reachable, no unknown `{{vars}}`, no cycles except
  declared loop edges, every For each has an input list.
- Unsaved changes are kept as a draft; leaving asks to save or discard.

### 5.4 Through the Mono chat

The Mono gets `workflows.list | get | add | update | run | stop | pause | resume | remove |
export | import` in the app CLI and a section in its context that teaches the schema
(mirroring how habits are taught). Edits made by the Mono show up on the board live, as
habit edits show up in the panel today.

## 6. Node types (v1)

| Type | Kind | Config | Output |
|---|---|---|---|
| `schedule` | trigger | habit schedule object (daily, weekdays, weekly, hourly, interval) | `run.startedAt` |
| `manual` | trigger | declared run inputs | the inputs |
| `session` | action | prompt template, project, model/harness, checkout (`root`, `newWorktree`, `worktreeOf: <nodeId>`), reuse session across loop rounds (default yes), nest under node, declared output fields | declared fields |
| `command` | action | shell command template, project, cwd (`root` or `worktreeOf: <nodeId>`), timeout | `exitCode`, `stdout` (tail, capped), `stderr` (tail, capped) |
| `askMono` | action | question template, allowed answers (list of strings) | `answer`, `reason` |
| `if` | flow | condition (`{{review.verdict}} == "approve"` style, simple ops: `==`, `!=`, `contains`, `empty`, `not empty`) or `decide: mono` with a question; two out handles `true` / `false` | none |
| `forEach` | flow (group) | `in: <list expression>`, `as: item`, `maxParallel` (default 3) | list of each branch's last outputs |
| `report` | end | optional report template; default report summarises every branch | none |

Exactly one trigger per workflow in v1. Multiple report nodes are not allowed in v1 (one,
at the end).

**Loop edge** (property of an edge, not a node): `to: <nodeId upstream in the same
branch>`, `maxRounds` (1-10, default 3), `decide: mono | <condition>`. When the source node
finishes: if rounds used < maxRounds, the decider chooses `loop` (go back to `to`) or
`exit` (follow the normal out edge). At maxRounds the app always exits and marks the
branch "max rounds reached" for the report.

## 7. Passing results between nodes

- Every action node has an **output contract**. For `session` nodes the user declares
  fields (name + type: string, number, boolean, list, object). The app appends to the
  session prompt an instruction to end the final message with a fenced block
  ` ```workflow-output ` containing JSON matching the fields.
- When the session finishes, the app parses the last such block from the final assistant
  message. Missing or invalid -> the app sends **one** follow-up message in the same
  session asking for the block again; still invalid -> the node fails.
- Template variables, resolved by the app before sending a prompt or command:
  `{{inputs.x}}`, `{{<nodeId>.<field>}}`, `{{item}}` / `{{item.field}}` inside For each,
  `{{run.round}}` inside a loop, `{{run.id}}`, `{{run.startedAt}}`. Lists and objects
  render as pretty JSON. Inside a loop, `{{<nodeId>...}}` is the latest round's output.
  Minimal conditionals: `{{#if x}}...{{/if}}`.
- Command templates escape interpolated values for the shell.
- Every node output is stored in the run record, so the Run page and the final report can
  show it.

## 8. Run engine

- The app (not the Mono agent) walks the graph. A run is created by the trigger (schedule
  due, Run now, or `workflows.run`).
- Session nodes start sessions through the same internal path the Mono's `sessions.start`
  uses, with the Mono as owner, so they appear under the Mono in the sidebar, may be
  nested (`nestUnder`), and are filed in a per-run folder named
  "<workflow name> · <date time>" (one folder per run, reused for all its sessions).
- For each: branches start up to `maxParallel` at a time; the group finishes when every
  branch has finished (done or failed).
- A failed node fails only its branch; other branches continue. The run's final status is
  `done`, `doneWithFailures`, `failed` (nothing succeeded), `stopped` or `interrupted`.
- **Ask Mono** and Mono-decided loops/Ifs: the app sends the Mono a short, structured
  request (the question, relevant node outputs, allowed answers) and parses a structured
  answer. These must not clutter the user's chat: they run as a background Mono turn or a
  small side session owned by the Mono (builder's choice based on what the codebase
  supports), and the decision plus reason are recorded in the run.
- **Report to Mono**: when the run finishes, the app posts the report into the Mono chat
  the same way a habit run reports (the habit "posted" path), including per branch: key
  outputs (e.g. PR links), loop rounds used, failures and max-rounds-reached flags.
- Concurrency: a workflow does not start a new run while its previous run is still
  running (the scheduled tick is skipped and noted, same spirit as habits).
- Stop: stops every running session of the run and marks it `stopped`.
- Approvals: see section 12.1.
- Persistence: run state (node states, outputs, round counters, session ids, decisions)
  is written to disk after every state change. On app start, any run still `running` is
  marked `interrupted`; the Run page offers **Resume**, which restarts unfinished nodes
  (a node that was mid-session is re-run; finished nodes are not).
- History: keep the most recent 20 runs per workflow.

## 9. Storage and file format

- Workflows are stored per Mono alongside its habits, in the Mono's own files, one JSON
  file per workflow (or one file for all, matching how habits are stored; builder's
  choice). Run records are stored separately from definitions so editing a workflow never
  rewrites run history.
- Board positions are part of the definition (`ui.position` per node) so an exported file
  opens with the same layout.

### 9.1 Export / import (YAML)

- Export writes a `.monoptah-workflow.yaml` file. JSON is accepted on import too.
- The file contains only the definition: no run history, no session ids, no absolute
  paths, no tokens, no Mono id.
- Anything machine-specific is a declared **input** (`type: project | string | number |
  boolean`), referenced as `{{inputs.x}}` or in `project: "{{inputs.project}}"`. On import
  the app asks the user to fill every input (projects chosen from the user's projects).
- Top-level `monoptahWorkflow: 1` version key. Unknown future versions are rejected with a
  clear message.
- **Import safety**: the import dialog lists every `command` node's command and every
  session prompt for review; an imported workflow is saved **paused** and the user must
  enable it.

Example (shape, not a final schema):

```yaml
monoptahWorkflow: 1
name: ClickUp -> Dev -> Review
description: Pull my ClickUp tickets, implement each, review up to 3 rounds.
inputs:
  project: { type: project }
  clickupList: { type: string, description: ClickUp list to pull from }
nodes:
  - id: trigger
    type: schedule
    schedule: { kind: weekdays, time: "09:00" }
  - id: fetch
    type: session
    project: "{{inputs.project}}"
    model: sonnet
    checkout: root
    prompt: List open tickets assigned to me in {{inputs.clickupList}}.
    output:
      tickets: { type: list }
  - id: each
    type: forEach
    in: "{{fetch.tickets}}"
    as: item
    maxParallel: 3
  - id: dev
    type: session
    parent: each
    project: "{{inputs.project}}"
    model: sonnet
    checkout: newWorktree
    prompt: |
      Implement {{item.title}} ({{item.url}}) and open a PR.
      {{#if review}}Fix these review issues: {{review.issues}}{{/if}}
    output: { pr_url: { type: string }, summary: { type: string } }
  - id: review
    type: session
    parent: each
    project: "{{inputs.project}}"
    model: opus
    checkout: { worktreeOf: dev }
    nestUnder: dev
    prompt: Review {{dev.pr_url}}. List blocking issues only.
    output: { verdict: { type: string }, issues: { type: list } }
  - id: report
    type: report
edges:
  - { from: trigger, to: fetch }
  - { from: fetch, to: each }
  - { from: dev, to: review }
  - { from: each, to: report }
  - { from: review, to: dev, loop: { maxRounds: 3, decide: mono } }
```

## 10. Constraints

- Fork rules in `CLAUDE.md` (branding, never-rename identifiers, dev build via
  `npm run tauri:dev`, verify with `npm run check` reading the exit status directly).
- React 19, TypeScript, Tauri; new dependency `@xyflow/react` (and a YAML library if none
  exists). No other new UI library.
- All code, identifiers, comments and UI copy in English.
- Do not regress habits; share schedule code with habits rather than copying it.

## 11. Decided by the user vs. left to the builder

User decided: Workflows tab next to Habits in the Mono right panel; cron trigger first,
events later; each node is a session-or-other node type as in section 6; results passed to
later nodes; per-ticket fan-out; loop between nodes with max rounds where the Mono decides;
report to the Mono at the end; export/import as a shareable template; React Flow board,
vertical top-to-bottom inside the right panel; the app's own design style; "+ Node" picker
like n8n; the Mono can create and edit workflows through chat.

Assistant's calls (user may revisit): YAML as the export format; app-driven run engine
with the Mono consulted only for decisions and the report; fenced `workflow-output` JSON
as the output contract; loops as edges; command node with import review and imported
workflows saved paused; failure isolated per branch; interrupted runs resumed manually;
20 runs of history; per-run session folder.

Left to the builder: exact storage file layout, how Ask Mono turns are kept out of the
visible chat, internal module boundaries, auto-layout for new nodes, exact copy.

## 12. Builder-check answers (assistant's calls, 2026-10-22)

1. **Permission mode.** Workflow sessions run exactly like habit runs: `full-access`, and
   any approval request is routed into the Mono chat with the same wait as habits
   (`HABIT_APPROVAL_WAIT_MS`). While waiting, the node shows status "waiting for approval".
   Stop withdraws pending approvals. Imported workflows stay safe through the import review
   and being saved paused (section 9.1). No per-node permission setting in v1.
2. **Inputs.** v1 has only **workflow-level inputs**, stored as editable values on the
   workflow (a "Inputs" section on the workflow page and in the board's workflow settings),
   usable by every trigger. Manual trigger has no run-time inputs in v1. Every `project`
   field on a node references a `project` input; the inspector's project picker creates or
   reuses such an input automatically, so definitions never hold absolute paths. Export
   writes input declarations without their values; import asks for the values.
3. **Report.** The app composes the report itself (no Mono turn) and posts it into the
   Mono chat through the habit "posted" path. Default content per branch: status, loop
   rounds used, max-rounds/failure flags, and the output fields of the branch's last
   session node. Each declared output field has an "In report" toggle (default on for
   string fields, off for list/object). A `report` node may override with a template.
4. **Resume and edits.** A run stores a snapshot of the definition it started with.
   Resume always continues from that snapshot. Edits apply to the next run; the Run page
   says so when the definition changed since the run started.
5. Also settled by the builder check: the engine lives in the frontend next to the habit
   runner; workflow sessions are started with completion notifications to the Mono turned
   off (the engine itself watches them); a session node has a timeout (default 2h,
   configurable) after which it fails.
