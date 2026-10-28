//! Behaviour checks ported from `RunGraph.test.ts` and `WorkflowRunPage.test.ts` that need no
//! window: what the graph of a run shows (cards, lanes, tabs, rounds), which node is picked when
//! none was, and what the details of a node say. The rest of the two files runs in a real page
//! in `run_tests.rs`.
use super::graph::{BarKind, Scene};
use super::run_model::*;
use monocode_core::mono::workflows::{
    board_model::FlowKind,
    model::{Workflow, WorkflowRun},
    ordered::Json,
    parse::parse_workflow,
    run_graph_model::{Pick, Picks, RunNodeStatus, RunView, default_run_view},
};
use serde_json::{Value, json};

const NOW: u64 = 2_000_000_000;

pub(super) fn definition_json(nodes: Value, edges: Value) -> Value {
    json!({
        "id": "wf-aaaaaaaa", "name": "Nightly checks", "inputs": {}, "inputValues": {},
        "nodes": nodes, "edges": edges, "enabled": true, "createdAt": 1,
    })
}

pub(super) fn workflow(nodes: Value, edges: Value) -> Workflow {
    parse_workflow(&Json::from(&definition_json(nodes, edges))).expect("a workflow")
}

pub(super) fn session(id: &str, extra: Value) -> Value {
    let mut node = json!({
        "id": id, "type": "session", "title": if id == "dev" { "Dev" } else { "Review" },
        "project": "{{inputs.project}}", "prompt": "Do it", "checkout": "root", "output": {},
    });
    for (key, value) in extra.as_object().unwrap() {
        node[key] = value.clone();
    }
    node
}

pub(super) fn run_of(w: &Workflow, status: &str, items: Value, nodes: Value) -> WorkflowRun {
    serde_json::from_value(json!({
        "id": "run-1", "workflowId": "wf-aaaaaaaa",
        "definition": serde_json::to_value(w).unwrap(),
        "status": status, "startedAt": 1_000_000, "trigger": "manual",
        "items": items, "nodes": nodes,
    }))
    .expect("a run")
}

pub(super) fn nr(node_id: &str, branch: &str, status: &str, extra: Value) -> Value {
    let mut node = json!({"nodeId": node_id, "branch": branch, "status": status, "round": 1});
    for (key, value) in extra.as_object().unwrap() {
        node[key] = value.clone();
    }
    node
}

pub(super) fn linear() -> Workflow {
    workflow(
        json!([{"id": "trigger", "type": "manual"}, session("dev", json!({})), {"id": "report", "type": "report"}]),
        json!([{"from": "trigger", "to": "dev"}, {"from": "dev", "to": "report"}]),
    )
}

/// trigger -> each (dev -> review inside) -> report.
pub(super) fn each_workflow() -> Workflow {
    workflow(
        json!([
            {"id": "trigger", "type": "manual"},
            {"id": "each", "type": "forEach", "in": "{{trigger.tickets}}", "title": "Tickets"},
            session("dev", json!({"parent": "each"})),
            session("review", json!({"parent": "each"})),
            {"id": "report", "type": "report"},
        ]),
        json!([
            {"from": "trigger", "to": "each"},
            {"from": "dev", "to": "review"},
            {"from": "each", "to": "report"},
        ]),
    )
}

pub(super) fn each_run(count: usize) -> WorkflowRun {
    let mut nodes = vec![
        nr("trigger", "", "done", json!({})),
        nr("each", "", "running", json!({})),
    ];
    for i in 0..count {
        let status = if i == 0 { "running" } else { "pending" };
        nodes.push(nr("dev", &format!("each#{i}"), status, json!({})));
        nodes.push(nr("review", &format!("each#{i}"), "pending", json!({})));
    }
    nodes.push(nr("report", "", "pending", json!({})));
    let items: Vec<Value> = (1..=count)
        .map(|i| json!({"title": format!("Ticket {i}")}))
        .collect();
    run_of(
        &each_workflow(),
        "running",
        json!({"each": items}),
        Value::Array(nodes),
    )
}

fn scene(run: &WorkflowRun, view: RunView) -> Scene {
    run_scene(run, view, &Picks::new(), NOW)
}

fn ids(scene: &Scene) -> Vec<String> {
    let mut ids: Vec<String> = scene.nodes.iter().map(|n| n.flow.id.clone()).collect();
    ids.sort();
    ids
}

fn lane_cards(scene: &Scene) -> Vec<String> {
    scene
        .nodes
        .iter()
        .filter(|n| n.flow.id.contains('@'))
        .map(|n| n.flow.id.clone())
        .collect()
}

fn statuses(scene: &Scene) -> Vec<&'static str> {
    scene
        .nodes
        .iter()
        .map(|n| n.flow.data.run.as_ref().unwrap().status.name())
        .collect()
}

// ---- RunGraph.test.ts ---------------------------------------------------------------------

#[test]
fn run_page_draws_one_card_per_run_node() {
    let run = run_of(
        &linear(),
        "running",
        json!({}),
        json!([
            nr("trigger", "", "done", json!({})),
            nr("dev", "", "done", json!({})),
            nr("report", "", "pending", json!({}))
        ]),
    );
    let s = scene(&run, RunView::Lanes);
    assert_eq!(s.nodes.len(), 3);
    let mut by_id: Vec<_> = s
        .nodes
        .iter()
        .map(|n| {
            (
                n.flow.id.clone(),
                n.flow.data.run.as_ref().unwrap().status.name(),
            )
        })
        .collect();
    by_id.sort();
    assert_eq!(
        by_id,
        vec![
            ("dev".into(), "done"),
            ("report".into(), "pending"),
            ("trigger".into(), "done")
        ]
    );
}

#[test]
fn a_run_with_no_nodes_recorded_yet_draws_every_card_pending() {
    let run = run_of(&linear(), "running", json!({}), json!([]));
    let s = scene(&run, RunView::Lanes);
    assert_eq!(s.nodes.len(), 3);
    assert!(statuses(&s).iter().all(|s| *s == "pending"));
}

#[test]
fn lanes_draw_every_item_side_by_side_and_single_draws_one() {
    let run = each_run(4);
    assert_eq!(lane_cards(&scene(&run, RunView::Lanes)).len(), 8);
    assert_eq!(
        lane_cards(&scene(&run, RunView::Single)),
        vec!["dev@each#0", "review@each#0"]
    );
}

#[test]
fn lanes_draw_compact_cards_and_the_group_counts_its_items() {
    let run = each_run(4);
    let lanes = scene(&run, RunView::Lanes);
    let cards: Vec<_> = lanes
        .nodes
        .iter()
        .filter(|n| n.flow.id.contains('@'))
        .collect();
    assert!(cards.iter().all(|n| n.flow.data.compact));
    let group = lanes.nodes.iter().find(|n| n.flow.id == "each").unwrap();
    assert_eq!(group.flow.data.run_items, Some(4));
    assert_eq!(group.flow.data.node.title.as_deref(), Some("Tickets"));
    let single = scene(&run, RunView::Single);
    assert!(
        single
            .nodes
            .iter()
            .filter(|n| n.flow.id.contains('@'))
            .all(|n| !n.flow.data.compact)
    );
}

#[test]
fn many_items_default_to_the_single_lane_with_a_tab_per_item() {
    let run = each_run(30);
    assert_eq!(default_run_view(&run), RunView::Single);
    let s = scene(&run, RunView::Single);
    assert_eq!(lane_cards(&s).len(), 2);
    let tabs = s
        .bars
        .iter()
        .find_map(|b| match &b.kind {
            BarKind::Tabs {
                items, selected, ..
            } => Some((items.len(), *selected, items[0].title.clone())),
            _ => None,
        })
        .expect("a tab strip");
    assert_eq!(tabs, (30, 0, "Ticket 1".to_string()));
}

#[test]
fn picking_a_tab_far_to_the_right_shows_that_item() {
    let run = each_run(30);
    let mut picks = Picks::new();
    picks.insert(
        "each".into(),
        Pick {
            round: None,
            item: Some(7),
        },
    );
    let s = run_scene(&run, RunView::Single, &picks, NOW);
    assert_eq!(lane_cards(&s), vec!["dev@each#7", "review@each#7"]);
    let selected = s.bars.iter().find_map(|b| match &b.kind {
        BarKind::Tabs {
            items, selected, ..
        } => items
            .iter()
            .find(|i| i.index == *selected)
            .map(|i| i.title.clone()),
        _ => None,
    });
    assert_eq!(selected.as_deref(), Some("Ticket 8"));
}

#[test]
fn a_single_lane_with_other_items_waiting_stacks_its_cards() {
    let run = each_run(3);
    let single = scene(&run, RunView::Single);
    assert!(
        single
            .nodes
            .iter()
            .filter(|n| n.flow.id.contains('@'))
            .all(|n| n.stacked)
    );
    assert!(
        single
            .nodes
            .iter()
            .filter(|n| !n.flow.id.contains('@'))
            .all(|n| !n.stacked)
    );
    // One item has nothing behind it, and lanes side by side have no stack.
    assert!(
        scene(&each_run(1), RunView::Single)
            .nodes
            .iter()
            .all(|n| !n.stacked)
    );
    assert!(scene(&run, RunView::Lanes).nodes.iter().all(|n| !n.stacked));
}

#[test]
fn the_picture_is_new_when_the_view_or_the_number_of_cards_changes() {
    let run = each_run(4);
    assert_ne!(
        scene(&run, RunView::Lanes).fit_key,
        scene(&run, RunView::Single).fit_key
    );
    assert_eq!(
        scene(&run, RunView::Lanes).fit_key,
        scene(&run, RunView::Lanes).fit_key
    );
    // Picking a round or an item that keeps the number of cards leaves the view where it is.
    assert_eq!(
        scene(&each_run(4), RunView::Single).fit_key,
        scene(&each_run(9), RunView::Single).fit_key
    );
}

#[test]
fn bars_title_each_lane_and_the_round_selector_sits_over_the_group() {
    let run = each_run(2);
    let s = scene(&run, RunView::Lanes);
    let titles: Vec<String> = s
        .bars
        .iter()
        .filter_map(|b| match &b.kind {
            BarKind::Lane { title, .. } => Some(title.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(titles, vec!["Ticket 1", "Ticket 2"]);
    // Titles sit above their lanes, left to right, as wide as the lane's cards.
    let lanes: Vec<_> = s
        .bars
        .iter()
        .filter(|b| matches!(b.kind, BarKind::Lane { .. }))
        .collect();
    assert!(lanes[0].rect.x < lanes[1].rect.x);
    let first = s.nodes.iter().find(|n| n.flow.id == "dev@each#0").unwrap();
    assert_eq!(lanes[0].rect.x, first.rect.x);
    assert_eq!(
        lanes[0].rect.w,
        first.rect.w.max(
            s.nodes
                .iter()
                .find(|n| n.flow.id == "review@each#0")
                .unwrap()
                .rect
                .w
        )
    );
    assert!(lanes[0].rect.bottom() <= first.rect.y);
    assert!(
        s.bars
            .iter()
            .all(|b| !matches!(b.kind, BarKind::Rounds { .. }))
    );
}

// ---- WorkflowRunPage.test.ts --------------------------------------------------------------

pub(super) fn lane_run() -> WorkflowRun {
    let w = workflow(
        json!([
            {"id": "trigger", "type": "manual"},
            {"id": "each", "type": "forEach", "in": "{{trigger.tickets}}", "title": "Tickets"},
            session("dev", json!({"parent": "each"})),
            {"id": "report", "type": "report"},
        ]),
        json!([{"from": "each", "to": "report"}]),
    );
    run_of(
        &w,
        "running",
        json!({"each": [{"title": "Fix login"}, {"name": "Billing"}, {}]}),
        json!([
            nr("trigger", "", "done", json!({})),
            nr("each", "", "running", json!({})),
            nr(
                "dev",
                "each#0",
                "done",
                json!({"round": 2, "sessionId": "session-1",
                "decision": {"value": "loop", "reason": "tests still failing"}})
            ),
            nr("dev", "each#1", "running", json!({})),
            nr("dev", "each#2", "waitingApproval", json!({})),
            nr("report", "", "pending", json!({})),
        ]),
    )
}

#[test]
fn run_page_shows_one_card_per_item_node_titled_by_item_with_round_2() {
    let run = lane_run();
    let s = scene(&run, RunView::Lanes);
    assert_eq!(
        ids(&s),
        vec![
            "dev@each#0",
            "dev@each#1",
            "dev@each#2",
            "each",
            "report",
            "trigger"
        ]
    );
    let titles: Vec<String> = s
        .bars
        .iter()
        .filter_map(|b| match &b.kind {
            BarKind::Lane { title, .. } => Some(title.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(titles, vec!["Fix login", "Billing", "#3"]);
    let card = |id: &str| {
        s.nodes
            .iter()
            .find(|n| n.flow.id == id)
            .unwrap()
            .flow
            .data
            .run
            .clone()
            .unwrap()
    };
    // The group box carries the summary, the cards their state.
    assert!(
        card("each").label.contains("1 done"),
        "{}",
        card("each").label
    );
    // The loop round shows only past the first one, with the decision's reason.
    assert_eq!(card("dev@each#0").round, 2);
    assert!(card("dev@each#0").label.contains("tests still failing"));
    assert_eq!(card("dev@each#1").round, 1);
    assert_eq!(card("dev@each#1").status, RunNodeStatus::Running);
    assert_eq!(card("dev@each#2").status, RunNodeStatus::Waiting);
    assert!(card("dev@each#2").label.contains("Waiting for approval"));
}

#[test]
fn the_rounds_of_a_for_each_a_loop_ran_again_can_be_picked() {
    let mut run = lane_run();
    run.items.insert(
        "each@r1".into(),
        vec![
            Json::from(&json!({"title": "Old login"})),
            Json::from(&json!({"name": "Old billing"})),
        ],
    );
    run.items.insert(
        "each".into(),
        vec![
            Json::from(&json!({"title": "Fix login"})),
            Json::from(&json!({"name": "Billing"})),
        ],
    );
    run.nodes = serde_json::from_value(json!([
        nr("trigger", "", "done", json!({})),
        nr("each", "", "running", json!({"round": 2})),
        nr("dev", "each#0", "running", json!({"sessionId": "live-0"})),
        nr("dev", "each#1", "pending", json!({})),
        nr(
            "dev",
            "each#0@r1",
            "failed",
            json!({"sessionId": "old-0", "error": "boom"})
        ),
        nr("dev", "each#1@r1", "done", json!({"sessionId": "old-1"})),
        nr("report", "", "pending", json!({})),
    ]))
    .unwrap();
    let s = scene(&run, RunView::Lanes);
    let rounds = s.bars.iter().find_map(|b| match &b.kind {
        BarKind::Rounds {
            rounds, selected, ..
        } => Some((rounds.clone(), *selected)),
        _ => None,
    });
    // The latest round shows by default.
    assert_eq!(rounds, Some((vec![1, 2], 2)));
    let titles = |s: &Scene| -> Vec<String> {
        s.bars
            .iter()
            .filter_map(|b| match &b.kind {
                BarKind::Lane { title, .. } => Some(title.clone()),
                _ => None,
            })
            .collect()
    };
    assert_eq!(titles(&s), vec!["Fix login", "Billing"]);
    let mut picks = Picks::new();
    picks.insert(
        "each".into(),
        Pick {
            round: Some(1),
            item: None,
        },
    );
    let old = run_scene(&run, RunView::Lanes, &picks, NOW);
    assert_eq!(titles(&old), vec!["Old login", "Old billing"]);
    let card = old
        .nodes
        .iter()
        .find(|n| n.flow.id == "dev@each#0@r1")
        .unwrap();
    assert_eq!(card.flow.data.run.as_ref().unwrap().status.name(), "failed");
}

#[test]
fn a_group_in_the_scene_is_a_frame_and_the_frames_come_first() {
    let s = scene(&each_run(2), RunView::Lanes);
    assert_eq!(s.nodes[0].flow.kind, FlowKind::Group);
    assert!(s.nodes[1..].iter().all(|n| n.flow.kind == FlowKind::Board));
}

// ---- which node is picked when none was ---------------------------------------------------

fn pick_run(nodes: Value) -> WorkflowRun {
    run_of(&linear(), "running", json!({}), nodes)
}

#[test]
fn details_start_on_the_running_node() {
    let run = pick_run(json!([
        nr("trigger", "", "done", json!({})),
        nr("dev", "", "running", json!({})),
        nr("report", "", "pending", json!({}))
    ]));
    assert_eq!(default_selection(&run).as_deref(), Some("dev|"));
}

#[test]
fn without_a_running_node_the_last_to_finish_is_picked() {
    let run = pick_run(json!([
        nr("trigger", "", "done", json!({"endedAt": 10})),
        nr("dev", "", "done", json!({"endedAt": 30})),
        nr("report", "", "pending", json!({})),
    ]));
    assert_eq!(default_selection(&run).as_deref(), Some("dev|"));
    // Nothing finished: the last node there is.
    let fresh = pick_run(json!([
        nr("trigger", "", "pending", json!({})),
        nr("dev", "", "pending", json!({}))
    ]));
    assert_eq!(default_selection(&fresh).as_deref(), Some("dev|"));
    assert_eq!(default_selection(&pick_run(json!([]))), None);
}

#[test]
fn a_running_node_beats_a_running_group() {
    let run = each_run(2);
    assert_eq!(default_selection(&run).as_deref(), Some("dev|each#0"));
}

#[test]
fn a_pick_the_run_no_longer_has_falls_back_to_the_default() {
    let run = pick_run(json!([nr("dev", "", "running", json!({}))]));
    assert_eq!(selection(&run, Some("dev|")).as_deref(), Some("dev|"));
    assert_eq!(selection(&run, Some("gone|")).as_deref(), Some("dev|"));
    assert_eq!(selection(&run, None).as_deref(), Some("dev|"));
}

// ---- the details panel --------------------------------------------------------------------

fn details(run: &WorkflowRun, key: &str) -> Details {
    details_of(run, key).expect("a node")
}

#[test]
fn clicking_a_node_shows_its_details_and_round_history() {
    let run = pick_run(json!([
        nr("trigger", "", "done", json!({})),
        nr(
            "dev",
            "",
            "running",
            json!({"round": 2, "startedAt": NOW - 5000, "history": [
                {"round": 1, "status": "done", "sessionId": "s-dev-1",
                 "decision": {"value": "loop", "reason": "needs tests"}, "startedAt": 1000, "endedAt": 13000}
            ]})
        ),
        nr("report", "", "pending", json!({})),
    ]));
    let d = details(&run, "dev|");
    assert_eq!(d.heading, "Dev");
    assert_eq!(d.kind, "Session");
    assert_eq!(d.status_label, "Running");
    assert_eq!(d.round, Some(2));
    assert_eq!(d.rounds.len(), 1);
    let record = &d.rounds[0];
    assert_eq!(record.round, 1);
    assert_eq!(record.status, RunNodeStatus::Done);
    assert_eq!(record.span.as_deref(), Some("12s"));
    assert_eq!(
        record.decision.as_deref(),
        Some("Decided loop: needs tests")
    );
    assert_eq!(record.session.as_deref(), Some("s-dev-1"));
}

#[test]
fn a_loop_that_has_the_node_pending_hides_the_old_rounds_output_and_session() {
    let run = pick_run(json!([nr(
        "dev",
        "",
        "pending",
        json!({
            "round": 2, "sessionId": "s-old", "output": {"summary": "old-round-summary"},
            "startedAt": 1000, "endedAt": 13000,
            "history": [{"round": 1, "status": "done", "sessionId": "s-old", "startedAt": 1000, "endedAt": 13000}],
        })
    )]));
    let d = details(&run, "dev|");
    assert_eq!(d.status_label, "Not started");
    assert!(d.outputs.is_empty());
    assert_eq!(d.session, None);
    assert_eq!(d.started, None);
    assert_eq!(d.took, Took::None);
    // The rounds the loop finished stay.
    assert_eq!(d.rounds.len(), 1);
    assert_eq!(d.rounds[0].session.as_deref(), Some("s-old"));
}

#[test]
fn a_node_waiting_for_approval_or_asking_says_so() {
    let run = pick_run(json!([
        nr("dev", "", "waitingApproval", json!({})),
        nr("report", "", "running", json!({"endedAt": 5})),
    ]));
    assert_eq!(details(&run, "dev|").status_label, "Waiting for approval");
    // A node that ended its work and is still running is asking the Mono.
    assert_eq!(details(&run, "report|").status_label, "Asking the Mono");
}

#[test]
fn long_output_is_cut_until_show_more() {
    let long = "x".repeat(500);
    let run = pick_run(json!([nr(
        "dev",
        "",
        "running",
        json!({"output": {"summary": long, "pr": "u1"}})
    )]));
    let d = details(&run, "dev|");
    let summary = d.outputs.iter().find(|o| o.name == "summary").unwrap();
    assert_eq!(summary.shown(false), format!("{}...", "x".repeat(200)));
    assert_eq!(summary.shown(true), long);
    assert!(summary.long);
    let pr = d.outputs.iter().find(|o| o.name == "pr").unwrap();
    assert_eq!(pr.shown(false), "u1");
    assert!(!pr.long);
}

#[test]
fn an_output_that_is_not_text_is_shown_as_json() {
    let run = pick_run(json!([nr(
        "dev",
        "",
        "done",
        json!({"output": {"n": 3, "list": ["a", 2], "flag": true}})
    )]));
    let d = details(&run, "dev|");
    let shown: Vec<(String, String)> = d
        .outputs
        .iter()
        .map(|o| (o.name.clone(), o.shown(false)))
        .collect();
    assert_eq!(
        shown,
        vec![
            ("n".into(), "3".into()),
            ("list".into(), "[\"a\",2]".into()),
            ("flag".into(), "true".into())
        ]
    );
}

fn command_run(output: Value) -> WorkflowRun {
    let w = workflow(
        json!([{"id": "trigger", "type": "manual"},
               {"id": "lint", "type": "command", "command": "npm run lint", "project": "{{inputs.project}}"}]),
        json!([{"from": "trigger", "to": "lint"}]),
    );
    run_of(
        &w,
        "failed",
        json!({}),
        json!([
            nr("trigger", "", "done", json!({})),
            nr(
                "lint",
                "",
                "failed",
                json!({"error": "exit 2", "output": output})
            ),
        ]),
    )
}

#[test]
fn a_commands_exit_code_and_tails_are_shown_and_not_repeated_as_output() {
    let run = command_run(
        json!({"exitCode": 2, "stdout": "ok so far", "stderr": "boom", "extra": "kept"}),
    );
    let d = details(&run, "lint|");
    assert_eq!(d.kind, "Command");
    let command = d.command.expect("the command block");
    assert_eq!(command.exit_code.as_deref(), Some("2"));
    assert_eq!(command.stdout.as_deref(), Some("ok so far"));
    assert_eq!(command.stderr.as_deref(), Some("boom"));
    assert_eq!(d.outputs.len(), 1);
    assert_eq!(d.outputs[0].name, "extra");
    assert_eq!(d.error.as_deref(), Some("exit 2"));
}

#[test]
fn a_long_tail_keeps_its_last_600_characters() {
    let text: String = (0..700)
        .map(|i| char::from(b'a' + (i % 26) as u8))
        .collect();
    let run = command_run(json!({"exitCode": 0, "stdout": text}));
    let d = details(&run, "lint|");
    let out = d.command.unwrap().stdout.unwrap();
    assert!(out.starts_with("..."));
    assert_eq!(out.chars().count(), 603);
    assert!(out.ends_with(&text[100..]));
}

#[test]
fn the_heading_names_the_item_a_lane_card_ran_for() {
    let run = lane_run();
    assert_eq!(details(&run, "dev|each#0").heading, "Dev \u{b7} Fix login");
    assert_eq!(details(&run, "dev|each#1").heading, "Dev \u{b7} Billing");
    assert_eq!(details(&run, "dev|each#2").heading, "Dev \u{b7} #3");
    assert_eq!(details(&run, "each|").kind, "For each");
    assert!(details_of(&run, "gone|").is_none());
}

#[test]
fn a_node_with_a_decision_shows_it_with_its_reason() {
    let run = lane_run();
    assert_eq!(
        details(&run, "dev|each#0").decision.as_deref(),
        Some("loop: tests still failing")
    );
    assert_eq!(
        details(&run, "dev|each#0").session.as_deref(),
        Some("session-1")
    );
    // A node without a session has nothing to open.
    assert_eq!(details(&run, "dev|each#1").session, None);
}
