//! Sharing a workflow as a file: YAML (or JSON) export, and the import a user reviews before
//! anything runs. Port of the React app's `workflowYaml.ts`; a file written by either app reads
//! in the other.
//!
//! The file holds name, description, input declarations (never their values), nodes with board
//! positions and edges. Every string in it is checked for absolute paths, so a file can be
//! handed to someone else.
use super::model::{
    new_workflow_id, Checkout, Cwd, Decide, Edge, InputValue, Node, NodeId, NodeKind, OutputField,
    OutputFieldType, Requirement, Workflow, WorkflowId, WorkflowInput, NODE_TYPES,
};
use super::models::{workflow_issues, ModelCatalog};
use super::ordered::{Json, OrderedMap};
use super::output::output_instruction;
use super::parse::{drop_stray_handles, parse_edge, parse_inputs, parse_node, read_requires};
use super::template::{js_space, js_trim};
use super::validate::{find_absolute_path, WorkflowIssue};
use super::yaml_emit::{stringify, Doc};
use super::yaml_read::{parse_yaml, ReadError};
use crate::mono::habits::Schedule;

/// The value of the top-level `monoptahWorkflow` key.
pub const WORKFLOW_FILE_VERSION: u32 = 1;
/// Larger files are refused before they are parsed.
pub const MAX_FILE_BYTES: usize = 512 * 1024;
const NESTING_MAX: usize = super::yaml_read::NESTING_MAX;
const TOO_DEEP: &str = "The workflow file is nested too deeply";

// --- export --------------------------------------------------------------------------------

fn text(s: &str) -> Doc {
    Doc::Str(s.to_string())
}

/// Appends `key: value` when there is a value, like `defined({...})` drops what is undefined.
fn put(entries: &mut Vec<(String, Doc)>, key: &str, value: Option<Doc>) {
    if let Some(value) = value {
        entries.push((key.to_string(), value));
    }
}

fn number(n: impl Into<f64>) -> Doc {
    Doc::Num(n.into())
}

/// The file writes a schedule the way the spec does, `{ kind, time, ... }`; fields a schedule
/// keeps at their defaults are left out. The time is always quoted, because a YAML 1.1 reader
/// takes a bare 09:00 for a number.
fn export_schedule(s: &Schedule) -> Doc {
    let mut out = vec![
        ("kind".to_string(), text(&s.schedule_kind)),
        ("time".to_string(), Doc::Quoted(s.time.clone())),
    ];
    put(
        &mut out,
        "minute",
        (s.minute != 0).then(|| number(s.minute)),
    );
    put(
        &mut out,
        "dayOfWeek",
        (s.day_of_week != 1).then(|| number(s.day_of_week)),
    );
    put(&mut out, "hours", s.interval_hours.map(number));
    put(
        &mut out,
        "intervalFrom",
        s.interval_from.as_deref().map(text),
    );
    Doc::Map(out)
}

fn export_output(output: &OrderedMap<OutputField>) -> Doc {
    Doc::Map(
        output
            .iter()
            .map(|(name, f)| {
                let ty = OutputFieldType::ALL
                    .iter()
                    .find(|(_, t)| *t == f.ty)
                    .map_or("string", |(n, _)| n);
                let mut field = vec![("type".to_string(), text(ty))];
                put(&mut field, "inReport", f.in_report.map(Doc::Bool));
                put(
                    &mut field,
                    "description",
                    f.description.as_deref().map(text),
                );
                (name.clone(), Doc::Map(field))
            })
            .collect(),
    )
}

fn worktree_of(id: &str) -> Doc {
    Doc::Map(vec![("worktreeOf".to_string(), text(id))])
}

fn export_node(node: &Node) -> Doc {
    let mut out = vec![
        ("id".to_string(), text(&node.id)),
        ("type".to_string(), text(node.kind.type_name())),
    ];
    put(&mut out, "title", node.title.as_deref().map(text));
    put(&mut out, "parent", node.parent.as_deref().map(text));
    let ui = node.ui.map(|ui| {
        Doc::Map(vec![
            ("x".to_string(), number(ui.x)),
            ("y".to_string(), number(ui.y)),
        ])
    });
    match &node.kind {
        NodeKind::Schedule { schedule } => {
            put(&mut out, "schedule", Some(export_schedule(schedule)));
        }
        NodeKind::Manual => {}
        NodeKind::Session {
            project,
            prompt,
            checkout,
            output,
            harness,
            model,
            effort,
            reuse_session,
            nest_under,
            timeout_minutes,
        } => {
            put(&mut out, "project", Some(text(project)));
            put(&mut out, "harness", harness.as_deref().map(text));
            put(&mut out, "model", model.as_deref().map(text));
            put(&mut out, "effort", effort.as_deref().map(text));
            put(
                &mut out,
                "checkout",
                Some(match checkout {
                    Checkout::Root => text("root"),
                    Checkout::NewWorktree => text("newWorktree"),
                    Checkout::WorktreeOf(id) => worktree_of(id),
                }),
            );
            put(&mut out, "nestUnder", nest_under.as_deref().map(text));
            put(&mut out, "reuseSession", reuse_session.map(Doc::Bool));
            put(&mut out, "timeoutMinutes", timeout_minutes.map(number));
            put(&mut out, "prompt", Some(text(prompt)));
            put(&mut out, "output", Some(export_output(output)));
        }
        NodeKind::Command {
            project,
            command,
            cwd,
            timeout_minutes,
        } => {
            put(&mut out, "project", Some(text(project)));
            put(
                &mut out,
                "cwd",
                cwd.as_ref().map(|cwd| match cwd {
                    Cwd::Root => text("root"),
                    Cwd::WorktreeOf(id) => worktree_of(id),
                }),
            );
            put(&mut out, "timeoutMinutes", timeout_minutes.map(number));
            put(&mut out, "command", Some(text(command)));
        }
        NodeKind::AskMono { question, answers } => {
            put(&mut out, "question", Some(text(question)));
            put(
                &mut out,
                "answers",
                Some(Doc::Seq(answers.iter().map(|a| text(a)).collect())),
            );
        }
        NodeKind::If {
            condition,
            decide,
            question,
        } => {
            put(&mut out, "condition", condition.as_deref().map(text));
            put(&mut out, "decide", decide.map(|Decide::Mono| text("mono")));
            put(&mut out, "question", question.as_deref().map(text));
        }
        NodeKind::Switch {
            value,
            cases,
            decide,
            question,
        } => {
            put(&mut out, "value", value.as_deref().map(text));
            put(
                &mut out,
                "cases",
                Some(Doc::Seq(cases.iter().map(|c| text(c)).collect())),
            );
            put(&mut out, "decide", decide.map(|Decide::Mono| text("mono")));
            put(&mut out, "question", question.as_deref().map(text));
        }
        NodeKind::ForEach {
            list,
            item,
            max_parallel,
        } => {
            put(&mut out, "in", Some(text(list)));
            put(&mut out, "as", item.as_deref().map(text));
            put(&mut out, "maxParallel", max_parallel.map(number));
        }
        NodeKind::Report { template } => {
            put(&mut out, "template", template.as_deref().map(text));
        }
    }
    put(&mut out, "ui", ui);
    Doc::Map(out)
}

fn export_edge(edge: &Edge) -> Doc {
    let mut out = vec![
        ("from".to_string(), text(&edge.from)),
        ("to".to_string(), text(&edge.to)),
    ];
    put(
        &mut out,
        "handle",
        edge.handle.as_ref().map(|h| text(h.as_str())),
    );
    put(
        &mut out,
        "loop",
        edge.loop_spec.as_ref().map(|l| {
            Doc::Map(vec![
                ("maxRounds".to_string(), number(l.max_rounds)),
                ("decide".to_string(), text(&l.decide)),
            ])
        }),
    );
    Doc::Map(out)
}

fn export_input(input: &WorkflowInput) -> Doc {
    let ty = super::model::InputType::ALL
        .iter()
        .find(|(_, t)| *t == input.ty)
        .map_or("string", |(n, _)| n);
    let mut out = vec![("type".to_string(), text(ty))];
    put(
        &mut out,
        "description",
        input.description.as_deref().map(text),
    );
    Doc::Map(out)
}

fn export_requirement(r: &Requirement) -> Doc {
    let mut out = vec![(r.kind.key().to_string(), text(&r.name))];
    put(&mut out, "hint", r.hint.as_deref().map(text));
    Doc::Map(out)
}

/// The shareable definition: only name, description, inputs (declarations, no values),
/// requirements, nodes (with board positions) and edges. The output is deterministic.
fn export_document(w: &Workflow) -> Doc {
    let mut out = vec![
        (
            "monoptahWorkflow".to_string(),
            Doc::Num(WORKFLOW_FILE_VERSION.into()),
        ),
        ("name".to_string(), text(&w.name)),
    ];
    put(&mut out, "description", w.description.as_deref().map(text));
    out.push((
        "inputs".to_string(),
        Doc::Map(
            w.inputs
                .iter()
                .map(|(k, v)| (k.clone(), export_input(v)))
                .collect(),
        ),
    ));
    if !w.requires.is_empty() {
        out.push((
            "requires".to_string(),
            Doc::Seq(w.requires.iter().map(export_requirement).collect()),
        ));
    }
    out.push((
        "nodes".to_string(),
        Doc::Seq(w.nodes.iter().map(export_node).collect()),
    ));
    out.push((
        "edges".to_string(),
        Doc::Seq(w.edges.iter().map(export_edge).collect()),
    ));
    Doc::Map(out)
}

/// The workflow as YAML text. Long prompts stay on one line instead of being folded.
pub fn export_workflow(w: &Workflow) -> String {
    stringify(&export_document(w))
}

/// Why this workflow cannot be shared, when a string in it (name, description, input
/// descriptions, prompts, commands...) reads as an absolute path: the same rule `preview_import`
/// applies, so a file the app writes is a file it can read back. None when it is fine.
pub fn export_path_problem(w: &Workflow) -> Option<String> {
    let problem = find_path_field(&export_document(w).to_json(), "", 0)?;
    if problem == TOO_DEEP {
        return Some(problem);
    }
    Some(format!("{problem}; use a project input or a relative path"))
}

// --- import --------------------------------------------------------------------------------

/// The definition a file holds, before it gets an id, a creation time and input values.
#[derive(Clone, Debug, PartialEq)]
pub struct WorkflowDraft {
    pub name: String,
    pub description: Option<String>,
    pub inputs: OrderedMap<WorkflowInput>,
    pub requires: Vec<Requirement>,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

impl From<&Workflow> for WorkflowDraft {
    fn from(w: &Workflow) -> Self {
        Self {
            name: w.name.clone(),
            description: w.description.clone(),
            inputs: w.inputs.clone(),
            requires: w.requires.clone(),
            nodes: w.nodes.clone(),
            edges: w.edges.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewCommand {
    pub node_id: NodeId,
    pub command: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewPrompt {
    pub node_id: NodeId,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImportPreview {
    pub workflow: WorkflowDraft,
    /// Every command node's command, verbatim, for the user to review.
    pub commands: Vec<ReviewCommand>,
    /// Every text that reaches an agent, as the agent will read it before templates are filled
    /// in: a session's prompt followed by the output instruction (which carries the output
    /// field descriptions), the questions put to the Mono with their answers, and a report's
    /// template (the report is posted into the Mono's own session, so the Mono reads it as its
    /// own reply).
    pub prompts: Vec<ReviewPrompt>,
    pub issues: Vec<WorkflowIssue>,
}

/// What a user reviews before a workflow runs: every command verbatim, and every text that
/// reaches an agent as the agent will read it before templates are filled in.
pub fn review_texts(nodes: &[Node]) -> (Vec<ReviewCommand>, Vec<ReviewPrompt>) {
    let mut commands = Vec::new();
    let mut prompts = Vec::new();
    let mut prompt = |node: &Node, text: String| {
        prompts.push(ReviewPrompt {
            node_id: node.id.clone(),
            text,
        })
    };
    for node in nodes {
        match &node.kind {
            NodeKind::Command { command, .. } => commands.push(ReviewCommand {
                node_id: node.id.clone(),
                command: command.clone(),
            }),
            NodeKind::Session {
                prompt: p, output, ..
            } => {
                let instruction = output_instruction(output);
                prompt(
                    node,
                    if instruction.is_empty() {
                        p.clone()
                    } else {
                        format!("{p}\n\n{instruction}")
                    },
                );
            }
            NodeKind::AskMono { question, answers } => {
                let answers = answers
                    .iter()
                    .map(|a| format!("- {a}"))
                    .collect::<Vec<_>>()
                    .join("\n");
                prompt(
                    node,
                    if answers.is_empty() {
                        question.clone()
                    } else {
                        format!("{question}\n\nAnswers:\n{answers}")
                    },
                );
            }
            NodeKind::If {
                question: Some(question),
                ..
            }
            | NodeKind::Switch {
                question: Some(question),
                ..
            } => prompt(node, question.clone()),
            NodeKind::Report {
                template: Some(template),
            } if !js_trim(template).is_empty() => prompt(node, template.clone()),
            _ => {}
        }
    }
    (commands, prompts)
}

/// The review of a workflow already stored, built from its current definition.
pub fn review_of(w: &Workflow, catalog: &ModelCatalog) -> ImportPreview {
    let (commands, prompts) = review_texts(&w.nodes);
    ImportPreview {
        workflow: WorkflowDraft::from(w),
        commands,
        prompts,
        issues: workflow_issues(w, catalog),
    }
}

/// The parsed file, or the message to show when it cannot be read.
fn parse_file(text: &str) -> Result<Json, String> {
    if text.len() > MAX_FILE_BYTES {
        return fail("Workflow file is too large (over 512 KiB)");
    }
    if js_trim(text).is_empty() {
        return fail("Workflow file is empty");
    }
    if text.trim_start_matches(js_space).starts_with('{') {
        if let Some(value) = Json::parse(text) {
            return Ok(value);
        }
        // Not JSON after all; the YAML reader reports what is wrong.
    }
    parse_yaml(text).map_err(|err| match err {
        ReadError::TooDeep => TOO_DEEP.to_string(),
        ReadError::Message(m) => format!(
            "Could not read the workflow file: {}",
            m.lines().next().unwrap_or("")
        ),
    })
}

/// The first string anywhere in `value` that reads as an absolute path, with where it sits.
/// Nesting past 64 levels is reported as `TOO_DEEP` rather than followed.
fn find_path_field(value: &Json, label: &str, depth: usize) -> Option<String> {
    if depth > NESTING_MAX {
        return Some(TOO_DEEP.to_string());
    }
    match value {
        Json::String(s) => find_absolute_path(s)
            .map(|found| format!("{label} contains an absolute path (\"{found}\")")),
        Json::Array(items) => items
            .iter()
            .enumerate()
            .find_map(|(i, item)| find_path_field(item, &format!("{label}[{i}]"), depth + 1)),
        Json::Object(map) => map.iter().find_map(|(key, item)| {
            let at = if label.is_empty() {
                key.clone()
            } else {
                format!("{label}.{key}")
            };
            find_path_field(item, &at, depth + 1)
        }),
        _ => None,
    }
}

fn describe_version(v: Option<&Json>) -> String {
    match v {
        Some(Json::Number(n)) => super::ordered::js_number_text(*n),
        Some(Json::String(s)) => s.clone(),
        _ => "missing".to_string(),
    }
}

/// `{ ...schedule, scheduleKind: kind, intervalHours: hours ?? intervalHours }`: the file's
/// friendly `kind`/`hours` become the stored names. A schedule that already says `scheduleKind`
/// or has no `kind` is left as it is.
fn import_schedule(raw: Option<&Json>) -> Option<Json> {
    let raw = raw?;
    let map = raw.as_object()?;
    if map.contains_key("scheduleKind") || !map.contains_key("kind") {
        return Some(raw.clone());
    }
    let mut out = OrderedMap::new();
    for (key, value) in map.iter() {
        if key != "kind" && key != "hours" {
            out.insert(key.clone(), value.clone());
        }
    }
    out.insert(
        "scheduleKind",
        map.get("kind").cloned().unwrap_or(Json::Null),
    );
    // `hours ?? intervalHours`: a null counts as missing.
    if let Some(hours) = map.get("hours").filter(|h| **h != Json::Null) {
        out.insert("intervalHours", hours.clone());
    }
    Some(Json::Object(out))
}

fn fail<T>(message: impl Into<String>) -> Result<T, String> {
    Err(message.into())
}

/// Reads YAML or JSON. Validation problems are reported in `issues`, not as a rejection.
/// `catalog` is what the session nodes' models are judged against.
pub fn preview_import(text: &str, catalog: &ModelCatalog) -> Result<ImportPreview, String> {
    let file = parse_file(text)?;
    let Json::Object(file) = file else {
        return fail("Not a Monoptah workflow file");
    };
    let file_json = Json::Object(file.clone());

    if file.get("monoptahWorkflow") != Some(&Json::Number(WORKFLOW_FILE_VERSION.into())) {
        return fail(format!(
            "Unsupported workflow file version: {} (this app reads version {WORKFLOW_FILE_VERSION})",
            describe_version(file.get("monoptahWorkflow"))
        ));
    }
    if let Some(problem) = find_path_field(&file_json, "", 0) {
        if problem == TOO_DEEP {
            return fail(TOO_DEEP);
        }
        return fail(format!("{problem}; use a project input or a relative path"));
    }

    let name = match file.get("name") {
        Some(Json::String(name)) if !js_trim(name).is_empty() => name.clone(),
        _ => return fail("The workflow file needs a name"),
    };
    let description = match file.get("description") {
        None => None,
        Some(Json::String(d)) => Some(d.clone()),
        Some(_) => return fail("description must be text"),
    };
    let Some(Json::Array(raw_nodes)) = file.get("nodes") else {
        return fail("The workflow file needs a list of nodes");
    };

    let mut nodes = Vec::new();
    for (index, raw) in raw_nodes.iter().enumerate() {
        let id = raw
            .get("id")
            .and_then(Json::as_str)
            .filter(|id| !id.is_empty());
        let (Json::Object(fields), Some(id)) = (raw, id) else {
            return fail(format!("nodes[{index}] needs an id"));
        };
        let ty = fields.get("type").and_then(Json::as_str);
        if !ty.is_some_and(|t| NODE_TYPES.contains(&t)) {
            let shown = fields
                .get("type")
                .map_or_else(|| "missing".to_string(), |t| t.stringify(false));
            return fail(format!("Node \"{id}\" has an unknown type: {shown}"));
        }
        let prepared = if ty == Some("schedule") {
            let mut with_schedule = fields.clone();
            match import_schedule(fields.get("schedule")) {
                Some(schedule) => with_schedule.insert("schedule", schedule),
                None => {
                    with_schedule.remove("schedule");
                }
            }
            Json::Object(with_schedule)
        } else {
            raw.clone()
        };
        let Some(node) = parse_node(&prepared) else {
            return fail(format!(
                "Node \"{id}\" is missing a required field or has one of the wrong type"
            ));
        };
        nodes.push(node);
    }

    let raw_edges: &[Json] = match file.get("edges") {
        None => &[],
        Some(Json::Array(edges)) => edges,
        Some(_) => return fail("edges must be a list"),
    };
    let mut edges = Vec::new();
    for (index, raw) in raw_edges.iter().enumerate() {
        let Some(edge) = parse_edge(raw) else {
            return fail(format!(
                "edge {} needs \"from\" and \"to\" (and a valid loop, if any)",
                index + 1
            ));
        };
        edges.push(edge);
    }
    drop_stray_handles(&nodes, &mut edges);

    let requires = read_requires(file.get("requires"))?;
    let workflow = WorkflowDraft {
        name,
        description,
        inputs: parse_inputs(file.get("inputs")),
        requires,
        nodes,
        edges,
    };
    let mut preview = Workflow::new("wf-preview", workflow.name.clone());
    preview.description = workflow.description.clone();
    preview.inputs = workflow.inputs.clone();
    preview.requires = workflow.requires.clone();
    preview.nodes = workflow.nodes.clone();
    preview.edges = workflow.edges.clone();
    let issues = workflow_issues(&preview, catalog);
    let (commands, prompts) = review_texts(&workflow.nodes);
    Ok(ImportPreview {
        workflow,
        commands,
        prompts,
        issues,
    })
}

// --- review text ---------------------------------------------------------------------------

/// The text with the characters that change how it reads or what it looks like, without
/// showing, made visible as `<U+XXXX>`: every format character (bidirectional controls,
/// zero-width characters, tag characters, soft hyphens), the line and paragraph separators, a
/// carriage return that is not half of a line break, and the other control characters. What is
/// reviewed is then what runs. Port of `visible()` in `WorkflowReviewSections.tsx`: a character
/// outside the BMP is one marker with its full number.
pub fn visible(text: &str) -> String {
    static FORMAT: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"^\p{Cf}$").expect("a valid pattern"));
    let hidden = |c: char, next: Option<char>| {
        if c.is_ascii() {
            return match c {
                '\r' => next != Some('\n'),
                '\n' | '\t' => false,
                _ => c.is_control(),
            };
        }
        c == '\u{2028}'
            || c == '\u{2029}'
            || c.is_control()
            || FORMAT.is_match(c.encode_utf8(&mut [0; 4]))
    };
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if hidden(c, chars.peek().copied()) {
            out.push_str(&format!("<U+{:04X}>", c as u32));
        } else {
            out.push(c);
        }
    }
    out
}

// --- values and finishing ------------------------------------------------------------------

/// What is wrong with one input's value, if anything.
fn input_value_problem(
    name: &str,
    input: &WorkflowInput,
    value: Option<&InputValue>,
) -> Option<String> {
    use super::model::InputType;
    let Some(value) = value else {
        return Some(format!("Missing a value for input \"{name}\""));
    };
    match input.ty {
        InputType::Project => match value {
            InputValue::String(s) if !js_trim(s).is_empty() => None,
            _ => Some(format!("Input \"{name}\" needs a project")),
        },
        InputType::String => match value {
            InputValue::String(_) => None,
            _ => Some(format!("Input \"{name}\" must be text")),
        },
        InputType::Number => match value {
            InputValue::Number(n) if n.is_finite() => None,
            _ => Some(format!("Input \"{name}\" must be a number")),
        },
        InputType::Boolean => match value {
            InputValue::Bool(_) => None,
            _ => Some(format!("Input \"{name}\" must be true or false")),
        },
    }
}

/// Every way `values` does not fit the declared `inputs`: a declared input with no value or one
/// of the wrong type (an empty project counts), and a value for an input that is not declared.
/// The one rule behind imports, `workflows.setInputs` and validating a workflow before it is
/// enabled.
pub fn input_value_problems(
    inputs: &OrderedMap<WorkflowInput>,
    values: &OrderedMap<InputValue>,
) -> Vec<String> {
    let mut problems: Vec<String> = inputs
        .iter()
        .filter_map(|(name, input)| input_value_problem(name, input, values.get(name)))
        .collect();
    problems.extend(
        values
            .keys()
            .filter(|name| !inputs.contains_key(name))
            .map(|name| format!("Unknown input \"{name}\"; the workflow declares no such input")),
    );
    problems
}

/// An interval schedule counts from the day it was created or imported.
pub fn anchor_node(mut node: Node, now: u64) -> Node {
    if let NodeKind::Schedule { schedule } = &mut node.kind {
        if schedule.interval_from.as_deref().is_none_or(str::is_empty) {
            *schedule = std::mem::take(schedule).anchor(now);
        }
    }
    node
}

/// The workflow to save after the user reviewed the preview: a new id, paused, with a value for
/// every declared input. The message names the input when a value is missing or has the wrong
/// type. The caller marks it as needing review and records where it came from.
pub fn finish_import(
    preview: &ImportPreview,
    input_values: &OrderedMap<InputValue>,
    now: u64,
) -> Result<Workflow, String> {
    finish_import_with_id(preview, input_values, now, new_workflow_id())
}

/// `finish_import` with the id chosen by the caller.
pub fn finish_import_with_id(
    preview: &ImportPreview,
    input_values: &OrderedMap<InputValue>,
    now: u64,
    id: WorkflowId,
) -> Result<Workflow, String> {
    // Values for inputs the file does not declare are dropped, not an error.
    let mut values = OrderedMap::new();
    for name in preview.workflow.inputs.keys() {
        if let Some(value) = input_values.get(name) {
            values.insert(name.clone(), value.clone());
        }
    }
    if let Some(problem) = input_value_problems(&preview.workflow.inputs, &values)
        .into_iter()
        .next()
    {
        return Err(problem);
    }
    let draft = preview.workflow.clone();
    let mut workflow = Workflow::new(id, draft.name);
    workflow.description = draft.description;
    workflow.inputs = draft.inputs;
    workflow.requires = draft.requires;
    workflow.input_values = values;
    workflow.nodes = draft.nodes;
    workflow.edges = draft.edges;
    workflow.created_at = now;
    Ok(workflow)
}
