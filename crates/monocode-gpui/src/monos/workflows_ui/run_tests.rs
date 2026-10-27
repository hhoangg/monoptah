//! Behaviour checks ported from `RunGraph.test.ts` and `WorkflowRunPage.test.ts` that need a real
//! page: what the page shows and does for a run saved in the Mono's files or going in the
//! runner. The rules that need no window are in `run_model_tests.rs`.
use super::graph::{BarKind, GraphCanvas, GraphEvent};
use super::run_model_tests::{
    each_run, each_workflow, lane_run, linear, nr, run_of, workflow as wf,
};
use super::tests::{A, build, history, put, route, saved};
use super::tests::{click, open_tab, page_notice, settle};
use super::*;
use crate::monos::tests::{Setup, setup};
use gpui::{Entity, TestAppContext, VisualTestContext};
use monocode_core::mono::workflows::{
    model::{RunStatus, WorkflowRun},
    store::update_runs,
};
use serde_json::json;

fn put_run(s: &Setup, run: &WorkflowRun) {
    update_runs(&MonoDir::new(&s.root, &s.id), &run.workflow_id, |_| {
        Ok((vec![run.clone()], ()))
    })
    .unwrap();
}

/// A workflow and a run of it saved in the Mono's files, the Workflows tab open.
fn saved_run(s: &Setup, w: &Workflow, run: &WorkflowRun, cx: &mut TestAppContext) {
    put(s, vec![w.clone()]);
    put_run(s, run);
    open_tab(s, cx);
}

fn open_run(s: &Setup, visual: &mut VisualTestContext) {
    s.manager.update(visual, |m, cx| {
        m.open_workflow_route(Route::WorkflowRun(A.into(), "run-1".into()), cx)
    });
    visual.run_until_parked();
    settle(s, visual);
}

fn canvas(s: &Setup, visual: &mut VisualTestContext) -> Entity<GraphCanvas> {
    s.manager
        .read_with(visual, |m, _| m.wf.run.canvas.clone().expect("a canvas"))
}

fn cards(s: &Setup, visual: &mut VisualTestContext) -> Vec<(String, &'static str)> {
    let canvas = canvas(s, visual);
    canvas.read_with(visual, |c, _| {
        let mut cards: Vec<_> = c
            .scene()
            .nodes
            .iter()
            .map(|n| {
                (
                    n.flow.id.clone(),
                    n.flow.data.run.as_ref().unwrap().status.name(),
                )
            })
            .collect();
        cards.sort();
        cards
    })
}

fn lane_cards(s: &Setup, visual: &mut VisualTestContext) -> Vec<String> {
    cards(s, visual)
        .into_iter()
        .map(|(id, _)| id)
        .filter(|id| id.contains('@'))
        .collect()
}

fn view_setting(s: &Setup) -> Option<String> {
    s.store
        .lock()
        .unwrap()
        .setting("native-workflow-run-view:wf-aaaaaaaa")
        .unwrap()
}

#[gpui::test]
fn run_page_draws_one_card_per_run_node(cx: &mut TestAppContext) {
    let s = setup(cx);
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
    saved_run(&s, &linear(), &run, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    open_run(&s, visual);
    assert_eq!(
        cards(&s, visual),
        vec![
            ("dev".into(), "done"),
            ("report".into(), "pending"),
            ("trigger".into(), "done")
        ]
    );
    for selector in ["wf-node-trigger", "wf-node-dev", "wf-node-report"] {
        assert!(visual.debug_bounds(selector).is_some(), "{selector}");
    }
    assert!(visual.debug_bounds("wf-graph").is_some());
}

#[gpui::test]
fn switch_toggles_lanes_and_single_and_is_remembered(cx: &mut TestAppContext) {
    let s = setup(cx);
    saved_run(&s, &each_workflow(), &each_run(4), cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    open_run(&s, visual);
    assert_eq!(lane_cards(&s, visual).len(), 8);
    click(visual, "wf-run-view");
    assert_eq!(lane_cards(&s, visual).len(), 2);
    visual.run_until_parked();
    assert_eq!(view_setting(&s).as_deref(), Some("single"));
    // Leave the page and come back: the choice is remembered.
    s.manager.update(visual, |m, _| m.route = None);
    visual.run_until_parked();
    open_run(&s, visual);
    assert_eq!(lane_cards(&s, visual).len(), 2);
    click(visual, "wf-run-view");
    assert_eq!(lane_cards(&s, visual).len(), 8);
    visual.run_until_parked();
    assert_eq!(view_setting(&s).as_deref(), Some("lanes"));
}

#[gpui::test]
fn no_switch_without_a_for_each(cx: &mut TestAppContext) {
    let s = setup(cx);
    let run = run_of(
        &linear(),
        "done",
        json!({}),
        json!([nr("trigger", "", "done", json!({}))]),
    );
    saved_run(&s, &linear(), &run, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    open_run(&s, visual);
    assert!(visual.debug_bounds("wf-run-view").is_none());
}

#[gpui::test]
fn tabs_scroll_horizontally_and_picking_one_shows_its_item(cx: &mut TestAppContext) {
    let s = setup(cx);
    saved_run(&s, &each_workflow(), &each_run(30), cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    open_run(&s, visual);
    // Many items default to the single lane view, with a tab for each.
    assert_eq!(lane_cards(&s, visual).len(), 2);
    let strip = visual
        .debug_bounds("wf-tabs-bar-tabs:each")
        .expect("the tab strip");
    let far = visual
        .debug_bounds("wf-tab-each-7")
        .expect("a tab to the right");
    assert!(far.right() > strip.right(), "the tab starts out of view");
    // Picking a tab far to the right (the canvas hears it as a pick) scrolls the strip itself
    // until the tab shows.
    canvas(&s, visual).update(visual, |_, cx| {
        cx.emit(GraphEvent::PickItem {
            group: "each".into(),
            index: 7,
        })
    });
    visual.run_until_parked();
    visual.refresh().unwrap();
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert_eq!(lane_cards(&s, visual), vec!["dev@each#7", "review@each#7"]);
    let strip = visual.debug_bounds("wf-tabs-bar-tabs:each").unwrap();
    let picked = visual.debug_bounds("wf-tab-each-7").unwrap();
    assert!(
        picked.right() <= strip.right() + px(1.),
        "{picked:?} {strip:?}"
    );
    assert!(
        picked.left() >= strip.left() - px(1.),
        "{picked:?} {strip:?}"
    );
    // The wheel over the strip scrolls it sideways once, and leaves the canvas where it is.
    let view_before = canvas(&s, visual).read_with(visual, |c, _| c.view());
    visual.simulate_event(gpui::ScrollWheelEvent {
        position: strip.center(),
        delta: gpui::ScrollDelta::Pixels(gpui::point(px(-30.), px(0.))),
        ..Default::default()
    });
    visual.run_until_parked();
    visual.refresh().unwrap();
    let moved = visual.debug_bounds("wf-tab-each-7").unwrap();
    let travelled = (moved.left() - picked.left()).as_f32();
    assert!(
        (travelled.abs() - 30.).abs() < 1.,
        "scrolled {travelled}, not once by 30"
    );
    let view_after = canvas(&s, visual).read_with(visual, |c, _| c.view());
    assert_eq!(view_before, view_after);
}

#[gpui::test]
fn lanes_draw_compact_cards_and_single_does_not(cx: &mut TestAppContext) {
    let s = setup(cx);
    saved_run(&s, &each_workflow(), &each_run(4), cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    open_run(&s, visual);
    let compact = |s: &Setup, visual: &mut VisualTestContext| {
        canvas(s, visual).read_with(visual, |c, _| {
            c.scene()
                .nodes
                .iter()
                .filter(|n| n.flow.id.contains('@'))
                .map(|n| n.flow.data.compact)
                .collect::<Vec<_>>()
        })
    };
    assert!(compact(&s, visual).iter().all(|c| *c));
    click(visual, "wf-run-view");
    assert!(compact(&s, visual).iter().all(|c| !*c));
}

#[gpui::test]
fn picking_a_round_of_a_for_each_a_loop_ran_again(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut run = lane_run();
    run.items.insert(
        "each@r1".into(),
        vec![monocode_core::mono::workflows::ordered::Json::from(
            &json!({"title": "Old login"}),
        )],
    );
    run.nodes = serde_json::from_value(json!([
        nr("trigger", "", "done", json!({})),
        nr("each", "", "running", json!({"round": 2})),
        nr("dev", "each#0", "running", json!({})),
        nr("dev", "each#0@r1", "failed", json!({"error": "boom"})),
        nr("report", "", "pending", json!({})),
    ]))
    .unwrap();
    run.items.insert(
        "each".into(),
        vec![monocode_core::mono::workflows::ordered::Json::from(
            &json!({"title": "Fix login"}),
        )],
    );
    let w = run.definition.clone();
    saved_run(&s, &w, &run, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    open_run(&s, visual);
    assert!(visual.debug_bounds("wf-round-each-1").is_some());
    assert!(visual.debug_bounds("wf-round-each-2").is_some());
    assert_eq!(lane_cards(&s, visual), vec!["dev@each#0"]);
    click(visual, "wf-round-each-1");
    assert_eq!(lane_cards(&s, visual), vec!["dev@each#0@r1"]);
    let status = cards(&s, visual)
        .into_iter()
        .find(|(id, _)| id == "dev@each#0@r1")
        .map(|(_, status)| status);
    assert_eq!(status, Some("failed"));
}

#[gpui::test]
fn clicking_a_node_shows_its_details_and_round_history(cx: &mut TestAppContext) {
    let s = setup(cx);
    let run = run_of(
        &linear(),
        "running",
        json!({}),
        json!([
            nr("trigger", "", "done", json!({})),
            nr(
                "dev",
                "",
                "running",
                json!({"round": 2, "startedAt": 1_000_000, "history": [
                {"round": 1, "status": "done", "sessionId": "s-dev-1",
                 "decision": {"value": "loop", "reason": "needs tests"}, "startedAt": 1000, "endedAt": 13000}]})
            ),
            nr("report", "", "pending", json!({})),
        ]),
    );
    saved_run(&s, &linear(), &run, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    open_run(&s, visual);
    // Details start on the running node.
    let heading = |s: &Setup, visual: &mut VisualTestContext| {
        s.manager.read_with(visual, |m, _| {
            m.wf.run
                .details
                .as_ref()
                .map(|d| (d.heading.clone(), d.status_label))
        })
    };
    assert_eq!(heading(&s, visual), Some(("Dev".into(), "Running")));
    click(visual, "wf-node-report");
    assert_eq!(heading(&s, visual), Some(("Report".into(), "Not started")));
    click(visual, "wf-node-dev");
    assert_eq!(heading(&s, visual), Some(("Dev".into(), "Running")));
    assert!(visual.debug_bounds("wf-round-record-1").is_some());
    // The round's session is not one of this app's, and the page says so.
    click(visual, "wf-open-session-round-1");
    assert_eq!(
        page_notice(&s, visual).as_deref(),
        Some("Could not open this session.")
    );
    assert_eq!(
        route(&s, visual),
        Some(Route::WorkflowRun(A.into(), "run-1".into()))
    );
}

#[gpui::test]
fn a_nodes_session_opens_in_the_panel_and_back_returns_to_the_run(cx: &mut TestAppContext) {
    let s = setup(cx);
    // A session the app lists, as a workflow run leaves them.
    let session = s
        .window
        .update(cx, |shell, _, _| shell.workspace.active.unwrap())
        .unwrap();
    let run = run_of(
        &linear(),
        "done",
        json!({}),
        json!([
            nr("trigger", "", "done", json!({})),
            nr("dev", "", "done", json!({"sessionId": session.to_string()})),
            nr("report", "", "done", json!({})),
        ]),
    );
    saved_run(&s, &linear(), &run, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    open_run(&s, visual);
    // A node without a session has nothing to open.
    click(visual, "wf-node-report");
    assert!(visual.debug_bounds("wf-open-session").is_none());
    click(visual, "wf-node-dev");
    click(visual, "wf-open-session");
    assert_eq!(route(&s, visual), Some(Route::Session(session)));
    assert_eq!(page_notice(&s, visual), None);
    settle(&s, visual);
    click(visual, "mono-page-back");
    assert_eq!(
        route(&s, visual),
        Some(Route::WorkflowRun(A.into(), "run-1".into()))
    );
    // Back from a session that was not opened from here goes to the overview.
    s.manager
        .update(visual, |m, _| m.route = Some(Route::Session(session)));
    visual.run_until_parked();
    settle(&s, visual);
    click(visual, "mono-page-back");
    assert_eq!(route(&s, visual), None);
}

#[gpui::test]
fn long_output_is_cut_until_show_more(cx: &mut TestAppContext) {
    let s = setup(cx);
    let long = "x".repeat(500);
    let run = run_of(
        &linear(),
        "running",
        json!({}),
        json!([nr(
            "dev",
            "",
            "running",
            json!({"output": {"summary": long, "pr": "u1"}})
        )]),
    );
    saved_run(&s, &linear(), &run, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    open_run(&s, visual);
    let all = |s: &Setup, visual: &mut VisualTestContext| {
        s.manager
            .read_with(visual, |m, _| m.wf.run.show_all.contains("dev|:summary"))
    };
    // Only the long value has the button.
    assert!(visual.debug_bounds("wf-show-more-pr").is_none());
    assert!(!all(&s, visual));
    click(visual, "wf-show-more-summary");
    assert!(all(&s, visual));
    click(visual, "wf-show-more-summary");
    assert!(!all(&s, visual));
}

#[gpui::test]
fn the_details_collapse_and_open_again(cx: &mut TestAppContext) {
    let s = setup(cx);
    let run = run_of(
        &linear(),
        "running",
        json!({}),
        json!([nr("dev", "", "running", json!({"output": {"a": "b"}}))]),
    );
    saved_run(&s, &linear(), &run, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    open_run(&s, visual);
    assert!(visual.debug_bounds("wf-details-body").is_some());
    click(visual, "wf-details-toggle");
    assert!(visual.debug_bounds("wf-details-body").is_none());
    click(visual, "wf-details-toggle");
    assert!(visual.debug_bounds("wf-details-body").is_some());
}

#[gpui::test]
fn a_command_shows_its_exit_code_and_tails(cx: &mut TestAppContext) {
    let s = setup(cx);
    let w = wf(
        json!([{"id": "trigger", "type": "manual"},
               {"id": "lint", "type": "command", "command": "npm run lint", "project": "{{inputs.project}}"}]),
        json!([{"from": "trigger", "to": "lint"}]),
    );
    let run = run_of(
        &w,
        "failed",
        json!({}),
        json!([
            nr("trigger", "", "done", json!({})),
            nr(
                "lint",
                "",
                "failed",
                json!({"error": "exit 2",
                   "output": {"exitCode": 2, "stdout": "ok so far", "stderr": "boom"}})
            )
        ]),
    );
    saved_run(&s, &w, &run, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    open_run(&s, visual);
    for selector in ["wf-exit-code", "wf-stdout", "wf-stderr"] {
        assert!(visual.debug_bounds(selector).is_some(), "{selector}");
    }
}

#[gpui::test]
fn a_running_run_offers_stop_run_and_stop_ends_it(cx: &mut TestAppContext) {
    let s = setup(cx);
    // A manual workflow that waits for an answer, so its run stays live.
    put(
        &s,
        vec![super::tests::workflow(json!({"runRequested": true})).tap_waiting()],
    );
    s.manager.update(cx, |m, cx| m.check_workflows(cx));
    cx.run_until_parked();
    let run_id = s
        .manager
        .read_with(cx, |m, _| m.workflows.running())
        .into_iter()
        .find(|(workflow, _)| workflow == A)
        .map(|(_, run)| run)
        .expect("a live run");
    open_tab(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    s.manager.update(visual, |m, cx| {
        m.open_workflow_route(Route::WorkflowRun(A.into(), run_id.clone()), cx)
    });
    visual.run_until_parked();
    settle(&s, visual);
    assert!(visual.debug_bounds("wf-run-resume").is_none());
    click(visual, "wf-run-stop");
    visual.run_until_parked();
    assert!(
        !s.manager
            .read_with(visual, |m, _| m.workflows.is_running(A))
    );
    assert_eq!(history(&s, A)[0].status, RunStatus::Stopped);
}

trait Waiting {
    fn tap_waiting(self) -> Workflow;
}
impl Waiting for Workflow {
    fn tap_waiting(self) -> Workflow {
        build(
            json!({"runRequested": true}),
            json!([
                {"id": "trigger", "type": "manual"},
                {"id": "ask", "type": "askMono", "question": "Ship it?", "answers": ["yes", "no"]},
                {"id": "report", "type": "report"},
            ]),
            json!([{"from": "trigger", "to": "ask"}, {"from": "ask", "to": "report"}]),
        )
    }
}

fn interrupted(w: &Workflow) -> WorkflowRun {
    run_of(
        w,
        "interrupted",
        json!({}),
        json!([
            nr("trigger", "", "done", json!({})),
            nr("report", "", "pending", json!({}))
        ]),
    )
}

#[gpui::test]
fn an_interrupted_run_shows_resume_and_resume_continues_it(cx: &mut TestAppContext) {
    let s = setup(cx);
    let w = super::tests::workflow(json!({}));
    saved_run(&s, &w, &interrupted(&w), cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    open_run(&s, visual);
    assert!(visual.debug_bounds("wf-run-stop").is_none());
    click(visual, "wf-run-resume");
    visual.run_until_parked();
    // The runner took the request and carried the run on to its end.
    assert_eq!(history(&s, A)[0].status, RunStatus::Done);
    assert_eq!(page_notice(&s, visual), None);
}

#[gpui::test]
fn resume_is_refused_while_another_run_of_the_workflow_is_going(cx: &mut TestAppContext) {
    let s = setup(cx);
    let w = super::tests::workflow(json!({}));
    saved_run(&s, &w, &interrupted(&w), cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    open_run(&s, visual);
    s.manager
        .read_with(visual, |m, _| m.workflow_signals.set_running(A, true));
    click(visual, "wf-run-resume");
    s.manager
        .read_with(visual, |m, _| m.workflow_signals.set_running(A, false));
    assert!(
        page_notice(&s, visual)
            .unwrap()
            .starts_with("Another run of this workflow is going")
    );
    assert_eq!(saved(&s)[0].resume_run_id, None);
    assert_eq!(history(&s, A)[0].status, RunStatus::Interrupted);
}

#[gpui::test]
fn resume_is_refused_while_the_workflow_still_needs_its_review(cx: &mut TestAppContext) {
    let s = setup(cx);
    let w = super::tests::workflow(json!({"needsReview": true}));
    saved_run(&s, &w, &interrupted(&w), cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    open_run(&s, visual);
    click(visual, "wf-run-resume");
    assert_eq!(
        page_notice(&s, visual).as_deref(),
        Some("Review this workflow before resuming its run.")
    );
    assert_eq!(saved(&s)[0].resume_run_id, None);
}

#[gpui::test]
fn the_page_notes_when_the_workflow_changed_since_the_run_started(cx: &mut TestAppContext) {
    let s = setup(cx);
    let w = each_workflow();
    let run = each_run(2);
    saved_run(&s, &w, &run, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    open_run(&s, visual);
    assert!(visual.debug_bounds("wf-run-changed").is_none());
    // Edited after the run took its copy.
    let mut edited = w.clone();
    edited.edges.clear();
    put(&s, vec![edited]);
    super::tests::refresh(&s, cx);
    visual.run_until_parked();
    assert!(visual.debug_bounds("wf-run-changed").is_some());
}

#[gpui::test]
fn board_positions_do_not_count_as_a_change(cx: &mut TestAppContext) {
    let s = setup(cx);
    let w = each_workflow();
    saved_run(&s, &w, &each_run(2), cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    open_run(&s, visual);
    let mut moved = w.clone();
    for node in &mut moved.nodes {
        node.ui = Some(monocode_core::mono::workflows::model::Ui { x: 5., y: 9. });
    }
    put(&s, vec![moved]);
    super::tests::refresh(&s, cx);
    visual.run_until_parked();
    assert!(visual.debug_bounds("wf-run-changed").is_none());
}

#[gpui::test]
fn a_run_that_is_no_longer_kept_says_so(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(&s, vec![super::tests::workflow(json!({}))]);
    open_tab(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    open_run(&s, visual);
    assert!(visual.debug_bounds("wf-run-gone").is_some());
    assert!(visual.debug_bounds("wf-graph").is_none());
}

#[gpui::test]
fn the_meta_row_and_an_error_show_for_a_failed_run(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut run = run_of(
        &linear(),
        "failed",
        json!({}),
        json!([nr("trigger", "", "done", json!({}))]),
    );
    run.ended_at = Some(run.started_at + 61_000);
    run.error = Some("The run could not make progress.".into());
    saved_run(&s, &linear(), &run, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    open_run(&s, visual);
    for selector in [
        "wf-run-status",
        "wf-run-started",
        "wf-run-took",
        "wf-run-error",
    ] {
        assert!(visual.debug_bounds(selector).is_some(), "{selector}");
    }
    assert!(visual.debug_bounds("wf-run-stop").is_none());
    assert!(visual.debug_bounds("wf-run-resume").is_none());
}

#[gpui::test]
fn zoom_buttons_zoom_within_the_limits_and_fit_returns_to_the_whole(cx: &mut TestAppContext) {
    let s = setup(cx);
    saved_run(&s, &each_workflow(), &each_run(4), cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    open_run(&s, visual);
    let zoom = |s: &Setup, visual: &mut VisualTestContext| {
        canvas(s, visual).read_with(visual, |c, _| c.view().zoom)
    };
    let fitted = zoom(&s, visual);
    assert!((0.3..=1.0).contains(&fitted), "{fitted}");
    click(visual, "wf-zoom-in");
    assert!(zoom(&s, visual) > fitted || fitted >= 1.5);
    for _ in 0..20 {
        click(visual, "wf-zoom-in");
    }
    assert!((zoom(&s, visual) - 1.5).abs() < 0.001);
    for _ in 0..30 {
        click(visual, "wf-zoom-out");
    }
    assert!((zoom(&s, visual) - 0.3).abs() < 0.001);
    click(visual, "wf-fit");
    assert!((zoom(&s, visual) - fitted).abs() < 0.001);
}

#[gpui::test]
fn dragging_the_background_pans_and_a_drag_over_a_card_does_not_pick_it(cx: &mut TestAppContext) {
    let s = setup(cx);
    let run = run_of(
        &linear(),
        "done",
        json!({}),
        json!([
            nr("trigger", "", "done", json!({})),
            nr("dev", "", "done", json!({})),
            nr("report", "", "done", json!({}))
        ]),
    );
    saved_run(&s, &linear(), &run, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    open_run(&s, visual);
    let before = canvas(&s, visual).read_with(visual, |c, _| c.view());
    let report = visual.debug_bounds("wf-node-report").unwrap();
    let from = report.center();
    let to = from + gpui::point(px(40.), px(25.));
    let picked = s.manager.read_with(visual, |m, _| m.wf.run.picked.clone());
    visual.simulate_mouse_down(from, gpui::MouseButton::Left, Default::default());
    visual.simulate_mouse_move(to, gpui::MouseButton::Left, Default::default());
    visual.simulate_mouse_up(to, gpui::MouseButton::Left, Default::default());
    visual.run_until_parked();
    let after = canvas(&s, visual).read_with(visual, |c, _| c.view());
    assert!(
        (after.x - before.x - 40.).abs() < 0.5 && (after.y - before.y - 25.).abs() < 0.5,
        "{before:?} {after:?}"
    );
    assert_eq!(
        s.manager.read_with(visual, |m, _| m.wf.run.picked.clone()),
        picked
    );
}

#[gpui::test]
fn the_lane_titles_tabs_and_rounds_are_strips_of_the_scene(cx: &mut TestAppContext) {
    let s = setup(cx);
    saved_run(&s, &each_workflow(), &each_run(3), cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    open_run(&s, visual);
    let kinds = canvas(&s, visual).read_with(visual, |c, _| {
        c.scene()
            .bars
            .iter()
            .map(|b| matches!(b.kind, BarKind::Lane { .. }))
            .collect::<Vec<_>>()
    });
    assert_eq!(kinds, vec![true, true, true]);
}

#[gpui::test]
fn the_loop_edge_has_its_label_in_the_single_view_and_none_in_lanes(cx: &mut TestAppContext) {
    let s = setup(cx);
    let w = wf(
        json!([
            {"id": "trigger", "type": "manual"},
            {"id": "each", "type": "forEach", "in": "{{trigger.tickets}}"},
            super::run_model_tests::session("dev", json!({"parent": "each"})),
            super::run_model_tests::session("review", json!({"parent": "each"})),
        ]),
        json!([
            {"from": "trigger", "to": "each"},
            {"from": "dev", "to": "review"},
            {"from": "review", "to": "dev", "loop": {"maxRounds": 3, "decide": "mono"}},
        ]),
    );
    let run = run_of(
        &w,
        "running",
        json!({"each": [{"title": "One"}, {"title": "Two"}]}),
        json!([
            nr("trigger", "", "done", json!({})),
            nr("each", "", "running", json!({})),
            nr("dev", "each#0", "running", json!({})),
            nr("review", "each#0", "pending", json!({})),
            nr("dev", "each#1", "pending", json!({})),
            nr("review", "each#1", "pending", json!({})),
        ]),
    );
    saved_run(&s, &w, &run, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    open_run(&s, visual);
    let labels = |s: &Setup, visual: &mut VisualTestContext| {
        canvas(s, visual).read_with(visual, |c, _| {
            c.scene()
                .edges
                .iter()
                .filter_map(|e| e.label.as_ref().map(|l| (e.id.clone(), l.text.clone())))
                .collect::<Vec<_>>()
        })
    };
    // Side by side the lanes are narrow and the loop edge hugs them without a label.
    assert!(labels(&s, visual).is_empty());
    click(visual, "wf-run-view");
    visual.refresh().unwrap();
    let found = labels(&s, visual);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].1, "Mono decides \u{b7} max 3");
    let selector: &'static str =
        Box::leak(format!("wf-loop-label-{}", found[0].0).into_boxed_str());
    assert!(visual.debug_bounds(selector).is_some());
}

#[gpui::test]
fn opening_a_run_again_starts_with_nothing_picked(cx: &mut TestAppContext) {
    let s = setup(cx);
    saved_run(&s, &each_workflow(), &each_run(30), cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    open_run(&s, visual);
    click(visual, "wf-node-report");
    let picked = |s: &Setup, visual: &mut VisualTestContext| {
        s.manager.read_with(visual, |m, _| m.wf.run.picked.clone())
    };
    assert_eq!(picked(&s, visual).as_deref(), Some("report|"));
    s.manager.update(visual, |m, _| m.route = None);
    visual.run_until_parked();
    open_run(&s, visual);
    assert_eq!(picked(&s, visual), None);
}

#[gpui::test]
fn running_labels_follow_the_clock_between_events(cx: &mut TestAppContext) {
    let s = setup(cx);
    let started = monocode_core::time::now_ms() - 5_000;
    let run = run_of(
        &linear(),
        "running",
        json!({}),
        json!([
            nr("trigger", "", "done", json!({})),
            nr("dev", "", "running", json!({"startedAt": started})),
            nr("report", "", "pending", json!({})),
        ]),
    );
    saved_run(&s, &linear(), &run, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    open_run(&s, visual);
    let label = |s: &Setup, visual: &mut VisualTestContext| {
        canvas(s, visual).read_with(visual, |c, _| {
            let dev = c.scene().nodes.iter().find(|n| n.flow.id == "dev").unwrap();
            (
                dev.flow.data.run.as_ref().unwrap().label.clone(),
                c.scene().fit_key.clone(),
                c.view(),
            )
        })
    };
    let (first, key, view) = label(&s, visual);
    assert!(first.starts_with("Running "), "{first}");
    assert!(!first.contains("10m"), "{first}");
    // Nothing in the run changed, ten minutes went by.
    s.manager
        .update(visual, |m, cx| m.run_page_sync_at(started + 605_000, cx));
    visual.run_until_parked();
    let (later, later_key, later_view) = label(&s, visual);
    assert_eq!(later, "Running 10m 5s");
    // The same picture: the view is not fitted again.
    assert_eq!((key, view), (later_key, later_view));
}

#[gpui::test]
fn enter_on_a_focused_card_picks_it(cx: &mut TestAppContext) {
    let s = setup(cx);
    let run = run_of(
        &linear(),
        "running",
        json!({}),
        json!([
            nr("trigger", "", "done", json!({})),
            nr("dev", "", "running", json!({})),
            nr("report", "", "pending", json!({})),
        ]),
    );
    saved_run(&s, &linear(), &run, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    open_run(&s, visual);
    let picked = |s: &Setup, visual: &mut VisualTestContext| {
        s.manager
            .read_with(visual, |m, _| m.wf.run.selection.clone())
    };
    assert_eq!(picked(&s, visual).as_deref(), Some("dev|"));
    // A click focuses the card; pick another node another way, then Enter on the focused card
    // picks it again.
    click(visual, "wf-node-trigger");
    assert_eq!(picked(&s, visual).as_deref(), Some("trigger|"));
    s.manager.update(visual, |m, cx| {
        m.wf.run.picked = Some("report|".into());
        m.run_page_rebuild(cx);
    });
    visual.run_until_parked();
    assert_eq!(picked(&s, visual).as_deref(), Some("report|"));
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    assert_eq!(picked(&s, visual).as_deref(), Some("trigger|"));
    s.manager.update(visual, |m, cx| {
        m.wf.run.picked = Some("report|".into());
        m.run_page_rebuild(cx);
    });
    visual.simulate_keystrokes("space");
    visual.run_until_parked();
    assert_eq!(picked(&s, visual).as_deref(), Some("trigger|"));
}
