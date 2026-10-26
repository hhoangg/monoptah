//! The workflow definition and run state: a graph of nodes a Mono runs for the user.
//!
//! Field names and shapes match the React app's `workflow.ts`, so `workflows.json` and the run
//! files are shared between both apps. Definitions never hold absolute paths or session ids.
use super::ordered::{serialize_number, serialize_opt_number, write_number, Json, OrderedMap};
use super::parse;
use crate::mono::habits::Schedule;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub type WorkflowId = String;
/// Matches `is_node_id`, so it is safe inside a `{{nodeId.field}}` path.
pub type NodeId = String;
/// "" for top level, `<forEachId>#<index>` inside a group, archived `<forEachId>#<index>@r<round>`.
pub type BranchKey = String;

pub const WORKFLOWS_MAX: usize = 20;
/// The longest time limit a session or command node may have: 24 hours.
pub const MAX_TIMEOUT_MINUTES: u32 = 1440;
pub const WORKFLOW_RUNS_KEPT: usize = 20;
/// The most iterations of a For each that run at once.
pub const MAX_PARALLEL_LIMIT: u32 = 10;
/// How many run at once when `maxParallel` is unset.
pub const DEFAULT_MAX_PARALLEL: u32 = 3;
/// The most passes a loop edge may allow (`maxRounds`).
pub const MAX_ROUNDS_LIMIT: u32 = 10;
pub const DEFAULT_SESSION_TIMEOUT_MINUTES: u32 = 120;
pub const DEFAULT_COMMAND_TIMEOUT_MINUTES: u32 = 10;
/// Limits on a definition sent through the app CLI.
pub const WORKFLOW_NAME_MAX: usize = 80;
pub const WORKFLOW_DESCRIPTION_MAX: usize = 1_000;
pub const WORKFLOW_DEFINITION_MAX_BYTES: usize = 512 * 1024;

pub const NODE_TYPES: [&str; 8] = [
    "schedule", "manual", "session", "command", "askMono", "if", "forEach", "report",
];

/// `^[a-z][a-z0-9_]{0,39}$`
pub fn is_node_id(id: &str) -> bool {
    let b = id.as_bytes();
    (1..=40).contains(&b.len())
        && b[0].is_ascii_lowercase()
        && b[1..]
            .iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_')
}

const BASE36: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";

/// "wf-" followed by 8 lowercase base36 characters.
pub fn new_workflow_id() -> WorkflowId {
    let bytes: [u8; 8] = rand::random();
    // 256 is not a multiple of 36, so the modulo skews slightly; an id is a label, not a secret.
    let id: String = bytes
        .iter()
        .map(|b| BASE36[(*b % 36) as usize] as char)
        .collect();
    format!("wf-{id}")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputFieldType {
    String,
    Number,
    Boolean,
    List,
    Object,
}

impl OutputFieldType {
    pub const ALL: [(&str, Self); 5] = [
        ("string", Self::String),
        ("number", Self::Number),
        ("boolean", Self::Boolean),
        ("list", Self::List),
        ("object", Self::Object),
    ];
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputField {
    #[serde(rename = "type")]
    pub ty: OutputFieldType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_report: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl OutputField {
    pub fn new(ty: OutputFieldType) -> Self {
        Self {
            ty,
            in_report: None,
            description: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Checkout {
    Root,
    NewWorktree,
    WorktreeOf(NodeId),
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Cwd {
    Root,
    WorktreeOf(NodeId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum InputType {
    Project,
    String,
    Number,
    Boolean,
}

impl InputType {
    pub const ALL: [(&str, Self); 4] = [
        ("project", Self::Project),
        ("string", Self::String),
        ("number", Self::Number),
        ("boolean", Self::Boolean),
    ];
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct WorkflowInput {
    #[serde(rename = "type")]
    pub ty: InputType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl WorkflowInput {
    pub fn new(ty: InputType) -> Self {
        Self {
            ty,
            description: None,
        }
    }
}

/// The value of a declared input.
#[derive(Clone, Debug, PartialEq)]
pub enum InputValue {
    String(String),
    Number(f64),
    Bool(bool),
}

impl Serialize for InputValue {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::String(v) => s.serialize_str(v),
            Self::Number(v) => write_number(*v, s),
            Self::Bool(v) => s.serialize_bool(*v),
        }
    }
}

/// Board position, kept so an exported file opens with the same layout.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Ui {
    #[serde(serialize_with = "serialize_number")]
    pub x: f64,
    #[serde(serialize_with = "serialize_number")]
    pub y: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Decide {
    #[serde(rename = "mono")]
    Mono,
}

/// What a node is. The `type` tag and the fields sit next to the base fields in the JSON.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum NodeKind {
    Schedule {
        schedule: Schedule,
    },
    Manual,
    #[serde(rename_all = "camelCase")]
    Session {
        /// Always `{{inputs.<name>}}` naming a `project` input.
        project: String,
        prompt: String,
        checkout: Checkout,
        output: OrderedMap<OutputField>,
        #[serde(skip_serializing_if = "Option::is_none")]
        harness: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        effort: Option<String>,
        /// Keep one session across loop rounds. Default true.
        #[serde(skip_serializing_if = "Option::is_none")]
        reuse_session: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        nest_under: Option<NodeId>,
        /// Default 120.
        #[serde(
            skip_serializing_if = "Option::is_none",
            serialize_with = "serialize_opt_number"
        )]
        timeout_minutes: Option<f64>,
    },
    #[serde(rename_all = "camelCase")]
    Command {
        project: String,
        command: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cwd: Option<Cwd>,
        /// Default 10.
        #[serde(
            skip_serializing_if = "Option::is_none",
            serialize_with = "serialize_opt_number"
        )]
        timeout_minutes: Option<f64>,
    },
    AskMono {
        question: String,
        answers: Vec<String>,
    },
    /// Either `condition` or `decide: "mono"` (with an optional `question`).
    If {
        #[serde(skip_serializing_if = "Option::is_none")]
        condition: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        decide: Option<Decide>,
        #[serde(skip_serializing_if = "Option::is_none")]
        question: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    ForEach {
        /// A list expression such as `{{fetch.tickets}}`.
        #[serde(rename = "in")]
        list: String,
        /// Name of the loop variable. Default "item".
        #[serde(rename = "as", skip_serializing_if = "Option::is_none")]
        item: Option<String>,
        /// 1 to 10. Default 3.
        #[serde(
            skip_serializing_if = "Option::is_none",
            serialize_with = "serialize_opt_number"
        )]
        max_parallel: Option<f64>,
    },
    Report {
        #[serde(skip_serializing_if = "Option::is_none")]
        template: Option<String>,
    },
}

impl NodeKind {
    /// The `type` tag as written in the file.
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Schedule { .. } => "schedule",
            Self::Manual => "manual",
            Self::Session { .. } => "session",
            Self::Command { .. } => "command",
            Self::AskMono { .. } => "askMono",
            Self::If { .. } => "if",
            Self::ForEach { .. } => "forEach",
            Self::Report { .. } => "report",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Node {
    pub id: NodeId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The For each node this node runs inside.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<NodeId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ui: Option<Ui>,
    #[serde(flatten)]
    pub kind: NodeKind,
}

impl Node {
    pub fn new(id: impl Into<NodeId>, kind: NodeKind) -> Self {
        Self {
            id: id.into(),
            title: None,
            parent: None,
            ui: None,
            kind,
        }
    }
    pub fn is_trigger(&self) -> bool {
        matches!(self.kind, NodeKind::Schedule { .. } | NodeKind::Manual)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Handle {
    #[serde(rename = "true")]
    True,
    #[serde(rename = "false")]
    False,
}

/// A back edge: after the source node, the decider may send the branch back to `to`.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoopSpec {
    /// 1 to 10.
    #[serde(serialize_with = "serialize_number")]
    pub max_rounds: f64,
    /// "mono", or a condition such as `{{review.verdict}} != "approve"`.
    pub decide: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Edge {
    pub from: NodeId,
    pub to: NodeId,
    /// Only on the out edges of an If node.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handle: Option<Handle>,
    #[serde(rename = "loop", skip_serializing_if = "Option::is_none")]
    pub loop_spec: Option<LoopSpec>,
}

impl Edge {
    pub fn new(from: impl Into<NodeId>, to: impl Into<NodeId>) -> Self {
        Self {
            from: from.into(),
            to: to.into(),
            handle: None,
            loop_spec: None,
        }
    }
}

fn is_false(v: &bool) -> bool {
    !*v
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Workflow {
    pub id: WorkflowId,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub inputs: OrderedMap<WorkflowInput>,
    /// Values for the declared inputs. Not exported.
    pub input_values: OrderedMap<InputValue>,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub enabled: bool,
    pub created_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_run_at: Option<u64>,
    #[serde(skip_serializing_if = "is_false")]
    pub run_requested: bool,
    /// Requests made from another window, kept in the file because only the main window drives
    /// runs; the driver clears each once it has acted on it.
    #[serde(skip_serializing_if = "is_false")]
    pub stop_requested: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resume_run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub imported_from: Option<String>,
    /// Set on a workflow made from a file (by the Mono through the CLI, or imported in the UI).
    /// Only the user, in the panel, may enable or run it, and only an approval of this exact
    /// version clears the mark. Never exported.
    #[serde(skip_serializing_if = "is_false")]
    pub needs_review: bool,
}

impl Workflow {
    /// A disabled, empty workflow.
    pub fn new(id: impl Into<WorkflowId>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            description: None,
            inputs: OrderedMap::new(),
            input_values: OrderedMap::new(),
            nodes: Vec::new(),
            edges: Vec::new(),
            enabled: false,
            created_at: 0,
            next_run_at: None,
            run_requested: false,
            stop_requested: false,
            resume_run_id: None,
            last_run_id: None,
            imported_from: None,
            needs_review: false,
        }
    }
    pub fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }
    /// The workflow's trigger node, when it has one.
    pub fn trigger(&self) -> Option<&Node> {
        self.nodes.iter().find(|n| n.is_trigger())
    }
}

/// The workflow's trigger node, when it has one.
pub fn trigger_of(w: &Workflow) -> Option<&Node> {
    w.trigger()
}

// A workflow (or part of one) read from a run file is parsed as leniently as workflows.json is.
macro_rules! lenient_deserialize {
    ($ty:ty, $parse:path) => {
        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let raw = super::ordered::Json::deserialize(d)?;
                $parse(&raw).ok_or_else(|| {
                    serde::de::Error::custom(concat!("unreadable ", stringify!($ty)))
                })
            }
        }
    };
}
lenient_deserialize!(Workflow, parse::parse_workflow);
lenient_deserialize!(Node, parse::parse_node);
lenient_deserialize!(Edge, parse::parse_edge);

/// `value` as JSON with object keys sorted, like the React app's `stableJson`.
fn stable_json(value: &Value) -> String {
    match value {
        Value::Array(items) => {
            let items: Vec<String> = items.iter().map(stable_json).collect();
            format!("[{}]", items.join(","))
        }
        Value::Object(map) => {
            let mut members: Vec<(&String, &Value)> = map.iter().collect();
            members.sort_by(|a, b| a.0.cmp(b.0));
            let members: Vec<String> = members
                .into_iter()
                .map(|(k, v)| {
                    format!(
                        "{}:{}",
                        serde_json::to_string(k).unwrap_or_default(),
                        stable_json(v)
                    )
                })
                .collect();
            format!("{{{}}}", members.join(","))
        }
        other => serde_json::to_string(other).unwrap_or_else(|_| "null".into()),
    }
}

/// A stable serialization of everything in a workflow that decides what runs or what reaches an
/// agent: its name, description, input declarations, nodes (every field but their board position)
/// and edges, loops included. Two workflows with the same fingerprint show the same review. It
/// leaves out input values and the fields the app manages (enabled, schedule state, run state).
pub fn definition_fingerprint(w: &Workflow) -> String {
    let nodes: Vec<Value> = w
        .nodes
        .iter()
        .map(|node| {
            let mut value = serde_json::to_value(node).unwrap_or(Value::Null);
            if let Value::Object(map) = &mut value {
                map.remove("ui");
            }
            value
        })
        .collect();
    let mut root = serde_json::Map::new();
    root.insert("name".into(), Value::String(w.name.clone()));
    root.insert(
        "description".into(),
        Value::String(w.description.clone().unwrap_or_default()),
    );
    root.insert(
        "inputs".into(),
        serde_json::to_value(&w.inputs).unwrap_or(Value::Null),
    );
    root.insert("nodes".into(), Value::Array(nodes));
    root.insert(
        "edges".into(),
        serde_json::to_value(&w.edges).unwrap_or(Value::Null),
    );
    stable_json(&Value::Object(root))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NodeStatus {
    Pending,
    Running,
    WaitingApproval,
    Done,
    Failed,
    Skipped,
    Stopped,
}

/// The parts of a For each branch key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Branch {
    pub group_id: NodeId,
    pub index: usize,
    /// Set on archived branches.
    pub round: Option<u32>,
}

fn digits(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())
}

/// `^(.+)#(\d+)(?:@r(\d+))?$`, or None for the top level.
pub fn parse_branch(branch: &str) -> Option<Branch> {
    // The tail holds no `#`, so the last `#` is the only one a greedy `.+` can stop at.
    let (group, tail) = branch.rsplit_once('#')?;
    if group.is_empty() || group.contains(['\n', '\r', '\u{2028}', '\u{2029}']) {
        return None;
    }
    let (index, round) = match tail.split_once("@r") {
        Some((index, round)) if digits(round) => (index, Some(round.parse().ok()?)),
        Some(_) => return None,
        None => (tail, None),
    };
    digits(index).then_some(())?;
    Some(Branch {
        group_id: group.to_string(),
        index: index.parse().ok()?,
        round,
    })
}

pub fn is_archived_branch(branch: &str) -> bool {
    parse_branch(branch).is_some_and(|b| b.round.is_some())
}

pub fn archived_branch(branch: &str, round: u32) -> BranchKey {
    format!("{branch}@r{round}")
}

/// Where the item list of a For each is kept once a loop round archived it.
pub fn archived_items_key(group_id: &str, round: u32) -> NodeId {
    format!("{group_id}@r{round}")
}

/// What a decider chose, and why.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Decision {
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// What a node did in one round of a loop, kept when the loop sends the branch back for the next.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoundRecord {
    pub round: u32,
    pub status: NodeStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<Decision>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeRun {
    pub node_id: NodeId,
    pub branch: BranchKey,
    pub status: NodeStatus,
    pub round: u32,
    /// The persisted id of the session the node ran in (a string in both apps).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// The loop session (round 2 or later) of a node that was interrupted while running, so a
    /// resumed run continues in it instead of opening a new session and worktree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reuse_session_id: Option<String>,
    /// Key order is kept as written (a prompt shows it), so this is not a serde_json map.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<OrderedMap<Json>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<Decision>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasked: Option<bool>,
    /// The rounds a loop already finished, oldest first. Never includes the current round.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history: Option<Vec<RoundRecord>>,
}

impl NodeRun {
    pub fn new(node_id: impl Into<NodeId>, branch: impl Into<BranchKey>) -> Self {
        Self {
            node_id: node_id.into(),
            branch: branch.into(),
            status: NodeStatus::Pending,
            round: 1,
            session_id: None,
            reuse_session_id: None,
            output: None,
            error: None,
            decision: None,
            started_at: None,
            ended_at: None,
            reasked: None,
            history: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RunStatus {
    Running,
    Done,
    DoneWithFailures,
    Failed,
    Stopped,
    Interrupted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RunTrigger {
    Schedule,
    Manual,
    Cli,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRun {
    pub id: String,
    pub workflow_id: WorkflowId,
    /// Snapshot of the definition the run started from.
    pub definition: Workflow,
    pub status: RunStatus,
    pub started_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<u64>,
    pub trigger: RunTrigger,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder_id: Option<String>,
    /// The lists For each nodes expanded, by node id (`<id>@r<round>` for an archived round).
    #[serde(default)]
    pub items: BTreeMap<NodeId, Vec<Json>>,
    #[serde(default)]
    pub nodes: Vec<NodeRun>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report_posted_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
