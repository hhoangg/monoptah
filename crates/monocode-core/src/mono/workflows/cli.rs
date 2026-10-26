//! The 16 `workflows.*` app actions a Mono calls through its bridge: port of
//! `agentAppWorkflows.ts`. Field allow-lists, error strings and result shapes are the React
//! app's, because the Mono and the workflow guide read them.
//!
//! `handle` is pure over a `WorkflowHost`: the real one (`StoreHost`) reads and writes the Mono's
//! files; tests use an in-memory one. Every action returns fast. `run`, `stop` and `resumeRun`
//! only write their request into `workflows.json` (`runRequested`, `stopRequested`,
//! `resumeRunId`) and call `request_check_now`; the runner does the work. The only slow part is
//! probing the model catalog, which `native_probe` bounds, so call `handle` off the UI thread.
//!
//! Local times in results (`nextRunAt`, `startedAt`, `endedAt`) read `YYYY-MM-DD HH:MM` in the
//! machine's time zone, where React printed `toLocaleString()`.
use super::guide::workflow_guide;
use super::model::{
    new_workflow_id, trigger_of, Edge, InputValue, Node, NodeKind, Workflow, WorkflowRun,
    WORKFLOWS_MAX, WORKFLOW_DEFINITION_MAX_BYTES, WORKFLOW_DESCRIPTION_MAX, WORKFLOW_NAME_MAX,
};
use super::models::{harnesses_used, native_probe, workflow_issues, ModelCatalog, Probe};
use super::ordered::{js_number_text, Json, OrderedMap};
use super::parse::{parse_edge, parse_inputs, parse_node};
use super::store::{
    clear_runs, load_runs, load_workflows, to_json, update_workflows, workflow_json, MonoDir,
};
use super::template::js_trim;
use super::validate::WorkflowIssue;
use super::yaml::{
    anchor_node, export_path_problem, export_workflow, finish_import, input_value_problems,
    preview_import, review_texts, ReviewCommand, ReviewPrompt, WorkflowDraft,
};
use crate::mono::habits::Schedule;
use crate::provider::Kind;
use chrono::{Local, TimeZone};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// What the actions need from the app. `update` hands the host back to the edit so it can probe
/// the catalog without borrowing the host twice; the edit may run more than once (a save that
/// loses a race is re-applied), and nothing is written when it fails.
pub trait WorkflowHost: Sized {
    fn load(&mut self) -> Result<Vec<Workflow>, String>;
    fn update<T>(
        &mut self,
        edit: impl FnMut(&mut Self, Vec<Workflow>) -> Result<(Vec<Workflow>, T), String>,
    ) -> Result<T, String>;
    /// A workflow's runs, newest first.
    fn runs(&mut self, workflow_id: &str) -> Result<Vec<WorkflowRun>, String>;
    /// Drops the history of a removed workflow. Failing is harmless, so it never fails.
    fn clear_runs(&mut self, workflow_id: &str);
    /// The models and providers `w`'s session nodes are judged against. Blocks while a CLI is
    /// probed.
    fn catalog(&mut self, w: &Workflow) -> ModelCatalog;
    /// Whether a run of the workflow is going.
    fn is_running(&self, workflow_id: &str) -> bool;
    /// Asks the runner to look at the workflow files now instead of at its next tick.
    fn request_check_now(&mut self);
    /// Milliseconds since the epoch.
    fn now(&self) -> u64;
}

/// What the runner and the actions tell each other, shared as an `Arc`: the runner calls
/// `set_running` when a run starts and ends, and an action that wrote a request (`run`, `stop`,
/// `resumeRun`) calls `request_check_now`, which sets a flag and rings the waker the scheduler
/// installed, so the request is picked up at once and not on its next look.
#[derive(Default)]
pub struct Signals {
    running: Mutex<HashSet<String>>,
    check_now: AtomicBool,
    waker: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
}

impl Signals {
    pub fn set_running(&self, workflow_id: &str, running: bool) {
        let mut ids = self.running.lock().unwrap_or_else(|e| e.into_inner());
        if running {
            ids.insert(workflow_id.to_string());
        } else {
            ids.remove(workflow_id);
        }
    }
    pub fn is_running(&self, workflow_id: &str) -> bool {
        self.running
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(workflow_id)
    }
    /// Rung by every `request_check_now`. It must not block (it runs inside an action).
    pub fn set_waker(&self, wake: impl Fn() + Send + Sync + 'static) {
        *self.waker.lock().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(wake));
    }
    pub fn request_check_now(&self) {
        self.check_now.store(true, Ordering::SeqCst);
        if let Some(wake) = self
            .waker
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            wake();
        }
    }
    /// True once per request.
    pub fn take_check_now(&self) -> bool {
        self.check_now.swap(false, Ordering::SeqCst)
    }
}

/// One Mono's workflows on disk (`workflows.json`, `workflow-runs/`), through `workflows::store`.
pub struct StoreHost {
    dir: MonoDir,
    signals: Arc<Signals>,
    probe: fn(Kind) -> Probe,
}

impl StoreHost {
    pub fn new(dir: MonoDir, signals: Arc<Signals>) -> Self {
        Self {
            dir,
            signals,
            probe: native_probe,
        }
    }
    /// Replaces how a harness is probed (tests must never reach real CLIs).
    pub fn with_probe(mut self, probe: fn(Kind) -> Probe) -> Self {
        self.probe = probe;
        self
    }
}

impl WorkflowHost for StoreHost {
    fn load(&mut self) -> Result<Vec<Workflow>, String> {
        Ok(load_workflows(&self.dir)?.workflows)
    }
    fn update<T>(
        &mut self,
        mut edit: impl FnMut(&mut Self, Vec<Workflow>) -> Result<(Vec<Workflow>, T), String>,
    ) -> Result<T, String> {
        let dir = self.dir.clone();
        update_workflows(&dir, |list| edit(self, list))
    }
    fn runs(&mut self, workflow_id: &str) -> Result<Vec<WorkflowRun>, String> {
        load_runs(&self.dir, workflow_id)
    }
    fn clear_runs(&mut self, workflow_id: &str) {
        clear_runs(&self.dir, workflow_id);
    }
    fn catalog(&mut self, w: &Workflow) -> ModelCatalog {
        ModelCatalog::for_harnesses(&harnesses_used(w), self.probe)
    }
    fn is_running(&self, workflow_id: &str) -> bool {
        self.signals.is_running(workflow_id)
    }
    fn request_check_now(&mut self) {
        self.signals.request_check_now();
    }
    fn now(&self) -> u64 {
        crate::time::now_ms()
    }
}

/// The fields of a workflow definition the CLI accepts; the app owns everything else.
const DEFINITION_FIELDS: [&str; 6] = [
    "name",
    "description",
    "inputs",
    "inputValues",
    "nodes",
    "edges",
];
const NOT_ENABLED: &str = "Resume the workflow before running it.";
const NEEDS_REVIEW: &str = "This workflow came from a file and the user has not reviewed it yet. Ask the user to review it and enable it in the Workflows panel.";
const RUNS_DEFAULT: f64 = 5.0;
const RUNS_MAX: f64 = 20.0;

/// The fields each action takes, space separated. `project` is accepted like on other actions
/// and ignored. None for an action that is not a workflows action.
pub fn fields(action: &str) -> Option<&'static str> {
    Some(match action {
        "workflows.list" | "workflows.guide" => "project",
        "workflows.get" | "workflows.run" | "workflows.stop" | "workflows.pause"
        | "workflows.resume" | "workflows.remove" | "workflows.export" => "id project",
        "workflows.add" => "workflow yaml values project",
        "workflows.validate" => "id workflow yaml values project",
        "workflows.update" => "id workflow project",
        "workflows.setInputs" => "id values project",
        "workflows.resumeRun" => "id runId project",
        "workflows.import" => "yaml values project",
        "workflows.runs" => "id limit project",
        _ => return None,
    })
}

// ---- small helpers ----------------------------------------------------------------------

fn st(text: impl Into<String>) -> Json {
    Json::String(text.into())
}

fn obj<const N: usize>(pairs: [(&str, Json); N]) -> Json {
    Json::Object(pairs.into_iter().collect())
}

fn ser<T: Serialize>(value: &T) -> Json {
    to_json(value).unwrap_or(Json::Null)
}

/// The length JS reports for a string: UTF-16 units.
fn js_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// `text()` of the React file: a non-empty string within `max`, trimmed.
fn text(value: Option<&Json>, name: &str, max: usize) -> Result<String, String> {
    match value {
        Some(Json::String(s)) if !js_trim(s).is_empty() && js_len(s) <= max => {
            Ok(js_trim(s).to_string())
        }
        _ => Err(format!(
            "{name} must be a non-empty string under {max} characters"
        )),
    }
}

/// `String(value)` for the few places the message prints a value it did not check.
fn js_string(value: Option<&Json>) -> String {
    match value {
        None => "undefined".into(),
        Some(Json::Null) => "null".into(),
        Some(Json::Bool(b)) => b.to_string(),
        Some(Json::Number(n)) => js_number_text(*n),
        Some(Json::String(s)) => s.clone(),
        Some(Json::Array(items)) => items
            .iter()
            .map(|i| match i {
                Json::Null => String::new(),
                other => js_string(Some(other)),
            })
            .collect::<Vec<_>>()
            .join(","),
        Some(Json::Object(_)) => "[object Object]".into(),
    }
}

/// `YYYY-MM-DD HH:MM` in the machine's time zone.
fn stamp(ms: u64) -> String {
    i64::try_from(ms)
        .ok()
        .and_then(|ms| Local.timestamp_millis_opt(ms).single())
        .map(|at| at.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_default()
}

fn issues_text(issues: &[WorkflowIssue]) -> String {
    issues
        .iter()
        .map(|issue| issue.message.as_str())
        .collect::<Vec<_>>()
        .join("; ")
}

fn issue_json(issue: &WorkflowIssue) -> Json {
    match &issue.node_id {
        Some(node) => obj([("nodeId", st(node)), ("message", st(&issue.message))]),
        None => obj([("message", st(&issue.message))]),
    }
}

fn issues_json(issues: &[WorkflowIssue]) -> Json {
    Json::Array(issues.iter().map(issue_json).collect())
}

fn commands_json(commands: &[ReviewCommand]) -> Json {
    Json::Array(
        commands
            .iter()
            .map(|c| obj([("nodeId", st(&c.node_id)), ("command", st(&c.command))]))
            .collect(),
    )
}

fn prompts_json(prompts: &[ReviewPrompt]) -> Json {
    Json::Array(
        prompts
            .iter()
            .map(|p| obj([("nodeId", st(&p.node_id)), ("text", st(&p.text))]))
            .collect(),
    )
}

fn find<'a>(list: &'a [Workflow], input: &Json) -> Result<&'a Workflow, String> {
    let id = text(input.get("id"), "id", 128)?;
    list.iter()
        .find(|w| w.id == id)
        .ok_or_else(|| "No workflow with that id; run workflows.list".into())
}

/// Swaps `next` in for the workflow with its id.
fn replace(list: Vec<Workflow>, next: Workflow) -> Vec<Workflow> {
    list.into_iter()
        .map(|w| if w.id == next.id { next.clone() } else { w })
        .collect()
}

/// Problems that keep a workflow from running: its graph, the models it names, and its input
/// values.
fn problems<H: WorkflowHost>(host: &mut H, w: &Workflow) -> Vec<WorkflowIssue> {
    let catalog = host.catalog(w);
    let mut issues = workflow_issues(w, &catalog);
    issues.extend(
        input_value_problems(&w.inputs, &w.input_values)
            .into_iter()
            .map(|message| WorkflowIssue {
                node_id: None,
                message,
            }),
    );
    issues
}

/// What `workflows.add` tells the Mono to do next. With problems the workflow cannot run, so
/// fixing comes first.
fn add_note(issues: &[WorkflowIssue], from_file: bool) -> String {
    if !issues.is_empty() {
        return format!(
            "Saved paused, but it has problems and cannot run yet. Fix the issues first: change the definition, check it with workflows.validate, and save it with workflows.update.{}",
            if from_file { " Check the commands and prompts with the user too." } else { "" }
        );
    }
    if from_file {
        "Saved paused. Check the commands and prompts with the user; only the user can enable it, in the Workflows panel.".into()
    } else {
        "Saved paused. Tell the user to open it in the Workflows panel and enable it there.".into()
    }
}

/// The values of the inputs that are declared; the rest belong to inputs that are gone.
fn declared_values(
    inputs: &OrderedMap<super::model::WorkflowInput>,
    values: &OrderedMap<InputValue>,
) -> OrderedMap<InputValue> {
    inputs
        .keys()
        .filter_map(|name| values.get(name).map(|v| (name.clone(), v.clone())))
        .collect()
}

fn schedule_of(w: &Workflow) -> Option<&Schedule> {
    match trigger_of(w).map(|n| &n.kind) {
        Some(NodeKind::Schedule { schedule }) => Some(schedule),
        _ => None,
    }
}

/// The workflow with `nextRunAt` right for its enabled state and schedule.
fn scheduled(w: &Workflow, now: u64) -> Result<Workflow, String> {
    let mut next = w.clone();
    next.next_run_at = None;
    if w.enabled {
        if let Some(schedule) = schedule_of(w) {
            next.next_run_at = Some(schedule.next(now)?);
        }
    }
    Ok(next)
}

fn trigger_view(w: &Workflow) -> Json {
    match trigger_of(w).map(|n| &n.kind) {
        None => obj([("type", st("none"))]),
        Some(NodeKind::Schedule { schedule }) => {
            obj([("type", st("schedule")), ("schedule", st(schedule.label()))])
        }
        Some(_) => obj([("type", st("manual"))]),
    }
}

fn run_summary_fields(run: &WorkflowRun) -> Vec<(&'static str, Json)> {
    let mut fields = vec![
        ("id", st(&run.id)),
        ("status", ser(&run.status)),
        ("trigger", ser(&run.trigger)),
        ("startedAt", st(stamp(run.started_at))),
    ];
    if let Some(ended) = run.ended_at.filter(|at| *at != 0) {
        fields.push(("endedAt", st(stamp(ended))));
    }
    if let Some(error) = run.error.as_ref().filter(|e| !e.is_empty()) {
        fields.push(("error", st(error)));
    }
    fields
}

fn run_summary(run: &WorkflowRun) -> Json {
    Json::Object(run_summary_fields(run).into_iter().collect())
}

/// A run with each node's status per branch (a branch is one item of a forEach).
fn run_detail(run: &WorkflowRun) -> Json {
    let nodes = run
        .nodes
        .iter()
        .map(|node| {
            let mut fields = vec![
                ("nodeId", st(&node.node_id)),
                ("branch", st(&node.branch)),
                ("status", ser(&node.status)),
                ("round", Json::Number(node.round as f64)),
            ];
            if let Some(error) = node.error.as_ref().filter(|e| !e.is_empty()) {
                fields.push(("error", st(error)));
            }
            Json::Object(fields.into_iter().collect())
        })
        .collect();
    let mut fields = run_summary_fields(run);
    fields.push(("nodes", Json::Array(nodes)));
    Json::Object(fields.into_iter().collect())
}

// ---- reading a definition ---------------------------------------------------------------

/// A schedule node may be written like a habit's schedule (`{"kind":"daily"}`) or as stored.
fn schedule_node_input(raw: &Json) -> Result<Json, String> {
    let (Json::Object(fields), Some(schedule @ Json::Object(inner))) = (raw, raw.get("schedule"))
    else {
        return Ok(raw.clone());
    };
    if !inner.contains_key("kind") {
        return Ok(raw.clone());
    }
    let parsed = Schedule::from_cli(&schedule.to_value())
        .map_err(|e| format!("Node \"{}\": {e}", js_string(raw.get("id"))))?;
    let mut fields = fields.clone();
    fields.insert("schedule", ser(&parsed));
    Ok(Json::Object(fields))
}

fn input_values(value: Option<&Json>, name: &str) -> Result<OrderedMap<InputValue>, String> {
    let Some(Json::Object(map)) = value else {
        return Err(format!("{name} must be an object of values"));
    };
    let mut values = OrderedMap::new();
    for (key, entry) in map.iter() {
        let value = match entry {
            Json::String(s) => InputValue::String(s.clone()),
            Json::Bool(b) => InputValue::Bool(*b),
            Json::Number(n) if n.is_finite() => InputValue::Number(*n),
            _ => {
                return Err(format!(
                    "{name}.{key} must be text, a number or true or false"
                ))
            }
        };
        values.insert(key.clone(), value);
    }
    Ok(values)
}

struct Parsed {
    definition: WorkflowDraft,
    values: Option<OrderedMap<InputValue>>,
}

fn definition_from(raw: Option<&Json>, now: u64, allowed: &[&str]) -> Result<Parsed, String> {
    let Some(raw @ Json::Object(map)) = raw else {
        return Err("workflow must be an object".into());
    };
    let unknown: Vec<&str> = map
        .keys()
        .map(String::as_str)
        .filter(|key| !allowed.contains(key))
        .collect();
    if !unknown.is_empty() {
        return Err(format!(
            "Unknown workflow fields: {}. Allowed: {}; the id, creation time and run state are managed by the app",
            unknown.join(", "),
            allowed.join(", ")
        ));
    }
    if js_len(&raw.stringify(false)) > WORKFLOW_DEFINITION_MAX_BYTES {
        return Err("The workflow is too large (over 512 KiB)".into());
    }
    let name = text(map.get("name"), "workflow.name", WORKFLOW_NAME_MAX)?;
    let description = match map.get("description") {
        None => None,
        some => Some(text(
            some,
            "workflow.description",
            WORKFLOW_DESCRIPTION_MAX,
        )?),
    };
    let entries = match map.get("nodes") {
        Some(Json::Array(nodes)) if !nodes.is_empty() => nodes,
        _ => return Err("workflow.nodes must be a list of nodes".into()),
    };
    let mut nodes: Vec<Node> = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        let is_schedule = entry.get("type").and_then(Json::as_str) == Some("schedule");
        let readable = if is_schedule {
            schedule_node_input(entry)?
        } else {
            entry.clone()
        };
        let Some(node) = parse_node(&readable) else {
            let id = entry
                .get("id")
                .and_then(Json::as_str)
                .filter(|id| !id.is_empty());
            return Err(format!(
                "nodes[{index}]{} has an unknown type, lacks a required field or has one of the wrong type",
                id.map(|id| format!(" (\"{id}\")")).unwrap_or_default()
            ));
        };
        nodes.push(anchor_node(node, now));
    }
    let edges: Vec<Edge> = match map.get("edges") {
        None => Vec::new(),
        Some(Json::Array(entries)) => {
            let mut edges = Vec::with_capacity(entries.len());
            for (index, entry) in entries.iter().enumerate() {
                edges.push(parse_edge(entry).ok_or_else(|| {
                    format!("edges[{index}] needs \"from\" and \"to\" (and a valid loop with maxRounds and decide, if any)")
                })?);
            }
            edges
        }
        Some(_) => return Err("workflow.edges must be a list".into()),
    };
    let inputs = match map.get("inputs") {
        None => OrderedMap::new(),
        Some(raw @ Json::Object(declared)) => {
            let inputs = parse_inputs(Some(raw));
            let lost: Vec<&str> = declared
                .keys()
                .map(String::as_str)
                .filter(|key| !inputs.contains_key(key))
                .collect();
            if !lost.is_empty() {
                return Err(format!(
                    "Invalid inputs: {} (use letters, digits and underscores, and a type of project, string, number or boolean)",
                    lost.join(", ")
                ));
            }
            inputs
        }
        Some(_) => return Err("workflow.inputs must be an object".into()),
    };
    let values = match map.get("inputValues") {
        None => None,
        some => Some(input_values(some, "workflow.inputValues")?),
    };
    Ok(Parsed {
        definition: WorkflowDraft {
            name,
            description,
            inputs,
            nodes,
            edges,
        },
        values,
    })
}

/// What a workflow made from a file asks the user to review.
struct Review {
    commands: Vec<ReviewCommand>,
    prompts: Vec<ReviewPrompt>,
}

/// The definition a `workflows.add` or `workflows.validate` call describes, from a JSON
/// `workflow` or a YAML file. Both actions parse through here so they cannot disagree.
fn new_workflow_from(input: &Json, now: u64) -> Result<(Workflow, Option<Review>), String> {
    if input.get("workflow").is_none() == input.get("yaml").is_none() {
        return Err("Pass either workflow (an object) or yaml (text), not both".into());
    }
    let from_yaml = input.get("yaml").is_some();
    let (definition, values, review);
    if !from_yaml {
        if input.get("values").is_some() {
            return Err("values goes with yaml; put inputValues in the workflow".into());
        }
        let parsed = definition_from(input.get("workflow"), now, &DEFINITION_FIELDS)?;
        definition = parsed.definition;
        values = parsed.values.unwrap_or_default();
        review = None;
    } else {
        let yaml = text(input.get("yaml"), "yaml", WORKFLOW_DEFINITION_MAX_BYTES)?;
        let preview = preview_import(&yaml, &ModelCatalog::unprobed())?;
        definition = WorkflowDraft {
            nodes: preview
                .workflow
                .nodes
                .into_iter()
                .map(|node| anchor_node(node, now))
                .collect(),
            ..preview.workflow
        };
        values = match input.get("values") {
            None => OrderedMap::new(),
            given => declared_values(&definition.inputs, &input_values(given, "values")?),
        };
        review = Some(Review {
            commands: preview.commands,
            prompts: preview.prompts,
        });
    }
    // Always paused: only the user, in the Workflows panel, enables what the CLI creates.
    let mut draft = Workflow::new(new_workflow_id(), definition.name);
    draft.description = definition.description;
    draft.inputs = definition.inputs;
    draft.input_values = values;
    draft.nodes = definition.nodes;
    draft.edges = definition.edges;
    draft.created_at = now;
    // Only the user, in the panel, may enable what came from a file.
    draft.needs_review = from_yaml;
    Ok((draft, review))
}

// ---- the actions ------------------------------------------------------------------------

/// Runs one `workflows.*` action. `Err` is the message the Mono reads.
pub fn handle<H: WorkflowHost>(action: &str, input: &Json, host: &mut H) -> Result<Json, String> {
    let Some(allowed) = fields(action) else {
        return Err(format!("Unknown app action: {action}"));
    };
    let Json::Object(given) = input else {
        return Err("Input must be an object".into());
    };
    let unknown: Vec<&str> = given
        .keys()
        .map(String::as_str)
        .filter(|key| !allowed.split_whitespace().any(|f| f == *key))
        .collect();
    if !unknown.is_empty() {
        return Err(format!("Unknown {action} fields: {}", unknown.join(", ")));
    }
    let now = host.now();
    match action {
        "workflows.list" => list(host),
        "workflows.get" => get(host, input),
        "workflows.add" => add(host, input, now),
        "workflows.validate" => validate(host, input, now),
        "workflows.guide" => Ok(obj([("guide", st(workflow_guide()))])),
        "workflows.update" => update(host, input, now),
        "workflows.setInputs" => set_inputs(host, input),
        "workflows.run" => run(host, input),
        "workflows.stop" => stop(host, input),
        "workflows.pause" => pause(host, input),
        "workflows.resume" => resume(host, input, now),
        "workflows.resumeRun" => resume_run(host, input),
        "workflows.remove" => remove(host, input),
        "workflows.export" => export(host, input),
        "workflows.import" => import(host, input, now),
        _ => runs(host, input),
    }
}

/// `handle` over serde_json values, for the app's bridge.
pub fn handle_value<H: WorkflowHost>(
    action: &str,
    input: &Value,
    host: &mut H,
) -> Result<Value, String> {
    handle(action, &Json::from(input), host).map(|result| result.to_value())
}

fn list<H: WorkflowHost>(host: &mut H) -> Result<Json, String> {
    let workflows = host.load()?;
    let mut out = Vec::with_capacity(workflows.len());
    for w in &workflows {
        let last = host.runs(&w.id)?.into_iter().next();
        out.push(obj([
            ("id", st(&w.id)),
            ("name", st(&w.name)),
            ("enabled", Json::Bool(w.enabled)),
            ("trigger", trigger_view(w)),
            (
                "nextRunAt",
                match w.next_run_at {
                    Some(at) if w.enabled => st(stamp(at)),
                    _ => Json::Null,
                },
            ),
            ("running", Json::Bool(host.is_running(&w.id))),
            (
                "lastRun",
                last.map_or(Json::Null, |run| {
                    obj([
                        ("id", st(&run.id)),
                        ("status", ser(&run.status)),
                        ("startedAt", st(stamp(run.started_at))),
                    ])
                }),
            ),
        ]));
    }
    Ok(obj([("workflows", Json::Array(out))]))
}

fn get<H: WorkflowHost>(host: &mut H, input: &Json) -> Result<Json, String> {
    let workflows = host.load()?;
    let w = find(&workflows, input)?;
    let runs = host.runs(&w.id)?;
    let stored = workflow_json(w)?;
    let part = |key: &str| stored.get(key).cloned().unwrap_or(Json::Null);
    let mut out = vec![("id", st(&w.id)), ("name", st(&w.name))];
    if let Some(description) = &w.description {
        out.push(("description", st(description)));
    }
    out.push(("enabled", Json::Bool(w.enabled)));
    out.push(("trigger", trigger_view(w)));
    out.push(("running", Json::Bool(host.is_running(&w.id))));
    out.push(("inputs", part("inputs")));
    out.push(("inputValues", part("inputValues")));
    out.push(("nodes", part("nodes")));
    out.push(("edges", part("edges")));
    out.push(("issues", issues_json(&problems(host, w))));
    out.push((
        "runs",
        Json::Array(runs.iter().take(5).map(run_summary).collect()),
    ));
    Ok(Json::Object(out.into_iter().collect()))
}

fn add<H: WorkflowHost>(host: &mut H, input: &Json, now: u64) -> Result<Json, String> {
    let (draft, review) = new_workflow_from(input, now)?;
    // Judged before the save: a cold model probe can take a while and must not sit inside the
    // hash-checked edit.
    let issues = problems(host, &draft);
    host.update(|_, mut list| {
        if list.len() >= WORKFLOWS_MAX {
            return Err(format!("A Mono can have at most {WORKFLOWS_MAX} workflows"));
        }
        let mut result = vec![
            ("id", st(&draft.id)),
            ("enabled", Json::Bool(false)),
            ("issues", issues_json(&issues)),
            ("note", st(add_note(&issues, review.is_some()))),
        ];
        if let Some(review) = &review {
            result.push(("commands", commands_json(&review.commands)));
            result.push(("prompts", prompts_json(&review.prompts)));
        }
        list.push(draft.clone());
        Ok((list, Json::Object(result.into_iter().collect())))
    })
}

fn validate<H: WorkflowHost>(host: &mut H, input: &Json, now: u64) -> Result<Json, String> {
    // The same parse and checks as workflows.add, without saving anything.
    let (parsed, _) = new_workflow_from(input, now)?;
    let list = host.load()?;
    let existing = match input.get("id") {
        None => None,
        Some(_) => {
            let id = text(input.get("id"), "id", 128)?;
            list.iter().find(|w| w.id == id)
        }
    };
    // Checking a change to an existing workflow (`id`) judges the values as update would: when
    // the change gives none, the stored values of the inputs it still declares stay.
    let given = match input.get("workflow") {
        Some(Json::Object(w)) => w.contains_key("inputValues"),
        _ => input.get("values").is_some(),
    };
    let draft = match existing {
        Some(existing) if !given => {
            let mut draft = parsed;
            draft.input_values = declared_values(&draft.inputs, &existing.input_values);
            draft
        }
        _ => parsed,
    };
    // Read only: add would be refused at the limit, so say so here instead of at save time.
    let mut issues = problems(host, &draft);
    // Checking a change to an existing workflow adds nothing, so the limit does not apply.
    if existing.is_none() && list.len() >= WORKFLOWS_MAX {
        issues.push(WorkflowIssue {
            node_id: None,
            message: format!(
                "A Mono can have at most {WORKFLOWS_MAX} workflows; remove one before adding another"
            ),
        });
    }
    let (commands, prompts) = review_texts(&draft.nodes);
    Ok(obj([
        ("issues", issues_json(&issues)),
        ("commands", commands_json(&commands)),
        ("prompts", prompts_json(&prompts)),
    ]))
}

fn update<H: WorkflowHost>(host: &mut H, input: &Json, now: u64) -> Result<Json, String> {
    let mut allowed = DEFINITION_FIELDS.to_vec();
    allowed.push("enabled");
    let parsed = definition_from(input.get("workflow"), now, &allowed)?;
    let wanted = match input.get("workflow").and_then(|w| w.get("enabled")) {
        None => None,
        Some(Json::Bool(b)) => Some(*b),
        Some(_) => return Err("enabled must be true or false".into()),
    };
    host.update(|host, list| {
        let current = find(&list, input)?.clone();
        if current.needs_review && wanted == Some(true) {
            return Err(NEEDS_REVIEW.into());
        }
        // Only the definition is replaced; the id, creation time and run state stay.
        let mut draft = current.clone();
        draft.name = parsed.definition.name.clone();
        draft.description = parsed.definition.description.clone();
        draft.inputs = parsed.definition.inputs.clone();
        draft.nodes = parsed.definition.nodes.clone();
        draft.edges = parsed.definition.edges.clone();
        // Values of inputs the new definition dropped go with them; given ones are checked as given.
        draft.input_values = match &parsed.values {
            Some(values) => values.clone(),
            None => declared_values(&draft.inputs, &current.input_values),
        };
        draft.enabled = wanted.unwrap_or(current.enabled);
        let issues = problems(host, &draft);
        // A workflow that cannot run is paused rather than left to fail on its schedule.
        let enabled = draft.enabled && issues.is_empty();
        draft.enabled = enabled;
        let same_schedule = schedule_of(&current) == schedule_of(&draft);
        // A new schedule, or turning it back on, counts from now; otherwise the time stands.
        let next =
            if enabled && (!current.enabled || !same_schedule || current.next_run_at.is_none()) {
                scheduled(&draft, now)?
            } else {
                draft
            };
        let result = obj([
            ("id", st(&next.id)),
            ("enabled", Json::Bool(enabled)),
            ("issues", issues_json(&issues)),
        ]);
        Ok((replace(list, next), result))
    })
}

fn set_inputs<H: WorkflowHost>(host: &mut H, input: &Json) -> Result<Json, String> {
    let values = input_values(input.get("values"), "values")?;
    host.update(|host, list| {
        let current = find(&list, input)?.clone();
        // Only the values given are checked here; the rest may still be missing.
        let declared: OrderedMap<_> = values
            .keys()
            .filter_map(|key| current.inputs.get(key).map(|i| (key.clone(), i.clone())))
            .collect();
        let bad = input_value_problems(&declared, &values);
        if !bad.is_empty() {
            let names: Vec<&str> = current.inputs.keys().map(String::as_str).collect();
            return Err(format!(
                "{}. Declared inputs: {}",
                bad.join("; "),
                if names.is_empty() {
                    "none".to_string()
                } else {
                    names.join(", ")
                }
            ));
        }
        let mut next = current;
        for (key, value) in values.iter() {
            next.input_values.insert(key.clone(), value.clone());
        }
        let result = obj([
            ("id", st(&next.id)),
            ("inputValues", ser(&next.input_values)),
            ("issues", issues_json(&problems(host, &next))),
        ]);
        Ok((replace(list, next), result))
    })
}

fn run<H: WorkflowHost>(host: &mut H, input: &Json) -> Result<Json, String> {
    let workflows = host.load()?;
    let current = find(&workflows, input)?.clone();
    if current.needs_review {
        return Err(NEEDS_REVIEW.into());
    }
    if host.is_running(&current.id) {
        return Err(
            "That workflow is already running; its report is posted here when it ends".into(),
        );
    }
    let issues = problems(host, &current);
    if !issues.is_empty() {
        return Err(format!(
            "The workflow cannot run yet: {}",
            issues_text(&issues)
        ));
    }
    // A paused workflow is paused on purpose; running it would go around that.
    if !current.enabled {
        return Err(NOT_ENABLED.into());
    }
    host.update(|_, list| {
        let mut latest = find(&list, input)?.clone();
        if latest.needs_review {
            return Err(NEEDS_REVIEW.into());
        }
        if !latest.enabled {
            return Err(NOT_ENABLED.into());
        }
        latest.run_requested = true;
        Ok((replace(list, latest), ()))
    })?;
    host.request_check_now();
    Ok(obj([
        ("id", st(&current.id)),
        (
            "note",
            st("It starts now, on its own. Its report is posted to this chat when it finishes."),
        ),
    ]))
}

fn stop<H: WorkflowHost>(host: &mut H, input: &Json) -> Result<Json, String> {
    let workflows = host.load()?;
    let current = find(&workflows, input)?.clone();
    if !host.is_running(&current.id) {
        return Err("That workflow is not running".into());
    }
    host.update(|_, list| {
        let list = list
            .into_iter()
            .map(|mut w| {
                if w.id == current.id {
                    w.stop_requested = true;
                }
                w
            })
            .collect();
        Ok((list, ()))
    })?;
    host.request_check_now();
    Ok(obj([
        ("id", st(&current.id)),
        ("note", st("Stop requested; the run ends within seconds.")),
    ]))
}

fn pause<H: WorkflowHost>(host: &mut H, input: &Json) -> Result<Json, String> {
    host.update(|_, list| {
        let mut next = find(&list, input)?.clone();
        next.enabled = false;
        let result = obj([("id", st(&next.id)), ("enabled", Json::Bool(false))]);
        Ok((replace(list, next), result))
    })
}

fn resume<H: WorkflowHost>(host: &mut H, input: &Json, now: u64) -> Result<Json, String> {
    host.update(|host, list| {
        let mut current = find(&list, input)?.clone();
        if current.needs_review {
            return Err(NEEDS_REVIEW.into());
        }
        let issues = problems(host, &current);
        if !issues.is_empty() {
            return Err(format!(
                "The workflow cannot be resumed yet: {}",
                issues_text(&issues)
            ));
        }
        current.enabled = true;
        let next = scheduled(&current, now)?;
        let result = obj([("id", st(&next.id)), ("enabled", Json::Bool(true))]);
        Ok((replace(list, next), result))
    })
}

fn resume_run<H: WorkflowHost>(host: &mut H, input: &Json) -> Result<Json, String> {
    let run_id = text(input.get("runId"), "runId", 128)?;
    let workflows = host.load()?;
    let current = find(&workflows, input)?.clone();
    if current.needs_review {
        return Err(NEEDS_REVIEW.into());
    }
    if host.is_running(&current.id) {
        return Err("That workflow is already running".into());
    }
    let run = host
        .runs(&current.id)?
        .into_iter()
        .find(|run| run.id == run_id)
        .ok_or("No run with that id; run workflows.runs")?;
    if run.status != super::model::RunStatus::Interrupted {
        let status = ser(&run.status);
        return Err(format!(
            "Only an interrupted run can be resumed; this one is {}",
            status.as_str().unwrap_or_default()
        ));
    }
    host.update(|_, list| {
        let list = list
            .into_iter()
            .map(|mut w| {
                if w.id == current.id {
                    w.resume_run_id = Some(run.id.clone());
                }
                w
            })
            .collect();
        Ok((list, ()))
    })?;
    host.request_check_now();
    Ok(obj([
        ("id", st(&current.id)),
        ("runId", st(&run.id)),
        ("note", st("It continues from where it stopped.")),
    ]))
}

fn remove<H: WorkflowHost>(host: &mut H, input: &Json) -> Result<Json, String> {
    let workflows = host.load()?;
    let current = find(&workflows, input)?.clone();
    if host.is_running(&current.id) {
        return Err("Stop the workflow before removing it".into());
    }
    host.update(|_, list| {
        Ok((
            list.into_iter().filter(|w| w.id != current.id).collect(),
            (),
        ))
    })?;
    // The history of a workflow that is gone is of no use; losing the cleanup is harmless.
    host.clear_runs(&current.id);
    Ok(obj([("removed", st(&current.name))]))
}

fn export<H: WorkflowHost>(host: &mut H, input: &Json) -> Result<Json, String> {
    let workflows = host.load()?;
    let current = find(&workflows, input)?;
    if let Some(problem) = export_path_problem(current) {
        return Err(format!("This workflow cannot be exported: {problem}"));
    }
    Ok(obj([("yaml", st(export_workflow(current)))]))
}

fn import<H: WorkflowHost>(host: &mut H, input: &Json, now: u64) -> Result<Json, String> {
    let yaml = text(input.get("yaml"), "yaml", WORKFLOW_DEFINITION_MAX_BYTES)?;
    let preview = preview_import(&yaml, &ModelCatalog::unprobed())?;
    let values = match input.get("values") {
        None => OrderedMap::new(),
        given => input_values(given, "values")?,
    };
    let mut created = finish_import(&preview, &values, now)?;
    created.nodes = created
        .nodes
        .into_iter()
        .map(|node| anchor_node(node, now))
        .collect();
    // Only the user, in the panel, may enable what came from a file.
    created.needs_review = true;
    host.update(|_, mut list| {
        if list.len() >= WORKFLOWS_MAX {
            return Err(format!("A Mono can have at most {WORKFLOWS_MAX} workflows"));
        }
        list.push(created.clone());
        Ok((list, ()))
    })?;
    Ok(obj([
        ("id", st(&created.id)),
        ("name", st(&created.name)),
        ("enabled", Json::Bool(false)),
        ("issues", issues_json(&problems(host, &created))),
        ("commands", commands_json(&preview.commands)),
        ("prompts", prompts_json(&preview.prompts)),
        (
            "note",
            st("Saved paused. Check the commands and prompts with the user; only the user can enable it, in the Workflows panel."),
        ),
    ]))
}

fn runs<H: WorkflowHost>(host: &mut H, input: &Json) -> Result<Json, String> {
    let workflows = host.load()?;
    let current = find(&workflows, input)?;
    let limit = match input.get("limit") {
        None | Some(Json::Null) => RUNS_DEFAULT,
        Some(Json::Number(n)) if n.fract() == 0.0 && (1.0..=RUNS_MAX).contains(n) => *n,
        Some(_) => return Err(format!("limit must be a whole number from 1 to {RUNS_MAX}")),
    };
    let runs = host.runs(&current.id)?;
    Ok(obj([(
        "runs",
        Json::Array(runs.iter().take(limit as usize).map(run_detail).collect()),
    )]))
}
