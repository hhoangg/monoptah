//! Port of `runGraphModel.test.ts` (same names, same assertions), plus checks of the Rust-side
//! contract (status names, labels, pick handling).
use super::board_model::*;
use super::model::*;
use super::ordered::Json;
use super::run_graph_model::*;
use super::ui_fixtures::{manual, n, report, run, workflow};
use serde_json::{json, Value};
use std::collections::BTreeMap;

const NOW: u64 = 2_000_000_000;

fn session(id: &str, extra: Value) -> Node {
    let mut v = json!({
        "id": id, "type": "session", "project": "{{inputs.project}}", "prompt": "Do it",
        "checkout": "root", "output": {},
    });
    for (k, x) in extra.as_object().unwrap() {
        v[k] = x.clone();
    }
    n(v)
}

fn nr(node_id: &str) -> NodeRun {
    NodeRun {
        status: NodeStatus::Done,
        ..NodeRun::new(node_id, "")
    }
}

fn nr_in(node_id: &str, branch: &str, status: NodeStatus) -> NodeRun {
    NodeRun {
        status,
        ..NodeRun::new(node_id, branch)
    }
}

fn items(v: Value) -> BTreeMap<NodeId, Vec<Json>> {
    v.as_object()
        .unwrap()
        .iter()
        .map(|(k, list)| {
            let Json::Array(list) = Json::from(list) else {
                panic!("not a list")
            };
            (k.clone(), list)
        })
        .collect()
}

fn picks() -> Picks {
    Picks::new()
}

fn views(r: &WorkflowRun) -> Vec<GroupView> {
    group_views(r, &picks(), NOW)
}

fn graph(r: &WorkflowRun, view: RunView) -> RunGraph {
    run_graph(r, view, &views(r), NOW)
}

fn state_of_node<'a>(nodes: &'a [FlowNode], id: &str) -> &'a RunNodeState {
    nodes
        .iter()
        .find(|n| n.id == id)
        .unwrap_or_else(|| panic!("no node {id}"))
        .data
        .run
        .as_ref()
        .unwrap()
}

fn edge_of<'a>(edges: &'a [FlowEdge], from: &str, to: &str, handle: Option<&str>) -> &'a FlowEdge {
    edges
        .iter()
        .find(|e| e.source == from && e.target == to && e.source_handle.as_deref() == handle)
        .unwrap_or_else(|| panic!("no edge {from} -> {to}"))
}

fn children(nodes: &[FlowNode]) -> Vec<&FlowNode> {
    nodes.iter().filter(|n| n.parent_id.is_some()).collect()
}

fn node_ids(nodes: &[&FlowNode]) -> Vec<String> {
    nodes.iter().map(|n| n.id.clone()).collect()
}

/// trigger -> dev -> report, no board positions.
fn linear() -> Workflow {
    let mut w = workflow();
    w.nodes = vec![manual(), session("dev", json!({})), report()];
    w.edges = vec![Edge::new("trigger", "dev"), Edge::new("dev", "report")];
    w
}

/// trigger -> each (dev -> review inside) -> report.
fn each_workflow() -> Workflow {
    let mut w = workflow();
    w.nodes = vec![
        manual(),
        n(
            json!({"id": "each", "type": "forEach", "in": "{{trigger.tickets}}", "title": "Tickets"}),
        ),
        session("dev", json!({"parent": "each"})),
        session("review", json!({"parent": "each"})),
        report(),
    ];
    w.edges = vec![
        Edge::new("trigger", "each"),
        Edge::new("dev", "review"),
        Edge::new("each", "report"),
    ];
    w
}

fn node_status(s: RunNodeStatus) -> NodeStatus {
    match s {
        RunNodeStatus::Done => NodeStatus::Done,
        RunNodeStatus::Running => NodeStatus::Running,
        RunNodeStatus::Waiting => NodeStatus::WaitingApproval,
        RunNodeStatus::Failed => NodeStatus::Failed,
        RunNodeStatus::Skipped => NodeStatus::Skipped,
        RunNodeStatus::Pending => NodeStatus::Pending,
        RunNodeStatus::Stopped => NodeStatus::Stopped,
    }
}

fn each_run(statuses: &[RunNodeStatus]) -> WorkflowRun {
    let mut nodes = vec![
        nr("trigger"),
        NodeRun {
            status: NodeStatus::Running,
            ..nr("each")
        },
    ];
    for (i, status) in statuses.iter().enumerate() {
        let branch = format!("each#{i}");
        nodes.push(nr_in("dev", &branch, node_status(*status)));
        nodes.push(nr_in("review", &branch, node_status(*status)));
    }
    nodes.push(nr_in("report", "", NodeStatus::Pending));
    let list: Vec<Value> = (0..statuses.len())
        .map(|i| json!({"title": format!("Ticket {}", i + 1)}))
        .collect();
    WorkflowRun {
        definition: each_workflow(),
        status: RunStatus::Running,
        items: items(json!({"each": list})),
        nodes,
        ..run()
    }
}

fn pending(count: usize) -> WorkflowRun {
    each_run(&vec![RunNodeStatus::Pending; count])
}

fn move_node(w: &mut Workflow, id: &str, x: f64, y: f64) {
    w.nodes.iter_mut().find(|m| m.id == id).unwrap().ui = Some(Ui { x, y });
}

fn apart(a: &FlowNode, b: &FlowNode) -> bool {
    a.position.x + a.width <= b.position.x
        || b.position.x + b.width <= a.position.x
        || a.position.y + a.height <= b.position.y
        || b.position.y + b.height <= a.position.y
}

use RunNodeStatus::{Done, Failed, Pending, Running};

#[test]
fn linear_run_maps_statuses_and_labels() {
    let r = WorkflowRun {
        definition: linear(),
        nodes: vec![
            nr("trigger"),
            NodeRun {
                started_at: Some(1000),
                ended_at: Some(13000),
                ..nr("dev")
            },
            nr_in("report", "", NodeStatus::Pending),
        ],
        ..run()
    };
    let RunGraph { nodes, edges } = graph(&r, RunView::Lanes);
    assert_eq!(state_of_node(&nodes, "trigger").status, Done);
    assert_eq!(state_of_node(&nodes, "dev").status, Done);
    assert!(state_of_node(&nodes, "dev").label.starts_with("Done"));
    assert!(state_of_node(&nodes, "dev").label.contains("12s"));
    assert_eq!(state_of_node(&nodes, "dev").node_run_key, "dev|");
    assert_eq!(state_of_node(&nodes, "report").status, Pending);
    assert_eq!(
        edge_of(&edges, "trigger", "dev", None).run,
        Some(RunEdgeState::Taken)
    );
    // Spec 5.1: a done source takes its edge even though the target has not started.
    assert_eq!(
        edge_of(&edges, "dev", "report", None).run,
        Some(RunEdgeState::Taken)
    );
}

#[test]
fn an_edge_out_of_a_done_node_is_taken_whether_its_target_is_pending_or_skipped() {
    for target in [NodeStatus::Pending, NodeStatus::Skipped] {
        let r = WorkflowRun {
            definition: linear(),
            nodes: vec![nr("trigger"), nr("dev"), nr_in("report", "", target)],
            ..run()
        };
        let g = graph(&r, RunView::Lanes);
        assert_eq!(
            edge_of(&g.edges, "dev", "report", None).run,
            Some(RunEdgeState::Taken)
        );
    }
    let not_done = WorkflowRun {
        definition: linear(),
        nodes: vec![
            nr("trigger"),
            nr_in("dev", "", NodeStatus::Failed),
            nr_in("report", "", NodeStatus::Pending),
        ],
        ..run()
    };
    let g = graph(&not_done, RunView::Lanes);
    assert_eq!(
        edge_of(&g.edges, "dev", "report", None).run,
        Some(RunEdgeState::Idle)
    );
}

#[test]
fn if_marks_only_the_taken_side() {
    let mut w = workflow();
    w.nodes = vec![
        manual(),
        n(json!({"id": "check", "type": "if", "condition": "{{trigger.x}}"})),
        session("yes", json!({})),
        session("no", json!({})),
    ];
    w.edges = vec![
        Edge::new("trigger", "check"),
        super::fixtures::edge(r#"{"from": "check", "to": "yes", "handle": "true"}"#),
        super::fixtures::edge(r#"{"from": "check", "to": "no", "handle": "false"}"#),
    ];
    let r = WorkflowRun {
        definition: w,
        nodes: vec![
            nr("trigger"),
            NodeRun {
                decision: Some(Decision {
                    value: "true".into(),
                    reason: None,
                }),
                ..nr("check")
            },
            nr_in("yes", "", NodeStatus::Pending),
            nr_in("no", "", NodeStatus::Skipped),
        ],
        ..run()
    };
    let RunGraph { nodes, edges } = graph(&r, RunView::Lanes);
    assert_eq!(
        edge_of(&edges, "check", "yes", Some("true")).run,
        Some(RunEdgeState::Taken)
    );
    assert_eq!(
        edge_of(&edges, "check", "no", Some("false")).run,
        Some(RunEdgeState::Idle)
    );
    assert_eq!(state_of_node(&nodes, "no").status, RunNodeStatus::Skipped);
    assert_eq!(state_of_node(&nodes, "check").label, "Done · true");
}

#[test]
fn keeps_the_duration_next_to_a_decision_a_failure_or_a_stop() {
    let r = WorkflowRun {
        definition: linear(),
        nodes: vec![
            NodeRun {
                decision: Some(Decision {
                    value: "loop".into(),
                    reason: Some("needs tests".into()),
                }),
                started_at: Some(0),
                ended_at: Some(12_000),
                ..nr("trigger")
            },
            NodeRun {
                error: Some("Exit code 2".into()),
                started_at: Some(0),
                ended_at: Some(65_000),
                ..nr_in("dev", "", NodeStatus::Failed)
            },
            NodeRun {
                started_at: Some(0),
                ended_at: Some(3000),
                ..nr_in("report", "", NodeStatus::Stopped)
            },
        ],
        ..run()
    };
    let RunGraph { nodes, .. } = graph(&r, RunView::Lanes);
    assert_eq!(
        state_of_node(&nodes, "trigger").label,
        "Done · loop: needs tests · 12s"
    );
    assert_eq!(
        state_of_node(&nodes, "dev").label,
        "Failed: Exit code 2 · 1m 5s"
    );
    assert_eq!(state_of_node(&nodes, "report").label, "Stopped · 3s");
}

#[test]
fn running_node_and_active_edge() {
    let now = 1_000_000_000;
    let r = WorkflowRun {
        definition: linear(),
        status: RunStatus::Running,
        nodes: vec![
            nr("trigger"),
            NodeRun {
                started_at: Some(now - 180_000),
                ..nr_in("dev", "", NodeStatus::Running)
            },
            nr_in("report", "", NodeStatus::Pending),
        ],
        ..run()
    };
    let RunGraph { nodes, edges } = run_graph(&r, RunView::Lanes, &views(&r), now);
    assert_eq!(state_of_node(&nodes, "dev").status, Running);
    assert_eq!(state_of_node(&nodes, "dev").label, "Running 3m");
    assert_eq!(
        edge_of(&edges, "trigger", "dev", None).run,
        Some(RunEdgeState::Active)
    );
}

#[test]
fn asking_a_loop_question_shows_waiting() {
    let r = WorkflowRun {
        definition: linear(),
        status: RunStatus::Running,
        nodes: vec![
            nr("trigger"),
            NodeRun {
                started_at: Some(1),
                ended_at: Some(2),
                ..nr_in("dev", "", NodeStatus::Running)
            },
            nr_in("report", "", NodeStatus::Pending),
        ],
        ..run()
    };
    let RunGraph { nodes, .. } = graph(&r, RunView::Lanes);
    assert_eq!(state_of_node(&nodes, "dev").status, RunNodeStatus::Waiting);
    assert_eq!(state_of_node(&nodes, "dev").label, "Asking the Mono");
}

#[test]
fn waiting_for_approval_and_a_failure_show_their_own_state() {
    let r = WorkflowRun {
        definition: linear(),
        status: RunStatus::Running,
        nodes: vec![
            nr("trigger"),
            NodeRun {
                error: Some("Exit code 2".into()),
                ..nr_in("dev", "", NodeStatus::Failed)
            },
            nr_in("report", "", NodeStatus::WaitingApproval),
        ],
        ..run()
    };
    let RunGraph { nodes, .. } = graph(&r, RunView::Lanes);
    let dev = state_of_node(&nodes, "dev");
    assert_eq!(dev.status, Failed);
    assert_eq!(dev.label, "Failed: Exit code 2");
    assert_eq!(
        state_of_node(&nodes, "report").status,
        RunNodeStatus::Waiting
    );
    assert_eq!(
        state_of_node(&nodes, "report").label,
        "Waiting for approval"
    );
}

#[test]
fn lanes_view_fans_a_for_each_into_one_lane_per_item() {
    let r = each_run(&[Done, Running, Failed, Pending]);
    let RunGraph { nodes, edges } = graph(&r, RunView::Lanes);
    let kids = children(&nodes);
    assert_eq!(kids.len(), 8);
    let mut unique = node_ids(&kids);
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), 8);
    assert_eq!(state_of_node(&nodes, "dev@each#0").status, Done);
    assert_eq!(state_of_node(&nodes, "review@each#1").status, Running);
    assert_eq!(state_of_node(&nodes, "dev@each#2").status, Failed);
    assert_eq!(state_of_node(&nodes, "dev@each#3").status, Pending);
    assert_eq!(
        state_of_node(&nodes, "dev@each#2").node_run_key,
        "dev|each#2"
    );
    assert!(kids.iter().all(|n| n.parent_id.as_deref() == Some("each")));
    // One dev -> review edge per lane, plus the two top-level ones.
    assert_eq!(
        edges
            .iter()
            .filter(|e| e.source.starts_with("dev@"))
            .count(),
        4
    );
    let group = nodes.iter().find(|n| n.id == "each").unwrap();
    let one = pending(1);
    let lone_graph = graph(&one, RunView::Lanes);
    let lone = lone_graph.nodes.iter().find(|n| n.id == "each").unwrap();
    assert!(group.width > lone.width);
    // Lanes sit side by side without overlapping.
    let xs: Vec<f32> = (0..4)
        .map(|i| {
            nodes
                .iter()
                .find(|n| n.id == format!("dev@each#{i}"))
                .unwrap()
                .position
                .x
        })
        .collect();
    for i in 1..xs.len() {
        assert!(xs[i] - xs[i - 1] >= LANE_CARD_W + LANE_GAP);
    }
}

#[test]
fn lanes_view_is_compact_four_lanes_of_two_nodes_fit_a_narrow_panel() {
    let r = each_run(&[Done, Running, Failed, Pending]);
    let RunGraph { nodes, edges } = graph(&r, RunView::Lanes);
    let group = nodes.iter().find(|n| n.id == "each").unwrap();
    assert!(group.width <= 500.0);
    let kids = children(&nodes);
    assert!(kids
        .iter()
        .all(|n| n.width == LANE_CARD_W && n.data.compact));
    // Every card stays inside the group box.
    for k in &kids {
        assert!(k.position.x + k.width <= group.width);
    }
    assert!(edges
        .iter()
        .filter(|e| e.source.starts_with("dev@"))
        .all(|e| e.compact));
    // One lane or the single view keeps the board's cards.
    let single = graph(&r, RunView::Single);
    assert!(children(&single.nodes)
        .iter()
        .all(|n| n.width == NODE_W && !n.data.compact));
    assert!(single.edges.iter().all(|e| !e.compact));
}

#[test]
fn a_for_each_says_how_many_items_it_has() {
    let r = each_run(&[Done, Pending, Pending]);
    let g = graph(&r, RunView::Single);
    assert_eq!(
        g.nodes
            .iter()
            .find(|n| n.id == "each")
            .unwrap()
            .data
            .run_items,
        Some(3)
    );
}

#[test]
fn single_view_shows_only_the_selected_item() {
    let r = each_run(&[Done, Running, Failed, Pending]);
    let mut pick = Picks::new();
    pick.insert(
        "each".into(),
        Pick {
            round: None,
            item: Some(2),
        },
    );
    let groups = group_views(&r, &pick, NOW);
    assert_eq!(groups[0].item, 2);
    let g = run_graph(&r, RunView::Single, &groups, NOW);
    let mut got = node_ids(&children(&g.nodes));
    got.sort();
    assert_eq!(got, ["dev@each#2", "review@each#2"]);
}

#[test]
fn picks_the_first_running_item_by_default() {
    let r = each_run(&[Done, Running, Failed, Pending]);
    assert_eq!(views(&r)[0].item, 1);
    assert_eq!(views(&each_run(&[Done, Done]))[0].item, 0);
}

#[test]
fn rounds_of_a_for_each_inside_a_loop() {
    let mut r = each_run(&[Running, Pending]);
    r.items = items(json!({
        "each": [{"title": "New A"}, {"title": "New B"}],
        "each@r1": [{"title": "Old A"}],
    }));
    r.nodes
        .iter_mut()
        .find(|n| n.node_id == "each")
        .unwrap()
        .round = 2;
    r.nodes.push(nr_in("dev", "each#0@r1", NodeStatus::Done));
    r.nodes.push(NodeRun {
        error: Some("nope".into()),
        ..nr_in("review", "each#0@r1", NodeStatus::Failed)
    });
    let group = views(&r).remove(0);
    assert_eq!(group.rounds, [1, 2]);
    assert_eq!(group.round, 2);
    let titles: Vec<&str> = group.items.iter().map(|i| i.title.as_str()).collect();
    assert_eq!(titles, ["New A", "New B"]);

    let mut pick = Picks::new();
    pick.insert(
        "each".into(),
        Pick {
            round: Some(1),
            item: None,
        },
    );
    let old = group_views(&r, &pick, NOW).remove(0);
    assert_eq!(old.round, 1);
    assert_eq!(
        old.items,
        vec![GroupItem {
            index: 0,
            title: "Old A".into(),
            status: Failed
        }]
    );
    let g = run_graph(&r, RunView::Lanes, &[old], NOW);
    let mut got = node_ids(&children(&g.nodes));
    got.sort();
    assert_eq!(got, ["dev@each#0@r1", "review@each#0@r1"]);
    assert_eq!(
        state_of_node(&g.nodes, "review@each#0@r1").node_run_key,
        "review|each#0@r1"
    );
}

#[test]
fn titles_an_item_by_name_else_by_place() {
    let mut r = pending(3);
    r.items = items(json!({"each": [{"name": " Billing "}, "plain", {"title": ""}]}));
    let titles: Vec<String> = views(&r)[0].items.iter().map(|i| i.title.clone()).collect();
    assert_eq!(titles, ["Billing", "#2", "#3"]);
}

#[test]
fn summarises_the_items_of_a_group() {
    let r = each_run(&[Done, Done, Running, Failed]);
    assert_eq!(views(&r)[0].summary, "2 done · 1 running · 1 failed");
}

#[test]
fn defaults_to_single_view_for_many_items() {
    assert_eq!(default_run_view(&pending(30)), RunView::Single);
    assert_eq!(default_run_view(&pending(3)), RunView::Lanes);
    assert_eq!(
        default_run_view(&WorkflowRun {
            definition: linear(),
            ..run()
        }),
        RunView::Lanes
    );
    let mut wide = pending(2);
    wide.definition
        .nodes
        .push(session("extra", json!({"parent": "each"})));
    wide.definition
        .nodes
        .push(session("more", json!({"parent": "each"})));
    assert_eq!(default_run_view(&wide), RunView::Single);
}

#[test]
fn empty_and_failed_for_each_have_no_lanes() {
    let mut empty = pending(0);
    empty
        .nodes
        .iter_mut()
        .find(|n| n.node_id == "each")
        .unwrap()
        .status = NodeStatus::Done;
    let group = views(&empty).remove(0);
    assert_eq!(group.summary, "No items");
    assert!(group.items.is_empty());
    let g = run_graph(&empty, RunView::Lanes, &[group], NOW);
    assert_eq!(children(&g.nodes).len(), 0);

    let failed = WorkflowRun {
        definition: each_workflow(),
        items: BTreeMap::new(),
        nodes: vec![
            nr("trigger"),
            NodeRun {
                error: Some("Expected a list for {{trigger.tickets}}".into()),
                ..nr_in("each", "", NodeStatus::Failed)
            },
        ],
        ..run()
    };
    let bad = views(&failed).remove(0);
    assert_eq!(bad.summary, "Expected a list for {{trigger.tickets}}");
    assert!(bad.rounds.is_empty());
    let g = run_graph(&failed, RunView::Single, &[bad], NOW);
    assert_eq!(children(&g.nodes).len(), 0);
}

#[test]
fn places_nodes_without_positions_like_the_board() {
    for view in [RunView::Lanes, RunView::Single] {
        let r = each_run(&[Done, Running, Pending]);
        let g = graph(&r, view);
        let top: Vec<&FlowNode> = g.nodes.iter().filter(|n| n.parent_id.is_none()).collect();
        assert_eq!(top.len(), 3);
        for n in &top {
            assert!(n.position.x.is_finite() && n.position.y.is_finite());
        }
        for i in 0..top.len() {
            for j in i + 1..top.len() {
                assert!(apart(top[i], top[j]));
            }
        }
    }
}

#[test]
fn keeps_a_widened_group_clear_of_a_sibling_beside_it() {
    let mut w = each_workflow();
    move_node(&mut w, "each", 0.0, 100.0);
    move_node(&mut w, "report", 300.0, 100.0);
    let r = WorkflowRun {
        definition: w,
        ..each_run(&[Done, Done, Done])
    };
    let g = graph(&r, RunView::Lanes);
    let group = g.nodes.iter().find(|n| n.id == "each").unwrap();
    let report = g.nodes.iter().find(|n| n.id == "report").unwrap();
    assert!(report.position.x >= group.position.x + group.width);
}

#[test]
fn renders_a_run_saved_before_history_existed() {
    // No history, no ui positions, and no items entry for the unexpanded group.
    let r = WorkflowRun {
        definition: each_workflow(),
        items: BTreeMap::new(),
        nodes: vec![
            nr("trigger"),
            nr_in("each", "", NodeStatus::Pending),
            nr_in("report", "", NodeStatus::Pending),
        ],
        ..run()
    };
    for view in [RunView::Lanes, RunView::Single] {
        let _ = graph(&r, view);
    }
    assert!(views(&r)[0].rounds.is_empty());
    assert_eq!(
        node_runs_by_key(&r).get("each|").map(|n| n.status),
        Some(NodeStatus::Pending)
    );
}

#[test]
fn copes_with_a_run_that_has_fewer_node_runs_than_the_definition() {
    let r = WorkflowRun {
        definition: linear(),
        nodes: vec![],
        ..run()
    };
    let g = graph(&r, RunView::Lanes);
    let dev = state_of_node(&g.nodes, "dev");
    assert_eq!(dev.status, Pending);
    assert_eq!(dev.round, 1);
}

#[test]
fn shows_the_round_on_a_node_that_looped_and_a_loop_edge_as_taken() {
    let mut w = workflow();
    w.nodes = vec![
        manual(),
        session("dev", json!({})),
        session("review", json!({})),
    ];
    w.edges = vec![
        Edge::new("trigger", "dev"),
        Edge::new("dev", "review"),
        super::fixtures::edge(
            r#"{"from": "review", "to": "dev", "loop": {"maxRounds": 3, "decide": "mono"}}"#,
        ),
    ];
    let r = WorkflowRun {
        definition: w,
        status: RunStatus::Running,
        nodes: vec![
            nr("trigger"),
            NodeRun {
                round: 2,
                started_at: Some(NOW),
                ..nr_in("dev", "", NodeStatus::Running)
            },
            NodeRun {
                round: 2,
                history: Some(vec![RoundRecord {
                    round: 1,
                    status: NodeStatus::Done,
                    session_id: None,
                    error: None,
                    decision: Some(Decision {
                        value: "loop".into(),
                        reason: Some("needs tests".into()),
                    }),
                    started_at: None,
                    ended_at: None,
                }]),
                ..nr_in("review", "", NodeStatus::Pending)
            },
        ],
        ..run()
    };
    let RunGraph { nodes, edges } = graph(&r, RunView::Lanes);
    assert_eq!(state_of_node(&nodes, "dev").round, 2);
    assert!(state_of_node(&nodes, "dev").label.contains("round 2"));
    let looped = edges.iter().find(|e| e.loop_spec.is_some()).unwrap();
    assert_eq!(looped.run, Some(RunEdgeState::Taken));
    assert!(
        nodes
            .iter()
            .find(|n| n.id == "review")
            .unwrap()
            .data
            .loop_out
    );
    assert!(nodes.iter().find(|n| n.id == "dev").unwrap().data.loop_in);
}

#[test]
fn a_for_each_in_a_loop_whose_current_round_failed_shows_that_round() {
    let r = WorkflowRun {
        definition: each_workflow(),
        items: items(json!({"each@r1": [{"title": "Old A"}]})),
        nodes: vec![
            nr("trigger"),
            NodeRun {
                round: 2,
                error: Some("Expected a list for {{trigger.tickets}}".into()),
                ..nr_in("each", "", NodeStatus::Failed)
            },
            nr_in("dev", "each#0@r1", NodeStatus::Done),
            nr_in("review", "each#0@r1", NodeStatus::Done),
        ],
        ..run()
    };
    let group = views(&r).remove(0);
    assert_eq!(group.rounds, [1, 2]);
    assert_eq!(group.round, 2);
    assert!(group.items.is_empty());
    assert_eq!(group.summary, "Expected a list for {{trigger.tickets}}");
    let g = run_graph(&r, RunView::Lanes, &[group], NOW);
    assert_eq!(children(&g.nodes).len(), 0);
    let mut pick = Picks::new();
    pick.insert(
        "each".into(),
        Pick {
            round: Some(1),
            item: None,
        },
    );
    let old = group_views(&r, &pick, NOW).remove(0);
    assert_eq!(old.summary, "1 done");
}

#[test]
fn no_items_is_only_for_a_for_each_that_expanded_to_an_empty_list() {
    let states = [
        (NodeStatus::Pending, None, "Not started"),
        (NodeStatus::Skipped, None, "Skipped"),
        (NodeStatus::Failed, Some("boom"), "boom"),
    ];
    for (status, error, summary) in states {
        let r = WorkflowRun {
            definition: each_workflow(),
            items: BTreeMap::new(),
            nodes: vec![
                nr("trigger"),
                NodeRun {
                    error: error.map(str::to_string),
                    ..nr_in("each", "", status)
                },
            ],
            ..run()
        };
        assert_eq!(views(&r)[0].summary, summary);
    }
    let no_record = WorkflowRun {
        definition: each_workflow(),
        items: BTreeMap::new(),
        nodes: vec![nr("trigger")],
        ..run()
    };
    assert_eq!(views(&no_record)[0].summary, "Not started");
    let g = graph(&no_record, RunView::Lanes);
    let group = g.nodes.iter().find(|n| n.id == "each").unwrap();
    assert_eq!(group.data.run.as_ref().unwrap().label, "Not started");
    let empty = WorkflowRun {
        definition: each_workflow(),
        items: items(json!({"each": []})),
        nodes: vec![nr("trigger"), nr("each")],
        ..run()
    };
    assert_eq!(views(&empty)[0].summary, "No items");
}

#[test]
fn defaults_to_single_view_when_the_children_sit_side_by_side() {
    let mut w = each_workflow();
    move_node(&mut w, "dev", 16.0, 32.0);
    move_node(&mut w, "review", 300.0, 32.0);
    let r = WorkflowRun {
        definition: w,
        ..pending(2)
    };
    assert_eq!(default_run_view(&r), RunView::Single);
}

#[test]
fn moves_everything_a_grown_group_would_cover_cascading() {
    let mut w = each_workflow();
    // A tall box to the right, and a node beyond it that only that box's move can reach.
    w.nodes.push(n(json!({"id": "tall", "type": "forEach", "in": "{{trigger.more}}", "ui": {"x": 300, "y": 150}})));
    w.nodes
        .push(session("far", json!({"ui": {"x": 900, "y": 260}})));
    w.nodes
        .push(session("below", json!({"ui": {"x": 0, "y": 300}})));
    w.nodes.push(session(
        "lower",
        json!({"ui": {"x": 0, "y": 300.0 + 48.0 + 4.0}}),
    ));
    move_node(&mut w, "each", 0.0, 0.0);
    move_node(&mut w, "trigger", 600.0, 400.0);
    move_node(&mut w, "report", 600.0, 500.0);
    let r = WorkflowRun {
        definition: w,
        ..each_run(&[Done, Done, Done])
    };
    let g = graph(&r, RunView::Lanes);
    let top: Vec<&FlowNode> = g.nodes.iter().filter(|n| n.parent_id.is_none()).collect();
    for i in 0..top.len() {
        for j in i + 1..top.len() {
            assert!(apart(top[i], top[j]), "{} vs {}", top[i].id, top[j].id);
        }
    }
}

#[test]
fn moves_each_node_below_or_beside_a_widened_group_exactly_once() {
    let mut w = each_workflow();
    let mut at = BTreeMap::new();
    let mut put = |w: &mut Workflow, id: String, x: f64, y: f64| {
        at.insert(id.clone(), (x, y));
        w.nodes.push(session(&id, json!({"ui": {"x": x, "y": y}})));
    };
    for i in 0..6 {
        put(
            &mut w,
            format!("down{i}"),
            0.0,
            300.0 + i as f64 * (48.0 + 10.0),
        );
    }
    for i in 0..6 {
        put(
            &mut w,
            format!("side{i}"),
            300.0 + i as f64 * (232.0 + 10.0),
            50.0,
        );
    }
    move_node(&mut w, "each", 0.0, 0.0);
    move_node(&mut w, "trigger", 700.0, 3000.0);
    move_node(&mut w, "report", 700.0, 3100.0);
    let r = WorkflowRun {
        definition: w.clone(),
        ..each_run(&[Done, Done, Done])
    };
    let g = graph(&r, RunView::Lanes);
    let base = to_flow(&w)
        .nodes
        .iter()
        .find(|n| n.id == "each")
        .unwrap()
        .width;
    let dx = (2.0 * 16.0 + 3.0 * LANE_CARD_W + 2.0 * LANE_GAP + 12.0 - base).max(0.0);
    assert!(dx > 0.0);
    let dy = LANE_HEADER;
    for i in [0, 1, 2, 5] {
        let down = g.nodes.iter().find(|n| n.id == format!("down{i}")).unwrap();
        let (x, y) = at[&format!("down{i}")];
        assert_eq!(
            down.position,
            Point {
                x: x as f32,
                y: y as f32 + dy
            }
        );
        let side = g.nodes.iter().find(|n| n.id == format!("side{i}")).unwrap();
        let (x, y) = at[&format!("side{i}")];
        assert_eq!(
            side.position,
            Point {
                x: x as f32 + dx,
                y: y as f32
            }
        );
    }
}

// --- Rust-side contract ---------------------------------------------------------------------

#[test]
fn status_names_are_the_ones_the_summary_and_the_react_app_use() {
    let names: Vec<&str> = [
        RunNodeStatus::Done,
        RunNodeStatus::Running,
        RunNodeStatus::Waiting,
        RunNodeStatus::Failed,
        RunNodeStatus::Skipped,
        RunNodeStatus::Pending,
        RunNodeStatus::Stopped,
    ]
    .iter()
    .map(|s| s.name())
    .collect();
    assert_eq!(
        names,
        ["done", "running", "waiting", "failed", "skipped", "pending", "stopped"]
    );
}

#[test]
fn labels_cover_every_status_and_cap_long_text() {
    assert_eq!(label_for(None, RunNodeStatus::Pending, NOW), "Not started");
    let skipped = nr_in("a", "", NodeStatus::Skipped);
    assert_eq!(
        label_for(Some(&skipped), RunNodeStatus::Skipped, NOW),
        "Skipped"
    );
    let running = nr_in("a", "", NodeStatus::Running);
    assert_eq!(
        label_for(Some(&running), RunNodeStatus::Running, NOW),
        "Running"
    );
    let long = NodeRun {
        error: Some(format!("a\n\n  b {}", "x".repeat(100))),
        ..nr_in("a", "", NodeStatus::Failed)
    };
    let label = label_for(Some(&long), RunNodeStatus::Failed, NOW);
    assert_eq!(label.chars().count(), "Failed: ".len() + 60);
    assert!(label.starts_with("Failed: a b xxx"));
    assert!(label.ends_with("..."));
    // A decision without a reason, and a duration under a second, say nothing extra.
    let quick = NodeRun {
        decision: Some(Decision {
            value: "false".into(),
            reason: Some(String::new()),
        }),
        started_at: Some(0),
        ended_at: Some(999),
        ..nr("a")
    };
    assert_eq!(
        label_for(Some(&quick), RunNodeStatus::Done, NOW),
        "Done · false"
    );
}

#[test]
fn a_pick_for_a_round_or_item_that_does_not_exist_falls_back() {
    let r = each_run(&[Done, Running]);
    let mut pick = Picks::new();
    pick.insert(
        "each".into(),
        Pick {
            round: Some(9),
            item: Some(7),
        },
    );
    let group = group_views(&r, &pick, NOW).remove(0);
    assert_eq!(group.round, 1);
    assert_eq!(group.item, 1);
}

#[test]
fn a_loop_edge_whose_source_is_asking_is_active_and_one_not_looped_is_idle() {
    let mut w = workflow();
    w.nodes = vec![
        manual(),
        session("dev", json!({})),
        session("review", json!({})),
    ];
    w.edges = vec![
        Edge::new("trigger", "dev"),
        Edge::new("dev", "review"),
        super::fixtures::edge(
            r#"{"from": "review", "to": "dev", "loop": {"maxRounds": 3, "decide": "mono"}}"#,
        ),
    ];
    let asking = WorkflowRun {
        definition: w.clone(),
        nodes: vec![
            nr("trigger"),
            nr("dev"),
            NodeRun {
                started_at: Some(1),
                ended_at: Some(2),
                ..nr_in("review", "", NodeStatus::Running)
            },
        ],
        ..run()
    };
    let g = graph(&asking, RunView::Lanes);
    assert_eq!(
        g.edges.iter().find(|e| e.loop_spec.is_some()).unwrap().run,
        Some(RunEdgeState::Active)
    );
    let quiet = WorkflowRun {
        definition: w,
        nodes: vec![nr("trigger"), nr("dev"), nr("review")],
        ..run()
    };
    let g = graph(&quiet, RunView::Lanes);
    assert_eq!(
        g.edges.iter().find(|e| e.loop_spec.is_some()).unwrap().run,
        Some(RunEdgeState::Idle)
    );
}

#[test]
fn an_edge_between_a_lane_and_the_outside_is_left_out() {
    let mut w = each_workflow();
    w.edges.push(Edge::new("trigger", "dev"));
    let r = WorkflowRun {
        definition: w,
        ..each_run(&[Done, Done])
    };
    let g = graph(&r, RunView::Lanes);
    assert!(g
        .edges
        .iter()
        .all(|e| !(e.source == "trigger" && e.target.starts_with("dev"))));
}

#[test]
fn a_node_below_the_widened_part_of_a_group_moves_down() {
    let mut w = each_workflow();
    w.nodes
        .push(session("corner", json!({"ui": {"x": 350, "y": 400}})));
    move_node(&mut w, "each", 0.0, 0.0);
    move_node(&mut w, "trigger", 700.0, 3000.0);
    move_node(&mut w, "report", 700.0, 3100.0);
    let r = WorkflowRun {
        definition: w,
        ..each_run(&[Done, Done, Done])
    };
    let g = graph(&r, RunView::Lanes);
    let corner = g.nodes.iter().find(|n| n.id == "corner").unwrap();
    assert_eq!(
        corner.position,
        Point {
            x: 350.0,
            y: 400.0 + LANE_HEADER
        }
    );
}

// --- the run page's own rules (WorkflowRunPage.tsx) ---------------------------------------------

#[test]
fn a_view_is_named_as_the_react_app_stores_it() {
    assert_eq!(RunView::Lanes.name(), "lanes");
    assert_eq!(RunView::Single.name(), "single");
    assert_eq!(RunView::parse("lanes"), Some(RunView::Lanes));
    assert_eq!(RunView::parse("single"), Some(RunView::Single));
    assert_eq!(RunView::parse("Lanes"), None);
    assert_eq!(RunView::parse(""), None);
}

#[test]
fn a_chosen_view_beats_the_one_desktop_stored_and_junk_is_ignored() {
    assert_eq!(
        chosen_view(Some("single"), Some("lanes")),
        Some(RunView::Single)
    );
    assert_eq!(chosen_view(None, Some("single")), Some(RunView::Single));
    assert_eq!(
        chosen_view(Some("junk"), Some("single")),
        Some(RunView::Single)
    );
    assert_eq!(chosen_view(None, None), None);
    assert_eq!(chosen_view(Some("junk"), Some("also junk")), None);
    assert_eq!(VIEW_KEY, "monocode.workflowRunView.");
}

#[test]
fn board_positions_do_not_make_a_workflow_changed_but_edges_do() {
    let w = workflow();
    let r = WorkflowRun {
        definition: w.clone(),
        ..run()
    };
    assert!(!definition_changed(&r, &w));
    let mut moved = w.clone();
    for node in &mut moved.nodes {
        node.ui = Some(Ui { x: 5.0, y: 9.0 });
    }
    assert!(!definition_changed(&r, &moved));
    let mut edited = w.clone();
    edited.edges.clear();
    assert!(definition_changed(&r, &edited));
}

#[test]
fn a_changed_node_or_input_makes_a_workflow_changed_but_a_rename_does_not() {
    let w = workflow();
    let r = WorkflowRun {
        definition: w.clone(),
        ..run()
    };
    let mut retitled = w.clone();
    retitled.nodes[1].title = Some("Send it".into());
    assert!(definition_changed(&r, &retitled));
    let mut with_input = w.clone();
    with_input.inputs.insert(
        "project",
        WorkflowInput {
            ty: InputType::Project,
            description: None,
        },
    );
    assert!(definition_changed(&r, &with_input));
    // The key reads inputs, nodes and edges only: what a run does, not what it is called.
    let mut renamed = w;
    renamed.name = "Other name".into();
    assert!(!definition_changed(&r, &renamed));
}

#[test]
fn a_switch_marks_only_the_edge_of_the_case_or_default_it_took() {
    let mut w = workflow();
    w.nodes = vec![
        manual(),
        n(
            json!({"id": "route", "type": "switch", "value": "{{trigger.x}}", "cases": ["bug", "feature"]}),
        ),
        session("fix", json!({})),
        session("plan", json!({})),
        session("other", json!({})),
    ];
    w.edges = vec![
        Edge::new("trigger", "route"),
        super::fixtures::edge(r#"{"from": "route", "to": "fix", "handle": "bug"}"#),
        super::fixtures::edge(r#"{"from": "route", "to": "plan", "handle": "feature"}"#),
        super::fixtures::edge(r#"{"from": "route", "to": "other", "handle": "default"}"#),
    ];
    let taken = |decision: &str| {
        let r = WorkflowRun {
            definition: w.clone(),
            nodes: vec![
                nr("trigger"),
                NodeRun {
                    decision: Some(Decision {
                        value: decision.into(),
                        reason: None,
                    }),
                    ..nr("route")
                },
                nr_in("fix", "", NodeStatus::Pending),
                nr_in("plan", "", NodeStatus::Pending),
                nr_in("other", "", NodeStatus::Pending),
            ],
            ..run()
        };
        let RunGraph { edges, .. } = graph(&r, RunView::Lanes);
        ["bug", "feature", "default"]
            .map(|h| {
                let to = match h {
                    "bug" => "fix",
                    "feature" => "plan",
                    _ => "other",
                };
                edge_of(&edges, "route", to, Some(h)).run
            })
            .to_vec()
    };
    use RunEdgeState::{Idle, Taken};
    assert_eq!(taken("feature"), vec![Some(Idle), Some(Taken), Some(Idle)]);
    assert_eq!(taken("default"), vec![Some(Idle), Some(Idle), Some(Taken)]);
    // A handle on an edge of a node that is not an If or Switch is ignored.
    let mut stray = w.clone();
    stray.edges.push(super::fixtures::edge(
        r#"{"from": "trigger", "to": "other", "handle": "bug"}"#,
    ));
    let r = WorkflowRun {
        definition: stray,
        nodes: vec![nr("trigger"), nr_in("other", "", NodeStatus::Pending)],
        ..run()
    };
    let RunGraph { edges, .. } = graph(&r, RunView::Lanes);
    assert_eq!(
        edge_of(&edges, "trigger", "other", Some("bug")).run,
        Some(Taken)
    );
}
