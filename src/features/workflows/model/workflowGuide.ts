import {
  DEFAULT_COMMAND_TIMEOUT_MINUTES,
  DEFAULT_MAX_PARALLEL,
  DEFAULT_SESSION_TIMEOUT_MINUTES,
  MAX_PARALLEL_LIMIT,
  MAX_ROUNDS_LIMIT,
  MAX_TIMEOUT_MINUTES,
  WORKFLOWS_MAX,
  WORKFLOW_DESCRIPTION_MAX,
  WORKFLOW_NAME_MAX,
  WORKFLOW_RUNS_KEPT,
  type Workflow,
} from "./workflow";

/**
 * What a Mono reads (`app workflows.guide`) before it builds or changes a workflow. The limits
 * come from the same constants the validator and the engine use, and every worked example is
 * checked against the validator by a test, so the guide cannot drift from the rules.
 */

type GuideExample = {
  name: string;
  /** One or two sentences on what the example shows, printed above its JSON. */
  why: string;
  workflow: Omit<Workflow, "id" | "createdAt" | "enabled">;
};

export const GUIDE_EXAMPLES: GuideExample[] = [
  {
    name: "Linear: one session, then the report",
    why: "A schedule starts one session that writes a digest, and a report template posts it. Use this shape for any single task that should run on a timetable.",
    workflow: {
      name: "Weekday repo digest",
      description: "Every weekday morning, summarize what changed in the repo yesterday.",
      inputs: { project: { type: "project" } },
      inputValues: { project: "my-project" },
      nodes: [
        {
          id: "trigger",
          type: "schedule",
          schedule: { scheduleKind: "weekdays", time: "09:00", minute: 0, dayOfWeek: 1 },
        },
        {
          id: "digest",
          type: "session",
          title: "Write the digest",
          project: "{{inputs.project}}",
          model: "claude:sonnet-5",
          checkout: "root",
          prompt:
            "Read the git history of the last day and summarize what changed, grouped by area. Mention anything that looks risky.",
          output: {
            summary: { type: "string", description: "The digest, in Markdown" },
          },
        },
        {
          id: "report",
          type: "report",
          template: "Repo digest for run {{run.id}}\n\n{{digest.summary}}",
        },
      ],
      edges: [
        { from: "trigger", to: "digest" },
        { from: "digest", to: "report" },
      ],
    },
  },
  {
    name: "Ticket fan-out: dev, then review in a loop",
    why: "A session lists tickets, a For each runs a dev session and a review session per ticket, and a loop edge sends the work back to dev until the reviewer approves or 3 rounds have passed. The condition is true to go back, so it reads != approve.",
    workflow: {
      name: "Tickets: dev and review",
      description: "Take open tickets, implement each in its own worktree and review it, up to 3 rounds.",
      inputs: {
        project: { type: "project" },
        list: { type: "string", description: "The ticket list to take work from" },
      },
      inputValues: { project: "my-project", list: "Sprint" },
      nodes: [
        { id: "trigger", type: "manual" },
        {
          id: "fetch",
          type: "session",
          title: "List tickets",
          project: "{{inputs.project}}",
          model: "claude:sonnet-5",
          checkout: "root",
          prompt: "List the open tickets assigned to me in {{inputs.list}}. Give each one a title and a url.",
          output: {
            tickets: { type: "list", description: "Objects with title and url" },
          },
        },
        {
          id: "each",
          type: "forEach",
          title: "Each ticket",
          in: "{{fetch.tickets}}",
          as: "ticket",
          maxParallel: 2,
        },
        {
          id: "dev",
          type: "session",
          title: "Dev",
          parent: "each",
          project: "{{inputs.project}}",
          model: "claude:sonnet-5",
          checkout: "newWorktree",
          prompt:
            "Implement the ticket {{ticket.title}} ({{ticket.url}}) and open a pull request.\n{{#if review}}Round {{run.round}}. Fix these review issues first: {{review.issues}}{{/if}}",
          output: {
            pr_url: { type: "string", description: "URL of the pull request" },
          },
        },
        {
          id: "review",
          type: "session",
          title: "Review",
          parent: "each",
          project: "{{inputs.project}}",
          model: "claude:opus-5",
          checkout: { worktreeOf: "dev" },
          nestUnder: "dev",
          prompt:
            "Review the pull request {{dev.pr_url}} for {{ticket.title}}. Report blocking issues only. Answer approve when there are none.",
          output: {
            verdict: { type: "string", description: "approve or changes" },
            issues: { type: "list", inReport: true, description: "Blocking issues, empty when approved" },
            pr_url: { type: "string", description: "The pull request you reviewed" },
          },
        },
        { id: "report", type: "report" },
      ],
      edges: [
        { from: "trigger", to: "fetch" },
        { from: "fetch", to: "each" },
        { from: "dev", to: "review" },
        {
          from: "review",
          to: "dev",
          loop: { maxRounds: 3, decide: '{{review.verdict}} != "approve"' },
        },
        { from: "each", to: "report" },
      ],
    },
  },
  {
    name: "If on a session output, with a command first",
    why: "A command reads recent history, a session judges it and sets a boolean, and an If sends risky changes to a deeper review while safe ones go straight to the report. The report template skips the review text when that step did not run.",
    workflow: {
      name: "Triage recent changes",
      description: "Look at recent commits under a path and review them in depth only when they look risky.",
      inputs: {
        project: { type: "project" },
        pathspec: { type: "string", description: "The folder to look at, relative to the project" },
      },
      inputValues: { project: "my-project", pathspec: "src" },
      nodes: [
        { id: "trigger", type: "manual" },
        {
          id: "recent",
          type: "command",
          title: "Recent commits",
          project: "{{inputs.project}}",
          command: "git log --oneline --max-count=20 -- {{inputs.pathspec}}",
          timeoutMinutes: 5,
        },
        {
          id: "assess",
          type: "session",
          title: "Assess",
          project: "{{inputs.project}}",
          model: "claude:sonnet-5",
          checkout: "root",
          prompt:
            "These are the latest commits under {{inputs.pathspec}}:\n{{recent.stdout}}\nSay in a few sentences what changed and whether any commit is risky.",
          output: {
            summary: { type: "string", description: "What changed, in a few sentences" },
            risky: { type: "boolean", description: "True when a commit needs a deeper look" },
          },
        },
        { id: "gate", type: "if", title: "Risky?", condition: "{{assess.risky}} == true" },
        {
          id: "deep_review",
          type: "session",
          title: "Deep review",
          project: "{{inputs.project}}",
          model: "claude:opus-5",
          checkout: "root",
          prompt: "Review the risky commits in depth. Context:\n{{assess.summary}}",
          output: { notes: { type: "string", description: "Findings, most serious first" } },
        },
        {
          id: "report",
          type: "report",
          template:
            "Changes under {{inputs.pathspec}}\n\n{{assess.summary}}\n\n{{#if deep_review}}Deep review:\n{{deep_review.notes}}{{/if}}",
        },
      ],
      edges: [
        { from: "trigger", to: "recent" },
        { from: "recent", to: "assess" },
        { from: "assess", to: "gate" },
        { from: "gate", to: "deep_review", handle: "true" },
        { from: "gate", to: "report", handle: "false" },
        { from: "deep_review", to: "report" },
      ],
    },
  },
];

const json = (value: unknown) => JSON.stringify(value, null, 2);

/** The authoring guide, as Markdown. */
export function workflowGuide(): string {
  const sections: string[] = [];
  const add = (...lines: string[]) => sections.push(lines.join("\n"));

  add(
    "# Writing a Monoptah workflow",
    "",
    "Read this whole guide before you add or change a workflow. Check every definition with `workflows.validate` before you save it.",
  );

  add(
    "## What a workflow is",
    "",
    "A workflow is a graph of nodes that the app runs for the user, on a schedule or on demand. Most nodes start a session (an agent run in one of the user's projects); the app passes each node's results to the nodes after it, runs For each groups once per list item, repeats steps in loops, and posts one report to your chat when the run ends. You build and change workflows through the `app workflows.*` actions; the user sees and runs them on the Workflows board.",
    "",
    "A workflow has exactly one trigger, exactly one `report` node reachable from it, and no absolute paths: projects are always inputs, so a workflow can be shared.",
  );

  add(
    "## The definition",
    "",
    "`workflows.add` and `workflows.validate` take `{\"workflow\": {...}}` (or `{\"yaml\": \"...\"}` with optional `values`, for an exported file). The workflow object has only these fields; the id, creation time, enabled state and run state belong to the app and are refused if you send them.",
    "",
    `- \`name\` (required, at most ${WORKFLOW_NAME_MAX} characters) and \`description\` (optional, at most ${WORKFLOW_DESCRIPTION_MAX} characters).`,
    "- `inputs`: an object of named inputs, each `{\"type\": \"project\" | \"string\" | \"number\" | \"boolean\", \"description\"?}`. Names use letters, digits and underscores.",
    "- `inputValues`: the value of each declared input. A `project` input takes the path or name of one of the Mono's projects. A missing or mistyped value is reported as an issue. Change values later with `workflows.setInputs`.",
    "- `nodes`: a list of nodes (below). `edges`: a list of `{\"from\", \"to\", \"handle\"?, \"loop\"?}`.",
    "",
    "Every node has `id` and `type`, and may have `title` (a readable label for the board), `parent` (the For each it runs inside) and `ui` (a board position; leave it out, the board places the nodes itself).",
    "",
    "A node id matches `^[a-z][a-z0-9_]{0,39}$` (lowercase letters, digits and underscores, starting with a letter), is unique, and must not be `run`, `inputs` or `results`, because ids appear in templates as `{{id.field}}`.",
    "",
    "### Projects are inputs",
    "",
    "Declare an input of type `project`, give it a value in `inputValues`, and write `\"project\": \"{{inputs.<name>}}\"` on every session and command node. That exact form is the only one accepted for `project`. No node text may contain an absolute path (a home or system directory such as `/Users/...`, `/home/...`, `/tmp/...`, `~/...`, or a drive like `C:\\`); use the project input or a relative path. A bare URL path like `/api/users` in a prompt is fine.",
    "",
    "### Node types",
    "",
    "`schedule` (trigger)",
    "- `schedule`: `{\"kind\": \"hourly\" | \"daily\" | \"weekdays\" | \"weekly\" | \"interval\", \"time\"?: \"HH:MM\", \"minute\"?: 0-59, \"dayOfWeek\"?: 0-6, \"hours\"?: 1-24}`. `time` is local 24-hour time (default 09:00); `minute` is for hourly; `dayOfWeek` is for weekly (0 is Sunday, default 1); `hours` is required for interval and runs every that many hours from `time` (default 00:00). The stored form `{\"scheduleKind\", \"time\", \"minute\", \"dayOfWeek\", \"intervalHours\"?}`, used in the examples below, is accepted too.",
    "",
    "`manual` (trigger)",
    "- No fields. It runs only when started with `workflows.run` or from the board. A workflow has either one `schedule` or one `manual` trigger, never both. A trigger exposes nothing to templates; use `{{run.startedAt}}` for the start time.",
    "",
    "`session`",
    "- `project` (required, `{{inputs.<name>}}`), `prompt` (required, a template), `checkout` (required): `\"root\"` (the project's own checkout), `\"newWorktree\"` (a fresh worktree) or `{\"worktreeOf\": \"<session node id>\"}` (continue in the worktree of an earlier session of the same branch; that node must be a session).",
    "- `output`: the fields the session reports back (see the output contract below). May be `{}`.",
    "- `harness`, `model`, `effort` (optional): see Models.",
    `- \`reuseSession\` (default true): in a loop, later rounds continue in the same session instead of starting a new one. \`nestUnder\` (optional): the id of another session node; this session is shown under it in the sidebar. \`timeoutMinutes\` (default ${DEFAULT_SESSION_TIMEOUT_MINUTES}, above 0, at most ${MAX_TIMEOUT_MINUTES}).`,
    "",
    "`command`",
    `- \`project\` (required), \`command\` (required, a shell command template, see Command rules), \`cwd\` (optional): \`"root"\` or \`{"worktreeOf": "<session node id>"}\`, \`timeoutMinutes\` (default ${DEFAULT_COMMAND_TIMEOUT_MINUTES}, at most ${MAX_TIMEOUT_MINUTES}). A command that exits with a non-zero code fails the node. Its output is \`exitCode\`, \`stdout\` and \`stderr\`.`,
    "",
    "`askMono`",
    "- `question` (required, a template) and `answers` (required, at least one string). The app asks you the question in a short single-decision request and you answer with one of the answers. Its output is `answer` and `reason`.",
    "",
    "`if`",
    "- Exactly one of `condition` (see Conditions) or `decide: \"mono\"` (you are asked true or false, with an optional `question` template). Its out edges must carry `handle: \"true\"` or `handle: \"false\"`; only the nodes on the matching side run, the others are skipped.",
    "",
    "`forEach`",
    `- \`in\` (required): exactly one list reference such as \`{{fetch.tickets}}\`. Its nodes are the nodes that name it in \`parent\`; they run once per item, up to \`maxParallel\` items at a time (whole number 1 to ${MAX_PARALLEL_LIMIT}, default ${DEFAULT_MAX_PARALLEL}). \`as\` (default \`item\`) names the item variable, readable as \`{{<as>.field}}\` inside the group; it must be letters, digits and underscores, not \`run\`, \`inputs\` or \`results\`, and not a node id.`,
    "- Inside the group, draw edges only between its own nodes; a child without an incoming edge starts at once. The group's own edges go to and from the nodes outside it. A For each cannot contain another For each or the report, and the trigger stays outside. When every item has finished, the group's output is `results`: the last output of each item.",
    "",
    "`report`",
    "- Exactly one, at the top level, with no outgoing edges. `template` (optional) is the report text; without it the app writes the report itself (see Reports).",
    "",
    "### Edges",
    "",
    "- Edges are `{\"from\": \"<id>\", \"to\": \"<id>\"}`. A node with several incoming edges waits for all of them; a node whose every incoming path was skipped is skipped too. Cycles are only allowed through loop edges.",
    "- `handle` (`\"true\"` or `\"false\"`) goes only on edges that leave an `if` node, and every such edge needs one.",
    "- A failed node fails its branch: the rest of that branch is skipped and the other items go on. The report is always posted, with the failures listed.",
    "",
    "### Loops",
    "",
    `A loop is an edge with \`loop: {\"maxRounds\": 1-${MAX_ROUNDS_LIMIT}, \"decide\": \"mono\" | "<condition>"}\` that goes back from a node to one of its ancestors in the same group. The source must be a session, command or askMono node, and a node has at most one loop edge. \`maxRounds\` counts passes through the body including the first, so 3 means at most two repeats; when it is reached the loop exits whatever the decision. \`decide: "mono"\` asks you "loop or exit?" after each pass with the outputs so far. With a condition (see Conditions) the loop goes back when it is **true** and exits when it is false, so an approval loop reads \`{{review.verdict}} != "approve"\`. Inside the loop, \`{{run.round}}\` is the current round and the outputs of the previous round stay readable (guard them with \`{{#if node}}...{{/if}}\`, since they are empty in round 1). A node that fails ends the loop for its branch.`,
  );

  add(
    "## Templates",
    "",
    "Prompts, questions, commands, conditions, `forEach.in` and report templates use `{{path}}`. A path is dot separated: `{{inputs.list}}`, `{{fetch.tickets}}`, `{{ticket.title}}`. A missing value renders as an empty string; a list or object renders as indented JSON. The validator rejects a path that names no input, no earlier node and no loop variable.",
    "",
    "Where each variable is readable (prompts, questions, conditions, `forEach.in` and commands):",
    "- `inputs.<name>`: a declared input.",
    "- `run.round`, `run.id`, `run.startedAt`.",
    "- `<nodeId>.<field>` for a node upstream of this one (over non-loop edges), and inside a loop for any node of the loop body. Outputs: a session's declared fields; a command's `exitCode`, `stdout`, `stderr`; an askMono's `answer`, `reason`; a For each's `results`. An unknown field is an issue.",
    "- `<as>.<path>` (default `item`) inside a For each; steps inside it also read everything upstream of the group.",
    "",
    "The report template is narrower. It reads `inputs.*`, `run.round|id|startedAt` plus `run.name`, `run.status`, `run.statusLabel` and `run.endedAt`, `results`, and the outputs of nodes at the top level. It cannot read steps inside a For each or the loop variable; their data reaches the report through `{{results}}` (one entry per item with `name`, `status`, `rounds`, `maxRoundsReached`, `errors`, `output`) or `{{<forEachId>.results}}`. Templates have no loops, so `{{results}}` prints as JSON; when you want a readable list, leave the template out and mark the fields to show with `inReport`.",
    "",
    "`{{#if path}}text{{/if}}` includes `text` only when the value is set: not empty, not false, not zero, not an empty list. It takes one path and nothing else (no comparison, no else, no nesting; use an `if` node for branching). It works in prompts, questions and report templates, and is refused in commands.",
  );

  add(
    "## Conditions",
    "",
    "Used by `if.condition` and by a loop's `decide`. One comparison, no `and` or `or`:",
    "- `<operand> == <operand>` and `<operand> != <operand>`: equal or not. Equality is strict, so the number `3` is not the string `\"3\"`; lists and objects compare by content.",
    "- `<operand> contains <operand>`: a substring of a string, or an item of a list.",
    "- `<operand> empty` and `<operand> not empty`: a missing value, `\"\"`, an empty list or an empty object is empty.",
    "",
    "An operand is `{{path}}`, a double-quoted string (`\"approve\"`, with JSON escapes), a number (`3`, `-1.5`) or `true` or `false`. Examples:",
    "- `{{review.verdict}} != \"approve\"`",
    "- `{{assess.risky}} == true`",
    "- `{{check.issues}} not empty`",
    "- `{{fetch.titles}} contains \"urgent\"`",
    "- `{{tests.stdout}} contains \"FAIL\"`",
    "",
    "A condition that cannot be read fails the node at run time, so run `workflows.validate` and keep to this grammar.",
  );

  add(
    "## Session output and reports",
    "",
    "A session node's `output` is an object of fields: `{\"verdict\": {\"type\": \"string\", \"description\": \"approve or changes\", \"inReport\": true}}`. The type is `string`, `number`, `boolean`, `list` or `object`; the name uses letters, digits and underscores.",
    "",
    "The app appends the contract to the prompt for you: the session must end its last reply with one fenced code block whose info string is `workflow-output` and whose body is a JSON object with every declared field, each of the declared type. Do not write that instruction in the prompt yourself; describe what each field means in its `description` instead. If the block is missing or wrong the session is asked once to fix it, and the node fails if it still does not comply. A session with no output fields needs no block.",
    "",
    "Without a report `template`, the app writes the report: for every For each item (or for the run itself when there is no For each) the status, errors, loop rounds used and the fields of the item's last session node that are shown. `inReport: true` shows a field, `inReport: false` hides it; unset, string fields are shown and the rest are hidden. If the report must contain something from an earlier step of an item, repeat it in a field of the last session, as `pr_url` does in the ticket example below.",
  );

  add(
    "## Command rules",
    "",
    "A command node runs a shell command in the project on macOS and Linux. Windows is not supported: a command step fails there when it is reached. The app single-quotes every value it puts into a command, so write variables **bare**: `git log -- {{inputs.pathspec}}`, never `\"{{inputs.pathspec}}\"` or `'{{inputs.pathspec}}'`. A variable inside quotes is an issue. The validator also refuses a command that:",
    "- uses `{{#if}}`;",
    "- has a backslash or `$` right before a variable, or a backslash at the end of a line;",
    "- uses syntax that cannot be quoted safely around variables: backticks, `$(`, `${`, `$'`, `$[`, `<<` (heredocs), `((`, `[[`, a glob qualifier such as `*(` or `?(`, a `(` glued to the word before it outside quotes, a `#` that starts a word (a comment), or the words `eval`, `let`, `declare` or `typeset`. Put the variable in a simple argument, or let a session do the work.",
    "",
    "Also never feed a variable to `bash -c`, `sh -c` or an array subscript: quoting does not stop the shell from reading its value a second time, and the validator cannot tell these apart from ordinary arguments.",
    "",
    "Dash rule: a value that starts with `-` would be read as an option when it is a word of its own. The check runs when the node starts and fails it. Put `--` before the variable (`rm -- {{inputs.name}}`) or use the `--option={{inputs.name}}` form, where the value follows other text in its word.",
    "",
    "No absolute paths at all in commands: any word starting with `/` or `~/` is refused, except `//` URLs and `/dev/null`, `/dev/stdin`, `/dev/stdout` and `/dev/stderr`. Use relative paths; the command already runs in the project (or in the worktree named by `cwd`).",
  );

  add(
    "## Models",
    "",
    "Leave `harness`, `model` and `effort` out to run a session with your own model. To choose, run `app models.list` and copy the exact ids: `model` is a catalog id such as `claude:sonnet-5` or `claude:opus-5`, not a name like `sonnet`; `harness` is a provider id from the same list and, when given, the model must belong to it; `effort` is one of that model's listed values, and is only checked together with a `model`. An id the app does not know, or one whose provider is not installed, is reported as an issue and the workflow cannot run until it is fixed.",
  );

  add(
    "## Limits",
    "",
    `- A Mono can have at most ${WORKFLOWS_MAX} workflows. The last ${WORKFLOW_RUNS_KEPT} runs of each are kept.`,
    `- \`maxParallel\` 1 to ${MAX_PARALLEL_LIMIT} (default ${DEFAULT_MAX_PARALLEL}); \`maxRounds\` 1 to ${MAX_ROUNDS_LIMIT}.`,
    `- \`timeoutMinutes\` above 0 and at most ${MAX_TIMEOUT_MINUTES} (24 hours); defaults ${DEFAULT_SESSION_TIMEOUT_MINUTES} for a session and ${DEFAULT_COMMAND_TIMEOUT_MINUTES} for a command.`,
    `- Name up to ${WORKFLOW_NAME_MAX} characters, description up to ${WORKFLOW_DESCRIPTION_MAX}, and a definition under 512 KiB.`,
    "- Long text in a node output is cut to keep a run record small.",
  );

  add(
    "## Working with the user",
    "",
    "1. The user describes what they want. Ask only what you cannot infer (which project, which models, how many review rounds, what should happen on failure), one or two questions at a time. Do not ask about things this guide already decides.",
    "2. Propose the steps in plain words in the chat before you build anything: the trigger, each step with its model, what each step reads and produces, the loops and conditions, and what the report says. Wait for the user's go.",
    "3. Write the definition and run `workflows.validate` with it. It saves nothing and returns `issues` (each with a `nodeId` and a message), plus the `commands` and `prompts` the steps would run. Fix the definition and validate again until `issues` is empty.",
    "4. Save it with `workflows.add`. A workflow added this way is always **saved paused**, however clean it is and whatever it says about being enabled. Then tell the user to open it on the Workflows board, review it and enable it there; do not enable it for them. A definition from a yaml file also needs the user's review before it can be enabled or run.",
    "5. For a change (\"make the reviewer Opus\", \"loop at most 2 rounds\"), read the definition with `workflows.get`, change it, validate the full new definition and save it with `workflows.update {\"id\": ..., \"workflow\": {...}}`. It replaces the whole definition and keeps the id, enabled state and run history; a workflow that ends up with issues is paused. The board shows the change live.",
    "6. A finished run posts its report to this chat. If a run fails, read it with `workflows.runs`, explain the cause to the user and fix the definition the same way.",
  );

  add(
    "## Worked examples",
    "",
    "Each of these passes `workflows.validate` as written (with the project input pointing at a real project of the Mono). Adapt them rather than starting from nothing.",
    ...GUIDE_EXAMPLES.flatMap((example) => [
      "",
      `### ${example.name}`,
      "",
      example.why,
      "",
      "```json",
      json(example.workflow),
      "```",
    ]),
  );

  return `${sections.join("\n\n")}\n`;
}
