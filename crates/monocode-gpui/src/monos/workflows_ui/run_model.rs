//! The run page without a window: the scene a run is drawn as, which node is picked when none
//! was, and what the details of a node say. Ports of `RunGraph.tsx`'s `barsOf` and
//! `defaultSelection` and of `RunNodeDetails.tsx`'s text rules.
use super::super::scheduler::duration;
use super::graph::{Bar, BarKind, Rect, Scene, TabItem};
use super::review::{node_label, node_title};
use monocode_core::mono::workflows::{
    board_model::{FlowKind, FlowNode, GROUP_PAD, NODE_W},
    model::{
        NodeKind, NodeRun, NodeStatus, RoundRecord, WorkflowRun, archived_items_key, parse_branch,
    },
    ordered::Json,
    run_graph_model::{
        GroupView, LANE_HEADER, Picks, RunNodeStatus, RunView, group_views, item_title,
        node_run_key, node_runs_by_key, run_graph, status_of,
    },
};

/// Height of the round selector over a group, and the strip above the lanes.
const ROUNDS_H: f32 = 22.;
const BAR_H: f32 = LANE_HEADER - 4.;
/// A value longer than this is cut until "Show more"; a command's tail keeps this many characters.
const SHORT: usize = 200;
const TAIL: usize = 600;

/// The strips that sit with each For each: lane titles or item tabs, and the round selector.
fn run_bars(nodes: &[FlowNode], groups: &[GroupView], view: RunView) -> Vec<Bar> {
    let mut bars = Vec::new();
    for group in nodes.iter().filter(|n| n.kind == FlowKind::Group) {
        let Some(gv) = groups.iter().find(|g| g.group_id == group.id) else {
            continue;
        };
        let kids: Vec<&FlowNode> = nodes
            .iter()
            .filter(|n| n.parent_id.as_deref() == Some(group.id.as_str()))
            .collect();
        if gv.rounds.len() > 1 {
            // Right of the column the edge into the group runs down, so that edge stays visible.
            let w = (group.width - (GROUP_PAD + NODE_W / 2. + 12.))
                .min(gv.rounds.len() as f32 * 64. + 8.);
            bars.push(Bar {
                id: format!("bar-rounds:{}", group.id),
                rect: Rect::new(
                    group.position.x + group.width - w,
                    group.position.y - ROUNDS_H - 4.,
                    w,
                    ROUNDS_H,
                ),
                kind: BarKind::Rounds {
                    group: group.id.clone(),
                    rounds: gv.rounds.clone(),
                    selected: gv.round,
                },
            });
        }
        if kids.is_empty() || gv.items.is_empty() {
            continue;
        }
        let top = kids
            .iter()
            .map(|k| k.position.y)
            .fold(f32::INFINITY, f32::min);
        let y = group.position.y + top - LANE_HEADER;
        if view == RunView::Single {
            bars.push(Bar {
                id: format!("bar-tabs:{}", group.id),
                rect: Rect::new(
                    group.position.x + GROUP_PAD,
                    y,
                    group.width - GROUP_PAD * 2.,
                    BAR_H,
                ),
                kind: BarKind::Tabs {
                    group: group.id.clone(),
                    items: gv
                        .items
                        .iter()
                        .map(|i| TabItem {
                            index: i.index,
                            title: i.title.clone(),
                            status: Some(i.status),
                        })
                        .collect(),
                    selected: gv.item,
                },
            });
            continue;
        }
        // One title above each lane, as wide as the lane's cards.
        let mut lanes: Vec<(String, f32, f32)> = Vec::new();
        for kid in &kids {
            let branch = kid.id.split_once('@').map_or(kid.id.as_str(), |(_, b)| b);
            let right = kid.position.x + kid.width;
            match lanes.iter_mut().find(|l| l.0 == branch) {
                Some(lane) => {
                    lane.1 = lane.1.min(kid.position.x);
                    lane.2 = lane.2.max(right);
                }
                None => lanes.push((branch.to_string(), kid.position.x, right)),
            }
        }
        lanes.sort_by(|a, b| a.1.total_cmp(&b.1));
        for (k, (_, x, right)) in lanes.iter().enumerate() {
            let Some(item) = gv.items.get(k) else {
                break;
            };
            bars.push(Bar {
                id: format!("bar-lane:{}:{k}", group.id),
                rect: Rect::new(group.position.x + x, y, right - x, BAR_H),
                kind: BarKind::Lane {
                    title: item.title.clone(),
                    status: item.status,
                },
            });
        }
    }
    bars
}

/// A run as the scene of the canvas: its definition snapshot with what the run did with each
/// node, the lanes of its For each nodes and the strips above them.
pub(super) fn run_scene(run: &WorkflowRun, view: RunView, picks: &Picks, now: u64) -> Scene {
    let groups = group_views(run, picks, now);
    let graph = run_graph(run, view, &groups, now);
    let bars = run_bars(&graph.nodes, &groups, view);
    let fit_key = format!("{}:{}", view.name(), graph.nodes.len());
    let mut scene = Scene::build(&graph.nodes, &graph.edges, bars, fit_key);
    // In a single lane the cards stack up behind it, for the items that are not shown.
    if view == RunView::Single {
        for node in &mut scene.nodes {
            node.stacked = node.flow.parent_id.as_deref().is_some_and(|parent| {
                groups
                    .iter()
                    .find(|g| g.group_id == parent)
                    .is_some_and(|g| g.items.len() > 1)
            });
        }
    }
    scene
}

/// The node to show details for when none was picked: the one running, else the last to finish.
pub(super) fn default_selection(run: &WorkflowRun) -> Option<String> {
    let is_group = |id: &str| {
        run.definition
            .nodes
            .iter()
            .any(|n| n.id == id && matches!(n.kind, NodeKind::ForEach { .. }))
    };
    let live = |n: &&NodeRun| matches!(n.status, NodeStatus::Running | NodeStatus::WaitingApproval);
    let running = run
        .nodes
        .iter()
        .find(|n| live(n) && !is_group(&n.node_id))
        .or_else(|| run.nodes.iter().find(live));
    let key = |n: &NodeRun| node_run_key(&n.node_id, &n.branch);
    if let Some(n) = running {
        return Some(key(n));
    }
    let mut last: Option<&NodeRun> = None;
    for n in &run.nodes {
        if let Some(ended) = n.ended_at
            && last.is_none_or(|l| ended >= l.ended_at.unwrap_or(0))
        {
            last = Some(n);
        }
    }
    last.or(run.nodes.last()).map(key)
}

/// The node whose details show: the one picked, or the default when the pick is gone (a loop
/// reset it, the run was replaced).
pub(super) fn selection(run: &WorkflowRun, picked: Option<&str>) -> Option<String> {
    match picked {
        Some(key) if node_runs_by_key(run).contains_key(key) => Some(key.to_string()),
        _ => default_selection(run),
    }
}

// --- the details ------------------------------------------------------------------------------

/// How long a node took, as the details say it.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Took {
    /// Not shown: the node has not started.
    None,
    Spent(u64),
    /// Still going since this time.
    Since(u64),
    /// Started and not over, not running either.
    Unknown,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct RoundLine {
    pub round: u32,
    pub status: RunNodeStatus,
    pub span: Option<String>,
    pub started: Option<u64>,
    pub session: Option<String>,
    pub decision: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct CommandBlock {
    pub exit_code: Option<String>,
    pub stdout: Option<String>,
    pub stderr: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct OutputLine {
    pub name: String,
    pub text: String,
    /// Longer than the short form: it has a "Show more".
    pub long: bool,
}

impl OutputLine {
    /// The value as the details show it: cut with "..." until all of it is asked for.
    pub(super) fn shown(&self, all: bool) -> String {
        if self.long && !all {
            format!("{}...", utf16_head(&self.text, SHORT))
        } else {
            self.text.clone()
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Details {
    pub heading: String,
    pub kind: &'static str,
    pub status: RunNodeStatus,
    pub status_label: &'static str,
    /// The round, once past the first.
    pub round: Option<u32>,
    pub started: Option<u64>,
    pub took: Took,
    pub decision: Option<String>,
    pub session: Option<String>,
    pub error: Option<String>,
    pub rounds: Vec<RoundLine>,
    pub command: Option<CommandBlock>,
    pub outputs: Vec<OutputLine>,
}

pub(super) fn status_label(status: RunNodeStatus) -> &'static str {
    match status {
        RunNodeStatus::Done => "Done",
        RunNodeStatus::Running => "Running",
        RunNodeStatus::Waiting => "Waiting for approval",
        RunNodeStatus::Failed => "Failed",
        RunNodeStatus::Skipped => "Skipped",
        RunNodeStatus::Pending => "Not started",
        RunNodeStatus::Stopped => "Stopped",
    }
}

fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// The longest start of `text` of at most `units` UTF-16 units (JS `slice(0, n)`).
fn utf16_head(text: &str, units: usize) -> &str {
    let mut used = 0;
    for (at, c) in text.char_indices() {
        used += c.len_utf16();
        if used > units {
            return &text[..at];
        }
    }
    text
}

/// The longest end of `text` of at most `units` UTF-16 units (JS `slice(-n)`).
fn utf16_tail(text: &str, units: usize) -> &str {
    let mut used = 0;
    for (at, c) in text.char_indices().rev() {
        used += c.len_utf16();
        if used > units {
            return &text[at + c.len_utf8()..];
        }
    }
    text
}

/// A value as the details show it: text as it is, anything else as JSON.
fn shown(value: &Json) -> String {
    match value {
        Json::String(text) => text.clone(),
        other => other.stringify(false),
    }
}

fn tail(text: &str) -> String {
    if utf16_len(text) > TAIL {
        format!("...{}", utf16_tail(text, TAIL))
    } else {
        text.to_string()
    }
}

fn decision_text(value: &str, reason: Option<&str>) -> String {
    match reason {
        Some(reason) if !reason.is_empty() => format!("{value}: {reason}"),
        _ => value.to_string(),
    }
}

fn round_line(record: &RoundRecord) -> RoundLine {
    let as_node = NodeRun {
        status: record.status,
        round: record.round,
        started_at: record.started_at,
        ended_at: record.ended_at,
        ..NodeRun::new("", "")
    };
    RoundLine {
        round: record.round,
        status: status_of(Some(&as_node)),
        span: match (record.started_at, record.ended_at) {
            (Some(start), Some(end)) => Some(duration(end.saturating_sub(start))),
            _ => None,
        },
        started: record.started_at,
        session: record.session_id.clone(),
        decision: record
            .decision
            .as_ref()
            .map(|d| format!("Decided {}", decision_text(&d.value, d.reason.as_deref()))),
        error: record.error.clone(),
    }
}

/// The title of the node, with the item it ran for when it sits in a For each lane.
fn heading(run: &WorkflowRun, node: &NodeRun) -> String {
    let def = run.definition.nodes.iter().find(|n| n.id == node.node_id);
    let title = def.map_or_else(|| node.node_id.clone(), node_title);
    let Some(parts) = parse_branch(&node.branch) else {
        return title;
    };
    let key = match parts.round {
        None => parts.group_id.clone(),
        Some(round) => archived_items_key(&parts.group_id, round),
    };
    let list = run.items.get(&key).map(Vec::as_slice);
    format!("{title} \u{b7} {}", item_title(list, parts.index))
}

/// What a run did with one node: its state, the rounds a loop already finished, why it was
/// decided the way it was, its output and, for a command, how it exited.
pub(super) fn details_of(run: &WorkflowRun, key: &str) -> Option<Details> {
    let node = *node_runs_by_key(run).get(key)?;
    let def = run.definition.nodes.iter().find(|n| n.id == node.node_id);
    let status = status_of(Some(node));
    // A loop resets a node to pending but leaves its last round's results; they are not current.
    let not_started = status == RunNodeStatus::Pending;
    let asking = monocode_core::mono::workflows::engine::is_asking(node);
    let is_command = def.is_some_and(|d| matches!(d.kind, NodeKind::Command { .. }));
    let output = match (&node.output, not_started) {
        (Some(output), false) => Some(output),
        _ => None,
    };
    let field = |name: &str| output.and_then(|o| o.get(name));
    let command = if is_command {
        let exit_code = field("exitCode").map(shown);
        let text = |name: &str| match field(name) {
            Some(Json::String(s)) if !s.is_empty() => Some(tail(s)),
            _ => None,
        };
        let (stdout, stderr) = (text("stdout"), text("stderr"));
        (exit_code.is_some() || stdout.is_some() || stderr.is_some()).then_some(CommandBlock {
            exit_code,
            stdout,
            stderr,
        })
    } else {
        None
    };
    let outputs = output
        .into_iter()
        .flat_map(|o| o.iter())
        .filter(|(name, _)| {
            !(is_command && ["exitCode", "stdout", "stderr"].contains(&name.as_str()))
        })
        .map(|(name, value)| {
            let text = shown(value);
            OutputLine {
                name: name.clone(),
                long: utf16_len(&text) > SHORT,
                text,
            }
        })
        .collect();
    let started = (!not_started).then_some(node.started_at).flatten();
    let took = match started {
        None => Took::None,
        Some(start) => match node.ended_at {
            Some(end) => Took::Spent(end.saturating_sub(start)),
            None if node.status == NodeStatus::Running => Took::Since(start),
            None => Took::Unknown,
        },
    };
    Some(Details {
        heading: heading(run, node),
        kind: def.map_or("Node", |d| node_label(&d.kind)),
        status,
        status_label: if asking {
            "Asking the Mono"
        } else {
            status_label(status)
        },
        round: (node.round > 1).then_some(node.round),
        started,
        took,
        decision: node
            .decision
            .as_ref()
            .map(|d| decision_text(&d.value, d.reason.as_deref())),
        session: if not_started {
            None
        } else {
            node.session_id.clone()
        },
        error: node.error.clone(),
        rounds: node.history.iter().flatten().map(round_line).collect(),
        command,
        outputs,
    })
}
