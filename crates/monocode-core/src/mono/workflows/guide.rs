//! What a Mono reads (`app workflows.guide`) before it builds or changes a workflow. The limits
//! come from the same constants the validator and the engine use, and every worked example is
//! checked against the validator by a test, so the guide cannot drift from the rules.
//!
//! The text is the React app's `workflowGuide.ts`, word for word.
use super::model::{
    DEFAULT_COMMAND_TIMEOUT_MINUTES, DEFAULT_MAX_PARALLEL, DEFAULT_SESSION_TIMEOUT_MINUTES,
    MAX_PARALLEL_LIMIT, MAX_ROUNDS_LIMIT, MAX_TIMEOUT_MINUTES, WORKFLOWS_MAX,
    WORKFLOW_DEFINITION_MAX_BYTES, WORKFLOW_DESCRIPTION_MAX, WORKFLOW_NAME_MAX, WORKFLOW_RUNS_KEPT,
};
use super::ordered::Json;

/// A worked example: a definition without `id`, `createdAt` and `enabled`, as the guide prints it.
pub struct GuideExample {
    pub name: &'static str,
    /// One or two sentences on what the example shows, printed above its JSON.
    pub why: &'static str,
    /// Kept as a `Json` tree so the printed key order is the written one.
    pub workflow: Json,
}

const EXAMPLE_0: &str = r##"{
  "name": "Weekday repo digest",
  "description": "Every weekday morning, summarize what changed in the repo yesterday.",
  "inputs": {
    "project": {
      "type": "project"
    }
  },
  "inputValues": {
    "project": "my-project"
  },
  "nodes": [
    {
      "id": "trigger",
      "type": "schedule",
      "schedule": {
        "scheduleKind": "weekdays",
        "time": "09:00",
        "minute": 0,
        "dayOfWeek": 1
      }
    },
    {
      "id": "digest",
      "type": "session",
      "title": "Write the digest",
      "project": "{{inputs.project}}",
      "model": "claude:sonnet-5",
      "checkout": "root",
      "prompt": "Read the git history of the last day and summarize what changed, grouped by area. Mention anything that looks risky.",
      "output": {
        "summary": {
          "type": "string",
          "description": "The digest, in Markdown"
        }
      }
    },
    {
      "id": "report",
      "type": "report",
      "template": "Repo digest for run {{run.id}}\n\n{{digest.summary}}"
    }
  ],
  "edges": [
    {
      "from": "trigger",
      "to": "digest"
    },
    {
      "from": "digest",
      "to": "report"
    }
  ]
}"##;
const EXAMPLE_1: &str = r##"{
  "name": "Tickets: dev and review",
  "description": "Take open tickets, implement each in its own worktree and review it, up to 3 rounds.",
  "inputs": {
    "project": {
      "type": "project"
    },
    "list": {
      "type": "string",
      "description": "The ticket list to take work from"
    }
  },
  "inputValues": {
    "project": "my-project",
    "list": "Sprint"
  },
  "nodes": [
    {
      "id": "trigger",
      "type": "manual"
    },
    {
      "id": "fetch",
      "type": "session",
      "title": "List tickets",
      "project": "{{inputs.project}}",
      "model": "claude:sonnet-5",
      "checkout": "root",
      "prompt": "List the open tickets assigned to me in {{inputs.list}}. Give each one a title and a url.",
      "output": {
        "tickets": {
          "type": "list",
          "description": "Objects with title and url"
        }
      }
    },
    {
      "id": "each",
      "type": "forEach",
      "title": "Each ticket",
      "in": "{{fetch.tickets}}",
      "as": "ticket",
      "maxParallel": 2
    },
    {
      "id": "dev",
      "type": "session",
      "title": "Dev",
      "parent": "each",
      "project": "{{inputs.project}}",
      "model": "claude:sonnet-5",
      "checkout": "newWorktree",
      "prompt": "Implement the ticket {{ticket.title}} ({{ticket.url}}) and open a pull request.\n{{#if review}}Round {{run.round}}. Fix these review issues first: {{review.issues}}{{/if}}",
      "output": {
        "pr_url": {
          "type": "string",
          "description": "URL of the pull request"
        }
      }
    },
    {
      "id": "review",
      "type": "session",
      "title": "Review",
      "parent": "each",
      "project": "{{inputs.project}}",
      "model": "claude:opus-5",
      "checkout": {
        "worktreeOf": "dev"
      },
      "nestUnder": "dev",
      "prompt": "Review the pull request {{dev.pr_url}} for {{ticket.title}}. Report blocking issues only. Answer approve when there are none.",
      "output": {
        "verdict": {
          "type": "string",
          "description": "approve or changes"
        },
        "issues": {
          "type": "list",
          "inReport": true,
          "description": "Blocking issues, empty when approved"
        },
        "pr_url": {
          "type": "string",
          "description": "The pull request you reviewed"
        }
      }
    },
    {
      "id": "report",
      "type": "report"
    }
  ],
  "edges": [
    {
      "from": "trigger",
      "to": "fetch"
    },
    {
      "from": "fetch",
      "to": "each"
    },
    {
      "from": "dev",
      "to": "review"
    },
    {
      "from": "review",
      "to": "dev",
      "loop": {
        "maxRounds": 3,
        "decide": "{{review.verdict}} != \"approve\""
      }
    },
    {
      "from": "each",
      "to": "report"
    }
  ]
}"##;
const EXAMPLE_2: &str = r##"{
  "name": "Triage recent changes",
  "description": "Look at recent commits under a path and review them in depth only when they look risky.",
  "inputs": {
    "project": {
      "type": "project"
    },
    "pathspec": {
      "type": "string",
      "description": "The folder to look at, relative to the project"
    }
  },
  "inputValues": {
    "project": "my-project",
    "pathspec": "src"
  },
  "nodes": [
    {
      "id": "trigger",
      "type": "manual"
    },
    {
      "id": "recent",
      "type": "command",
      "title": "Recent commits",
      "project": "{{inputs.project}}",
      "command": "git log --oneline --max-count=20 -- {{inputs.pathspec}}",
      "timeoutMinutes": 5
    },
    {
      "id": "assess",
      "type": "session",
      "title": "Assess",
      "project": "{{inputs.project}}",
      "model": "claude:sonnet-5",
      "checkout": "root",
      "prompt": "These are the latest commits under {{inputs.pathspec}}:\n{{recent.stdout}}\nSay in a few sentences what changed and whether any commit is risky.",
      "output": {
        "summary": {
          "type": "string",
          "description": "What changed, in a few sentences"
        },
        "risky": {
          "type": "boolean",
          "description": "True when a commit needs a deeper look"
        }
      }
    },
    {
      "id": "gate",
      "type": "if",
      "title": "Risky?",
      "condition": "{{assess.risky}} == true"
    },
    {
      "id": "deep_review",
      "type": "session",
      "title": "Deep review",
      "project": "{{inputs.project}}",
      "model": "claude:opus-5",
      "checkout": "root",
      "prompt": "Review the risky commits in depth. Context:\n{{assess.summary}}",
      "output": {
        "notes": {
          "type": "string",
          "description": "Findings, most serious first"
        }
      }
    },
    {
      "id": "report",
      "type": "report",
      "template": "Changes under {{inputs.pathspec}}\n\n{{assess.summary}}\n\n{{#if deep_review}}Deep review:\n{{deep_review.notes}}{{/if}}"
    }
  ],
  "edges": [
    {
      "from": "trigger",
      "to": "recent"
    },
    {
      "from": "recent",
      "to": "assess"
    },
    {
      "from": "assess",
      "to": "gate"
    },
    {
      "from": "gate",
      "to": "deep_review",
      "handle": "true"
    },
    {
      "from": "gate",
      "to": "report",
      "handle": "false"
    },
    {
      "from": "deep_review",
      "to": "report"
    }
  ]
}"##;

fn example(raw: &str) -> Json {
    Json::parse(raw).expect("a guide example is valid JSON")
}

pub fn guide_examples() -> Vec<GuideExample> {
    vec![
        GuideExample {
            name: r##"Linear: one session, then the report"##,
            why: r##"A schedule starts one session that writes a digest, and a report template posts it. Use this shape for any single task that should run on a timetable."##,
            workflow: example(EXAMPLE_0),
        },
        GuideExample {
            name: r##"Ticket fan-out: dev, then review in a loop"##,
            why: r##"A session lists tickets, a For each runs a dev session and a review session per ticket, and a loop edge sends the work back to dev until the reviewer approves or 3 rounds have passed. The condition is true to go back, so it reads != approve."##,
            workflow: example(EXAMPLE_1),
        },
        GuideExample {
            name: r##"If on a session output, with a command first"##,
            why: r##"A command reads recent history, a session judges it and sets a boolean, and an If sends risky changes to a deeper review while safe ones go straight to the report. The report template skips the review text when that step did not run."##,
            workflow: example(EXAMPLE_2),
        },
    ]
}

fn add(sections: &mut Vec<String>, lines: &[&str]) {
    sections.push(lines.join("\n"));
}

fn examples_section() -> String {
    let mut lines = vec![
        "## Worked examples".to_string(),
        String::new(),
        "Each of these passes `workflows.validate` as written (with the project input pointing at a real project of the Mono). Adapt them rather than starting from nothing.".to_string(),
    ];
    for example in guide_examples() {
        lines.extend([
            String::new(),
            format!("### {}", example.name),
            String::new(),
            example.why.to_string(),
            String::new(),
            "```json".to_string(),
            example.workflow.stringify(true),
            "```".to_string(),
        ]);
    }
    lines.join("\n")
}

/// The authoring guide, as Markdown.
pub fn workflow_guide() -> String {
    let definition_kib = WORKFLOW_DEFINITION_MAX_BYTES / 1024;
    let mut sections: Vec<String> = Vec::new();
    add(
        &mut sections,
        &[
            r##"# Writing a Monoptah workflow"##,
            "",
            r##"Read this whole guide before you add or change a workflow. Check every definition with `workflows.validate` before you save it."##,
        ],
    );
    add(
        &mut sections,
        &[
            r##"## What a workflow is"##,
            "",
            r##"A workflow is a graph of nodes that the app runs for the user, on a schedule or on demand. Most nodes start a session (an agent run in one of the user's projects); the app passes each node's results to the nodes after it, runs For each groups once per list item, repeats steps in loops, and posts one report to your chat when the run ends. You build and change workflows through the `app workflows.*` actions; the user sees and runs them on the Workflows board."##,
            "",
            r##"A workflow has exactly one trigger, exactly one `report` node reachable from it, and no absolute paths: projects are always inputs, so a workflow can be shared."##,
        ],
    );
    add(
        &mut sections,
        &[
            r##"## The definition"##,
            "",
            r##"`workflows.add` and `workflows.validate` take `{"workflow": {...}}` (or `{"yaml": "..."}` with optional `values`, for an exported file). The workflow object has only these fields; the id, creation time, enabled state and run state belong to the app and are refused if you send them."##,
            "",
            format!(r##"- `name` (required, at most {WORKFLOW_NAME_MAX} characters) and `description` (optional, at most {WORKFLOW_DESCRIPTION_MAX} characters)."##).as_str(),
            r##"- `inputs`: an object of named inputs, each `{"type": "project" | "string" | "number" | "boolean", "description"?}`. Names use letters, digits and underscores."##,
            r##"- `inputValues`: the value of each declared input. A `project` input takes the path or name of one of the Mono's projects. A missing or mistyped value is reported as an issue. Change values later with `workflows.setInputs`."##,
            r##"- `nodes`: a list of nodes (below). `edges`: a list of `{"from", "to", "handle"?, "loop"?}`."##,
            "",
            r##"The app is lenient about extras inside the definition: a misspelled or unknown optional field on a node or edge is ignored without a warning (so a typo such as `timeoutMinute` silently does nothing), and a session output field with an unknown `type` is dropped. An input with a bad name or type is refused with an error. Check the result of `workflows.get` after saving if you are unsure a field was read."##,
            "",
            r##"Every node has `id` and `type`, and may have `title` (a readable label for the board), `parent` (the For each it runs inside) and `ui` (a board position; leave it out, the board places the nodes itself)."##,
            "",
            r##"A node id matches `^[a-z][a-z0-9_]{0,39}$` (lowercase letters, digits and underscores, starting with a letter), is unique, and must not be `run`, `inputs` or `results`, because ids appear in templates as `{{id.field}}`."##,
            "",
            r##"### Projects are inputs"##,
            "",
            r##"Declare an input of type `project`, give it a value in `inputValues`, and write `"project": "{{inputs.<name>}}"` on every session and command node. That exact form is the only one accepted for `project`. No node text may contain an absolute path (a home or system directory such as `/Users/...`, `/home/...`, `/tmp/...`, `~/...`, or a drive like `C:\`); use the project input or a relative path. A bare URL path like `/api/users` in a prompt is fine."##,
            "",
            r##"### Node types"##,
            "",
            r##"`schedule` (trigger)"##,
            r##"- `schedule`: `{"kind": "hourly" | "daily" | "weekdays" | "weekly" | "interval", "time"?: "HH:MM", "minute"?: 0-59, "dayOfWeek"?: 0-6, "hours"?: 1-24}`. `time` is local 24-hour time (default 09:00); `minute` is for hourly; `dayOfWeek` is for weekly (0 is Sunday, default 1); `hours` is required for interval and runs every that many hours from `time` (default 00:00). The stored form `{"scheduleKind", "time", "minute", "dayOfWeek", "intervalHours"?}`, used in the examples below, is accepted too."##,
            "",
            r##"`manual` (trigger)"##,
            r##"- No fields. It runs only when started with `workflows.run` or from the board. A workflow has either one `schedule` or one `manual` trigger, never both. A trigger exposes nothing to templates; use `{{run.startedAt}}` for the start time."##,
            "",
            r##"`session`"##,
            r##"- `project` (required, `{{inputs.<name>}}`), `prompt` (required, a template), `checkout` (required): `"root"` (the project's own checkout), `"newWorktree"` (a fresh worktree) or `{"worktreeOf": "<session node id>"}` (continue in the worktree of an earlier session of the same branch; that node must be a session)."##,
            r##"- `output`: the fields the session reports back (see the output contract below). May be `{}`."##,
            r##"- `harness`, `model`, `effort` (optional): see Models."##,
            format!(r##"- `reuseSession` (default true): in a loop, later rounds continue in the same session instead of starting a new one. `nestUnder` (optional): the id of another session node; this session is shown under it in the sidebar. `timeoutMinutes` (default {DEFAULT_SESSION_TIMEOUT_MINUTES}, above 0, at most {MAX_TIMEOUT_MINUTES})."##).as_str(),
            "",
            r##"`command`"##,
            format!(r##"- `project` (required), `command` (required, a shell command template, see Command rules), `cwd` (optional): `"root"` or `{{"worktreeOf": "<session node id>"}}`, `timeoutMinutes` (default {DEFAULT_COMMAND_TIMEOUT_MINUTES}, at most {MAX_TIMEOUT_MINUTES}). A command that exits with a non-zero code fails the node. Its output is `exitCode`, `stdout` and `stderr`."##).as_str(),
            "",
            r##"`askMono`"##,
            r##"- `question` (required, a template) and `answers` (required, at least one string). The app asks you the question in a short single-decision request and you answer with one of the answers. Its output is `answer` and `reason`."##,
            "",
            r##"`if`"##,
            r##"- Exactly one of `condition` (see Conditions) or `decide: "mono"` (you are asked true or false, with an optional `question` template). Its out edges must carry `handle: "true"` or `handle: "false"`; only the nodes on the matching side run, the others are skipped."##,
            "",
            r##"`switch`"##,
            r##"- `cases` (required, a non-empty list of distinct, non-blank strings; `default` is reserved) and exactly one of `value` (a template such as `{{triage.kind}}`) or `decide: "mono"` (you are asked to pick one case or `default`, with an optional `question` template). The rendered `value` is compared with each case by exact string equality; no match takes `default`. Its out edges must carry `handle: "<case>"` or `handle: "default"`, at most one edge per handle; only the nodes on the taken side run, the others are skipped. Example: `{"id": "route", "type": "switch", "value": "{{triage.kind}}", "cases": ["bug", "feature"]}` with edges `{"from": "route", "to": "fix", "handle": "bug"}`, `{"from": "route", "to": "plan", "handle": "feature"}` and `{"from": "route", "to": "report", "handle": "default"}`."##,
            "",
            r##"`forEach`"##,
            format!(r##"- `in` (required): exactly one list reference such as `{{{{fetch.tickets}}}}`. Its nodes are the nodes that name it in `parent`; they run once per item, up to `maxParallel` items at a time (whole number 1 to {MAX_PARALLEL_LIMIT}, default {DEFAULT_MAX_PARALLEL}). `as` (default `item`) names the item variable, readable as `{{{{<as>.field}}}}` inside the group; it must be letters, digits and underscores, not `run`, `inputs` or `results`, and not a node id."##).as_str(),
            r##"- Inside the group, draw edges only between its own nodes; a child without an incoming edge starts at once. The group's own edges go to and from the nodes outside it. A For each cannot contain another For each or the report, and the trigger stays outside. When every item has finished, the group's output is `results`: the last output of each item."##,
            "",
            r##"`report`"##,
            r##"- Exactly one, at the top level, with no outgoing edges. `template` (optional) is the report text; without it the app writes the report itself (see Session output and reports)."##,
            "",
            r##"### Edges"##,
            "",
            r##"- Edges are `{"from": "<id>", "to": "<id>"}`. A node with several incoming edges waits for all of them; a node whose every incoming path was skipped is skipped too. Cycles are only allowed through loop edges."##,
            r##"- `handle` goes only on edges that leave an `if` node (`"true"` or `"false"`) or a `switch` node (a case or `"default"`), and every such edge needs one."##,
            r##"- A failed node fails its branch: the rest of that branch is skipped and the other items go on. The report is always posted, with the failures listed."##,
            "",
            r##"### Loops"##,
            "",
            format!(r##"A loop is an edge with `loop: {{"maxRounds": 1-{MAX_ROUNDS_LIMIT}, "decide": "mono" | "<condition>"}}` that goes back from a node to one of its ancestors in the same group. The source must be a session, command or askMono node, and a node has at most one loop edge. `maxRounds` counts passes through the body including the first, so 3 means at most two repeats; when it is reached the loop exits whatever the decision. `decide: "mono"` asks you "loop or exit?" after each pass with the outputs so far. With a condition (see Conditions) the loop goes back when it is **true** and exits when it is false, so an approval loop reads `{{{{review.verdict}}}} != "approve"`. Inside the loop, `{{{{run.round}}}}` is the current round and the outputs of the previous round stay readable (guard them with `{{{{#if node}}}}...{{{{/if}}}}`, since they are empty in round 1). A node that fails ends the loop for its branch."##).as_str(),
        ],
    );
    add(
        &mut sections,
        &[
            r##"## Templates"##,
            "",
            r##"Prompts, questions, commands, conditions, `switch.value`, `forEach.in` and report templates use `{{path}}`. A path is dot separated: `{{inputs.list}}`, `{{fetch.tickets}}`, `{{ticket.title}}`. A missing value renders as an empty string; a list or object renders as indented JSON. The validator rejects a path that names no input, no earlier node and no loop variable."##,
            "",
            r##"Where each variable is readable (prompts, questions, conditions, `forEach.in` and commands):"##,
            r##"- `inputs.<name>`: a declared input."##,
            r##"- `run.round`, `run.id`, `run.startedAt`."##,
            r##"- `<nodeId>.<field>` for a node upstream of this one (over non-loop edges), and inside a loop for any node of the loop body. Outputs: a session's declared fields; a command's `exitCode`, `stdout`, `stderr`; an askMono's `answer`, `reason`; a For each's `results`. An unknown field is an issue."##,
            r##"- `<as>.<path>` (default `item`) inside a For each; steps inside it also read everything upstream of the group."##,
            "",
            r##"The report template is narrower. It reads `inputs.*`, `run.round|id|startedAt` plus `run.name`, `run.status`, `run.statusLabel` and `run.endedAt`, `results`, and the outputs of top-level nodes that are upstream of the report. It cannot read steps inside a For each or the loop variable; their data reaches the report through `{{results}}` or `{{<forEachId>.results}}`. `{{results}}` is a list with one entry per piece of work: `branch`, `name`, `status`, `rounds`, `maxRoundsReached`, `errors` and `output` (the item's last session output). It starts with a `Run` entry for the work outside any For each, and has extra entries for earlier loop rounds that failed or ran out of rounds. Templates have no loops, so `{{results}}` prints as JSON; when you want a readable list, leave the template out and mark the fields to show with `inReport`."##,
            "",
            r##"`{{#if path}}text{{/if}}` includes `text` only when the value is set: not empty, not false, not zero, not an empty list. It takes one path and nothing else (no comparison, no else, no nesting; use an `if` or `switch` node for branching). It works in prompts, questions and report templates, and is refused in commands."##,
        ],
    );
    add(
        &mut sections,
        &[
            r##"## Conditions"##,
            "",
            r##"Used by `if.condition` and by a loop's `decide`. One comparison, no `and` or `or`:"##,
            r##"- `<operand> == <operand>` and `<operand> != <operand>`: equal or not. Equality is strict, so the number `3` is not the string `"3"`; lists and objects compare by content."##,
            r##"- `<operand> contains <operand>`: a substring of a string, or an item of a list."##,
            r##"- `<operand> empty` and `<operand> not empty`: a missing value, `""`, an empty list or an empty object is empty."##,
            "",
            r##"An operand is `{{path}}`, a double-quoted string (`"approve"`, with JSON escapes), a number (`3`, `-1.5`) or `true` or `false`. Examples:"##,
            r##"- `{{review.verdict}} != "approve"`"##,
            r##"- `{{assess.risky}} == true`"##,
            r##"- `{{check.issues}} not empty`"##,
            r##"- `{{fetch.titles}} contains "urgent"`"##,
            r##"- `{{tests.stdout}} contains "FAIL"`"##,
            "",
            r##"`workflows.validate` checks every condition against this grammar and reports one it cannot read (for example an unquoted word, single quotes, `and`, or an unknown operator) as an issue on the node or loop edge. An unreadable condition would otherwise fail the node when the run reaches it."##,
        ],
    );
    add(
        &mut sections,
        &[
            r##"## Session output and reports"##,
            "",
            r##"A session node's `output` is an object of fields: `{"verdict": {"type": "string", "description": "approve or changes", "inReport": true}}`. The type is `string`, `number`, `boolean`, `list` or `object`; the name uses letters, digits and underscores."##,
            "",
            r##"The app appends the contract to the prompt for you: the session must end its last reply with one fenced code block whose info string is `workflow-output` and whose body is a JSON object with every declared field, each of the declared type. Do not write that instruction in the prompt yourself; describe what each field means in its `description` instead. If the block is missing or wrong the session is asked once to fix it, and the node fails if it still does not comply. A session with no output fields needs no block."##,
            "",
            r##"Without a report `template`, the app writes the report: for every For each item the status, errors, loop rounds used and the reported fields of the item's last session node, plus a `Run` entry for the work outside any For each (shown when it failed or has something to show; next to a For each it lists the reported fields of every top-level session). `inReport: true` shows a field, `inReport: false` hides it; unset, string fields are shown and the rest are hidden. If the report must contain something from an earlier step of an item, repeat it in a field of the last session, as `pr_url` does in the ticket example below."##,
            "",
            r##"With a report `template`, the report body is only your template. Failures, errors and loop notes are not added: they appear only if the template prints `{{results}}`. The report title (the workflow name and the run status, such as completed with failures) is always added, so the status stays visible."##,
        ],
    );
    add(
        &mut sections,
        &[
            r##"## Command rules"##,
            "",
            r##"A command node runs a shell command in the project on macOS and Linux. Windows is not supported: a command step fails there when it is reached. The app single-quotes every value it puts into a command, so write variables **bare**: `git log -- {{inputs.pathspec}}`, never `"{{inputs.pathspec}}"` or `'{{inputs.pathspec}}'`. A variable inside quotes is an issue. The validator also refuses a command that:"##,
            r##"- uses `{{#if}}`;"##,
            r##"- has a backslash or `$` right before a variable, or a backslash at the end of a line;"##,
            r##"- uses syntax that cannot be quoted safely around variables: backticks, `$(`, `${`, `$'`, `$[`, `<<` (heredocs), `((`, `[[`, a glob qualifier such as `*(` or `?(`, a `(` glued to the word before it outside quotes, a `#` that starts a word (a comment), or the words `eval`, `let`, `declare` or `typeset`. Put the variable in a simple argument, or let a session do the work."##,
            "",
            r##"Also never feed a variable to `bash -c`, `sh -c` or an array subscript: quoting does not stop the shell from reading its value a second time, and the validator cannot tell these apart from ordinary arguments."##,
            "",
            r##"Dash rule: a value that starts with `-` would be read as an option when it is a word of its own. The check runs when the node starts and fails it. Put `--` before the variable (`rm -- {{inputs.name}}`) or use the `--option={{inputs.name}}` form, where the value follows other text in its word."##,
            "",
            r##"No absolute paths at all in commands: any word starting with `/` or `~/` is refused, except `//` URLs and `/dev/null`, `/dev/stdin`, `/dev/stdout` and `/dev/stderr`. Use relative paths; the command already runs in the project (or in the worktree named by `cwd`)."##,
        ],
    );
    add(
        &mut sections,
        &[
            r##"## Models"##,
            "",
            r##"Leave `harness`, `model` and `effort` out to run a session with your own model. To choose, run `app models.list` and copy the exact ids: `model` is a catalog id such as `claude:sonnet-5` or `claude:opus-5`, not a name like `sonnet`; `harness` is a provider id from the same list and, when given, the model must belong to it; `effort` is one of that model's listed values, and is only checked together with a `model`. An id the app does not know, or one whose provider is not installed, is reported as an issue and the workflow cannot run until it is fixed."##,
        ],
    );
    add(
        &mut sections,
        &[
            r##"## Limits"##,
            "",
            format!(r##"- A Mono can have at most {WORKFLOWS_MAX} workflows. The last {WORKFLOW_RUNS_KEPT} runs of each are kept."##).as_str(),
            format!(r##"- `maxParallel` 1 to {MAX_PARALLEL_LIMIT} (default {DEFAULT_MAX_PARALLEL}); `maxRounds` 1 to {MAX_ROUNDS_LIMIT}."##).as_str(),
            format!(r##"- `timeoutMinutes` above 0 and at most {MAX_TIMEOUT_MINUTES} (24 hours); defaults {DEFAULT_SESSION_TIMEOUT_MINUTES} for a session and {DEFAULT_COMMAND_TIMEOUT_MINUTES} for a command."##).as_str(),
            format!(r##"- Name up to {WORKFLOW_NAME_MAX} characters, description up to {WORKFLOW_DESCRIPTION_MAX}, and a definition under {definition_kib} KiB."##).as_str(),
            r##"- The workflow limit applies to new workflows. Checking a change to an existing one (`workflows.validate` with its `id`) does not count against it."##,
            r##"- Long text in a node output is cut to keep a run record small."##,
        ],
    );
    add(
        &mut sections,
        &[
            r##"## Working with the user"##,
            "",
            r##"1. The user describes what they want. Ask only what you cannot infer (which project, which models, how many review rounds, what should happen on failure), one or two questions at a time. Do not ask about things this guide already decides."##,
            r##"2. Propose the steps in plain words in the chat before you build anything: the trigger, each step with its model, what each step reads and produces, the loops and conditions, and what the report says. Wait for the user's go."##,
            r##"3. Write the definition and run `workflows.validate` with it. It saves nothing and returns `issues`, plus the `commands` and `prompts` the steps would run. An issue has a `message` and, when it belongs to one node, a `nodeId`; problems with the graph as a whole, the inputs or the workflow limit have no `nodeId`. Fix the definition and validate again until `issues` is empty. When you are checking a change to an existing workflow, also pass its `id`."##,
            r##"4. Save it with `workflows.add`. A workflow added this way is always **saved paused**, however clean it is and whatever it says about being enabled. Then tell the user to open it on the Workflows board, review it and enable it there; do not enable it for them. A definition from a yaml file also needs the user's review before it can be enabled or run."##,
            r##"5. For a change ("make the reviewer Opus", "loop at most 2 rounds"), read the definition with `workflows.get`, change it, validate the full new definition with `workflows.validate {"id": ..., "workflow": {...}}` and save it with `workflows.update {"id": ..., "workflow": {...}}`. `workflows.get` also returns `id`, `enabled`, `trigger`, `running`, `issues` and `runs`; send back only the definition fields `name`, `description`, `inputs`, `inputValues`, `nodes` and `edges`, because any other field is refused (leave `enabled` out too; the user enables a workflow). `workflows.update` replaces the whole definition and keeps the id, enabled state and run history; a workflow that ends up with issues is paused. The board shows the change live."##,
            r##"6. A finished run posts its report to this chat. If a run fails, read it with `workflows.runs`, explain the cause to the user and fix the definition the same way."##,
        ],
    );
    sections.push(examples_section());

    format!("{}\n", sections.join("\n\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mono::workflows::model::{NodeKind, Workflow, NODE_TYPES};
    use crate::mono::workflows::models::{
        workflow_issues, CatalogHarness, CatalogModel, ModelCatalog,
    };
    use crate::mono::workflows::ordered::OrderedMap;
    use crate::mono::workflows::parse::parse_workflow;
    use crate::provider::Kind;

    /// Claude installed with the two models the examples name; nothing else.
    fn catalog() -> ModelCatalog {
        let models = ["claude-sonnet-5", "claude-opus-5"]
            .map(|id| CatalogModel {
                id: id.into(),
                alias_target: None,
                efforts: vec![],
            })
            .to_vec();
        ModelCatalog {
            probed: true,
            harnesses: Kind::ALL
                .into_iter()
                .map(|kind| CatalogHarness {
                    id: kind.id().into(),
                    installed: kind == Kind::Claude,
                    models: (kind == Kind::Claude).then(|| models.clone()),
                })
                .collect(),
        }
    }

    /// The input values an example needs: a path for each project input, a word for the rest.
    fn values_for(inputs: &Json) -> Json {
        let mut values = OrderedMap::new();
        for (name, input) in inputs.as_object().unwrap().iter() {
            let value = match input.get("type").and_then(Json::as_str) {
                Some("project") => Json::String("/x".into()),
                Some("number") => Json::Number(1.0),
                Some("boolean") => Json::Bool(true),
                _ => Json::String("x".into()),
            };
            values.insert(name.clone(), value);
        }
        Json::Object(values)
    }

    fn as_workflow(example: &GuideExample) -> Workflow {
        let Json::Object(mut fields) = example.workflow.clone() else {
            panic!("example is not an object")
        };
        fields.insert("id", Json::String("wf-aaaaaaaa".into()));
        fields.insert("createdAt", Json::Number(0.0));
        fields.insert("enabled", Json::Bool(false));
        let values = values_for(fields.get("inputs").unwrap());
        fields.insert("inputValues", values);
        parse_workflow(&Json::Object(fields)).expect("example parses as a workflow")
    }

    /// The whole number written right after `marker` in the guide.
    fn number_after(guide: &str, marker: &str) -> u64 {
        let at = guide
            .find(marker)
            .unwrap_or_else(|| panic!("missing in guide: {marker}"));
        guide[at + marker.len()..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>()
            .parse()
            .unwrap_or_else(|_| panic!("no number after: {marker}"))
    }

    #[test]
    fn has_at_least_the_three_required_examples() {
        let examples = guide_examples();
        assert!(examples.len() >= 3);
        let names = examples
            .iter()
            .map(|e| e.name)
            .collect::<Vec<_>>()
            .join(" | ");
        assert!(names.to_lowercase().contains("linear"));
        let lower = names.to_lowercase();
        assert!(lower.contains("fan-out") || lower.contains("ticket"));
        assert!(names
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .any(|word| word == "If"));
    }

    #[test]
    fn every_guide_example_validates() {
        for example in guide_examples() {
            let issues = workflow_issues(&as_workflow(&example), &catalog());
            assert_eq!(issues, [], "{}", example.name);
        }
    }

    #[test]
    fn guide_embeds_every_example_and_the_limits() {
        let guide = workflow_guide();
        for example in guide_examples() {
            assert!(
                guide.contains(&example.workflow.stringify(true)),
                "{}",
                example.name
            );
            assert!(guide.contains(example.why), "{}", example.name);
        }
        assert!(guide.contains(&format!("at most {WORKFLOWS_MAX} workflows")));
        assert!(guide.contains(&format!("last {WORKFLOW_RUNS_KEPT} runs")));
        assert!(guide.contains(&format!(
            "1 to {MAX_PARALLEL_LIMIT} (default {DEFAULT_MAX_PARALLEL})"
        )));
        assert!(guide.contains(&format!("`maxRounds` 1 to {MAX_ROUNDS_LIMIT}")));
        assert!(guide.contains(&format!("at most {MAX_TIMEOUT_MINUTES} (24 hours)")));
        assert!(guide.contains(&format!(
            "defaults {DEFAULT_SESSION_TIMEOUT_MINUTES} for a session and {DEFAULT_COMMAND_TIMEOUT_MINUTES} for a command"
        )));
        assert!(guide.contains(&format!(
            "under {} KiB",
            WORKFLOW_DEFINITION_MAX_BYTES / 1024
        )));
        assert!(guide.contains(&format!("at most {WORKFLOW_NAME_MAX} characters")));
        assert!(guide.contains(&format!("at most {WORKFLOW_DESCRIPTION_MAX} characters")));
    }

    #[test]
    fn every_interpolated_limit_is_the_model_constant() {
        let guide = workflow_guide();
        let cases: [(&str, u64); 15] = [
            ("A Mono can have at most ", WORKFLOWS_MAX as u64),
            ("The last ", WORKFLOW_RUNS_KEPT as u64),
            ("`maxParallel` 1 to ", MAX_PARALLEL_LIMIT.into()),
            ("whole number 1 to ", MAX_PARALLEL_LIMIT.into()),
            (
                &format!("1 to {MAX_PARALLEL_LIMIT} (default "),
                DEFAULT_MAX_PARALLEL.into(),
            ),
            ("`maxRounds` 1 to ", MAX_ROUNDS_LIMIT.into()),
            ("\"maxRounds\": 1-", MAX_ROUNDS_LIMIT.into()),
            (
                "`timeoutMinutes` above 0 and at most ",
                MAX_TIMEOUT_MINUTES.into(),
            ),
            ("defaults ", DEFAULT_SESSION_TIMEOUT_MINUTES.into()),
            (
                &format!("defaults {DEFAULT_SESSION_TIMEOUT_MINUTES} for a session and "),
                DEFAULT_COMMAND_TIMEOUT_MINUTES.into(),
            ),
            (
                "`timeoutMinutes` (default ",
                DEFAULT_SESSION_TIMEOUT_MINUTES.into(),
            ),
            (
                "a definition under ",
                (WORKFLOW_DEFINITION_MAX_BYTES / 1024) as u64,
            ),
            (
                "`description` (optional, at most ",
                WORKFLOW_DESCRIPTION_MAX as u64,
            ),
            ("`name` (required, at most ", WORKFLOW_NAME_MAX as u64),
            ("Name up to ", WORKFLOW_NAME_MAX as u64),
        ];
        for (marker, expected) in cases {
            assert_eq!(number_after(&guide, marker), expected, "{marker}");
        }
    }

    #[test]
    fn examples_use_catalog_model_ids() {
        let mut models = Vec::new();
        for example in guide_examples() {
            for node in &as_workflow(&example).nodes {
                if let NodeKind::Session {
                    model: Some(model), ..
                } = &node.kind
                {
                    models.push(model.clone());
                }
            }
        }
        assert!(!models.is_empty());
        for model in models {
            assert!(["claude:sonnet-5", "claude:opus-5"].contains(&model.as_str()));
        }
    }

    #[test]
    fn example_json_is_what_the_react_guide_prints() {
        for (example, raw) in guide_examples()
            .iter()
            .zip([EXAMPLE_0, EXAMPLE_1, EXAMPLE_2])
        {
            assert_eq!(example.workflow.stringify(true), raw, "{}", example.name);
        }
    }

    #[test]
    fn guide_mentions_every_node_type() {
        let guide = workflow_guide();
        for ty in NODE_TYPES {
            assert!(guide.contains(&format!("`{ty}`")), "{ty}");
        }
    }

    #[test]
    fn guide_states_the_points_reviewers_asked_for() {
        let guide = workflow_guide();
        for point in [
            "only your template",
            "only if the template prints `{{results}}`",
            "upstream of the report",
            "`Run` entry",
            "problems with the graph as a whole",
            "pass its `id`",
            "does not count against it",
            "`name`, `description`, `inputs`, `inputValues`, `nodes` and `edges`",
            "checks every condition against this grammar",
        ] {
            assert!(guide.contains(point), "{point}");
        }
    }

    #[test]
    fn guide_covers_the_rules_the_validator_enforces() {
        let guide = workflow_guide();
        for topic in [
            "app models.list",
            "workflows.validate",
            "workflows.add",
            "workflows.update",
            "saved paused",
            "workflow-output",
            "inReport",
            "{{#if",
            "not empty",
            "loop",
            "maxRounds",
            "handle",
            "results",
            "Windows",
            "absolute path",
            "bare",
            "--",
            "eval",
            "backtick",
        ] {
            assert!(guide.contains(topic), "{topic}");
        }
        // A loop condition is true to go back for another round, so an approval loop uses !=.
        assert!(guide.contains(r#"{{review.verdict}} != "approve""#));
    }

    #[test]
    fn guide_has_no_dashes() {
        let guide = workflow_guide();
        assert!(!guide.contains('\u{2014}'));
        assert!(!guide.contains('\u{2013}'));
    }

    #[test]
    fn guide_is_deterministic() {
        assert_eq!(workflow_guide(), workflow_guide());
    }
}
