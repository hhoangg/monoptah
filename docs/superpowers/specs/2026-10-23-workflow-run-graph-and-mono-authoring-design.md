# Workflow run graph and Mono-assisted authoring - Design

Date: 2026-10-23. Status: agreed with the user in chat; this document records it. Builds on
`2026-10-22-mono-workflows-design.md` (the Workflows feature on branch `feature/mono-workflows`).

## 1. Intent

Two follow-ups after the first real use of Workflows:

1. **Run graph.** The run page today is a flat list of node rows (`WorkflowRunPage.tsx`). The
   user wants to *see* a run: the whole workflow as it was when the run started, with the
   nodes and lines that already ran or are running highlighted. A For each must show the
   fan-out the run actually did (the definition has one session inside the For each; a run
   over 4 tickets shows 4 branches of that session).
2. **Mono-assisted authoring.** Workflows are complex enough that building or editing them by
   hand invites mistakes. The user wants to describe a workflow in the Mono chat and let the
   Mono build or change it, with the Mono knowing the workflow feature well enough to do it
   right. Manual editing on the board stays exactly as it is; this is an extra, more
   convenient way in, not a replacement.

## 2. Goals

1. Run page shows a read-only graph of the run's definition snapshot, drawn with the same
   look as the board editor, with run state on every node and edge.
2. For each groups show one lane per item, in two switchable views:
   - **Lanes** (side by side, overview; the user's "A").
   - **Single lane with item tabs** (one item at full width; the user's "B").
   A small icon in the run page header switches between them.
3. Selecting a node shows its details under the graph.
4. The Mono can fetch a complete authoring guide for workflows on demand, can dry-run
   validation of a definition before saving, and is taught the conversational flow
   (clarify, propose, save paused, user reviews and enables). Workflows the Mono creates are
   always saved paused (decided by the user).
5. A "Ask Mono to build" entry point from the Workflows tab.

## 3. Non-goals (this round)

- The "Wait until" node, per-item pipelines that outlive the batch run, and remembering
  tickets already taken across runs. These were discussed but are a separate, later project.
  The authoring guide must not describe them.
- Editing from the run graph (it is read-only).
- A graph preview card inside the chat (see 6.4, builder's call).
- Changing how workflows run (engine semantics stay as they are).

## 4. Anti-goals

- Off-brand UI: the run graph must look like the board (same node cards, tokens, React Flow
  restyling in `workflowBoard.css`); no React Flow default look, no new palette or fonts.
- A Mono that can bypass the user's review: everything the Mono creates or imports keeps the
  current safety rules (validation gates, `needsReview` for file imports, saved paused when it
  has issues, the user enables). Nothing in this round lets the Mono enable or run something
  the panel would refuse.
- A guide that drifts from the code: the authoring guide and the validation the Mono gets must
  come from the same source as the board's rules, so they cannot disagree.
- Losing the current run page information (status, started, took, stop/resume buttons, the
  "workflow changed since this run started" note, open-session links, round and decision
  info).

## 5. Run graph (part 1)

### 5.1 What is drawn

- The run's definition snapshot (`run.definition`), not the current workflow. Positions come
  from `ui` in the snapshot; nodes without positions are auto-placed the way the board does.
- Same node cards as the board (`boardNodes.tsx`), in a read-only mode: no handles to drag, no
  inspector, no connect, no delete; pan, zoom and fit stay (same zoom IconButtons as the
  board).
- Run state per node, from `run.nodes` (NodeRun):
  - done: green accent (the app's existing emerald used by status dots),
  - running: `--mono-color` accent with the app's existing pulse,
  - waiting for approval / asking the Mono (`isAsking`): amber,
  - failed: red,
  - skipped / not started (pending): dimmed, dashed border.
  - Second line: status label, duration for finished nodes, "round n" when n > 1, the If
    decision (`true`/`false`) or loop decision with reason, error text for failed nodes
    (truncated, full text in details).
- Edges: an edge is "taken" when its source is done and (for If) the decision matches the
  edge handle; taken edges use the success color, edges into a running node use the Mono
  color, untaken or not-yet edges are dimmed and dashed. Loop edges keep the board's dashed
  Mono-color loop style and label.
- Top-level nodes appear once. Nodes inside a For each appear once per item (see 5.2).
  Archived rounds of a loop that contains a For each (`<group>#<i>@r<n>` branches) are shown
  as earlier rounds of that item (5.3).

### 5.2 For each views

Both views draw the For each group box with a header label "For each <title> · N" plus a
summary ("2 done · 1 running · 1 failed").

- **Lanes view:** inside the group, one lane per item, side by side, each lane a column with
  the group's child nodes laid out by their definition positions (scaled to the lane width).
  Lane header: item title (`title`/`name` of the item, else `#n`) with the lane status dot.
  Many items: the canvas is wider than the panel; the user pans or zooms. Items not yet
  admitted (maxParallel) show as not started.
- **Single-lane view:** the group shows item tabs (title + status dot) and one lane at full
  group width; a slight stack of cards behind it signals there are more. The selected tab is
  remembered while the page is open.
- **Switch:** an icon button in the run page header toggles Lanes / Single. Default: Lanes when
  every For each in the run has at most 4 items and at most 3 child nodes, otherwise Single.
  The last choice is remembered per workflow (localStorage, `monocode.` prefix).
- A run with no For each shows no switch.

### 5.3 Loops and rounds

- A node that ran several rounds shows its latest round on the card ("round n").
- The details panel lists earlier rounds (round, status, decision and reason, error, session
  link, start and end). Today the engine resets a loop body's NodeRuns in place and keeps no
  per-round history (only For each child branches are archived as `<group>#<i>@r<n>`), and a
  "loop" decision is never stored. Builder's call: add a data-only per-round history to
  NodeRun (e.g. `history?: RoundRecord[]`, appended by `resetLoop` before it resets a node and
  by `decideLoop` for the loop decision), persisted with the run; no change to what the engine
  runs or decides. Runs saved before the change simply show no history.
- Archived For each rounds: when a For each inside a loop expanded several times, the group
  box gets a small round selector ("Round 1 · Round 2 ..."); lanes then show the items of the
  selected round titled from that round's own item list (`items["<group>@r<n>"]`), so items are
  never matched across rounds by index. Default: the latest round. (Builder's call.)

### 5.4 Details panel

Under the graph (same panel, collapsible), for the selected node (default: the running node,
else the last finished node):
- title, type, status, started, duration,
- round history and decisions with reasons,
- error text for failed nodes,
- output fields (short values inline, long values truncated with a "Show more"),
- "Open session" when the node has a session id (uses the existing `onOpenSession` and its
  existing error notice),
- for command nodes: exit code and the stdout/stderr tails.

### 5.5 Page around the graph

Keep the current header and meta (status, started, took, Stop run / Resume, the
"changed since this run started" note). The current flat list of lanes is replaced by the graph
and the details panel. The page refreshes live as today (files-changed + interval while
running).

## 6. Mono-assisted authoring (part 2)

### 6.1 Authoring guide on demand

- New app CLI action `workflows.guide` (no input) returning plain text (Markdown) the Mono
  reads before building or changing a workflow. Content:
  - the definition shape: top-level fields, every node type and its fields with types,
    defaults and limits, edges, If handles, loop edges (`loop: {maxRounds, decide}`), the For
    each group (`parent`, `in`, `as`, `maxParallel`), inputs and projects-as-inputs,
  - template variables and where each is readable (prompts, commands, conditions, report
    templates and their narrower scope), `{{#if}}` rules, the condition grammar,
  - session output contract and `inReport`,
  - command node safety rules (bare variables only, the unquotable syntax list, dash rule,
    no absolute paths, Windows unsupported),
  - models: harness/model/effort take exact ids from `app models.list`,
  - limits (workflows per Mono, maxParallel, maxRounds, timeouts),
  - 2-3 complete worked examples as JSON definitions that pass validation, at least: a
    linear session -> report; a ticket fan-out with dev -> review loop decided by a review
    verdict condition; an If on a session output.
- The guide must be generated from, or tested against, the same constants and validator the
  board uses, so it cannot drift: every worked example in the guide is validated by a unit
  test with `workflowIssues`, and limits are interpolated from the model constants.
- The always-on `<workflows_rules>` stays short; it gains one sentence telling the Mono to
  read `app workflows.guide` before adding or changing a workflow and to use
  `workflows.validate` before saving. `MONO_PROMPT_VERSION` stays 21 (main and the latest
  tag are still on 20, so 21 is unreleased and already covers this change).

### 6.2 Dry-run validation

- New app CLI action `workflows.validate {"workflow": {...}}` (or `{"yaml": "..."}`) that runs
  the exact checks `workflows.add` runs (definition parse, `workflowIssues` including model
  availability, input value checks when values are given, command template checks) and
  returns `{issues, commands, prompts}` without saving anything.

### 6.3 Conversational flow (taught in the guide)

1. The user describes the workflow. The Mono asks only what it cannot infer (project, models,
   review rounds, what happens on failure), one or two questions at a time.
2. The Mono proposes the steps in plain words in the chat (trigger, each step with its model,
   what each step reads and produces, loops and conditions, the report).
3. On the user's go, the Mono validates (`workflows.validate`) until clean, saves with
   `workflows.add` (or `workflows.update` for a change), and tells the user how to open it on
   the board and enable it.
   **Decided by the user: a workflow created through the CLI is always saved paused.**
   `workflows.add` (JSON or YAML) never saves an enabled workflow, whatever the input says or
   however clean it is; the user enables it in the panel. `workflows.update` of an existing
   workflow keeps today's behaviour (it keeps the stored enabled state unless `enabled` is
   given, and cannot enable a workflow with issues or one that needs review). Update the CLI
   tests and the help text in `src-tauri/src/control_cli.rs` accordingly.
4. Later changes go through chat the same way ("make the reviewer Opus", "loop at most 2
   rounds"); the board reflects them live as today.
Safety stays as today: the Mono cannot clear `needsReview`, and file imports stay paused.

### 6.4 Entry point

- Workflows tab header gets an action "Ask Mono to build a workflow" (shared icon, like the
  other header actions). It focuses the Mono chat composer with a prefilled draft
  ("Help me build a workflow that ...") the user completes and sends. It must not send by
  itself.
- Builder's call (not decided by the user): no in-chat graph card this round; the Mono lists
  the steps in text and the user opens the board. If the app's existing `chat.card` mechanism
  can show a workflow card with an "Open board" action at low cost, it may be added; otherwise
  skip.

## 7. Constraints

- Fork rules in `CLAUDE.md` (branding, never-rename identifiers, `npm run tauri:dev` only,
  `npm run check` read directly, `./scripts/check-fork-branding.sh`).
- No new dependencies. React Flow (`@xyflow/react`) is already a dependency.
- English for code, comments, UI copy; no em or en dash characters.
- Reuse the board's node components and layout code for the run graph rather than copying.
- Commits on `feature/mono-workflows`, never pushed.

## 8. Left to the builder

- Internal module boundaries (e.g. a `runGraphModel.ts` that turns a run into React Flow nodes
  and edges, tested purely).
- Exact lane sizes, the auto-placement of nodes without positions, copy of labels.
- How the prefilled chat draft reaches the composer (reuse whatever the app already does for
  prefilled Mono prompts).
