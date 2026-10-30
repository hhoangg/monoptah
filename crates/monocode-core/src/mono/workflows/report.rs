//! What a finished run tells the Mono: the report the app composes itself, plus the prompt and
//! reply parsing for the questions the engine puts to the Mono (Ask Mono nodes, Mono-decided Ifs
//! and loops). Texts are copied from the React app; the Mono reads them.
use super::engine::{run_scope, EngineAction, BRANCH_FAILED_REASON, MAX_ROUNDS_REASON};
use super::model::{
    archived_branch, archived_items_key, parse_branch, NodeKind, NodeRun, NodeStatus, OutputField,
    OutputFieldType, RunStatus, Workflow, WorkflowRun,
};
use super::ordered::{js_number_text, Json, OrderedMap};
use super::output::{parse_workflow_output, OUTPUT_FENCE};
use super::template::{js_space, js_trim, render_template};
use std::collections::{BTreeMap, BTreeSet};

fn status_name(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Running => "running",
        RunStatus::Done => "done",
        RunStatus::DoneWithFailures => "doneWithFailures",
        RunStatus::Failed => "failed",
        RunStatus::Stopped => "stopped",
        RunStatus::Interrupted => "interrupted",
    }
}

fn status_label(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Running => "running",
        RunStatus::Done => "completed",
        RunStatus::DoneWithFailures => "completed with failures",
        RunStatus::Failed => "failed",
        RunStatus::Stopped => "stopped",
        RunStatus::Interrupted => "interrupted",
    }
}

/// A string longer than this (in UTF-16 units, like JS) is shown as a quoted block, not inline.
const INLINE_STRING_MAX: usize = 200;
const ACTION_TYPES: [&str; 5] = ["session", "command", "askMono", "if", "switch"];

struct BranchResult {
    branch: String,
    name: String,
    status: &'static str,
    rounds: u32,
    max_rounds_reached: bool,
    /// One entry per failed node.
    errors: Vec<String>,
    /// Nodes whose loop ended because the branch had failed.
    loops_cut_short: Vec<String>,
    /// Output of the branch's last session node, whole (templates may use any field).
    output: OrderedMap<Json>,
    /// The fields of that output the report shows.
    shown: Vec<(String, Json)>,
}

/// `text.replace(/\s*[\r\n]+\s*/g, " ").trim()`: every white space run holding a line break
/// becomes one space.
fn one_line(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if !js_space(c) {
            out.push(c);
            continue;
        }
        let mut run = String::from(c);
        while let Some(&next) = chars.peek() {
            if !js_space(next) {
                break;
            }
            run.push(next);
            chars.next();
        }
        if run.contains(['\r', '\n']) {
            out.push(' ');
        } else {
            out.push_str(&run);
        }
    }
    js_trim(&out).to_string()
}

/// A fence longer than any backtick run in `body`, so the body cannot close it early.
fn fence_for(body: &str) -> String {
    let (mut longest, mut run) = (0, 0);
    for c in body.chars() {
        run = if c == '`' { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    "`".repeat(3.max(longest + 1))
}

fn node_label(run: &WorkflowRun, node_id: &str) -> String {
    run.definition
        .nodes
        .iter()
        .find(|n| n.id == node_id)
        .and_then(|n| n.title.clone())
        .unwrap_or_else(|| node_id.to_string())
}

/// "title" or "name" of a For each item when it is an object that has one; else "#<n>".
fn item_label(item: Option<&Json>, index: usize) -> String {
    if let Some(Json::Object(record)) = item {
        for key in ["title", "name"] {
            if let Some(Json::String(value)) = record.get(key) {
                if !js_trim(value).is_empty() {
                    return one_line(value);
                }
            }
        }
    }
    format!("#{}", index + 1)
}

fn branch_status(runs: &[&NodeRun]) -> &'static str {
    if runs.is_empty() {
        return "pending";
    }
    let has = |status: NodeStatus| runs.iter().any(|r| r.status == status);
    if has(NodeStatus::Failed) {
        "failed"
    } else if has(NodeStatus::Stopped) {
        "stopped"
    } else if has(NodeStatus::WaitingApproval) {
        "waiting for approval"
    } else if has(NodeStatus::Running) {
        "running"
    } else if has(NodeStatus::Pending) {
        "pending"
    } else if has(NodeStatus::Done) {
        "done"
    } else {
        "skipped"
    }
}

fn finished_at(r: &NodeRun) -> u64 {
    r.ended_at.or(r.started_at).unwrap_or(0)
}

fn session_fields<'a>(run: &'a WorkflowRun, r: &NodeRun) -> Option<&'a OrderedMap<OutputField>> {
    match &run
        .definition
        .nodes
        .iter()
        .find(|n| n.id == r.node_id)?
        .kind
    {
        NodeKind::Session { output, .. } => Some(output),
        _ => None,
    }
}

/// The session node run that finished last in the branch and produced output.
fn last_session_run<'a>(run: &WorkflowRun, runs: &[&'a NodeRun]) -> Option<&'a NodeRun> {
    let mut best: Option<&NodeRun> = None;
    for r in runs {
        if session_fields(run, r).is_none() || r.status != NodeStatus::Done || r.output.is_none() {
            continue;
        }
        if best.is_none_or(|b| finished_at(r) >= finished_at(b)) {
            best = Some(r);
        }
    }
    best
}

/// Whether the report shows a field: `inReport` when declared, else on for strings and off for
/// the rest.
pub fn field_in_report(field: &OutputField) -> bool {
    field
        .in_report
        .unwrap_or(field.ty == OutputFieldType::String)
}

/// Fields the report shows.
fn reported_fields(
    fields: &OrderedMap<OutputField>,
    output: &OrderedMap<Json>,
) -> Vec<(String, Json)> {
    fields
        .iter()
        .filter(|(_, field)| field_in_report(field))
        .filter_map(|(name, _)| Some((name.clone(), output.get(name)?.clone())))
        .collect()
}

/// Reported fields of every finished session node in the branch, as `<node>.<field>`, in finish
/// order.
fn all_session_fields(run: &WorkflowRun, runs: &[&NodeRun]) -> Vec<(String, Json)> {
    let mut finished: Vec<&NodeRun> = runs
        .iter()
        .copied()
        .filter(|r| r.status == NodeStatus::Done && r.output.is_some())
        .collect();
    finished.sort_by_key(|r| finished_at(r));
    let empty = OrderedMap::new();
    finished
        .into_iter()
        .filter_map(|r| {
            let fields = session_fields(run, r)?;
            let output = r.output.as_ref().unwrap_or(&empty);
            Some(
                reported_fields(fields, output)
                    .into_iter()
                    .map(|(name, value)| (format!("{}.{name}", r.node_id), value))
                    .collect::<Vec<_>>(),
            )
        })
        .flatten()
        .collect()
}

fn node_type(run: &WorkflowRun, node_id: &str) -> &'static str {
    run.definition
        .nodes
        .iter()
        .find(|n| n.id == node_id)
        .map_or("", |n| n.kind.type_name())
}

fn result_of(run: &WorkflowRun, branch: &str, name: String, all_sessions: bool) -> BranchResult {
    let loop_nodes: BTreeSet<&str> = run
        .definition
        .edges
        .iter()
        .filter(|e| e.loop_spec.is_some())
        .map(|e| e.from.as_str())
        .collect();
    let runs: Vec<&NodeRun> = run
        .nodes
        .iter()
        .filter(|r| r.branch == branch && ACTION_TYPES.contains(&node_type(run, &r.node_id)))
        .collect();
    let last = last_session_run(run, &runs);
    let output = last.and_then(|r| r.output.clone()).unwrap_or_default();
    let shown = if all_sessions {
        all_session_fields(run, &runs)
    } else {
        last.and_then(|r| session_fields(run, r))
            .map(|fields| reported_fields(fields, &output))
            .unwrap_or_default()
    };
    let reason_is = |r: &NodeRun, reason: &str| {
        r.decision
            .as_ref()
            .is_some_and(|d| d.reason.as_deref() == Some(reason))
    };
    BranchResult {
        branch: branch.to_string(),
        name,
        status: branch_status(&runs),
        rounds: runs
            .iter()
            .filter(|r| loop_nodes.contains(r.node_id.as_str()))
            .map(|r| r.round)
            .max()
            .unwrap_or(0),
        max_rounds_reached: runs.iter().any(|r| reason_is(r, MAX_ROUNDS_REASON)),
        errors: runs
            .iter()
            .filter(|r| r.status == NodeStatus::Failed)
            .map(|r| {
                // `r.error ?? "failed"`: an empty text stays empty.
                let error = r.error.as_deref().unwrap_or("failed");
                format!("{}: {}", node_label(run, &r.node_id), one_line(error))
            })
            .collect(),
        loops_cut_short: runs
            .iter()
            .filter(|r| reason_is(r, BRANCH_FAILED_REASON))
            .map(|r| node_label(run, &r.node_id))
            .collect(),
        output,
        shown,
    }
}

/// The indices of a For each's archived branches by loop round, from the branches the run has and
/// from the item lists it archived. A round is known by either, so a group whose current list is
/// gone or shorter still reports what failed in an earlier round.
fn archived_indices(run: &WorkflowRun, group_id: &str) -> BTreeMap<u32, BTreeSet<usize>> {
    let mut rounds: BTreeMap<u32, BTreeSet<usize>> = BTreeMap::new();
    for r in &run.nodes {
        if let Some(parts) = parse_branch(&r.branch) {
            if let Some(round) = parts.round {
                if parts.group_id == group_id {
                    rounds.entry(round).or_default().insert(parts.index);
                }
            }
        }
    }
    let prefix = format!("{group_id}@r");
    for (key, list) in &run.items {
        let Some(digits) = key.strip_prefix(&prefix) else {
            continue;
        };
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let Ok(round) = digits.parse::<u32>() else {
            continue;
        };
        rounds.entry(round).or_default().extend(0..list.len());
    }
    rounds
}

struct Collected {
    results: Vec<BranchResult>,
    empty: Vec<String>,
    groups: usize,
}

/// Branch results in report order, plus the For each nodes that had nothing to run.
fn collect(run: &WorkflowRun) -> Collected {
    let groups: Vec<_> = run
        .definition
        .nodes
        .iter()
        .filter(|n| matches!(n.kind, NodeKind::ForEach { .. }))
        .collect();
    let mut results = Vec::new();
    let mut empty = Vec::new();
    for group in &groups {
        let items = run.items.get(&group.id);
        let archived = archived_indices(run, &group.id);
        if items.is_none() && archived.is_empty() {
            continue;
        }
        if items.is_some_and(|list| list.is_empty()) {
            empty.push(node_label(run, &group.id));
        }
        let current: &[Json] = items.map_or(&[], Vec::as_slice);
        let archived_max = archived
            .values()
            .flatten()
            .map(|i| *i as i64)
            .max()
            .unwrap_or(-1);
        let last = (current.len() as i64 - 1).max(archived_max);
        for index in 0..=last.max(-1) {
            let index = index as usize;
            let key = format!("{}#{index}", group.id);
            if index < current.len() {
                results.push(result_of(
                    run,
                    &key,
                    item_label(Some(&current[index]), index),
                    false,
                ));
            }
            // Earlier loop rounds are archived. They only matter when something went wrong in them.
            for (round, known) in &archived {
                if !known.contains(&index) {
                    continue;
                }
                let old = run
                    .items
                    .get(&archived_items_key(&group.id, *round))
                    .and_then(|list| list.get(index));
                let earlier = result_of(
                    run,
                    &archived_branch(&key, *round),
                    format!("{} · round {round}", item_label(old, index)),
                    false,
                );
                if earlier.status == "failed"
                    || earlier.max_rounds_reached
                    || !earlier.errors.is_empty()
                {
                    results.push(earlier);
                }
            }
        }
    }
    // Work outside any For each. With no For each at all this is the whole run.
    // Next to a For each it also lists the outputs of steps before or after it (e.g. a summary).
    let top = result_of(run, "", "Run".to_string(), !groups.is_empty());
    if groups.is_empty()
        || top.status == "failed"
        || !top.errors.is_empty()
        || !top.shown.is_empty()
    {
        results.insert(0, top);
    }
    Collected {
        results,
        empty,
        groups: groups.len(),
    }
}

/// `String(value)` for a scalar, `JSON.stringify(value, null, 2)` for a list or object.
fn render_value(name: &str, value: &Json) -> Vec<String> {
    match value {
        Json::String(text) => {
            if text.encode_utf16().count() <= INLINE_STRING_MAX {
                return vec![format!("- {name}: {}", one_line(text))];
            }
            let mut lines = vec![format!("- {name}:")];
            // `split(/\r?\n/)`
            let parts: Vec<&str> = text.split('\n').collect();
            let last = parts.len() - 1;
            for (i, part) in parts.into_iter().enumerate() {
                let line = if i < last {
                    part.strip_suffix('\r').unwrap_or(part)
                } else {
                    part
                };
                lines.push(format!("  > {line}"));
            }
            lines
        }
        Json::Null => vec![format!("- {name}: null")],
        Json::Bool(b) => vec![format!("- {name}: {b}")],
        Json::Number(n) => vec![format!("- {name}: {}", js_number_text(*n))],
        Json::Array(_) | Json::Object(_) => {
            let json = value.stringify(true);
            let fence = fence_for(&json);
            let mut lines = vec![format!("- {name}:"), format!("  {fence}json")];
            lines.extend(json.split('\n').map(|line| format!("  {line}")));
            lines.push(format!("  {fence}"));
            lines
        }
    }
}

fn render_branch(result: &BranchResult) -> String {
    let mut lines = vec![
        format!("### {}", result.name),
        format!("- Status: {}", result.status),
    ];
    if result.rounds > 0 {
        lines.push(format!("- Loop rounds used: {}", result.rounds));
    }
    if result.max_rounds_reached {
        lines.push("- Max rounds reached".to_string());
    }
    for node in &result.loops_cut_short {
        lines.push(format!("- Loop after {node} ended: branch failed"));
    }
    for error in &result.errors {
        lines.push(format!("- Error in {error}"));
    }
    for (name, value) in &result.shown {
        lines.extend(render_value(name, value));
    }
    lines.join("\n")
}

/// A composed report: the title and the Markdown to post.
#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    pub title: String,
    pub text: String,
}

fn results_json(results: &[BranchResult]) -> Json {
    Json::Array(
        results
            .iter()
            .map(|r| {
                let mut entry = OrderedMap::new();
                entry.insert("branch", Json::String(r.branch.clone()));
                entry.insert("name", Json::String(r.name.clone()));
                entry.insert("status", Json::String(r.status.to_string()));
                entry.insert("rounds", Json::Number(f64::from(r.rounds)));
                entry.insert("maxRoundsReached", Json::Bool(r.max_rounds_reached));
                entry.insert(
                    "errors",
                    Json::Array(r.errors.iter().cloned().map(Json::String).collect()),
                );
                entry.insert("output", Json::Object(r.output.clone()));
                Json::Object(entry)
            })
            .collect(),
    )
}

/// The report template of a definition, when it has one that is not blank.
fn report_template(definition: &Workflow) -> Option<&str> {
    let node = definition
        .nodes
        .iter()
        .find(|n| matches!(n.kind, NodeKind::Report { .. }))?;
    match &node.kind {
        NodeKind::Report {
            template: Some(template),
        } if !js_trim(template).is_empty() => Some(template),
        _ => None,
    }
}

/// The report the app posts to the Mono when a run ends, as Markdown.
pub fn compose_report(run: &WorkflowRun) -> Report {
    let workflow_name = &run.definition.name;
    let label = status_label(run.status);
    let title = format!("{workflow_name} · {label}");
    let Collected {
        results,
        empty,
        groups,
    } = collect(run);

    let body = if let Some(template) = report_template(&run.definition) {
        // The top-level scope (node outputs, inputs) first, so `{{<nodeId>.<field>}}` works as the
        // validator promises; the report's own `run` and `results` then take precedence.
        let Json::Object(mut scope) = run_scope(run, "", None) else {
            unreachable!("run_scope is an object")
        };
        let mut info = match scope.get("run") {
            Some(Json::Object(info)) => info.clone(),
            _ => OrderedMap::new(),
        };
        info.insert("name", Json::String(workflow_name.clone()));
        info.insert("status", Json::String(status_name(run.status).to_string()));
        info.insert("statusLabel", Json::String(label.to_string()));
        if let Some(ended) = run.ended_at {
            info.insert("endedAt", Json::Number(ended as f64));
        }
        scope.insert("run", Json::Object(info));
        scope.insert("results", results_json(&results));
        render_template(template, &Json::Object(scope), false)
    } else {
        let mut parts = Vec::new();
        if let Some(error) = run.error.as_deref().filter(|e| !e.is_empty()) {
            parts.push(format!("Run error: {}", one_line(error)));
        }
        for label in &empty {
            parts.push(if groups > 1 {
                format!("No items in {label}")
            } else {
                "No items".to_string()
            });
        }
        parts.extend(results.iter().map(render_branch));
        parts.join("\n\n")
    };
    let text = format!("## {title}\n\n{body}");
    Report {
        title,
        text: text.trim_end_matches(js_space).to_string(),
    }
}

/// The request the Mono receives for an Ask Mono node, or a Mono-decided If or loop. None for an
/// action that is not a question.
pub fn ask_mono_prompt(action: &EngineAction, workflow_name: &str) -> Option<String> {
    let EngineAction::AskMono {
        node_id,
        question,
        answers,
        context,
        ..
    } = action
    else {
        return None;
    };
    let context = Json::Object(context.clone()).stringify(true);
    let fence = fence_for(&context);
    let allowed = answers
        .iter()
        .map(|a| Json::String(a.clone()).stringify(false))
        .collect::<Vec<_>>()
        .join(", ");
    Some(
        [
            format!(
                "You are being consulted by a running workflow, \"{}\", at step \"{node_id}\".",
                one_line(workflow_name)
            ),
            "This is a single decision request. Do not start any work or use any tools; just decide."
                .to_string(),
            String::new(),
            "Question:".to_string(),
            question.clone(),
            String::new(),
            "Relevant outputs so far:".to_string(),
            format!("{fence}json"),
            context,
            fence,
            String::new(),
            format!("Allowed answers: {allowed}"),
            String::new(),
            format!(
                "Reply with a short explanation if you like, and end with exactly one fenced code block whose info string is `{OUTPUT_FENCE}`:"
            ),
            format!("```{OUTPUT_FENCE}"),
            "{\"answer\": <one of the allowed answers>, \"reason\": \"...\"}".to_string(),
            "```".to_string(),
        ]
        .join("\n"),
    )
}

/// What the Mono decided: one of the offered answers, and why when it said.
#[derive(Clone, Debug, PartialEq)]
pub struct MonoDecision {
    pub value: String,
    pub reason: Option<String>,
}

/// The Mono's decision from its reply: `answer` must be one of `answers`; `reason` is optional.
/// Err is the message to put back to the Mono.
pub fn parse_mono_decision(reply: &str, answers: &[String]) -> Result<MonoDecision, String> {
    let mut fields = OrderedMap::new();
    fields.insert("answer", OutputField::new(OutputFieldType::String));
    let parsed = parse_workflow_output(reply, &fields)?;
    let Some(Json::String(answer)) = parsed.get("answer") else {
        return Err("Field \"answer\" must be a JSON string.".to_string());
    };
    if !answers.contains(answer) {
        let allowed = answers
            .iter()
            .map(|a| Json::String(a.clone()).stringify(false))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "Answer {} is not allowed; choose one of: {allowed}.",
            Json::String(answer.clone()).stringify(false)
        ));
    }
    let reason = match parsed.get("reason") {
        Some(Json::String(reason)) if !reason.is_empty() => Some(reason.clone()),
        _ => None,
    };
    Ok(MonoDecision {
        value: answer.clone(),
        reason,
    })
}
