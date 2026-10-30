//! Pure run state machine for workflows: no I/O, no timers, no clock. The runner feeds it events
//! and executes the actions it returns. Port of `workflowEngine.ts`.
//!
//! Contract with the runner:
//! - A node that is `running` with `ended_at` set is not running anything: it has finished and the
//!   Mono is being asked whether to loop (see `is_asking`). The runner must not apply a session or
//!   command timeout to it and must not run it again.
//! - `start_run`, `apply_event` and `resume_run` return a run that is already "settled": every step
//!   the engine can take by itself (triggers, If conditions, For each expansion, skipping untaken
//!   sides, failing a branch, condition loops, the final status) has been applied.
//! - `next_actions` only reads. It lists what the runner must do for nodes that are ready. An
//!   action is repeated until the matching `NodeStarted` event is applied, and never after.
//!
//! Every function takes the run by reference and returns a new one. An event that changes nothing
//! returns a clone equal to the input.
use super::condition::eval_condition;
use super::model::*;
use super::ordered::{js_number_text, Json, OrderedMap};
use super::output::output_instruction;
use super::template::{
    js_space, js_trim, lookup, render_template, shell_dash_problems, shell_template_problems,
};
use std::collections::{HashMap, HashSet};

/// Why the Mono is asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AskPurpose {
    AskMono,
    If,
    Switch,
    Loop,
}

/// What the runner must do next.
#[derive(Clone, Debug, PartialEq)]
pub enum EngineAction {
    StartSession {
        node_id: NodeId,
        branch: BranchKey,
        round: u32,
        prompt: String,
        reuse_session_id: Option<String>,
    },
    /// `command` is already rendered with shell quoting.
    RunCommand {
        node_id: NodeId,
        branch: BranchKey,
        command: String,
    },
    AskMono {
        node_id: NodeId,
        branch: BranchKey,
        purpose: AskPurpose,
        question: String,
        answers: Vec<String>,
        context: OrderedMap<Json>,
    },
    PostReport,
}

/// What happened outside the engine. `at` is the time the runner saw it (ms since the epoch, the
/// unit the run file uses).
#[derive(Clone, Debug, PartialEq)]
pub enum EngineEvent {
    NodeStarted {
        node_id: NodeId,
        branch: BranchKey,
        session_id: Option<String>,
        at: u64,
    },
    NodeFinished {
        node_id: NodeId,
        branch: BranchKey,
        output: OrderedMap<Json>,
        at: u64,
    },
    NodeFailed {
        node_id: NodeId,
        branch: BranchKey,
        error: String,
        at: u64,
    },
    Decided {
        node_id: NodeId,
        branch: BranchKey,
        value: String,
        reason: Option<String>,
        at: u64,
    },
    WaitingApproval {
        node_id: NodeId,
        branch: BranchKey,
        waiting: bool,
    },
    ReportPosted {
        at: u64,
    },
    Stopped {
        at: u64,
    },
}

impl EngineEvent {
    fn at(&self) -> Option<u64> {
        match self {
            Self::NodeStarted { at, .. }
            | Self::NodeFinished { at, .. }
            | Self::NodeFailed { at, .. }
            | Self::Decided { at, .. }
            | Self::ReportPosted { at }
            | Self::Stopped { at } => Some(*at),
            Self::WaitingApproval { .. } => None,
        }
    }
}

pub const MAX_ROUNDS_REASON: &str = "max rounds reached";
pub const BRANCH_FAILED_REASON: &str = "branch failed";

// --- graph ----------------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct LoopEdge<'a> {
    to: &'a str,
    spec: &'a LoopSpec,
}

struct Graph<'a> {
    nodes: &'a [Node],
    /// The first node with each id.
    by_id: HashMap<&'a str, &'a Node>,
    /// Non-loop edges whose endpoints exist and share a parent, by target.
    incoming: HashMap<&'a str, Vec<&'a Edge>>,
    outgoing: HashMap<&'a str, Vec<&'a str>>,
    /// Children of each For each node, in definition order.
    children: HashMap<&'a str, Vec<&'a Node>>,
    /// The loop edge leaving a node, when it can run one.
    loops: HashMap<&'a str, LoopEdge<'a>>,
}

fn graph_of(def: &Workflow) -> Graph<'_> {
    let mut by_id: HashMap<&str, &Node> = HashMap::new();
    for n in &def.nodes {
        by_id.entry(n.id.as_str()).or_insert(n);
    }
    let mut incoming: HashMap<&str, Vec<&Edge>> = HashMap::new();
    let mut outgoing: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut loops: HashMap<&str, LoopEdge> = HashMap::new();
    for e in &def.edges {
        let (Some(from), Some(to)) = (by_id.get(e.from.as_str()), by_id.get(e.to.as_str())) else {
            continue;
        };
        // Edges that cross a For each boundary are not supported: the group is the boundary.
        if from.parent != to.parent {
            continue;
        }
        if let Some(spec) = &e.loop_spec {
            // Only nodes that finish with a value the decider can look at can loop.
            let loopable = matches!(
                from.kind,
                NodeKind::Session { .. } | NodeKind::Command { .. } | NodeKind::AskMono { .. }
            );
            if loopable {
                loops.entry(e.from.as_str()).or_insert(LoopEdge {
                    to: e.to.as_str(),
                    spec,
                });
            }
            continue;
        }
        incoming.entry(e.to.as_str()).or_default().push(e);
        outgoing
            .entry(e.from.as_str())
            .or_default()
            .push(e.to.as_str());
    }
    let mut children: HashMap<&str, Vec<&Node>> = HashMap::new();
    for n in &def.nodes {
        if let Some(parent) = &n.parent {
            children.entry(parent.as_str()).or_default().push(n);
        }
    }
    Graph {
        nodes: &def.nodes,
        by_id,
        incoming,
        outgoing,
        children,
        loops,
    }
}

impl Graph<'_> {
    fn kind(&self, id: &str) -> Option<&NodeKind> {
        self.by_id.get(id).map(|n| &n.kind)
    }
    fn is_type(&self, id: &str, f: fn(&NodeKind) -> bool) -> bool {
        self.kind(id).is_some_and(f)
    }
    fn successors(&self, id: &str) -> Vec<String> {
        self.outgoing
            .get(id)
            .map(|v| v.iter().map(|s| s.to_string()).collect())
            .unwrap_or_default()
    }
    fn predecessors(&self, id: &str) -> Vec<String> {
        self.incoming
            .get(id)
            .map(|v| v.iter().map(|e| e.from.clone()).collect())
            .unwrap_or_default()
    }
}

fn is_for_each_kind(k: &NodeKind) -> bool {
    matches!(k, NodeKind::ForEach { .. })
}
fn is_report_kind(k: &NodeKind) -> bool {
    matches!(k, NodeKind::Report { .. })
}

/// Everything reachable from `start` by `next`, in the order found (`start` only when a cycle
/// leads back to it).
fn reach(start: &str, next: impl Fn(&str) -> Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut order = Vec::new();
    let mut queue = vec![start.to_string()];
    while let Some(id) = queue.pop() {
        for n in next(&id) {
            if seen.insert(n.clone()) {
                order.push(n.clone());
                queue.push(n);
            }
        }
    }
    order
}

/// `to`, `from` and every node between them over non-loop edges.
fn loop_body(g: &Graph, from: &str, to: &str) -> Vec<String> {
    let below = reach(to, |id| g.successors(id));
    let above: HashSet<String> = reach(from, |id| g.predecessors(id)).into_iter().collect();
    let mut body = vec![to.to_string()];
    if from != to {
        body.push(from.to_string());
    }
    for id in below {
        if above.contains(&id) && !body.contains(&id) {
            body.push(id);
        }
    }
    body
}

/// Every node that lies in the body of a loop: the ones the engine runs again each round.
pub fn looped_node_ids(def: &Workflow) -> HashSet<NodeId> {
    let g = graph_of(def);
    let mut looped = HashSet::new();
    for (from, edge) in &g.loops {
        looped.extend(loop_body(&g, from, edge.to));
    }
    looped
}

/// Longest distance from a node with no predecessors; breaks ties between equal end times.
fn depth_of(g: &Graph, id: &str, memo: &mut HashMap<String, usize>) -> usize {
    if let Some(known) = memo.get(id) {
        return *known;
    }
    memo.insert(id.to_string(), 0); // guards against a cycle in a hand-edited definition
    let mut depth = 0;
    for e in g.incoming.get(id).map(Vec::as_slice).unwrap_or_default() {
        depth = depth.max(depth_of(g, &e.from, memo) + 1);
    }
    memo.insert(id.to_string(), depth);
    depth
}

// --- draft ----------------------------------------------------------------------------------

struct Draft<'a> {
    run: WorkflowRun,
    g: Graph<'a>,
    /// Slot of a node run in `run.nodes`. Node runs are only ever appended, so indexes hold.
    index: HashMap<String, usize>,
    /// Timestamp stamped on every change the engine makes by itself.
    at: u64,
}

fn slot(node_id: &str, branch: &str) -> String {
    format!("{node_id}\u{0}{branch}")
}

/// `def` is the definition `run` carries; the graph borrows it while `run` is edited.
fn draft_of(run: WorkflowRun, def: &Workflow, at: u64) -> Draft<'_> {
    let index = run
        .nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (slot(&n.node_id, &n.branch), i))
        .collect();
    Draft {
        run,
        g: graph_of(def),
        index,
        at,
    }
}

impl Draft<'_> {
    fn at_slot(&self, node_id: &str, branch: &str) -> Option<usize> {
        self.index.get(&slot(node_id, branch)).copied()
    }
    fn add_node_run(&mut self, node_id: &str, branch: &str) {
        self.run.nodes.push(NodeRun::new(node_id, branch));
        self.index
            .insert(slot(node_id, branch), self.run.nodes.len() - 1);
    }
}

fn last_activity(run: &WorkflowRun) -> u64 {
    let mut at = run.started_at;
    for n in &run.nodes {
        at = at
            .max(n.started_at.unwrap_or(0))
            .max(n.ended_at.unwrap_or(0));
    }
    at.max(run.ended_at.unwrap_or(0))
        .max(run.report_posted_at.unwrap_or(0))
}

// --- small predicates ------------------------------------------------------------------------

/// A finished node whose loop edge has not been decided yet (exit or loop).
fn open_loop(g: &Graph, r: &NodeRun) -> bool {
    !is_archived_branch(&r.branch)
        && r.status == NodeStatus::Done
        && r.decision.is_none()
        && g.loops.contains_key(r.node_id.as_str())
}

/// A finished node whose loop edge is being put to the Mono (the node itself is not running).
pub fn is_asking(r: &NodeRun) -> bool {
    r.status == NodeStatus::Running && r.ended_at.is_some()
}

fn unsettled(g: &Graph, r: &NodeRun) -> bool {
    // An archived branch belongs to a finished loop round: it never runs again.
    if is_archived_branch(&r.branch) {
        return false;
    }
    matches!(
        r.status,
        NodeStatus::Pending | NodeStatus::Running | NodeStatus::WaitingApproval
    ) || open_loop(g, r)
}

fn is_report(g: &Graph, r: &NodeRun) -> bool {
    g.is_type(&r.node_id, is_report_kind)
}

fn branch_key(for_each_id: &str, index: usize) -> BranchKey {
    format!("{for_each_id}#{index}")
}

// --- scope -----------------------------------------------------------------------------------

fn input_json(value: &InputValue) -> Json {
    match value {
        InputValue::String(s) => Json::String(s.clone()),
        InputValue::Number(n) => Json::Number(*n),
        InputValue::Bool(b) => Json::Bool(*b),
    }
}

/// The name of the loop variable of a For each (`as`, else "item").
fn item_name(group: Option<&Node>) -> String {
    match group.map(|n| &n.kind) {
        Some(NodeKind::ForEach {
            item: Some(name), ..
        }) => name.clone(),
        _ => "item".to_string(),
    }
}

fn build_scope<'n>(
    run: &WorkflowRun,
    group: impl Fn(&str) -> Option<&'n Node>,
    branch: &str,
    node_id: Option<&str>,
) -> OrderedMap<Json> {
    let mut scope = OrderedMap::new();
    let mut round = 1;
    for n in &run.nodes {
        if !n.branch.is_empty() && n.branch != branch {
            continue;
        }
        if let Some(output) = &n.output {
            scope.insert(n.node_id.clone(), Json::Object(output.clone()));
        }
        if node_id == Some(n.node_id.as_str()) && n.branch == branch {
            round = n.round;
        }
    }
    if let Some(split) = parse_branch(branch) {
        let name = item_name(group(&split.group_id));
        match run
            .items
            .get(&split.group_id)
            .and_then(|items| items.get(split.index))
        {
            Some(item) => scope.insert(name, item.clone()),
            // JS stores `undefined` over whatever the name held.
            None => {
                scope.remove(&name);
            }
        }
    }
    let mut inputs = OrderedMap::new();
    for (k, v) in run.definition.input_values.iter() {
        inputs.insert(k.clone(), input_json(v));
    }
    scope.insert("inputs", Json::Object(inputs));
    let mut info = OrderedMap::new();
    info.insert("id", Json::String(run.id.clone()));
    info.insert("startedAt", Json::Number(run.started_at as f64));
    info.insert("round", Json::Number(f64::from(round)));
    scope.insert("run", Json::Object(info));
    scope
}

/// Template scope for a node running in `branch`: `inputs`, `run`, the loop variable inside a
/// For each, and the latest output of every node in the branch or at the top level. With
/// `node_id`, `run.round` is that node's current round (1 outside loops). The result is a
/// `Json::Object`.
pub fn run_scope(run: &WorkflowRun, branch: &str, node_id: Option<&str>) -> Json {
    let def = &run.definition;
    let group = |id: &str| def.nodes.iter().find(|n| n.id == id);
    Json::Object(build_scope(run, group, branch, node_id))
}

fn scope_in(d: &Draft, branch: &str, node_id: Option<&str>) -> OrderedMap<Json> {
    build_scope(&d.run, |id| d.g.by_id.get(id).copied(), branch, node_id)
}

/// Outputs the Mono needs to decide: the named nodes, the loop variable and the round.
fn context_for(d: &Draft, r: &NodeRun, ids: &[String]) -> OrderedMap<Json> {
    let scope = scope_in(d, &r.branch, Some(&r.node_id));
    let mut context = OrderedMap::new();
    for id in ids {
        if let Some(value) = scope.get(id) {
            context.insert(id.clone(), value.clone());
        }
    }
    if let Some(split) = parse_branch(&r.branch) {
        let name = item_name(d.g.by_id.get(split.group_id.as_str()).copied());
        match scope.get(&name) {
            Some(value) => context.insert(name, value.clone()),
            None => {
                context.remove(&name);
            }
        }
    }
    context.insert("round", Json::Number(f64::from(r.round)));
    context
}

// --- admission of For each branches -----------------------------------------------------------

/// Branches allowed to make progress. Top level always; inside a For each every branch that has
/// started and not finished, plus as many untouched branches (in index order) as `maxParallel`
/// leaves room for.
fn admitted_branches(d: &Draft) -> HashSet<BranchKey> {
    let mut admitted = HashSet::from([String::new()]);
    let mut by_branch: HashMap<&str, Vec<&NodeRun>> = HashMap::new();
    for n in &d.run.nodes {
        by_branch.entry(n.branch.as_str()).or_default().push(n);
    }
    for node in d.g.nodes {
        let NodeKind::ForEach { max_parallel, .. } = &node.kind else {
            continue;
        };
        let Some(items) = d.run.items.get(&node.id) else {
            continue;
        };
        let max = max_parallel.unwrap_or(f64::from(DEFAULT_MAX_PARALLEL));
        let mut active = 0.0;
        let mut waiting = Vec::new();
        for i in 0..items.len() {
            let key = branch_key(&node.id, i);
            let runs = by_branch
                .get(key.as_str())
                .map(Vec::as_slice)
                .unwrap_or_default();
            if !runs.iter().any(|r| unsettled(&d.g, r)) {
                continue;
            }
            if runs
                .iter()
                .any(|r| r.status != NodeStatus::Pending || r.round > 1 || r.output.is_some())
            {
                admitted.insert(key);
                active += 1.0;
            } else {
                waiting.push(key);
            }
        }
        for key in waiting {
            if active >= max {
                break;
            }
            admitted.insert(key);
            active += 1.0;
        }
    }
    admitted
}

// --- readiness --------------------------------------------------------------------------------

#[derive(PartialEq, Eq)]
enum Gate {
    Ready,
    Blocked,
    Dead,
}

/// Whether a pending node can start. Ready when every predecessor in the same branch is done
/// (an If or switch predecessor counts only along the matching handle). Dead when no predecessor leads
/// to it, so it is skipped. Blocked while a predecessor is still to finish.
fn gate_of(d: &Draft, r: &NodeRun) -> Gate {
    let Some(edges) =
        d.g.incoming
            .get(r.node_id.as_str())
            .filter(|e| !e.is_empty())
    else {
        return Gate::Ready;
    };
    let mut live = 0;
    for e in edges {
        let Some(src) = d.at_slot(&e.from, &r.branch).map(|i| &d.run.nodes[i]) else {
            continue;
        };
        match src.status {
            NodeStatus::Done => {
                if open_loop(&d.g, src) {
                    return Gate::Blocked;
                }
                if matches!(
                    d.g.kind(&e.from),
                    Some(NodeKind::If { .. } | NodeKind::Switch { .. })
                ) {
                    let taken = src.decision.as_ref().map(|x| x.value.as_str());
                    if taken == e.handle.as_ref().map(Handle::as_str) {
                        live += 1;
                    }
                } else {
                    live += 1;
                }
            }
            NodeStatus::Skipped | NodeStatus::Failed | NodeStatus::Stopped => {}
            _ => return Gate::Blocked,
        }
    }
    if live > 0 {
        Gate::Ready
    } else {
        Gate::Dead
    }
}

// --- state changes the engine makes by itself --------------------------------------------------

/// A failed node ends its branch for good: it must not start another loop round.
fn branch_failed(d: &Draft, branch: &str) -> bool {
    d.run
        .nodes
        .iter()
        .any(|n| n.branch == branch && n.status == NodeStatus::Failed)
}

/// Fails the node and skips whatever is still pending in its branch (other branches go on).
fn fail_node(d: &mut Draft, i: usize, error: String) {
    let at = d.at;
    let r = &mut d.run.nodes[i];
    r.status = NodeStatus::Failed;
    r.error = Some(error);
    r.started_at.get_or_insert(at);
    r.ended_at = Some(at);
    let branch = r.branch.clone();
    for k in 0..d.run.nodes.len() {
        let o = &d.run.nodes[k];
        if o.branch != branch || o.status != NodeStatus::Pending || is_report(&d.g, o) {
            continue;
        }
        let o = &mut d.run.nodes[k];
        o.status = NodeStatus::Skipped;
        o.ended_at = Some(at);
    }
}

fn finish_node(d: &mut Draft, i: usize, output: Option<OrderedMap<Json>>) {
    let at = d.at;
    let r = &mut d.run.nodes[i];
    r.status = NodeStatus::Done;
    r.started_at.get_or_insert(at);
    r.ended_at = Some(at);
    if output.is_some() {
        r.output = output;
    }
}

/// A snapshot of the round a node just finished, for `NodeRun.history`. A node that did not run
/// in that round (skipped, or never started) still holds an earlier round's session, so the
/// record leaves it out. A record keeps no output: it would multiply the size of a run by its
/// rounds, and the archived lanes of a For each keep their children's outputs.
fn round_record(r: &NodeRun) -> RoundRecord {
    let ran = r.started_at.is_some() && r.status != NodeStatus::Skipped;
    RoundRecord {
        round: r.round,
        status: r.status,
        session_id: r.session_id.clone().filter(|_| ran),
        error: r.error.clone().filter(|_| ran),
        decision: r.decision.clone().filter(|_| ran),
        started_at: r.started_at,
        ended_at: r.ended_at,
    }
}

/// Sends the branch back to `to`: that node, `from` and everything between them become pending
/// for the next round. Their outputs stay until they run again, so the next round can read the
/// previous one. Session ids stay too, so a looped session can be continued.
fn reset_loop(d: &mut Draft, from: usize, to: &str) {
    let finished_round = d.run.nodes[from].round;
    let round = finished_round.saturating_add(1);
    let (from_id, branch) = {
        let r = &d.run.nodes[from];
        (r.node_id.clone(), r.branch.clone())
    };
    let mut dropped: Vec<String> = Vec::new();
    for id in loop_body(&d.g, &from_id, to) {
        let Some(k) = d.at_slot(&id, &branch) else {
            continue;
        };
        let r = &mut d.run.nodes[k];
        let record = round_record(r);
        r.history.get_or_insert_with(Vec::new).push(record);
        r.status = NodeStatus::Pending;
        r.round = round;
        r.error = None;
        r.decision = None;
        r.started_at = None;
        r.ended_at = None;
        r.reasked = None;
        if d.g.is_type(&id, is_for_each_kind) && !dropped.contains(&id) {
            dropped.push(id);
        }
    }
    // Anything downstream of `to` that an earlier round skipped (the untaken side of an If) gets
    // another chance; the gate logic skips it again when it is still dead.
    for id in reach(to, |x| d.g.successors(x)) {
        let Some(k) = d.at_slot(&id, &branch) else {
            continue;
        };
        if d.run.nodes[k].status != NodeStatus::Skipped || d.g.is_type(&id, is_report_kind) {
            continue;
        }
        let r = &mut d.run.nodes[k];
        r.status = NodeStatus::Pending;
        r.round = round;
        r.ended_at = None;
    }
    // A For each inside the loop body expands again next round, with a fresh item list. The round
    // that just finished is archived, not deleted: its sessions and failures stay on the run.
    for id in dropped {
        let items = d.run.items.get(&id).cloned();
        let kids: HashSet<&str> =
            d.g.children
                .get(id.as_str())
                .map(|c| c.iter().map(|n| n.id.as_str()).collect())
                .unwrap_or_default();
        let mut archive_round = finished_round;
        // A hand-edited run at u32::MAX must not wrap or spin: stop at the last round.
        while archive_round < u32::MAX
            && d.run
                .items
                .contains_key(&archived_items_key(&id, archive_round))
        {
            archive_round = archive_round.saturating_add(1);
        }
        let mut archived = false;
        for k in 0..d.run.nodes.len() {
            let n = &d.run.nodes[k];
            let Some(parts) = parse_branch(&n.branch) else {
                continue;
            };
            if parts.round.is_some() || parts.group_id != id || !kids.contains(n.node_id.as_str()) {
                continue;
            }
            d.index.remove(&slot(&n.node_id, &n.branch));
            let renamed = archived_branch(&n.branch, archive_round);
            d.index.insert(slot(&n.node_id, &renamed), k);
            d.run.nodes[k].branch = renamed;
            archived = true;
        }
        if archived {
            if let Some(items) = items {
                d.run
                    .items
                    .insert(archived_items_key(&id, archive_round), items);
            }
        }
        d.run.items.remove(&id);
    }
}

/// Applies a loop decision to a finished node: another round, or exit.
fn decide_loop(d: &mut Draft, i: usize, value: &str, reason: Option<&str>) {
    let node_id = d.run.nodes[i].node_id.clone();
    let Some(spec) = d.g.loops.get(node_id.as_str()).copied() else {
        return;
    };
    let round = d.run.nodes[i].round;
    let branch = d.run.nodes[i].branch.clone();
    if value == "loop" && branch_failed(d, &branch) {
        // The question was already in flight when a node of the branch failed.
        let r = &mut d.run.nodes[i];
        r.status = NodeStatus::Done;
        r.decision = Some(Decision {
            value: "exit".into(),
            reason: Some(BRANCH_FAILED_REASON.into()),
        });
    } else if value == "loop" && f64::from(round) < spec.spec.max_rounds {
        let r = &mut d.run.nodes[i];
        r.status = NodeStatus::Done;
        r.decision = Some(Decision {
            value: "loop".into(),
            reason: reason.map(String::from),
        });
        reset_loop(d, i, spec.to);
    } else if value == "exit" || value == "loop" {
        // A "loop" answer at the last round is treated as exit: the app always exits there.
        let r = &mut d.run.nodes[i];
        r.status = NodeStatus::Done;
        r.decision = Some(if value == "loop" {
            Decision {
                value: "exit".into(),
                reason: Some(MAX_ROUNDS_REASON.into()),
            }
        } else {
            Decision {
                value: "exit".into(),
                reason: reason.map(String::from),
            }
        });
    } else {
        fail_node(
            d,
            i,
            format!("Invalid loop decision \"{value}\" (expected loop or exit)"),
        );
    }
}

fn last_output(d: &Draft, group: &str, branch: &str) -> Json {
    let mut best: Option<((u64, usize, usize), usize)> = None;
    let kids =
        d.g.children
            .get(group)
            .map(Vec::as_slice)
            .unwrap_or_default();
    for (order, kid) in kids.iter().enumerate() {
        let Some(k) = d.at_slot(&kid.id, branch) else {
            continue;
        };
        let r = &d.run.nodes[k];
        if r.status != NodeStatus::Done || r.output.is_none() {
            continue;
        }
        let key = (
            r.ended_at.unwrap_or(0),
            depth_of(&d.g, &kid.id, &mut HashMap::new()),
            order,
        );
        if best.is_none_or(|(b, _)| key > b) {
            best = Some((key, k));
        }
    }
    match best.and_then(|(_, k)| d.run.nodes[k].output.clone()) {
        Some(output) => Json::Object(output),
        None => Json::Null,
    }
}

/// `^\s*\{\{\s*([^{}\s]+)\s*\}\}\s*$`: the path inside a lone `{{path}}`.
fn list_expression(text: &str) -> Option<&str> {
    let inner = js_trim(text).strip_prefix("{{")?.strip_suffix("}}")?;
    let path = js_trim(inner);
    (!path.is_empty() && !path.contains(|c: char| c == '{' || c == '}' || js_space(c)))
        .then_some(path)
}

fn expand_for_each(d: &mut Draft, i: usize, node: &Node, list: &str) {
    let r = &d.run.nodes[i];
    let scope = Json::Object(scope_in(d, &r.branch, Some(&r.node_id)));
    let value = list_expression(list)
        .and_then(|path| lookup(&scope, path))
        .and_then(|found| found.as_array().cloned());
    let Some(items) = value else {
        fail_node(d, i, format!("Expected a list for {list}"));
        return;
    };
    let count = items.len();
    d.run.items.insert(node.id.clone(), items);
    let at = d.at;
    d.run.nodes[i].started_at.get_or_insert(at);
    if count == 0 {
        let mut output = OrderedMap::new();
        output.insert("results", Json::Array(Vec::new()));
        finish_node(d, i, Some(output));
        return;
    }
    d.run.nodes[i].status = NodeStatus::Running;
    for k in 0..count {
        for child in
            d.g.children
                .get(node.id.as_str())
                .cloned()
                .unwrap_or_default()
        {
            d.add_node_run(&child.id, &branch_key(&node.id, k));
        }
    }
}

/// One pass of everything the engine decides on its own. Returns whether anything changed.
fn pass(d: &mut Draft) -> bool {
    let mut changed = false;
    let admitted = admitted_branches(d);
    // Node runs added during the pass (a For each expanding) wait for the next one.
    for i in 0..d.run.nodes.len() {
        let (node_id, branch) = (
            d.run.nodes[i].node_id.clone(),
            d.run.nodes[i].branch.clone(),
        );
        let Some(node) = d.g.by_id.get(node_id.as_str()).copied() else {
            continue;
        };
        if is_archived_branch(&branch) {
            continue;
        }

        if open_loop(&d.g, &d.run.nodes[i]) {
            let spec = d.g.loops[node_id.as_str()];
            let round = f64::from(d.run.nodes[i].round);
            if branch_failed(d, &branch) {
                d.run.nodes[i].decision = Some(Decision {
                    value: "exit".into(),
                    reason: Some(BRANCH_FAILED_REASON.into()),
                });
                changed = true;
            } else if round >= spec.spec.max_rounds {
                d.run.nodes[i].decision = Some(Decision {
                    value: "exit".into(),
                    reason: Some(MAX_ROUNDS_REASON.into()),
                });
                changed = true;
            } else if spec.spec.decide != "mono" {
                let scope = Json::Object(scope_in(d, &branch, Some(&node_id)));
                match eval_condition(&spec.spec.decide, &scope) {
                    Err(error) => fail_node(d, i, error),
                    Ok(true) => {
                        d.run.nodes[i].decision = Some(Decision {
                            value: "loop".into(),
                            reason: None,
                        });
                        reset_loop(d, i, spec.to);
                    }
                    Ok(false) => {
                        d.run.nodes[i].decision = Some(Decision {
                            value: "exit".into(),
                            reason: None,
                        });
                    }
                }
                changed = true;
            }
            continue;
        }

        if matches!(node.kind, NodeKind::ForEach { .. })
            && d.run.nodes[i].status == NodeStatus::Running
        {
            let count = d.run.items.get(&node.id).map_or(0, Vec::len);
            let finished = (0..count).all(|k| {
                let key = branch_key(&node.id, k);
                !d.run
                    .nodes
                    .iter()
                    .any(|n| n.branch == key && unsettled(&d.g, n))
            });
            if finished {
                let results = (0..count)
                    .map(|k| last_output(d, &node.id, &branch_key(&node.id, k)))
                    .collect();
                let mut output = OrderedMap::new();
                output.insert("results", Json::Array(results));
                finish_node(d, i, Some(output));
                changed = true;
            }
            continue;
        }

        if d.run.nodes[i].status != NodeStatus::Pending || is_report_kind(&node.kind) {
            continue;
        }
        if !branch.is_empty() && !admitted.contains(&branch) {
            continue;
        }
        match gate_of(d, &d.run.nodes[i]) {
            Gate::Blocked => continue,
            Gate::Dead => {
                let at = d.at;
                let r = &mut d.run.nodes[i];
                r.status = NodeStatus::Skipped;
                r.ended_at = Some(at);
                changed = true;
                continue;
            }
            Gate::Ready => {}
        }

        match &node.kind {
            NodeKind::Schedule { .. } | NodeKind::Manual => {
                finish_node(d, i, Some(OrderedMap::new()));
                changed = true;
            }
            NodeKind::ForEach { list, .. } => {
                if node.parent.is_some() {
                    fail_node(d, i, "Nested For each is not supported".into());
                } else {
                    expand_for_each(d, i, node, list);
                }
                changed = true;
            }
            NodeKind::If {
                condition, decide, ..
            } => {
                if decide.is_none() {
                    let scope = Json::Object(scope_in(d, &branch, Some(&node_id)));
                    match eval_condition(condition.as_deref().unwrap_or(""), &scope) {
                        Err(error) => fail_node(d, i, error),
                        Ok(value) => {
                            d.run.nodes[i].decision = Some(Decision {
                                value: value.to_string(),
                                reason: None,
                            });
                            finish_node(d, i, None);
                        }
                    }
                    changed = true;
                }
            }
            NodeKind::Switch {
                value,
                cases,
                decide,
                ..
            } => {
                if decide.is_none() {
                    let scope = Json::Object(scope_in(d, &branch, Some(&node_id)));
                    let rendered = render_template(value.as_deref().unwrap_or(""), &scope, false);
                    let taken = if cases.contains(&rendered) {
                        rendered
                    } else {
                        SWITCH_DEFAULT.to_string()
                    };
                    d.run.nodes[i].decision = Some(Decision {
                        value: taken,
                        reason: None,
                    });
                    finish_node(d, i, None);
                    changed = true;
                }
            }
            NodeKind::Command { command, .. } => {
                // The dash check needs the values, so it runs once the node is ready to start.
                let mut problems = shell_template_problems(command);
                if problems.is_empty() {
                    let scope = Json::Object(scope_in(d, &branch, Some(&node_id)));
                    problems.extend(shell_dash_problems(command, &scope));
                }
                if !problems.is_empty() {
                    fail_node(d, i, problems.join("; "));
                    changed = true;
                }
            }
            // Session and Ask Mono nodes are the runner's to start.
            _ => {}
        }
    }
    changed
}

fn final_status(d: &Draft) -> RunStatus {
    if !d.run.nodes.iter().any(|n| n.status == NodeStatus::Failed) {
        return RunStatus::Done;
    }
    let succeeded = d.run.nodes.iter().any(|n| {
        n.status == NodeStatus::Done
            && matches!(
                d.g.kind(&n.node_id),
                Some(NodeKind::Session { .. } | NodeKind::Command { .. })
            )
    });
    if succeeded {
        RunStatus::DoneWithFailures
    } else {
        RunStatus::Failed
    }
}

/// Applies every step the engine can take alone, then ends the run when nothing is left.
fn settle(d: &mut Draft) {
    if d.run.status != RunStatus::Running {
        return;
    }
    for _ in 0..10_000 {
        if !pass(d) {
            break;
        }
    }
    if d.run
        .nodes
        .iter()
        .any(|n| !is_report(&d.g, n) && unsettled(&d.g, n))
    {
        return;
    }
    // Everything else is finished; the report still has to be posted.
    if d.run
        .nodes
        .iter()
        .any(|n| is_report(&d.g, n) && n.status == NodeStatus::Pending)
    {
        return;
    }
    d.run.status = final_status(d);
    d.run.ended_at = Some(d.at);
}

// --- public API --------------------------------------------------------------------------------

pub fn start_run(def: &Workflow, trigger: RunTrigger, now: u64, id: &str) -> WorkflowRun {
    let definition = def.clone();
    let g = graph_of(&definition);
    let top: Vec<&Node> = definition
        .nodes
        .iter()
        .filter(|n| n.parent.is_none())
        .collect();
    let roots: Vec<&Node> = top.iter().copied().filter(|n| n.is_trigger()).collect();
    // Without a trigger (an invalid definition) start from the nodes nothing leads to.
    let starts: Vec<&Node> = if roots.is_empty() {
        top.iter()
            .copied()
            .filter(|n| {
                !is_report_kind(&n.kind)
                    && g.incoming.get(n.id.as_str()).is_none_or(|e| e.is_empty())
            })
            .collect()
    } else {
        roots
    };
    let mut reachable: HashSet<String> = starts.iter().map(|n| n.id.clone()).collect();
    for s in &starts {
        reachable.extend(reach(&s.id, |x| g.successors(x)));
    }
    let nodes = top
        .iter()
        .map(|n| {
            let mut r = NodeRun::new(n.id.as_str(), "");
            if !reachable.contains(&n.id) {
                r.status = NodeStatus::Skipped;
            }
            r
        })
        .collect();
    let run = WorkflowRun {
        id: id.to_string(),
        workflow_id: definition.id.clone(),
        definition: definition.clone(),
        status: RunStatus::Running,
        started_at: now,
        ended_at: None,
        trigger,
        folder_id: None,
        items: Default::default(),
        nodes,
        report_posted_at: None,
        error: None,
    };
    let mut d = draft_of(run, &definition, now);
    // A schedule trigger exposes when it fired; a manual one has nothing to expose.
    for n in &top {
        let Some(i) = d.at_slot(&n.id, "") else {
            continue;
        };
        if matches!(n.kind, NodeKind::Schedule { .. })
            && d.run.nodes[i].status == NodeStatus::Pending
        {
            let mut output = OrderedMap::new();
            output.insert("startedAt", Json::Number(now as f64));
            finish_node(&mut d, i, Some(output));
        }
    }
    settle(&mut d);
    d.run
}

fn describe_node(node: &Node) -> String {
    format!("\"{}\"", node.title.as_deref().unwrap_or(&node.id))
}

pub fn next_actions(run: &WorkflowRun) -> Vec<EngineAction> {
    if run.status != RunStatus::Running {
        return Vec::new();
    }
    let d = draft_of(run.clone(), &run.definition, last_activity(run));
    let admitted = admitted_branches(&d);
    let mut actions = Vec::new();

    for r in &d.run.nodes {
        let Some(node) = d.g.by_id.get(r.node_id.as_str()).copied() else {
            continue;
        };
        if is_archived_branch(&r.branch) {
            continue;
        }

        if open_loop(&d.g, r) {
            let spec = d.g.loops[r.node_id.as_str()];
            if spec.spec.decide != "mono" || f64::from(r.round) >= spec.spec.max_rounds {
                continue;
            }
            let to = d.g.by_id.get(spec.to).copied();
            actions.push(EngineAction::AskMono {
                node_id: r.node_id.clone(),
                branch: r.branch.clone(),
                purpose: AskPurpose::Loop,
                question: format!(
                    "{} finished round {} of {}. Should the work go back to {} for another round, or exit?",
                    describe_node(node),
                    r.round,
                    js_number_text(spec.spec.max_rounds),
                    to.map_or_else(|| spec.to.to_string(), describe_node),
                ),
                answers: vec!["loop".into(), "exit".into()],
                context: context_for(&d, r, &loop_body(&d.g, &r.node_id, spec.to)),
            });
            continue;
        }

        if r.status != NodeStatus::Pending
            || (!r.branch.is_empty() && !admitted.contains(&r.branch))
        {
            continue;
        }
        if gate_of(&d, r) != Gate::Ready {
            continue;
        }
        let scope = Json::Object(scope_in(&d, &r.branch, Some(&r.node_id)));
        let before = d.g.predecessors(&r.node_id);

        match &node.kind {
            NodeKind::Session {
                prompt,
                output,
                reuse_session,
                ..
            } => {
                let instruction = output_instruction(output);
                let prompt = render_template(prompt, &scope, false);
                let reuse = r
                    .session_id
                    .as_ref()
                    .or(r.reuse_session_id.as_ref())
                    .filter(|id| !id.is_empty());
                actions.push(EngineAction::StartSession {
                    node_id: r.node_id.clone(),
                    branch: r.branch.clone(),
                    round: r.round,
                    prompt: if instruction.is_empty() {
                        prompt
                    } else {
                        format!("{prompt}\n\n{instruction}")
                    },
                    reuse_session_id: if *reuse_session != Some(false) && r.round > 1 {
                        reuse.cloned()
                    } else {
                        None
                    },
                });
            }
            NodeKind::Command { command, .. } => {
                // Commands with unsafe quoting or option-like values were failed while settling.
                if shell_dash_problems(command, &scope).is_empty() {
                    actions.push(EngineAction::RunCommand {
                        node_id: r.node_id.clone(),
                        branch: r.branch.clone(),
                        command: render_template(command, &scope, true),
                    });
                }
            }
            NodeKind::AskMono { question, answers } => {
                actions.push(EngineAction::AskMono {
                    node_id: r.node_id.clone(),
                    branch: r.branch.clone(),
                    purpose: AskPurpose::AskMono,
                    question: render_template(question, &scope, false),
                    answers: answers.clone(),
                    context: context_for(&d, r, &before),
                });
            }
            NodeKind::If {
                decide: Some(_),
                question,
                ..
            } => {
                actions.push(EngineAction::AskMono {
                    node_id: r.node_id.clone(),
                    branch: r.branch.clone(),
                    purpose: AskPurpose::If,
                    question: match question.as_deref().filter(|q| !q.is_empty()) {
                        Some(q) => render_template(q, &scope, false),
                        None => format!(
                            "Should the \"true\" side of {} be taken? Answer true or false.",
                            describe_node(node)
                        ),
                    },
                    answers: vec!["true".into(), "false".into()],
                    context: context_for(&d, r, &before),
                });
            }
            NodeKind::Switch {
                decide: Some(_),
                cases,
                question,
                ..
            } => {
                let mut answers = cases.clone();
                answers.push(SWITCH_DEFAULT.to_string());
                actions.push(EngineAction::AskMono {
                    node_id: r.node_id.clone(),
                    branch: r.branch.clone(),
                    purpose: AskPurpose::Switch,
                    question: match question.as_deref().filter(|q| !q.is_empty()) {
                        Some(q) => render_template(q, &scope, false),
                        None => format!(
                            "Which case of {} applies? Answer with one of: {}.",
                            describe_node(node),
                            answers.join(", ")
                        ),
                    },
                    answers,
                    context: context_for(&d, r, &before),
                });
            }
            _ => {}
        }
    }

    let report_pending = d
        .run
        .nodes
        .iter()
        .any(|n| is_report(&d.g, n) && n.status == NodeStatus::Pending);
    if report_pending
        && run.report_posted_at.is_none()
        && !d
            .run
            .nodes
            .iter()
            .any(|n| !is_report(&d.g, n) && unsettled(&d.g, n))
    {
        actions.push(EngineAction::PostReport);
    }
    actions
}

fn stop_run(run: &WorkflowRun, at: u64) -> WorkflowRun {
    if run.status != RunStatus::Running && run.status != RunStatus::Interrupted {
        return run.clone();
    }
    let mut out = run.clone();
    out.status = RunStatus::Stopped;
    out.ended_at = Some(at);
    for n in &mut out.nodes {
        match n.status {
            NodeStatus::Running | NodeStatus::WaitingApproval => {
                n.status = NodeStatus::Stopped;
                n.ended_at = Some(at);
            }
            NodeStatus::Pending => {
                n.status = NodeStatus::Skipped;
                n.ended_at = Some(at);
            }
            _ => {}
        }
    }
    out
}

fn unstarted(status: NodeStatus) -> bool {
    matches!(
        status,
        NodeStatus::Pending | NodeStatus::Running | NodeStatus::WaitingApproval
    )
}

pub fn apply_event(run: &WorkflowRun, event: &EngineEvent) -> WorkflowRun {
    if let EngineEvent::Stopped { at } = event {
        return stop_run(run, *at);
    }
    if run.status != RunStatus::Running {
        return run.clone();
    }
    let mut d = draft_of(
        run.clone(),
        &run.definition,
        event.at().unwrap_or_else(|| last_activity(run)),
    );

    if let EngineEvent::ReportPosted { at } = event {
        let report = d
            .run
            .nodes
            .iter()
            .position(|n| is_report(&d.g, n) && n.status == NodeStatus::Pending);
        if let Some(i) = report {
            finish_node(&mut d, i, None);
        }
        d.run.report_posted_at = Some(*at);
        settle(&mut d);
        return d.run;
    }

    let (node_id, branch) = match event {
        EngineEvent::NodeStarted {
            node_id, branch, ..
        }
        | EngineEvent::NodeFinished {
            node_id, branch, ..
        }
        | EngineEvent::NodeFailed {
            node_id, branch, ..
        }
        | EngineEvent::Decided {
            node_id, branch, ..
        }
        | EngineEvent::WaitingApproval {
            node_id, branch, ..
        } => (node_id, branch),
        EngineEvent::ReportPosted { .. } | EngineEvent::Stopped { .. } => return run.clone(),
    };
    let Some(i) = d.at_slot(node_id, branch) else {
        return run.clone();
    };
    let Some(node) = d.g.by_id.get(node_id.as_str()).copied() else {
        return run.clone();
    };
    let status = d.run.nodes[i].status;

    match event {
        EngineEvent::NodeStarted { session_id, at, .. } => {
            if status == NodeStatus::Pending {
                let r = &mut d.run.nodes[i];
                r.status = NodeStatus::Running;
                r.started_at = Some(*at);
                r.ended_at = None;
                r.reuse_session_id = None;
                if let Some(id) = session_id {
                    r.session_id = Some(id.clone());
                }
            } else if open_loop(&d.g, &d.run.nodes[i]) {
                // The loop question went out: the node stays finished but is no longer to be
                // re-asked.
                d.run.nodes[i].status = NodeStatus::Running;
            } else {
                return run.clone();
            }
        }
        EngineEvent::NodeFinished { output, .. } => {
            if !unstarted(status) || is_asking(&d.run.nodes[i]) {
                return run.clone();
            }
            finish_node(&mut d, i, Some(output.clone()));
        }
        EngineEvent::NodeFailed { error, .. } => {
            if matches!(
                status,
                NodeStatus::Done | NodeStatus::Failed | NodeStatus::Skipped
            ) {
                return run.clone();
            }
            fail_node(&mut d, i, error.clone());
        }
        EngineEvent::WaitingApproval { waiting, .. } => {
            if *waiting && status == NodeStatus::Running && !is_asking(&d.run.nodes[i]) {
                d.run.nodes[i].status = NodeStatus::WaitingApproval;
            } else if !*waiting && status == NodeStatus::WaitingApproval {
                d.run.nodes[i].status = NodeStatus::Running;
            } else {
                return run.clone();
            }
        }
        EngineEvent::Decided { value, reason, .. } => {
            let loops = d.g.loops.contains_key(node_id.as_str());
            if loops && (is_asking(&d.run.nodes[i]) || open_loop(&d.g, &d.run.nodes[i])) {
                decide_loop(&mut d, i, value, reason.as_deref());
            } else if !unstarted(status) {
                return run.clone();
            } else {
                match &node.kind {
                    NodeKind::If { .. } => {
                        if value != "true" && value != "false" {
                            fail_node(
                                &mut d,
                                i,
                                format!("Invalid decision \"{value}\" (expected true or false)"),
                            );
                        } else {
                            d.run.nodes[i].decision = Some(Decision {
                                value: value.clone(),
                                reason: reason.clone(),
                            });
                            finish_node(&mut d, i, None);
                        }
                    }
                    NodeKind::Switch { cases, .. } => {
                        if value != SWITCH_DEFAULT && !cases.contains(value) {
                            fail_node(
                                &mut d,
                                i,
                                format!(
                                    "Invalid decision \"{value}\" (expected one of the cases or {SWITCH_DEFAULT})"
                                ),
                            );
                        } else {
                            d.run.nodes[i].decision = Some(Decision {
                                value: value.clone(),
                                reason: reason.clone(),
                            });
                            finish_node(&mut d, i, None);
                        }
                    }
                    NodeKind::AskMono { answers, .. } => {
                        if !answers.contains(value) {
                            fail_node(
                                &mut d,
                                i,
                                format!(
                                    "Mono answered \"{value}\", which is not an allowed answer"
                                ),
                            );
                        } else {
                            let mut output = OrderedMap::new();
                            output.insert("answer", Json::String(value.clone()));
                            output
                                .insert("reason", Json::String(reason.clone().unwrap_or_default()));
                            finish_node(&mut d, i, Some(output));
                        }
                    }
                    _ => return run.clone(),
                }
            }
        }
        EngineEvent::ReportPosted { .. } | EngineEvent::Stopped { .. } => {}
    }
    settle(&mut d);
    d.run
}

/// Running nodes go back to pending; finished nodes stay finished. A node in loop round 2 or
/// later keeps its session as `reuse_session_id`, so resuming continues it.
pub fn mark_interrupted(run: &WorkflowRun) -> WorkflowRun {
    if run.status != RunStatus::Running {
        return run.clone();
    }
    let g = graph_of(&run.definition);
    let mut out = run.clone();
    out.status = RunStatus::Interrupted;
    for n in &mut out.nodes {
        if n.status != NodeStatus::Running && n.status != NodeStatus::WaitingApproval {
            continue;
        }
        // A For each group is bookkeeping: its branches carry the real state.
        if g.is_type(&n.node_id, is_for_each_kind) {
            continue;
        }
        // Waiting on the Mono about a loop: the node itself had finished, only the question is
        // lost.
        if is_asking(n) {
            n.status = NodeStatus::Done;
            continue;
        }
        // From round 2 on the node continues the loop session; a first round starts fresh.
        let session = n.session_id.take();
        let reuse = n.reuse_session_id.take();
        n.started_at = None;
        n.reuse_session_id = if n.round > 1 { session.or(reuse) } else { None };
        n.status = NodeStatus::Pending;
    }
    out
}

pub fn resume_run(run: &WorkflowRun) -> WorkflowRun {
    if run.status != RunStatus::Interrupted {
        return run.clone();
    }
    let mut resumed = run.clone();
    resumed.status = RunStatus::Running;
    resumed.ended_at = None;
    let mut d = draft_of(resumed, &run.definition, last_activity(run));
    settle(&mut d);
    d.run
}
