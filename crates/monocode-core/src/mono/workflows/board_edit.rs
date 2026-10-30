//! The board editor's rules with no window in them: adding, connecting, looping, dropping and
//! deleting nodes, what a field of the inspector does to a node, and how a draft is saved over
//! the stored copy. Port of the logic in `WorkflowBoard.tsx` and `NodeInspector.tsx`; the GPUI
//! board draws and calls these.
use super::board_model::{
    auto_place, connection_kind, definition_key, deletion, drop_crossing_edges, drop_nodes,
    fit_groups, from_flow, groupable, is_group, make_edge, new_node, project_input_for, push_below,
    settle_drop, BoardNodeData, ConnectionKind, Flow, FlowEdge, FlowKind, FlowNode, NodeType,
    Point, EMPTY_GROUP_H, GAP_Y, GROUP_PAD, NODE_H, NODE_W,
};
use super::engine::looped_node_ids;
use super::model::{
    trigger_of, Checkout, Cwd, Decide, Handle, InputValue, LoopSpec, Node, NodeId, NodeKind,
    OutputField, Workflow, WorkflowInput, WORKFLOWS_MAX,
};
use super::models::{workflow_issues, ModelCatalog};
use super::ordered::OrderedMap;
use super::report::field_in_report;
use super::template::js_trim;
use super::validate::WorkflowIssue;
use super::yaml::{anchor_node, input_value_problems};

/// What the board says when a flagged workflow is asked to be enabled by a save.
pub const REVIEW_FIRST: &str = "Review this workflow before enabling it.";
/// What the board says when the stored copy is not the one it loaded.
pub const CHANGED_ELSEWHERE: &str = "This workflow changed elsewhere";

/// A loop made by dragging a connection back: three rounds, the Mono decides.
pub fn default_loop() -> LoopSpec {
    LoopSpec {
        max_rounds: 3.0,
        decide: "mono".into(),
    }
}

/// The parts of a workflow the board keeps next to its nodes and edges.
#[derive(Clone, Debug, PartialEq)]
pub struct Meta {
    pub name: String,
    pub description: Option<String>,
    pub enabled: bool,
    pub inputs: OrderedMap<WorkflowInput>,
    pub input_values: OrderedMap<InputValue>,
}

impl Meta {
    pub fn of(w: &Workflow) -> Self {
        Self {
            name: w.name.clone(),
            description: w.description.clone(),
            enabled: w.enabled,
            inputs: w.inputs.clone(),
            input_values: w.input_values.clone(),
        }
    }
}

/// The workflow the board holds now: `base` with the drawn nodes and edges and the meta on it.
pub fn current(base: &Workflow, flow: &Flow, meta: &Meta) -> Workflow {
    let mut w = from_flow(base, &flow.nodes, &flow.edges);
    w.name = meta.name.clone();
    w.description = meta.description.clone();
    w.enabled = meta.enabled;
    w.inputs = meta.inputs.clone();
    w.input_values = meta.input_values.clone();
    w
}

/// Everything wrong with a workflow that would keep it from running. A step with no project
/// says so in the words of the field.
pub fn problems_of(w: &Workflow, catalog: &ModelCatalog) -> Vec<WorkflowIssue> {
    let unset: Vec<&str> = w
        .nodes
        .iter()
        .filter(|n| match &n.kind {
            NodeKind::Session { project, .. } | NodeKind::Command { project, .. } => {
                project.is_empty()
            }
            _ => false,
        })
        .map(|n| n.id.as_str())
        .collect();
    let mut issues: Vec<WorkflowIssue> = workflow_issues(w, catalog)
        .into_iter()
        .map(|issue| {
            let missing = issue
                .node_id
                .as_deref()
                .is_some_and(|id| unset.contains(&id))
                && issue.message.starts_with("project must be exactly");
            if missing {
                WorkflowIssue {
                    message: "Choose a project".into(),
                    ..issue
                }
            } else {
                issue
            }
        })
        .collect();
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

// ---- saving ------------------------------------------------------------------------------

/// Why a save did not go through.
#[derive(Clone, Debug, PartialEq)]
pub enum SaveRefusal {
    /// The stored copy is not the one the board last saw; here it is.
    Changed(Box<Workflow>),
    /// Said to the user as it is.
    Message(String),
}

/// The stored copy of a workflow with the board's definition on it, the fields the app manages
/// kept. A schedule that is unchanged keeps its place; a new, edited or resumed one starts afresh.
pub fn merged(stored: Option<&Workflow>, next: &Workflow, now: u64) -> Workflow {
    let mut base = match stored {
        Some(stored) => stored.clone(),
        None => {
            let mut fresh = Workflow::new(next.id.clone(), next.name.clone());
            fresh.created_at = next.created_at;
            fresh
        }
    };
    base.next_run_at = None;
    base.id = next.id.clone();
    base.name = next.name.clone();
    base.description = next.description.clone().filter(|d| !js_trim(d).is_empty());
    base.inputs = next.inputs.clone();
    base.input_values = next
        .input_values
        .iter()
        .filter(|(key, _)| next.inputs.contains_key(key))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    base.nodes = next
        .nodes
        .iter()
        .map(|node| anchor_node(node.clone(), now))
        .collect();
    base.edges = next.edges.clone();
    base.enabled = next.enabled;
    let Some(NodeKind::Schedule { schedule }) = trigger_of(&base).map(|n| &n.kind) else {
        return base;
    };
    if !base.enabled {
        return base;
    }
    let before = stored.and_then(trigger_of).map(|n| &n.kind);
    let same = stored.is_some_and(|s| s.enabled)
        && matches!(before, Some(NodeKind::Schedule { schedule: old }) if old == schedule)
        && stored.is_some_and(|s| s.next_run_at.is_some());
    base.next_run_at = if same {
        stored.and_then(|s| s.next_run_at)
    } else {
        schedule.next(now).ok()
    };
    base
}

/// `list` with the draft saved into it: the new list and the workflow as stored. `seen` is the
/// `definition_key` of the stored copy the board last saw; `overwrite` goes past a stored copy
/// that differs.
pub fn save_into(
    mut list: Vec<Workflow>,
    next: &Workflow,
    overwrite: bool,
    seen: &str,
    now: u64,
) -> Result<(Vec<Workflow>, Workflow), SaveRefusal> {
    let at = list.iter().position(|w| w.id == next.id);
    let stored = at.map(|i| &list[i]);
    // Not over a definition this board has not seen; Overwrite is the explicit way past.
    if let Some(stored) = stored {
        if !overwrite && definition_key(stored) != seen {
            return Err(SaveRefusal::Changed(Box::new(stored.clone())));
        }
    } else if list.len() >= WORKFLOWS_MAX {
        return Err(SaveRefusal::Message(format!(
            "At most {WORKFLOWS_MAX} workflows."
        )));
    }
    // The stored copy decides, not what the board loaded: it may have been flagged since.
    if stored.is_some_and(|s| s.needs_review) && next.enabled {
        return Err(SaveRefusal::Message(REVIEW_FIRST.into()));
    }
    let result = merged(stored, next, now);
    match at {
        Some(i) => list[i] = result.clone(),
        None => list.push(result.clone()),
    }
    Ok((list, result))
}

// ---- the picker -----------------------------------------------------------------------------

pub struct PickerGroup {
    pub title: &'static str,
    pub items: &'static [(NodeType, &'static str)],
}

/// The menu behind "+ Node": every kind of node, grouped by its part in the workflow.
pub fn picker_groups() -> &'static [PickerGroup] {
    &[
        PickerGroup {
            title: "Trigger",
            items: &[
                (NodeType::Schedule, "Runs on a schedule"),
                (NodeType::Manual, "Runs when you start it"),
            ],
        },
        PickerGroup {
            title: "Action",
            items: &[
                (NodeType::Session, "A session works on a project"),
                (NodeType::Command, "A shell command"),
                (NodeType::AskMono, "A question for the Mono"),
            ],
        },
        PickerGroup {
            title: "Flow",
            items: &[
                (NodeType::If, "Go one way or the other"),
                (NodeType::Switch, "Pick one of several ways"),
                (NodeType::ForEach, "Repeat for each item of a list"),
            ],
        },
        PickerGroup {
            title: "End",
            items: &[(NodeType::Report, "Tells the Mono how it went")],
        },
    ]
}

/// Why a type cannot be added now, if it cannot.
pub fn picker_unavailable(
    ty: NodeType,
    has_trigger: bool,
    has_report: bool,
) -> Option<&'static str> {
    match ty {
        NodeType::Schedule | NodeType::Manual if has_trigger => Some("A workflow has one trigger"),
        NodeType::Report if has_report => Some("A workflow has one Report node"),
        _ => None,
    }
}

/// `NODE_LABEL` of the board.
pub fn type_label(ty: NodeType) -> &'static str {
    match ty {
        NodeType::Schedule => "Schedule",
        NodeType::Manual => "Manual",
        NodeType::Session => "Session",
        NodeType::Command => "Command",
        NodeType::AskMono => "Ask Mono",
        NodeType::If => "If",
        NodeType::Switch => "Switch",
        NodeType::ForEach => "For each",
        NodeType::Report => "Report to Mono",
    }
}

// ---- editing the flow -------------------------------------------------------------------------

fn handle_of(name: Option<&str>) -> Option<Handle> {
    name.filter(|n| !n.is_empty()).map(Handle::named)
}

/// Whether a connection from `source` (leaving by the If handle `handle`, if any) to `target`
/// may be made, as a plain edge or as a loop.
pub fn can_connect(w: &Workflow, source: &str, handle: Option<&str>, target: &str) -> bool {
    connection_kind(w, source, target, handle_of(handle)) != ConnectionKind::Invalid
}

/// Whether a loop from `from` back to `to` may be added now (both still there, `to` an ancestor,
/// `from` free to start one).
pub fn loop_allowed(w: &Workflow, from: &str, to: &str) -> bool {
    connection_kind(w, from, to, None) == ConnectionKind::Loop
}

/// The flow with the node (and edge) given selected and nothing else.
pub fn select(flow: &Flow, node: Option<&str>, edge: Option<&str>) -> Flow {
    let mut out = flow.clone();
    for n in &mut out.nodes {
        n.selected = node == Some(n.id.as_str());
    }
    for e in &mut out.edges {
        e.selected = edge == Some(e.id.as_str());
    }
    out
}

fn is_trigger_type(ty: NodeType) -> bool {
    matches!(ty, NodeType::Schedule | NodeType::Manual)
}

fn sort_parents_first(nodes: Vec<FlowNode>) -> Vec<FlowNode> {
    let (groups, rest): (Vec<_>, Vec<_>) =
        nodes.into_iter().partition(|n| n.kind == FlowKind::Group);
    groups.into_iter().chain(rest).collect()
}

/// `w` (the workflow the flow draws) with a new node of type `ty` added to the flow: where
/// the board puts it (above the trigger, before the Report, in the selected For each, or under
/// the lowest node of its scope) and wired after the node that led nowhere. Returns the new
/// flow, the new node selected, and its id.
pub fn add_node(flow: &Flow, w: &Workflow, ty: NodeType) -> (Flow, NodeId) {
    let node = new_node(ty, w);
    let selected = flow.nodes.iter().find(|n| n.selected);
    let parent: Option<NodeId> = if groupable(&node) {
        match selected {
            Some(s) if s.kind == FlowKind::Group => Some(s.id.clone()),
            Some(s) => s.parent_id.clone(),
            None => None,
        }
    } else {
        None
    };
    let step = NODE_H + GAP_Y;
    let to_node = |position: Point, parent_id: Option<NodeId>| -> FlowNode {
        let group = is_group(&node);
        FlowNode {
            id: node.id.clone(),
            kind: if group {
                FlowKind::Group
            } else {
                FlowKind::Board
            },
            position,
            width: if group {
                NODE_W + GROUP_PAD * 2.0
            } else {
                NODE_W
            },
            height: if group { EMPTY_GROUP_H } else { NODE_H },
            parent_id,
            selected: true,
            data: BoardNodeData::new(node.clone()),
        }
    };
    let mut deselected = flow.nodes.clone();
    for n in &mut deselected {
        n.selected = false;
    }
    let report = deselected
        .iter()
        .find(|n| matches!(n.data.node.kind, NodeKind::Report { .. }) && n.parent_id.is_none())
        .cloned();
    let flowing = matches!(
        ty,
        NodeType::Session | NodeType::Command | NodeType::AskMono | NodeType::ForEach
    );
    let mut edges = flow.edges.clone();
    let nodes: Vec<FlowNode>;
    if is_trigger_type(ty) {
        let top = deselected
            .iter()
            .filter(|n| n.parent_id.is_none())
            .map(|n| n.position.y)
            .fold(0.0_f32, f32::min);
        let mut all = deselected;
        all.push(to_node(
            Point {
                x: 0.0,
                y: top - step,
            },
            None,
        ));
        nodes = all;
    } else if let (None, Some(report), true) = (&parent, &report, flowing) {
        // Before the Report node, which stays last: it moves down and the edges into it follow.
        let grow = if ty == NodeType::ForEach {
            EMPTY_GROUP_H + GAP_Y
        } else {
            step
        };
        // At the Report's place, or below whatever hangs lowest above it, with a gap.
        let bottom = deselected
            .iter()
            .filter(|n| n.parent_id.is_none() && n.id != report.id)
            .map(|n| {
                n.position.y
                    + if n.kind == FlowKind::Group {
                        n.height
                    } else {
                        NODE_H
                    }
            })
            .fold(f32::NEG_INFINITY, f32::max);
        let y = report.position.y.max(bottom + GAP_Y);
        let at = Point {
            x: if ty == NodeType::ForEach {
                report.position.x - GROUP_PAD
            } else {
                report.position.x
            },
            y,
        };
        let mut all: Vec<FlowNode> = deselected
            .into_iter()
            .map(|mut n| {
                if n.id == report.id {
                    n.position = Point {
                        x: n.position.x,
                        y: y + grow,
                    };
                }
                n
            })
            .collect();
        all.push(to_node(at, None));
        nodes = all;
        let into: Vec<_> = flow
            .edges
            .iter()
            .filter(|e| e.loop_spec.is_none() && e.target == report.id)
            .cloned()
            .collect();
        edges.retain(|e| !(e.loop_spec.is_none() && e.target == report.id));
        edges.extend(into.iter().map(|e| {
            make_edge(
                &e.source,
                &node.id,
                handle_of(e.source_handle.as_deref()),
                None,
            )
        }));
        if !into.is_empty() {
            edges.push(make_edge(&node.id, &report.id, None, None));
        }
    } else {
        let place = auto_place(w, parent.as_deref());
        let at = if is_group(&node) && parent.is_none() {
            Point {
                x: place.x - GROUP_PAD,
                y: place.y,
            }
        } else {
            place
        };
        let mut all = deselected;
        all.push(to_node(at, parent.clone()));
        nodes = all;
        // Wired after the node that hangs lowest in its scope, when that one leads nowhere yet.
        let y_of = |n: &Node| n.ui.map_or(0.0, |u| u.y);
        let mut lowest: Option<&Node> = None;
        for n in w.nodes.iter().filter(|n| n.parent == parent) {
            if lowest.is_none_or(|low| y_of(n) >= y_of(low)) {
                lowest = Some(n);
            }
        }
        if let Some(lowest) = lowest {
            let free = !matches!(
                lowest.kind,
                NodeKind::Report { .. } | NodeKind::If { .. } | NodeKind::Switch { .. }
            ) && !w
                .edges
                .iter()
                .any(|e| e.loop_spec.is_none() && e.from == lowest.id);
            if free {
                let mut wider = w.clone();
                let mut added = node.clone();
                added.parent = parent.clone();
                wider.nodes.push(added);
                if connection_kind(&wider, &lowest.id, &node.id, None) == ConnectionKind::Edge {
                    edges.push(make_edge(&lowest.id, &node.id, None, None));
                }
            }
        }
    }
    let placed = sort_parents_first(nodes);
    let fitted = push_below(&placed, &fit_groups(&placed));
    (
        Flow {
            nodes: fitted,
            edges,
        },
        node.id,
    )
}

/// What dragging a connection from `source` to `target` makes.
pub enum Connected {
    Invalid,
    /// A way back to an ancestor: the board asks for the loop's rounds and decider first.
    Loop,
    Edge(Flow),
}

/// Connects `source` to `target`; `handle` is the If handle ("true" or "false") it left from.
pub fn connect(
    flow: &Flow,
    w: &Workflow,
    source: &str,
    target: &str,
    handle: Option<&str>,
) -> Connected {
    let handle = handle_of(handle);
    match connection_kind(w, source, target, handle.clone()) {
        ConnectionKind::Invalid => Connected::Invalid,
        ConnectionKind::Loop => Connected::Loop,
        ConnectionKind::Edge => {
            let mut out = flow.clone();
            // An If node's true or false, and each handle of a Switch, leads to one place.
            if let Some(h) = &handle {
                out.edges.retain(|e| {
                    !(e.loop_spec.is_none()
                        && e.source == source
                        && e.source_handle.as_deref() == Some(h.as_str()))
                });
            }
            out.edges.push(make_edge(source, target, handle, None));
            Connected::Edge(out)
        }
    }
}

/// The flow with the loop `from` -> `to` added.
pub fn add_loop(flow: &Flow, from: &str, to: &str, spec: LoopSpec) -> Flow {
    let mut out = flow.clone();
    out.edges.push(make_edge(from, to, None, Some(spec)));
    out
}

/// Changes the loop that leaves node `id`, or takes it away with None.
pub fn set_loop(flow: &Flow, id: &str, spec: Option<LoopSpec>) -> Flow {
    let Some(at) = flow
        .edges
        .iter()
        .position(|e| e.loop_spec.is_some() && e.source == id)
    else {
        return flow.clone();
    };
    let mut out = flow.clone();
    match spec {
        Some(spec) => {
            let old = &flow.edges[at];
            out.edges[at] = make_edge(&old.source, &old.target, None, Some(spec));
        }
        None => {
            out.edges.remove(at);
        }
    }
    out
}

/// The node `next` in place of the one with its id.
pub fn update_node(flow: &Flow, next: &Node) -> Flow {
    let mut out = flow.clone();
    for n in &mut out.nodes {
        if n.id == next.id {
            n.data.node = next.clone();
        }
    }
    out
}

/// After the node `id` was dragged: it joins the For each it was dropped on or leaves the one it
/// was in, and a node that changed frame is no longer wired across it.
pub fn drag_stop(flow: &Flow, id: &str) -> Flow {
    let nodes = settle_drop(&flow.nodes, id);
    let parent = |list: &[FlowNode]| {
        list.iter()
            .find(|n| n.id == id)
            .and_then(|n| n.parent_id.clone())
    };
    let moved = parent(&flow.nodes) != parent(&nodes);
    let edges = if moved {
        drop_crossing_edges(&nodes, &flow.edges, &[id.to_string()])
    } else {
        flow.edges.clone()
    };
    Flow { nodes, edges }
}

/// The flow without the nodes (and the wires) in `ids`.
pub fn remove_node(flow: &Flow, id: &str) -> Flow {
    drop_nodes(&flow.nodes, &flow.edges, &[id.to_string()])
}

/// What the Delete key removes: the selected nodes (a For each takes only its frame) and edges.
pub fn delete_selected(flow: &Flow) -> Flow {
    let picked: Vec<&str> = flow
        .nodes
        .iter()
        .filter(|n| n.selected)
        .map(|n| n.id.as_str())
        .collect();
    // The canvas lists the children of a deleted For each too.
    let doomed: Vec<FlowNode> = flow
        .nodes
        .iter()
        .filter(|n| n.selected || n.parent_id.as_deref().is_some_and(|p| picked.contains(&p)))
        .cloned()
        .collect();
    let touching: Vec<_> = flow
        .edges
        .iter()
        .filter(|e| e.selected || doomed.iter().any(|n| n.id == e.source || n.id == e.target))
        .cloned()
        .collect();
    if doomed.is_empty() && touching.is_empty() {
        return flow.clone();
    }
    let gone = deletion(&doomed, &touching);
    let ids: Vec<NodeId> = gone.nodes.iter().map(|n| n.id.clone()).collect();
    let mut out = drop_nodes(&flow.nodes, &flow.edges, &ids);
    out.edges
        .retain(|e| !gone.edges.iter().any(|g| g.id == e.id));
    out
}

// ---- the inspector's fields --------------------------------------------------------------------

/// The input a `{{inputs.x}}` reference names.
pub fn project_ref_name(project: &str) -> Option<&str> {
    let name = project.strip_prefix("{{inputs.")?.strip_suffix("}}")?;
    usable_name(name).then_some(name)
}

/// The path a step's project reference stands for: the value of the input it names.
pub fn project_path(w: &Workflow, node: &Node) -> Option<String> {
    let (NodeKind::Session { project, .. } | NodeKind::Command { project, .. }) = &node.kind else {
        return None;
    };
    match w.input_values.get(project_ref_name(project)?) {
        Some(InputValue::String(path)) => Some(path.clone()),
        _ => None,
    }
}

/// A step bound to a project: the node, and the inputs and values of the workflow with it.
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectChoice {
    pub node: Node,
    pub inputs: OrderedMap<WorkflowInput>,
    pub input_values: OrderedMap<InputValue>,
}

fn with_project(node: &Node, reference: &str) -> Node {
    let mut out = node.clone();
    if let NodeKind::Session { project, .. } | NodeKind::Command { project, .. } = &mut out.kind {
        *project = reference.to_string();
    }
    out
}

/// Choosing the project `path` (called `label`) for a step: the step names an input that holds
/// it, and an input the step left goes with it when nothing else mentions it.
pub fn project_choice(w: &Workflow, node: &Node, path: &str, label: &str) -> ProjectChoice {
    let (NodeKind::Session { project, .. } | NodeKind::Command { project, .. }) = &node.kind else {
        return ProjectChoice {
            node: node.clone(),
            inputs: w.inputs.clone(),
            input_values: w.input_values.clone(),
        };
    };
    let name = project_ref_name(project);
    let made = project_input_for(w, path, label, name);
    let mut inputs = made.inputs;
    let mut input_values = made.input_values;
    if let Some(name) = name {
        let is_project = matches!(
            inputs.get(name),
            Some(i) if i.ty == super::model::InputType::Project
        );
        if made.reference != *project && is_project {
            // Loop conditions mention inputs too, so the edges are searched along with the nodes.
            let nodes: Vec<Node> = w
                .nodes
                .iter()
                .map(|n| {
                    if n.id == node.id {
                        with_project(n, "")
                    } else {
                        n.clone()
                    }
                })
                .collect();
            let rest = serde_json::json!({"nodes": nodes, "edges": w.edges}).to_string();
            if !rest.contains(&format!("inputs.{name}")) {
                inputs.remove(name);
                input_values.remove(name);
            }
        }
    }
    ProjectChoice {
        node: with_project(node, &made.reference),
        inputs,
        input_values,
    }
}

/// A plain identifier: letters, digits and underscores, not starting with a digit.
pub fn usable_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && name != "__proto__"
}

/// What is wrong with the name typed for a new input, if anything.
pub fn input_name_problem(name: &str, taken: &dyn Fn(&str) -> bool) -> Option<String> {
    if name.is_empty() {
        None
    } else if !usable_name(name) {
        Some("Use letters, digits and underscores".into())
    } else if taken(name) {
        Some("Already declared".into())
    } else {
        None
    }
}

/// The name a new output field starts with.
pub fn next_field_name(fields: &OrderedMap<OutputField>) -> String {
    let mut name = "field".to_string();
    let mut n = 2;
    while fields.contains_key(&name) {
        name = format!("field_{n}");
        n += 1;
    }
    name
}

/// The fields with `from` called `to`, in the same place.
pub fn rename_field(
    fields: &OrderedMap<OutputField>,
    from: &str,
    to: &str,
) -> OrderedMap<OutputField> {
    fields
        .iter()
        .map(|(key, field)| {
            (
                if key == from {
                    to.to_string()
                } else {
                    key.clone()
                },
                field.clone(),
            )
        })
        .collect()
}

/// The fields with "In report" of `key` set to the opposite of what it effectively is.
pub fn toggle_in_report(fields: &OrderedMap<OutputField>, key: &str) -> OrderedMap<OutputField> {
    fields
        .iter()
        .map(|(k, field)| {
            let mut field = field.clone();
            if k == key {
                field.in_report = Some(!field_in_report(&field));
            }
            (k.clone(), field)
        })
        .collect()
}

pub fn checkout_value(checkout: &Checkout) -> String {
    match checkout {
        Checkout::Root => "root".into(),
        Checkout::NewWorktree => "newWorktree".into(),
        Checkout::WorktreeOf(id) => format!("worktree:{id}"),
    }
}

pub fn parse_checkout(value: &str) -> Checkout {
    match value.strip_prefix("worktree:") {
        Some(id) => Checkout::WorktreeOf(id.to_string()),
        None if value == "newWorktree" => Checkout::NewWorktree,
        None => Checkout::Root,
    }
}

pub fn cwd_value(cwd: Option<&Cwd>) -> String {
    match cwd {
        Some(Cwd::WorktreeOf(id)) => format!("worktree:{id}"),
        _ => "root".into(),
    }
}

pub fn parse_cwd(value: &str) -> Option<Cwd> {
    value
        .strip_prefix("worktree:")
        .map(|id| Cwd::WorktreeOf(id.to_string()))
}

/// The session nodes other than `except`: what a worktree or a nest can be of.
pub fn sessions_except<'a>(w: &'a Workflow, except: &str) -> Vec<&'a Node> {
    w.nodes
        .iter()
        .filter(|n| matches!(n.kind, NodeKind::Session { .. }) && n.id != except)
        .collect()
}

/// Whether the node is in the body of a loop (the engine runs it again each round).
pub fn in_loop(w: &Workflow, id: &str) -> bool {
    looped_node_ids(w).contains(id)
}

/// An If node decided by the Mono or by a condition (which starts empty).
pub fn if_decided_by(node: &Node, mono: bool) -> Node {
    let mut out = node.clone();
    if let NodeKind::If {
        condition, decide, ..
    } = &mut out.kind
    {
        if mono {
            *condition = None;
            *decide = Some(Decide::Mono);
        } else {
            *decide = None;
            *condition = Some(String::new());
        }
    }
    out
}

/// A Switch decided by the Mono or by a value (which starts empty).
pub fn switch_decided_by(node: &Node, mono: bool) -> Node {
    let mut out = node.clone();
    if let NodeKind::Switch { value, decide, .. } = &mut out.kind {
        if mono {
            *value = None;
            *decide = Some(Decide::Mono);
        } else {
            *decide = None;
            *value = Some(String::new());
        }
    }
    out
}

/// The flow with the Switch `id` changed by `edit` on its cases, and every out edge of it
/// passed through `edges` (which keeps, renames or drops it).
fn edit_switch(
    flow: &Flow,
    id: &str,
    edit: impl FnOnce(&mut Vec<String>),
    edges: impl Fn(&FlowEdge) -> Option<Option<Handle>>,
) -> Flow {
    let mut out = flow.clone();
    let Some(node) = out.nodes.iter_mut().find(|n| n.id == id) else {
        return out;
    };
    let NodeKind::Switch { cases, .. } = &mut node.data.node.kind else {
        return out;
    };
    edit(cases);
    out.edges = flow
        .edges
        .iter()
        .filter_map(|e| {
            if e.loop_spec.is_some() || e.source != id {
                return Some(e.clone());
            }
            let handle = edges(e)?;
            let mut next = make_edge(&e.source, &e.target, handle, None);
            next.selected = e.selected;
            Some(next)
        })
        .collect();
    out
}

/// The Switch `id` with `case` added as its last case. A case already there is not added twice.
pub fn add_switch_case(flow: &Flow, id: &str, case: &str) -> Flow {
    edit_switch(
        flow,
        id,
        |cases| {
            if !cases.iter().any(|c| c == case) {
                cases.push(case.to_string());
            }
        },
        |e| Some(e.source_handle.as_deref().map(Handle::named)),
    )
}

/// Whether a line leaving the node `id` already uses `name` as its handle.
pub fn out_edge_uses_handle(flow: &Flow, id: &str, name: &str) -> bool {
    flow.edges.iter().any(|e| {
        e.loop_spec.is_none() && e.source == id && e.source_handle.as_deref() == Some(name)
    })
}

/// The Switch `id` with the case `from` called `to`, in the same place; the edges that left by
/// `from` leave by `to`. A `to` that a line of the node already leaves by (and `from` does not)
/// would capture that line, so nothing changes then.
pub fn rename_switch_case(flow: &Flow, id: &str, from: &str, to: &str) -> Flow {
    if from != to && out_edge_uses_handle(flow, id, to) {
        return flow.clone();
    }
    edit_switch(
        flow,
        id,
        |cases| {
            for c in cases.iter_mut().filter(|c| *c == from) {
                *c = to.to_string();
            }
        },
        |e| {
            Some(e.source_handle.as_deref().map(|h| {
                if h == from {
                    Handle::named(to)
                } else {
                    Handle::named(h)
                }
            }))
        },
    )
}

/// The Switch `id` without the case `case`; the edges that left by it are removed with it.
pub fn remove_switch_case(flow: &Flow, id: &str, case: &str) -> Flow {
    edit_switch(
        flow,
        id,
        |cases| cases.retain(|c| c != case),
        |e| match e.source_handle.as_deref() {
            Some(h) if h == case => None,
            other => Some(other.map(Handle::named)),
        },
    )
}

/// A loop decided by the Mono or by a condition (which starts empty).
pub fn loop_decided_by(spec: &LoopSpec, mono: bool) -> LoopSpec {
    let decide = if mono {
        "mono".to_string()
    } else if spec.decide == "mono" {
        String::new()
    } else {
        spec.decide.clone()
    };
    LoopSpec {
        decide,
        ..spec.clone()
    }
}
