//! Lenient reading of workflow files. A node or edge that cannot be read could not run, and
//! dropping it quietly would change what the workflow does, so the whole entry is left out.
use super::model::{
    Checkout, Cwd, Decide, Edge, Handle, InputType, InputValue, LoopSpec, Node, NodeKind,
    OutputField, OutputFieldType, Ui, Workflow, WorkflowInput, NODE_TYPES,
};
use super::ordered::{Json, OrderedMap};
use crate::mono::habits::Schedule;
use std::collections::HashSet;

fn str_of(v: &Json, key: &str) -> Option<String> {
    v.get(key).and_then(Json::as_str).map(str::to_string)
}
fn num_of(v: &Json, key: &str) -> Option<f64> {
    v.get(key).and_then(Json::as_f64)
}
fn bool_of(v: &Json, key: &str) -> Option<bool> {
    v.get(key).and_then(Json::as_bool)
}
fn is_rec(v: &Json) -> bool {
    matches!(v, Json::Object(_))
}

/// `^[A-Za-z_][A-Za-z0-9_]*$`
pub fn is_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Whether `name` may be a key of an input or output map. Names end up in `{{a.b}}` paths, and
/// "__proto__" is refused for parity with the React app, which builds plain objects.
pub fn is_map_key(name: &str) -> bool {
    is_name(name) && name != "__proto__"
}

/// A number that does not fit the habit schedule's integer fields becomes the largest value, so
/// validation still reports it with the habit's own message instead of the draft being dropped.
fn small_whole(n: f64, max: f64) -> f64 {
    if n.fract() == 0.0 && (0.0..=max).contains(&n) {
        n
    } else {
        max
    }
}

fn parse_schedule(raw: Option<&Json>) -> Option<Schedule> {
    let raw = raw.filter(|r| is_rec(r))?;
    let kind = str_of(raw, "scheduleKind").filter(|k| !k.is_empty())?;
    Some(Schedule {
        // Whether the kind is one a habit knows is validate_workflow's call, so a draft survives.
        schedule_kind: kind,
        time: str_of(raw, "time").unwrap_or_else(|| "09:00".into()),
        minute: small_whole(num_of(raw, "minute").unwrap_or(0.0), u32::MAX as f64) as u32,
        day_of_week: small_whole(num_of(raw, "dayOfWeek").unwrap_or(1.0), u32::MAX as f64) as u32,
        interval_hours: num_of(raw, "intervalHours").map(|n| small_whole(n, u8::MAX as f64) as u8),
        interval_from: str_of(raw, "intervalFrom"),
    })
}

fn worktree_of(raw: &Json) -> Option<String> {
    raw.get("worktreeOf")
        .and_then(Json::as_str)
        .map(str::to_string)
}

fn parse_checkout(raw: Option<&Json>) -> Option<Checkout> {
    match raw? {
        Json::String(s) if s == "root" => Some(Checkout::Root),
        Json::String(s) if s == "newWorktree" => Some(Checkout::NewWorktree),
        other @ Json::Object(_) => worktree_of(other).map(Checkout::WorktreeOf),
        _ => None,
    }
}

fn parse_output(raw: Option<&Json>) -> Option<OrderedMap<OutputField>> {
    let Some(raw) = raw else {
        return Some(OrderedMap::new());
    };
    let fields = raw.as_object()?;
    let mut output = OrderedMap::new();
    for (name, field) in fields.iter() {
        let ty = field.get("type").and_then(Json::as_str).and_then(|t| {
            OutputFieldType::ALL
                .iter()
                .find(|(n, _)| *n == t)
                .map(|(_, ty)| *ty)
        });
        let (true, Some(ty)) = (is_map_key(name) && is_rec(field), ty) else {
            continue;
        };
        output.push_unique(
            name.clone(),
            OutputField {
                ty,
                in_report: bool_of(field, "inReport"),
                description: str_of(field, "description"),
            },
        );
    }
    Some(output)
}

pub fn parse_node(raw: &Json) -> Option<Node> {
    if !is_rec(raw) {
        return None;
    }
    let id = str_of(raw, "id").filter(|id| !id.is_empty())?;
    let ty = raw.get("type").and_then(Json::as_str)?;
    if !NODE_TYPES.contains(&ty) {
        return None;
    }
    let ui = raw.get("ui").and_then(|ui| {
        Some(Ui {
            x: num_of(ui, "x")?,
            y: num_of(ui, "y")?,
        })
    });
    let kind = match ty {
        "schedule" => NodeKind::Schedule {
            schedule: parse_schedule(raw.get("schedule"))?,
        },
        "manual" => NodeKind::Manual,
        "session" => NodeKind::Session {
            project: str_of(raw, "project")?,
            prompt: str_of(raw, "prompt")?,
            checkout: parse_checkout(raw.get("checkout"))?,
            output: parse_output(raw.get("output"))?,
            harness: str_of(raw, "harness"),
            model: str_of(raw, "model"),
            effort: str_of(raw, "effort"),
            reuse_session: bool_of(raw, "reuseSession"),
            nest_under: str_of(raw, "nestUnder"),
            timeout_minutes: num_of(raw, "timeoutMinutes"),
        },
        "command" => NodeKind::Command {
            project: str_of(raw, "project")?,
            command: str_of(raw, "command")?,
            cwd: match raw.get("cwd") {
                Some(Json::String(s)) if s == "root" => Some(Cwd::Root),
                Some(o @ Json::Object(_)) => worktree_of(o).map(Cwd::WorktreeOf),
                _ => None,
            },
            timeout_minutes: num_of(raw, "timeoutMinutes"),
        },
        "askMono" => NodeKind::AskMono {
            question: str_of(raw, "question")?,
            answers: raw
                .get("answers")?
                .as_array()?
                .iter()
                .filter_map(|a| a.as_str().map(str::to_string))
                .collect(),
        },
        "if" => NodeKind::If {
            condition: str_of(raw, "condition"),
            decide: (str_of(raw, "decide").as_deref() == Some("mono")).then_some(Decide::Mono),
            question: str_of(raw, "question"),
        },
        "forEach" => NodeKind::ForEach {
            list: str_of(raw, "in")?,
            item: str_of(raw, "as"),
            max_parallel: num_of(raw, "maxParallel"),
        },
        _ => NodeKind::Report {
            template: str_of(raw, "template"),
        },
    };
    Some(Node {
        id,
        title: str_of(raw, "title"),
        parent: str_of(raw, "parent"),
        ui,
        kind,
    })
}

pub fn parse_edge(raw: &Json) -> Option<Edge> {
    if !is_rec(raw) {
        return None;
    }
    let from = str_of(raw, "from").filter(|s| !s.is_empty())?;
    let to = str_of(raw, "to").filter(|s| !s.is_empty())?;
    let loop_spec = match raw.get("loop") {
        None => None,
        Some(l) if is_rec(l) => Some(LoopSpec {
            max_rounds: num_of(l, "maxRounds")?,
            decide: str_of(l, "decide")?,
        }),
        Some(_) => return None,
    };
    let handle = match raw.get("handle").and_then(Json::as_str) {
        Some("true") => Some(Handle::True),
        Some("false") => Some(Handle::False),
        _ => None,
    };
    Some(Edge {
        from,
        to,
        handle,
        loop_spec,
    })
}

pub fn parse_inputs(raw: Option<&Json>) -> OrderedMap<WorkflowInput> {
    let mut inputs = OrderedMap::new();
    let Some(map) = raw.and_then(Json::as_object) else {
        return inputs;
    };
    for (name, input) in map.iter() {
        let ty = input.get("type").and_then(Json::as_str).and_then(|t| {
            InputType::ALL
                .iter()
                .find(|(n, _)| *n == t)
                .map(|(_, ty)| *ty)
        });
        let (true, Some(ty)) = (is_map_key(name) && is_rec(input), ty) else {
            continue;
        };
        inputs.push_unique(
            name.clone(),
            WorkflowInput {
                ty,
                description: str_of(input, "description"),
            },
        );
    }
    inputs
}

fn parse_input_values(raw: Option<&Json>) -> OrderedMap<InputValue> {
    let mut values = OrderedMap::new();
    let Some(map) = raw.and_then(Json::as_object) else {
        return values;
    };
    for (name, value) in map.iter() {
        if !is_map_key(name) {
            continue;
        }
        match value {
            Json::String(s) => values.push_unique(name.clone(), InputValue::String(s.clone())),
            Json::Number(n) if n.is_finite() => {
                values.push_unique(name.clone(), InputValue::Number(*n))
            }
            Json::Bool(b) => values.push_unique(name.clone(), InputValue::Bool(*b)),
            _ => {}
        }
    }
    values
}

pub fn parse_workflow(raw: &Json) -> Option<Workflow> {
    if !is_rec(raw) {
        return None;
    }
    let id = str_of(raw, "id").filter(|id| !id.is_empty())?;
    let nodes: Option<Vec<Node>> = raw
        .get("nodes")?
        .as_array()?
        .iter()
        .map(parse_node)
        .collect();
    let edges: Option<Vec<Edge>> = match raw.get("edges").and_then(Json::as_array) {
        Some(edges) => edges.iter().map(parse_edge).collect(),
        None => Some(Vec::new()),
    };
    let (nodes, edges) = (nodes?, edges?);
    let flag = |key: &str| raw.get(key) == Some(&Json::Bool(true));
    Some(Workflow {
        id,
        name: str_of(raw, "name").unwrap_or_default(),
        description: str_of(raw, "description"),
        inputs: parse_inputs(raw.get("inputs")),
        input_values: parse_input_values(raw.get("inputValues")),
        nodes,
        edges,
        // An entry that says nothing about being enabled stays paused.
        enabled: flag("enabled"),
        created_at: num_of(raw, "createdAt").unwrap_or(0.0) as u64,
        next_run_at: num_of(raw, "nextRunAt").map(|n| n as u64),
        run_requested: flag("runRequested"),
        stop_requested: flag("stopRequested"),
        resume_run_id: str_of(raw, "resumeRunId"),
        last_run_id: str_of(raw, "lastRunId"),
        imported_from: str_of(raw, "importedFrom"),
        needs_review: flag("needsReview"),
    })
}

/// What workflows.json holds: the entries that parse, and the ones that do not, as they were written.
#[derive(Clone, Debug, PartialEq)]
pub struct WorkflowFile {
    pub workflows: Vec<Workflow>,
    pub unreadable: Vec<Json>,
}

/// Splits workflows.json into readable and unreadable entries. An entry is unreadable when it
/// does not parse as a workflow (a node of a type this app does not know, a damaged field) or
/// repeats an earlier id. None when the text is not a JSON list, so a caller can refuse to
/// overwrite it. A blank text is an empty file.
pub fn read_workflow_file(text: &str) -> Option<WorkflowFile> {
    if super::template::js_trim(text).is_empty() {
        return Some(WorkflowFile {
            workflows: Vec::new(),
            unreadable: Vec::new(),
        });
    }
    let Json::Array(entries) = Json::parse(text)? else {
        return None;
    };
    let mut file = WorkflowFile {
        workflows: Vec::new(),
        unreadable: Vec::new(),
    };
    let mut seen = HashSet::new();
    for raw in entries {
        match parse_workflow(&raw) {
            Some(workflow) if seen.insert(workflow.id.clone()) => file.workflows.push(workflow),
            _ => file.unreadable.push(raw),
        }
    }
    Some(file)
}

/// workflows.json as written by the app; anything malformed is left out.
pub fn parse_workflows(text: &str) -> Vec<Workflow> {
    read_workflow_file(text)
        .map(|f| f.workflows)
        .unwrap_or_default()
}
