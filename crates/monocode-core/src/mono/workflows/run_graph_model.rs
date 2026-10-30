//! The pure side of the run graph: a run's definition snapshot as flow nodes and edges, each
//! carrying what the run did with it. Port of `runGraphModel.ts`. A For each is drawn either as
//! one lane per item (side by side) or as one lane at a time; the lane's child nodes get ids that
//! name their branch (`<node>@<branch>`), so the same definition node can appear in every lane.
use super::board_model::{to_flow, FlowEdge, FlowKind, FlowNode, Point, GROUP_PAD, NODE_W};
use super::engine::is_asking;
use super::model::{
    archived_branch, archived_items_key, parse_branch, stable_json, BranchKey, Edge, Node, NodeId,
    NodeKind, NodeRun, NodeStatus, RunStatus, Workflow, WorkflowRun,
};
use super::ordered::Json;
use super::template::{js_space, js_trim};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap, HashSet};

/// How one node stands in a run, as the card shows it. `Waiting` is a node waiting for approval
/// or asking the Mono.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunNodeStatus {
    Done,
    Running,
    Waiting,
    Failed,
    Skipped,
    Pending,
    Stopped,
}

impl RunNodeStatus {
    /// The status as the React app names it (also the word in a For each summary).
    pub fn name(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::Running => "running",
            Self::Waiting => "waiting",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
            Self::Pending => "pending",
            Self::Stopped => "stopped",
        }
    }
}

/// What the run did with one node (or one lane's copy of it).
#[derive(Clone, Debug, PartialEq)]
pub struct RunNodeState {
    pub status: RunNodeStatus,
    /// `<nodeId>|<branch>`, the key `node_runs_by_key` uses; selects a node.
    pub node_run_key: String,
    pub round: u32,
    /// The card's second line, e.g. "Done · 12s", "Running 3m", "Done · true", "Failed: <error>".
    pub label: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunEdgeState {
    Taken,
    Active,
    Idle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunView {
    Lanes,
    Single,
}

/// The React app's localStorage key prefix for the view a workflow's runs were last shown in.
pub const VIEW_KEY: &str = "monocode.workflowRunView.";

impl RunView {
    /// The word stored for the view, as the React app writes it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Lanes => "lanes",
            Self::Single => "single",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "lanes" => Some(Self::Lanes),
            "single" => Some(Self::Single),
            _ => None,
        }
    }
}

/// The view picked last: this app's own choice, else the one the desktop app stored.
pub fn chosen_view(native: Option<&str>, desktop: Option<&str>) -> Option<RunView> {
    native
        .and_then(RunView::parse)
        .or_else(|| desktop.and_then(RunView::parse))
}

/// Whether the run has a For each, the only thing the two views draw differently.
pub fn has_for_each(run: &WorkflowRun) -> bool {
    run.definition
        .nodes
        .iter()
        .any(|n| matches!(n.kind, NodeKind::ForEach { .. }))
}

/// The parts of a definition that change what a run does (inputs, nodes without their board
/// position, edges); the name and the board positions do not count.
fn run_definition_key(w: &Workflow) -> String {
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

/// Whether the workflow was edited after the run took its copy.
pub fn definition_changed(run: &WorkflowRun, current: &Workflow) -> bool {
    run_definition_key(&run.definition) != run_definition_key(current)
}

/// One item of a For each in the selected round.
#[derive(Clone, Debug, PartialEq)]
pub struct GroupItem {
    pub index: usize,
    pub title: String,
    pub status: RunNodeStatus,
}

/// A For each as the run page shows it: its rounds, the selected round's items, the selected
/// item for the single view, and a one-line summary.
#[derive(Clone, Debug, PartialEq)]
pub struct GroupView {
    pub group_id: NodeId,
    /// Rounds this For each expanded in, ascending; empty when it never expanded.
    pub rounds: Vec<u32>,
    /// The selected round (default the latest).
    pub round: u32,
    pub items: Vec<GroupItem>,
    /// The selected item for the single view (default the first running one, else the first).
    pub item: usize,
    /// "2 done · 1 running · 1 failed", "No items", or the failure text.
    pub summary: String,
}

/// The round and item the user picked for a For each; None takes the default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pick {
    pub round: Option<u32>,
    pub item: Option<usize>,
}

pub type Picks = HashMap<NodeId, Pick>;

/// The nodes and edges of a run graph. Lane children are `FlowNode`s with `parent_id` set to the
/// For each and the `@<branch>` id; every node has `data.run` and every edge has `run`.
#[derive(Clone, Debug, PartialEq)]
pub struct RunGraph {
    pub nodes: Vec<FlowNode>,
    pub edges: Vec<FlowEdge>,
}

/// Space above the lanes of a group, for lane titles or item tabs.
pub const LANE_HEADER: f32 = 28.0;
/// Width of a card in the lanes view: four lanes of two nodes fit the panel on the first frame.
pub const LANE_CARD_W: f32 = 92.0;
/// Space between two lanes; wide enough for the loop arc that runs down a lane's right.
pub const LANE_GAP: f32 = 26.0;
/// Room right of the last lane for that arc.
const LANE_EDGE: f32 = 12.0;

const LANE_SCALE: f32 = LANE_CARD_W / NODE_W;
const LANES_MAX_ITEMS: usize = 4;
const LANES_MAX_CHILDREN: usize = 3;
const LABEL_MAX: usize = 60;

pub fn node_run_key(node_id: &str, branch: &str) -> String {
    format!("{node_id}|{branch}")
}

pub fn node_runs_by_key(run: &WorkflowRun) -> HashMap<String, &NodeRun> {
    run.nodes
        .iter()
        .map(|n| (node_run_key(&n.node_id, &n.branch), n))
        .collect()
}

type ByKey<'a> = HashMap<String, &'a NodeRun>;

pub fn status_of(r: Option<&NodeRun>) -> RunNodeStatus {
    let Some(r) = r else {
        return RunNodeStatus::Pending;
    };
    match r.status {
        NodeStatus::WaitingApproval => RunNodeStatus::Waiting,
        NodeStatus::Running if is_asking(r) => RunNodeStatus::Waiting,
        NodeStatus::Running => RunNodeStatus::Running,
        NodeStatus::Pending => RunNodeStatus::Pending,
        NodeStatus::Done => RunNodeStatus::Done,
        NodeStatus::Failed => RunNodeStatus::Failed,
        NodeStatus::Skipped => RunNodeStatus::Skipped,
        NodeStatus::Stopped => RunNodeStatus::Stopped,
    }
}

/// "45s", "3m", "1m 5s". Under a second reads as 1s.
fn span(ms: i64) -> String {
    let seconds = (((ms as f64) / 1000.0 + 0.5).floor() as i64).max(1);
    if seconds < 60 {
        return format!("{seconds}s");
    }
    let (minutes, rest) = (seconds / 60, seconds % 60);
    if rest != 0 {
        format!("{minutes}m {rest}s")
    } else {
        format!("{minutes}m")
    }
}

/// `text` on one line, cut to 60 UTF-16 units with "..." like the React label.
fn short(text: &str) -> String {
    let mut flat = String::new();
    let mut in_space = false;
    for c in text.chars() {
        if js_space(c) {
            if !in_space {
                flat.push(' ');
            }
            in_space = true;
        } else {
            flat.push(c);
            in_space = false;
        }
    }
    let flat = js_trim(&flat);
    if flat.encode_utf16().count() <= LABEL_MAX {
        return flat.to_string();
    }
    let mut out = String::new();
    let mut units = 0;
    for c in flat.chars() {
        units += c.len_utf16();
        if units > LABEL_MAX - 3 {
            break;
        }
        out.push(c);
    }
    out + "..."
}

/// How long a node that has finished took, when it is long enough to say.
fn took(r: Option<&NodeRun>) -> Option<String> {
    let r = r?;
    let (start, end) = (r.started_at?, r.ended_at?);
    let ms = end as i64 - start as i64;
    (ms >= 1000).then(|| span(ms))
}

/// The card's second line for a node (or None for one the run has not reached) in `status`.
pub fn label_for(r: Option<&NodeRun>, status: RunNodeStatus, now: u64) -> String {
    let mut parts: Vec<String> = Vec::new();
    match status {
        RunNodeStatus::Done => {
            parts.push("Done".into());
            if let Some(decision) = r.and_then(|r| r.decision.as_ref()) {
                let reason = match decision.reason.as_deref() {
                    Some(reason) if !reason.is_empty() => format!(": {}", short(reason)),
                    _ => String::new(),
                };
                parts.push(format!("{}{reason}", decision.value));
            }
            parts.extend(took(r));
        }
        RunNodeStatus::Running => parts.push(match r.and_then(|r| r.started_at) {
            Some(start) => format!("Running {}", span(now as i64 - start as i64)),
            None => "Running".into(),
        }),
        RunNodeStatus::Waiting => parts.push(
            if r.is_some_and(is_asking) {
                "Asking the Mono"
            } else {
                "Waiting for approval"
            }
            .into(),
        ),
        RunNodeStatus::Failed => {
            parts.push(match r.and_then(|r| r.error.as_deref()) {
                Some(error) if !error.is_empty() => format!("Failed: {}", short(error)),
                _ => "Failed".into(),
            });
            parts.extend(took(r));
        }
        RunNodeStatus::Skipped => parts.push("Skipped".into()),
        RunNodeStatus::Stopped => {
            parts.push("Stopped".into());
            parts.extend(took(r));
        }
        RunNodeStatus::Pending => parts.push("Not started".into()),
    }
    if let Some(r) = r {
        if r.round > 1 {
            parts.push(format!("round {}", r.round));
        }
    }
    parts.join(" · ")
}

fn state_of(by_key: &ByKey, node_id: &str, branch: &str, now: u64) -> RunNodeState {
    let key = node_run_key(node_id, branch);
    let r = by_key.get(&key).copied();
    let status = status_of(r);
    RunNodeState {
        status,
        round: r.map_or(1, |r| r.round),
        label: label_for(r, status, now),
        node_run_key: key,
    }
}

// --- For each: rounds, items, branches --------------------------------------------------------

fn live_branch(group_id: &str, index: usize) -> BranchKey {
    format!("{group_id}#{index}")
}

/// The branch of one item in one round; `live` is the round the For each is in now.
fn branch_of(group_id: &str, index: usize, round: u32, live: Option<u32>) -> BranchKey {
    let branch = live_branch(group_id, index);
    if Some(round) == live {
        branch
    } else {
        archived_branch(&branch, round)
    }
}

struct Rounds<'a> {
    /// Ascending.
    rounds: Vec<u32>,
    /// The round the For each is expanded in now, when it is.
    live: Option<u32>,
    group_id: &'a str,
    run: &'a WorkflowRun,
    /// The widest branch index + 1 seen per round (round 0 is the live one).
    widest: HashMap<u32, usize>,
}

impl Rounds<'_> {
    /// The item list of a round, when the run kept it.
    fn list(&self, round: u32) -> Option<&[Json]> {
        let key = if Some(round) == self.live {
            self.group_id.to_string()
        } else {
            archived_items_key(self.group_id, round)
        };
        self.run.items.get(&key).map(Vec::as_slice)
    }

    /// How many items a round had. Branches of the live round are kept under round 0.
    fn count(&self, round: u32) -> usize {
        let slot = if Some(round) == self.live { 0 } else { round };
        self.list(round)
            .map_or(0, <[Json]>::len)
            .max(self.widest.get(&slot).copied().unwrap_or(0))
    }
}

fn rounds_of<'a>(run: &'a WorkflowRun, group_id: &'a str) -> Rounds<'a> {
    let mut archived: BTreeSet<u32> = BTreeSet::new();
    let mut widest: HashMap<u32, usize> = HashMap::new();
    for n in &run.nodes {
        let Some(parts) = parse_branch(&n.branch) else {
            continue;
        };
        if parts.group_id != group_id {
            continue;
        }
        let round = parts.round.unwrap_or(0);
        if round > 0 {
            archived.insert(round);
        }
        let slot = widest.entry(round).or_insert(0);
        *slot = (*slot).max(parts.index + 1);
    }
    let prefix = format!("{group_id}@r");
    for key in run.items.keys() {
        // An item list the run kept without any branch left (hand-edited or trimmed data).
        let tail = key
            .strip_prefix(&prefix)
            .and_then(|t| t.parse::<u32>().ok());
        if let Some(tail) = tail.filter(|t| *t > 0 && *key == archived_items_key(group_id, *t)) {
            archived.insert(tail);
        }
    }
    let own = run
        .nodes
        .iter()
        .find(|n| n.node_id == group_id && n.branch.is_empty())
        .map_or(1, |n| n.round);
    let last_archived = archived.iter().next_back().copied().unwrap_or(0);
    let live = run
        .items
        .contains_key(group_id)
        .then(|| own.max(last_archived + 1));
    // A loop sent the group round again but it has not expanded (yet, or it failed): that round
    // is the current one even though it has no items, so it is what shows by default.
    let current = live.or((last_archived > 0 && own > last_archived).then_some(own));
    let mut rounds: Vec<u32> = archived.into_iter().chain(current).collect();
    rounds.sort_unstable();
    Rounds {
        rounds,
        live,
        group_id,
        run,
        widest,
    }
}

/// What an item is called: its `title` or `name` when it has one, else `#<place>`.
pub fn item_title(list: Option<&[Json]>, index: usize) -> String {
    if let Some(Json::Object(fields)) = list.and_then(|l| l.get(index)) {
        for key in ["title", "name"] {
            if let Some(Json::String(value)) = fields.get(key) {
                let value = js_trim(value);
                if !value.is_empty() {
                    return value.to_string();
                }
            }
        }
    }
    format!("#{}", index + 1)
}

/// How one item stands, from the nodes of its lane.
fn item_status(run: &WorkflowRun, runs: &[&NodeRun]) -> RunNodeStatus {
    if runs.is_empty() {
        return RunNodeStatus::Pending;
    }
    let states: Vec<RunNodeStatus> = runs.iter().map(|r| status_of(Some(r))).collect();
    for first in [
        RunNodeStatus::Failed,
        RunNodeStatus::Running,
        RunNodeStatus::Waiting,
    ] {
        if states.contains(&first) {
            return first;
        }
    }
    if states.contains(&RunNodeStatus::Stopped) {
        return RunNodeStatus::Stopped;
    }
    let settled = states
        .iter()
        .filter(|s| matches!(s, RunNodeStatus::Done | RunNodeStatus::Skipped))
        .count();
    if settled == states.len() {
        return if states.contains(&RunNodeStatus::Done) {
            RunNodeStatus::Done
        } else {
            RunNodeStatus::Skipped
        };
    }
    // Some nodes done and the rest not started: between two nodes of the lane.
    if settled == 0 {
        return RunNodeStatus::Pending;
    }
    if run.status == RunStatus::Running {
        RunNodeStatus::Running
    } else {
        RunNodeStatus::Stopped
    }
}

const SUMMARY_ORDER: [RunNodeStatus; 7] = [
    RunNodeStatus::Done,
    RunNodeStatus::Running,
    RunNodeStatus::Waiting,
    RunNodeStatus::Failed,
    RunNodeStatus::Skipped,
    RunNodeStatus::Stopped,
    RunNodeStatus::Pending,
];

fn children_of<'a>(run: &'a WorkflowRun, group_id: &str) -> Vec<&'a Node> {
    run.definition
        .nodes
        .iter()
        .filter(|n| {
            n.parent.as_deref() == Some(group_id) && !matches!(n.kind, NodeKind::ForEach { .. })
        })
        .collect()
}

fn for_each_ids(run: &WorkflowRun) -> impl Iterator<Item = &str> {
    run.definition
        .nodes
        .iter()
        .filter(|n| matches!(n.kind, NodeKind::ForEach { .. }))
        .map(|n| n.id.as_str())
}

/// Every For each of the run's definition as the run page shows it. `pick` holds what the user
/// chose (round, item); `now` is the clock in ms for "Running 3m".
pub fn group_views(run: &WorkflowRun, pick: &Picks, now: u64) -> Vec<GroupView> {
    let by_key = node_runs_by_key(run);
    for_each_ids(run)
        .map(|group_id| {
            let info = rounds_of(run, group_id);
            let picked = pick.get(group_id).copied().unwrap_or_default();
            let round = match picked.round {
                Some(wanted) if info.rounds.contains(&wanted) => wanted,
                _ => info
                    .rounds
                    .last()
                    .copied()
                    .or_else(|| by_key.get(&node_run_key(group_id, "")).map(|r| r.round))
                    .unwrap_or(1),
            };
            let known = info.rounds.contains(&round);
            let list = if known { info.list(round) } else { None };
            let count = if known { info.count(round) } else { 0 };
            let kids = children_of(run, group_id);
            let items: Vec<GroupItem> = (0..count)
                .map(|index| {
                    let branch = branch_of(group_id, index, round, info.live);
                    let runs: Vec<&NodeRun> = kids
                        .iter()
                        .filter_map(|k| by_key.get(&node_run_key(&k.id, &branch)).copied())
                        .collect();
                    GroupItem {
                        index,
                        title: item_title(list, index),
                        status: item_status(run, &runs),
                    }
                })
                .collect();
            let item = match picked.item {
                Some(chosen) if chosen < count => chosen,
                _ => items
                    .iter()
                    .position(|i| {
                        matches!(i.status, RunNodeStatus::Running | RunNodeStatus::Waiting)
                    })
                    .unwrap_or(0),
            };
            let summary = if list.is_some_and(|l| l.is_empty()) {
                "No items".to_string()
            } else if count == 0 {
                // Not expanded in this round: say where the group stands, or why it failed.
                let own = by_key.get(&node_run_key(group_id, "")).copied();
                match own {
                    Some(own)
                        if own.status == NodeStatus::Failed
                            && own.error.as_deref().is_some_and(|e| !e.is_empty()) =>
                    {
                        own.error.clone().unwrap_or_default()
                    }
                    _ => state_of(&by_key, group_id, "", now).label,
                }
            } else {
                SUMMARY_ORDER
                    .iter()
                    .filter_map(|s| {
                        let n = items.iter().filter(|i| i.status == *s).count();
                        (n > 0).then(|| format!("{n} {}", s.name()))
                    })
                    .collect::<Vec<_>>()
                    .join(" · ")
            };
            GroupView {
                group_id: group_id.to_string(),
                rounds: info.rounds,
                round,
                items,
                item,
                summary,
            }
        })
        .collect()
}

/// The width of one lane of a group: its children's snapshot layout, left padding left out.
fn lane_width_of(kids: &[&FlowNode]) -> f32 {
    kids.iter()
        .map(|k| k.position.x + NODE_W - GROUP_PAD)
        .fold(NODE_W, f32::max)
}

/// Lanes when every For each is small (up to 4 items, up to 3 narrow nodes in it), else one lane
/// at a time.
pub fn default_run_view(run: &WorkflowRun) -> RunView {
    let flow = to_flow(&run.definition).nodes;
    for group_id in for_each_ids(run) {
        // Children laid out side by side make a lane too wide to repeat.
        let kids: Vec<&FlowNode> = flow
            .iter()
            .filter(|n| n.parent_id.as_deref() == Some(group_id))
            .collect();
        if lane_width_of(&kids) > NODE_W * 1.5 {
            return RunView::Single;
        }
        let info = rounds_of(run, group_id);
        let items = info
            .rounds
            .iter()
            .map(|r| info.count(*r))
            .max()
            .unwrap_or(0);
        if items > LANES_MAX_ITEMS || children_of(run, group_id).len() > LANES_MAX_CHILDREN {
            return RunView::Single;
        }
    }
    RunView::Lanes
}

// --- edges ------------------------------------------------------------------------------------

fn edge_state(def: &Edge, branch: &str, by_key: &ByKey, branching: bool) -> RunEdgeState {
    let Some(source) = by_key.get(&node_run_key(&def.from, branch)).copied() else {
        return RunEdgeState::Idle;
    };
    let target = by_key.get(&node_run_key(&def.to, branch)).copied();
    if def.loop_spec.is_some() {
        if is_asking(source) {
            return RunEdgeState::Active;
        }
        let is_loop =
            |d: &Option<super::model::Decision>| d.as_ref().is_some_and(|d| d.value == "loop");
        let looped = is_loop(&source.decision)
            || source
                .history
                .iter()
                .flatten()
                .any(|h| is_loop(&h.decision));
        return if looped {
            RunEdgeState::Taken
        } else {
            RunEdgeState::Idle
        };
    }
    if source.status != NodeStatus::Done {
        return RunEdgeState::Idle;
    }
    // Only the handles of an If or Switch pick a side; a stray one elsewhere is ignored.
    if let Some(handle) = def.handle.as_ref().filter(|_| branching) {
        if source.decision.as_ref().map(|d| d.value.as_str()) != Some(handle.as_str()) {
            return RunEdgeState::Idle;
        }
    }
    if target.is_some_and(|t| matches!(t.status, NodeStatus::Running | NodeStatus::WaitingApproval))
    {
        return RunEdgeState::Active;
    }
    RunEdgeState::Taken
}

// --- the graph --------------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Rect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

/// Every box reached from `start` by following `follows`, each once.
fn reached(
    boxes: &[Rect],
    others: &[usize],
    start: &[usize],
    follows: impl Fn(&Rect, &Rect) -> bool,
) -> Vec<usize> {
    let mut seen: HashSet<usize> = start.iter().copied().collect();
    let mut queue: Vec<usize> = start.to_vec();
    while let Some(m) = queue.pop() {
        for &o in others {
            if seen.contains(&o) || !follows(&boxes[m], &boxes[o]) {
                continue;
            }
            seen.insert(o);
            queue.push(o);
        }
    }
    seen.into_iter().collect()
}

fn overlaps_y(a: &Rect, b: &Rect) -> bool {
    b.y < a.y + a.h && b.y + b.h > a.y
}

fn overlaps_x(a: &Rect, b: &Rect) -> bool {
    b.x < a.x + a.w && b.x + b.w > a.x
}

/// Makes room for the group at `at` that grows by `dx` and `dy`: what sits to its right moves
/// right and what sits below moves down, and so does whatever those moves would then cover. The
/// boxes to move are collected first, so each moves once, by exactly `dx` or `dy`.
fn grow(boxes: &mut [Rect], at: usize, dx: f32, dy: f32) {
    let g = boxes[at];
    let others: Vec<usize> = (0..boxes.len()).filter(|i| *i != at).collect();
    if dx > 0.0 {
        let seeds: Vec<usize> = others
            .iter()
            .copied()
            .filter(|&i| boxes[i].x >= g.x + g.w && overlaps_y(&g, &boxes[i]))
            .collect();
        for i in reached(boxes, &others, &seeds, |m, o| {
            o.x >= m.x + m.w && overlaps_y(m, o)
        }) {
            boxes[i].x += dx;
        }
    }
    if dy > 0.0 {
        let wider = Rect { w: g.w + dx, ..g };
        let seeds: Vec<usize> = others
            .iter()
            .copied()
            .filter(|&i| boxes[i].y >= g.y + g.h && overlaps_x(&wider, &boxes[i]))
            .collect();
        for i in reached(boxes, &others, &seeds, |m, o| {
            o.y >= m.y + m.h && overlaps_x(m, o)
        }) {
            boxes[i].y += dy;
        }
    }
    boxes[at].w += dx;
    boxes[at].h += dy;
}

struct Lane {
    branch: BranchKey,
    x: f32,
}

/// The run graph for `view`: the definition laid out like the board, each For each replaced by
/// the lanes of the items in `groups` (all of them in the lanes view, the selected one in the
/// single view), groups grown to hold them with whatever they would cover moved aside. `now` is
/// the clock in ms for "Running 3m".
pub fn run_graph(run: &WorkflowRun, view: RunView, groups: &[GroupView], now: u64) -> RunGraph {
    let base = to_flow(&run.definition);
    let by_key = node_runs_by_key(run);
    let group_view: HashMap<&str, &GroupView> =
        groups.iter().map(|g| (g.group_id.as_str(), g)).collect();
    let loop_from: HashSet<&str> = run
        .definition
        .edges
        .iter()
        .filter(|e| e.loop_spec.is_some())
        .map(|e| e.from.as_str())
        .collect();
    let loop_to: HashSet<&str> = run
        .definition
        .edges
        .iter()
        .filter(|e| e.loop_spec.is_some())
        .map(|e| e.to.as_str())
        .collect();
    let compact = view == RunView::Lanes;

    // The branches each group draws a lane for, in order.
    let mut lanes: HashMap<NodeId, Vec<Lane>> = HashMap::new();
    let mut growth: Vec<(NodeId, f32, f32)> = Vec::new();
    let mut nodes: Vec<FlowNode> = Vec::new();
    let mut boxes: Vec<Rect> = Vec::new();
    let mut box_at: HashMap<NodeId, usize> = HashMap::new();

    for flow in base.nodes.iter().filter(|n| n.parent_id.is_none()) {
        let rect = Rect {
            x: flow.position.x,
            y: flow.position.y,
            w: flow.width,
            h: flow.height,
        };
        match box_at.get(&flow.id) {
            Some(&i) => boxes[i] = rect,
            None => {
                box_at.insert(flow.id.clone(), boxes.len());
                boxes.push(rect);
            }
        }
        let mut node = flow.clone();
        node.data.loop_in = loop_to.contains(flow.id.as_str());
        node.data.loop_out = loop_from.contains(flow.id.as_str());
        let state = state_of(&by_key, &flow.id, "", now);
        if flow.kind == FlowKind::Group {
            let gv = group_view.get(flow.id.as_str()).copied();
            let info = rounds_of(run, &flow.id);
            let kids: Vec<&FlowNode> = base
                .nodes
                .iter()
                .filter(|n| n.parent_id.as_deref() == Some(flow.id.as_str()))
                .collect();
            let picked: Vec<&GroupItem> = match gv {
                Some(gv) if view == RunView::Lanes => gv.items.iter().collect(),
                Some(gv) => gv.items.iter().filter(|i| i.index == gv.item).collect(),
                None => Vec::new(),
            };
            let picks: Vec<&GroupItem> = if kids.is_empty() { Vec::new() } else { picked };
            // Side by side, a lane is a narrow column of compact cards; alone it keeps the
            // board's width.
            let lane_w = if compact {
                kids.iter()
                    .map(|k| (k.position.x - GROUP_PAD) * LANE_SCALE + LANE_CARD_W)
                    .fold(LANE_CARD_W, f32::max)
            } else {
                lane_width_of(&kids)
            };
            let round = gv.map_or(1, |g| g.round);
            let group_lanes: Vec<Lane> = picks
                .iter()
                .enumerate()
                .map(|(k, item)| Lane {
                    branch: branch_of(&flow.id, item.index, round, info.live),
                    x: GROUP_PAD + k as f32 * (lane_w + LANE_GAP),
                })
                .collect();
            if !picks.is_empty() {
                let n = picks.len() as f32;
                let wide = GROUP_PAD * 2.0 + n * lane_w + (n - 1.0) * LANE_GAP + LANE_EDGE;
                let dx = if compact {
                    (wide - rect.w).max(0.0)
                } else {
                    (n - 1.0) * (lane_w + LANE_GAP)
                };
                match growth.iter_mut().find(|g| g.0 == flow.id) {
                    Some(slot) => *slot = (flow.id.clone(), dx, LANE_HEADER),
                    None => growth.push((flow.id.clone(), dx, LANE_HEADER)),
                }
            }
            lanes.insert(flow.id.clone(), group_lanes);
            node.data.run = Some(RunNodeState {
                label: gv.map_or_else(|| state.label.clone(), |g| g.summary.clone()),
                ..state
            });
            node.data.run_items = gv.map(|g| g.items.len());
        } else {
            node.data.run = Some(state);
        }
        nodes.push(node);
    }

    // Grow the groups that got lanes, moving whatever they would now cover.
    let mut order: Vec<(NodeId, f32, f32)> = growth;
    order.sort_by(|a, b| {
        let (ba, bb) = (boxes[box_at[&a.0]], boxes[box_at[&b.0]]);
        ba.y.total_cmp(&bb.y).then(ba.x.total_cmp(&bb.x))
    });
    for (id, dx, dy) in &order {
        grow(&mut boxes, box_at[id], *dx, *dy);
    }
    for n in &mut nodes {
        let Some(&i) = box_at.get(&n.id) else {
            continue;
        };
        let rect = boxes[i];
        n.position = Point {
            x: rect.x,
            y: rect.y,
        };
        if n.kind == FlowKind::Group {
            n.width = rect.w;
            n.height = rect.h;
        }
    }

    // The nodes inside the lanes.
    for flow in base.nodes.iter().filter(|n| n.parent_id.is_some()) {
        let parent = flow.parent_id.as_deref().unwrap_or_default();
        for lane in lanes.get(parent).into_iter().flatten() {
            let mut node = flow.clone();
            node.id = format!("{}@{}", flow.id, lane.branch);
            node.position = Point {
                x: lane.x + (flow.position.x - GROUP_PAD) * if compact { LANE_SCALE } else { 1.0 },
                y: flow.position.y + LANE_HEADER,
            };
            if compact {
                node.width = LANE_CARD_W;
            }
            node.data.compact = compact;
            node.data.run = Some(state_of(&by_key, &flow.id, &lane.branch, now));
            node.data.loop_in = loop_to.contains(flow.id.as_str());
            node.data.loop_out = loop_from.contains(flow.id.as_str());
            nodes.push(node);
        }
    }

    let parent_of: HashMap<&str, Option<&str>> = base
        .nodes
        .iter()
        .map(|n| (n.id.as_str(), n.parent_id.as_deref()))
        .collect();
    let scope = |id: &str| parent_of.get(id).copied().flatten();
    let mut edges: Vec<FlowEdge> = Vec::new();
    let branching = |id: &str| {
        run.definition
            .nodes
            .iter()
            .any(|n| n.id == id && matches!(n.kind, NodeKind::If { .. } | NodeKind::Switch { .. }))
    };
    for (flow, def) in base.edges.iter().zip(&run.definition.edges) {
        let (from, to) = (scope(&def.from), scope(&def.to));
        if from.is_none() && to.is_none() {
            let mut edge = flow.clone();
            edge.run = Some(edge_state(def, "", &by_key, branching(&def.from)));
            edges.push(edge);
        } else if from.is_some() && from == to {
            for lane in lanes.get(from.unwrap_or_default()).into_iter().flatten() {
                let mut edge = flow.clone();
                edge.id = format!("{}@{}", flow.id, lane.branch);
                edge.source = format!("{}@{}", def.from, lane.branch);
                edge.target = format!("{}@{}", def.to, lane.branch);
                edge.run = Some(edge_state(def, &lane.branch, &by_key, branching(&def.from)));
                edge.compact = compact;
                edges.push(edge);
            }
        }
        // An edge between a lane and the outside has nowhere to attach and is left out.
    }
    RunGraph { nodes, edges }
}
