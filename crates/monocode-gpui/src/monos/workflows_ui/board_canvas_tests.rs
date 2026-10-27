//! The gestures of the board canvas: connecting, looping, dragging, deleting and focus.
use super::board::{BoardField, BoardPick, InspectorView};
use super::board_tests::*;
use super::graph::GraphEvent;
use super::tests::click;
use super::*;
use crate::monos::tests::{Setup, setup};
use gpui::{Modifiers, TestAppContext, VisualTestContext};
use monocode_core::mono::workflows::board_model::NodeType;
use serde_json::json;

// ---- the canvas: connect, loop, drag, delete -----------------------------------------------

pub(super) fn edges_of(s: &Setup, visual: &mut VisualTestContext) -> Vec<(String, String, bool)> {
    on_board(s, visual, |b| {
        b.flow
            .edges
            .iter()
            .map(|e| (e.source.clone(), e.target.clone(), e.loop_spec.is_some()))
            .collect()
    })
}

#[gpui::test]
fn connecting_validates_with_connection_kind(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    do_with(&s, visual, |m, cx| m.board_add(NodeType::Command, cx));
    let command = node_ids(&s, visual)
        .into_iter()
        .find(|id| id.starts_with("command"))
        .unwrap();
    let before = edges_of(&s, visual);
    // Into a trigger, and across a For each boundary: nothing happens.
    for (from, to) in [("fetch", "trigger"), ("fetch", "dev")] {
        event(
            &s,
            visual,
            GraphEvent::Connect {
                source: from.into(),
                handle: None,
                target: to.into(),
                at: at(),
            },
        );
    }
    assert_eq!(edges_of(&s, visual), before);
    assert!(on_board(&s, visual, |b| b.loop_ask.is_none()));
    // The canvas only lets a valid connection light up.
    let canvas = on_board(&s, visual, |b| b.canvas.clone());
    assert!(!canvas.read_with(visual, |c, _| c.may_connect("report", None, &command)));
    assert!(!canvas.read_with(visual, |c, _| c.may_connect("fetch", None, "trigger")));
    assert!(canvas.read_with(visual, |c, _| c.may_connect("fetch", None, &command)));
    // A forward connection adds the edge.
    event(
        &s,
        visual,
        GraphEvent::Connect {
            source: "fetch".into(),
            handle: None,
            target: command.clone(),
            at: at(),
        },
    );
    assert!(edges_of(&s, visual).contains(&("fetch".into(), command, false)));
}

#[gpui::test]
fn a_connection_back_up_a_branch_asks_for_a_loop_and_adding_it_draws_the_dashed_edge(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    let mut w = ticket(json!({}));
    w.edges.retain(|e| e.loop_spec.is_none());
    let mut visual = open_board(&s, &w, cx);
    let visual = &mut visual;
    event(
        &s,
        visual,
        GraphEvent::Connect {
            source: "review".into(),
            handle: None,
            target: "dev".into(),
            at: at(),
        },
    );
    assert!(visual.debug_bounds("wf-loop-ask").is_some());
    assert!(!edges_of(&s, visual).iter().any(|e| e.2));
    // Cancel changes nothing.
    click(visual, "wf-loop-cancel");
    assert!(visual.debug_bounds("wf-loop-ask").is_none());
    assert!(!edges_of(&s, visual).iter().any(|e| e.2));
    // Ask again, change the rounds and the decider, add.
    event(
        &s,
        visual,
        GraphEvent::Connect {
            source: "review".into(),
            handle: None,
            target: "dev".into(),
            at: at(),
        },
    );
    pick(&s, visual, BoardPick::MaxRounds(None), "5");
    pick(&s, visual, BoardPick::Decider(None), "condition");
    type_in(
        &s,
        visual,
        BoardField::LoopCondition(None),
        "{{review.verdict}} != \"approve\"",
    );
    click(visual, "wf-loop-add");
    let looped = on_board(&s, visual, |b| {
        b.flow
            .edges
            .iter()
            .find(|e| e.loop_spec.is_some())
            .and_then(|e| e.loop_spec.clone())
    })
    .expect("a loop");
    assert_eq!(looped.max_rounds, 5.0);
    assert_eq!(looped.decide, "{{review.verdict}} != \"approve\"");
    assert!(edges_of(&s, visual).contains(&("review".into(), "dev".into(), true)));
}

#[gpui::test]
fn the_inspector_edits_and_removes_a_loop(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    event(&s, visual, GraphEvent::Click("review".into()));
    pick(&s, visual, BoardPick::MaxRounds(Some("review".into())), "7");
    let rounds = on_board(&s, visual, |b| {
        b.flow
            .edges
            .iter()
            .find_map(|e| e.loop_spec.clone())
            .map(|l| l.max_rounds)
    });
    assert_eq!(rounds, Some(7.0));
    click(visual, "wf-loop-remove");
    assert!(!edges_of(&s, visual).iter().any(|e| e.2));
}

#[gpui::test]
fn dragging_a_node_over_a_for_each_reparents_it_and_out_of_it_unparents_it(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    let each = on_board(&s, visual, |b| {
        b.flow
            .nodes
            .iter()
            .find(|n| n.id == "each")
            .map(|n| n.position)
            .unwrap()
    });
    event(
        &s,
        visual,
        GraphEvent::Drag {
            id: "fetch".into(),
            to: monocode_core::mono::workflows::board_model::Point {
                x: each.x + 40.,
                y: each.y + 40.,
            },
        },
    );
    event(&s, visual, GraphEvent::Drop("fetch".into()));
    let parent = on_board(&s, visual, |b| {
        b.flow
            .nodes
            .iter()
            .find(|n| n.id == "fetch")
            .and_then(|n| n.parent_id.clone())
    });
    assert_eq!(parent.as_deref(), Some("each"));
    // The wires across the frame went.
    assert!(
        !edges_of(&s, visual)
            .iter()
            .any(|e| e.0 == "trigger" && e.1 == "fetch")
    );
    // And out again.
    event(
        &s,
        visual,
        GraphEvent::Drag {
            id: "dev".into(),
            to: monocode_core::mono::workflows::board_model::Point { x: 900., y: 900. },
        },
    );
    event(&s, visual, GraphEvent::Drop("dev".into()));
    let parent = on_board(&s, visual, |b| {
        b.flow
            .nodes
            .iter()
            .find(|n| n.id == "dev")
            .and_then(|n| n.parent_id.clone())
    });
    assert_eq!(parent, None);
    assert!(on_board(&s, visual, |b| b.changed()));
}

#[gpui::test]
fn delete_removes_the_selected_node_but_not_while_the_inspector_is_open(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    do_with(&s, visual, |m, cx| {
        m.board_graph_event(
            &GraphEvent::Drag {
                id: "fetch".into(),
                to: monocode_core::mono::workflows::board_model::Point { x: 0., y: 90. },
            },
            cx,
        )
    });
    // Selected by the drag; the inspector is open: Delete does nothing.
    do_with(&s, visual, |m, cx| {
        m.board_view(Some(InspectorView::Settings), cx)
    });
    event(&s, visual, GraphEvent::Delete);
    assert!(node_ids(&s, visual).contains(&"fetch".to_string()));
    do_with(&s, visual, |m, cx| m.board_view(None, cx));
    event(&s, visual, GraphEvent::Delete);
    assert!(!node_ids(&s, visual).contains(&"fetch".to_string()));
    assert!(
        !edges_of(&s, visual)
            .iter()
            .any(|e| e.0 == "fetch" || e.1 == "fetch")
    );
}

#[gpui::test]
fn deleting_a_for_each_keeps_its_nodes(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    event(&s, visual, GraphEvent::Click("each".into()));
    click(visual, "wf-inspector-delete");
    let ids = node_ids(&s, visual);
    assert!(!ids.contains(&"each".to_string()));
    assert!(ids.contains(&"dev".to_string()) && ids.contains(&"review".to_string()));
    assert!(on_board(&s, visual, |b| b.view.is_none()));
    assert!(edges_of(&s, visual).contains(&("dev".into(), "review".into(), false)));
}

#[gpui::test]
fn focus_handles_of_deleted_nodes_are_pruned(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    visual.refresh().unwrap();
    let canvas = on_board(&s, visual, |b| b.canvas.clone());
    assert!(
        canvas
            .read_with(visual, |c, _| c.focus_ids())
            .contains(&"fetch".to_string())
    );
    do_with(&s, visual, |m, cx| m.board_remove("fetch", cx));
    visual.refresh().unwrap();
    assert!(
        !canvas
            .read_with(visual, |c, _| c.focus_ids())
            .contains(&"fetch".to_string())
    );
}

#[gpui::test]
fn a_shortcut_with_enter_on_a_focused_card_is_not_swallowed(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    visual.refresh().unwrap();
    let canvas = on_board(&s, visual, |b| b.canvas.clone());
    let focus = canvas
        .read_with(visual, |c, _| c.card_focus("fetch"))
        .expect("a focus handle");
    visual.update(|window, cx| window.focus(&focus, cx));
    visual.simulate_keystrokes("cmd-enter");
    assert_eq!(on_board(&s, visual, |b| b.view.clone()), None);
    visual.simulate_keystrokes("enter");
    assert_eq!(
        on_board(&s, visual, |b| b.view.clone()),
        Some(InspectorView::Node("fetch".into()))
    );
}

#[gpui::test]
fn dragging_a_card_with_the_mouse_moves_it_past_two_pixels_and_a_press_is_a_click(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    visual.refresh().unwrap();
    let bounds = visual.debug_bounds("wf-node-fetch").unwrap();
    let start = bounds.center();
    // Down, a one pixel wobble, up: a click, which opens the inspector and moves nothing.
    let before = on_board(&s, visual, |b| {
        b.flow
            .nodes
            .iter()
            .find(|n| n.id == "fetch")
            .unwrap()
            .position
    });
    visual.simulate_mouse_down(start, gpui::MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        start + gpui::point(px(1.), px(1.)),
        gpui::MouseButton::Left,
        Modifiers::default(),
    );
    visual.simulate_mouse_up(start, gpui::MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(
        on_board(&s, visual, |b| b.view.clone()),
        Some(InspectorView::Node("fetch".into()))
    );
    assert_eq!(
        on_board(&s, visual, |b| b
            .flow
            .nodes
            .iter()
            .find(|n| n.id == "fetch")
            .unwrap()
            .position),
        before
    );
    do_with(&s, visual, |m, cx| m.board_view(None, cx));
    visual.refresh().unwrap();
    let canvas = on_board(&s, visual, |b| b.canvas.clone());
    let zoom = canvas.read_with(visual, |c, _| c.view().zoom);
    // A real drag.
    let bounds = visual.debug_bounds("wf-node-fetch").unwrap();
    let start = bounds.center();
    visual.simulate_mouse_down(start, gpui::MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        start + gpui::point(px(30.), px(0.)),
        gpui::MouseButton::Left,
        Modifiers::default(),
    );
    visual.simulate_mouse_move(
        start + gpui::point(px(60.), px(0.)),
        gpui::MouseButton::Left,
        Modifiers::default(),
    );
    assert!(canvas.read_with(visual, |c, _| c.dragging_node()));
    visual.simulate_mouse_up(
        start + gpui::point(px(60.), px(0.)),
        gpui::MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    let after = on_board(&s, visual, |b| {
        b.flow
            .nodes
            .iter()
            .find(|n| n.id == "fetch")
            .unwrap()
            .position
    });
    assert!(
        (after.x - before.x - 60. / zoom).abs() < 0.5,
        "{after:?} {before:?} {zoom}"
    );
    assert_eq!(after.y, before.y);
    assert!(!canvas.read_with(visual, |c, _| c.dragging_node()));
    // A drag does not open the inspector.
    assert_eq!(on_board(&s, visual, |b| b.view.clone()), None);
}

#[gpui::test]
fn dragging_from_a_handle_draws_a_connection_and_letting_go_on_a_card_connects(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    let mut w = ticket(json!({}));
    w.edges
        .retain(|e| !(e.from == "trigger" && e.to == "fetch"));
    let mut visual = open_board(&s, &w, cx);
    let visual = &mut visual;
    visual.refresh().unwrap();
    let handle = visual
        .debug_bounds("wf-handle-trigger-out")
        .expect("a handle");
    let target = visual.debug_bounds("wf-node-fetch").unwrap().center();
    visual.simulate_mouse_down(
        handle.center(),
        gpui::MouseButton::Left,
        Modifiers::default(),
    );
    visual.simulate_mouse_move(target, gpui::MouseButton::Left, Modifiers::default());
    let canvas = on_board(&s, visual, |b| b.canvas.clone());
    assert!(canvas.read_with(visual, |c, _| c.connecting()));
    visual.simulate_mouse_up(target, gpui::MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(edges_of(&s, visual).contains(&("trigger".into(), "fetch".into(), false)));
    assert!(!canvas.read_with(visual, |c, _| c.connecting()));
}

fn ask_loop(s: &Setup, visual: &mut VisualTestContext) {
    event(
        s,
        visual,
        GraphEvent::Connect {
            source: "review".into(),
            handle: None,
            target: "dev".into(),
            at: at(),
        },
    );
    assert!(visual.debug_bounds("wf-loop-ask").is_some());
}

#[gpui::test]
fn a_press_outside_the_new_loop_card_closes_it(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut w = ticket(json!({}));
    w.edges.retain(|e| e.loop_spec.is_none());
    let mut visual = open_board(&s, &w, cx);
    let visual = &mut visual;
    ask_loop(&s, visual);
    let scrim = visual.debug_bounds("wf-loop-scrim").expect("a scrim");
    let card = visual.debug_bounds("wf-loop-ask").unwrap();
    let outside = gpui::point(scrim.right() - px(4.), scrim.bottom() - px(4.));
    assert!(!card.contains(&outside));
    visual.simulate_click(outside, Default::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("wf-loop-ask").is_none());
    assert!(!edges_of(&s, visual).iter().any(|e| e.2));
}

#[gpui::test]
fn add_loop_after_the_target_was_deleted_adds_no_edge(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut w = ticket(json!({}));
    w.edges.retain(|e| e.loop_spec.is_none());
    let mut visual = open_board(&s, &w, cx);
    let visual = &mut visual;
    ask_loop(&s, visual);
    do_with(&s, visual, |m, cx| m.board_remove("dev", cx));
    do_with(&s, visual, |m, cx| m.board_confirm_loop(cx));
    assert!(!edges_of(&s, visual).iter().any(|e| e.2));
    assert!(on_board(&s, visual, |b| b.loop_ask.is_none()));
}
