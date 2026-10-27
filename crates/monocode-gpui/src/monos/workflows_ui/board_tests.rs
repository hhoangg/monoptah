//! The board editor: behaviour checks ported from `WorkflowBoard.test.ts` and
//! `NodeInspector.test.ts` (the rules, not the DOM), and the gestures on the canvas. The pure rules
//! are in core's `board_edit_tests.rs` and `graph/hit_tests.rs`.
use super::board::{BoardField, BoardPick, BoardState, InspectorView};
use super::graph::GraphEvent;
use super::tests::{A, build, listing, put, saved};
use super::tests::{click, open_tab, page_notice, route, settle};
use super::*;
use crate::monos::tests::{Setup, setup};
use gpui::{TestAppContext, VisualTestContext};
use monocode_core::mono::workflows::model::{NodeKind, Workflow};
use serde_json::{Value, json};

/// The ClickUp -> Dev -> Review example, laid out (`ticketWorkflow` of the React fixtures).
pub(super) fn ticket(extra: Value) -> Workflow {
    let mut all = json!({
        "name": "ClickUp -> Dev -> Review",
        "inputs": {"project": {"type": "project"}, "clickupList": {"type": "string"}},
        "inputValues": {"project": "/work/api", "clickupList": "Sprint"},
    });
    for (key, value) in extra.as_object().unwrap() {
        if value.is_null() {
            all.as_object_mut().unwrap().remove(key);
        } else {
            all[key] = value.clone();
        }
    }
    build(
        all,
        json!([
            {"id": "trigger", "type": "schedule", "title": "Weekdays", "ui": {"x": 0, "y": 0},
             "schedule": {"scheduleKind": "weekdays", "time": "09:00", "minute": 0, "dayOfWeek": 1}},
            {"id": "fetch", "type": "session", "title": "Fetch tickets", "project": "{{inputs.project}}",
             "model": "claude:sonnet-5", "checkout": "root", "output": {"tickets": {"type": "list"}},
             "prompt": "List open tickets assigned to me in {{inputs.clickupList}}.", "ui": {"x": 0, "y": 90}},
            {"id": "each", "type": "forEach", "title": "Each ticket", "in": "{{fetch.tickets}}",
             "as": "item", "maxParallel": 3, "ui": {"x": -16, "y": 190}},
            {"id": "dev", "type": "session", "title": "Dev", "parent": "each", "project": "{{inputs.project}}",
             "model": "claude:sonnet-5", "checkout": "newWorktree", "ui": {"x": 16, "y": 40},
             "prompt": "Implement {{item.title}}", "output": {"pr_url": {"type": "string"}, "count": {"type": "number"}}},
            {"id": "review", "type": "session", "title": "Review", "parent": "each", "project": "{{inputs.project}}",
             "checkout": {"worktreeOf": "dev"}, "nestUnder": "dev", "ui": {"x": 16, "y": 130},
             "prompt": "Review {{dev.pr_url}}.", "output": {"verdict": {"type": "string"}}},
            {"id": "report", "type": "report", "title": "Report to Mono", "ui": {"x": 0, "y": 480}},
        ]),
        json!([
            {"from": "trigger", "to": "fetch"}, {"from": "fetch", "to": "each"},
            {"from": "dev", "to": "review"}, {"from": "each", "to": "report"},
            {"from": "review", "to": "dev", "loop": {"maxRounds": 3, "decide": "mono"}},
        ]),
    )
}

pub(super) fn open_board(s: &Setup, w: &Workflow, cx: &mut TestAppContext) -> VisualTestContext {
    put(s, vec![w.clone()]);
    open_tab(s, cx);
    let mut visual = VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    s.manager.update(&mut visual, |m, cx| {
        m.open_workflow_route(Route::WorkflowBoard(w.id.clone()), cx)
    });
    visual.run_until_parked();
    settle(s, &mut visual);
    visual
}

pub(super) fn on_board<R>(
    s: &Setup,
    visual: &mut VisualTestContext,
    f: impl FnOnce(&BoardState) -> R,
) -> R {
    s.manager
        .read_with(visual, |m, _| f(m.wf.board.as_ref().expect("a board")))
}

pub(super) fn do_with(
    s: &Setup,
    visual: &mut VisualTestContext,
    f: impl FnOnce(&mut Monos, &mut Context<Monos>),
) {
    s.manager.update(visual, f);
    visual.run_until_parked();
}

/// Types into a field of the board.
pub(super) fn type_in(s: &Setup, visual: &mut VisualTestContext, spec: BoardField, text: &str) {
    let key = spec.key();
    let text = text.to_string();
    do_with(s, visual, |m, cx| {
        let field = m
            .fields
            .get(&key)
            .unwrap_or_else(|| panic!("no field {key}"))
            .clone();
        field.update(cx, |f, cx| f.set_text(text, cx));
    });
}

pub(super) fn rename(s: &Setup, visual: &mut VisualTestContext, name: &str) {
    type_in(s, visual, BoardField::Name, name);
}

pub(super) fn node_ids(s: &Setup, visual: &mut VisualTestContext) -> Vec<String> {
    on_board(s, visual, |b| {
        let mut ids: Vec<String> = b.flow.nodes.iter().map(|n| n.id.clone()).collect();
        ids.sort();
        ids
    })
}

pub(super) fn can_save(s: &Setup, visual: &mut VisualTestContext) -> bool {
    on_board(s, visual, BoardState::can_save)
}

pub(super) fn event(s: &Setup, visual: &mut VisualTestContext, e: GraphEvent) {
    do_with(s, visual, |m, cx| m.board_graph_event(&e, cx));
}

pub(super) fn at() -> gpui::Point<gpui::Pixels> {
    gpui::point(px(120.), px(200.))
}

pub(super) fn pick(s: &Setup, visual: &mut VisualTestContext, pick: BoardPick, value: &str) {
    let value = value.to_string();
    do_with(s, visual, |m, cx| m.board_picked(&pick, &value, cx));
}

pub(super) fn stored_first(s: &Setup) -> Workflow {
    saved(s).remove(0)
}

pub(super) fn node_of(
    s: &Setup,
    visual: &mut VisualTestContext,
    id: &str,
) -> monocode_core::mono::workflows::model::Node {
    on_board(s, visual, |b| b.node(id).expect("a node").clone())
}

// ---- WorkflowBoard.test.ts ----------------------------------------------------------------

#[gpui::test]
fn renders_one_card_per_workflow_node(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    assert_eq!(
        node_ids(&s, visual),
        vec!["dev", "each", "fetch", "report", "review", "trigger"]
    );
    for selector in [
        "wf-node-trigger",
        "wf-node-fetch",
        "wf-node-each",
        "wf-node-dev",
        "wf-node-review",
        "wf-node-report",
    ] {
        assert!(visual.debug_bounds(selector).is_some(), "{selector}");
    }
    assert!(visual.debug_bounds("wf-board-name").is_some());
    assert!(visual.debug_bounds("wf-board-add").is_some());
    assert!(visual.debug_bounds("wf-board-more").is_some());
}

#[gpui::test]
fn save_is_off_for_an_enabled_workflow_with_issues_and_save_paused_still_works(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    let mut w = ticket(json!({}));
    if let NodeKind::Session { prompt, .. } = &mut w.nodes[1].kind {
        *prompt = "List {{inputs.nope}}".into();
    }
    let mut visual = open_board(&s, &w, cx);
    let visual = &mut visual;
    rename(&s, visual, "Edited");
    // Changed, yet blocked: it is enabled and has a problem.
    assert!(on_board(&s, visual, |b| b.changed()));
    assert!(!can_save(&s, visual));
    let issues = on_board(&s, visual, |b| b.issues.clone());
    assert_eq!(issues.len(), 1);
    assert!(issues[0].message.contains("inputs.nope"));
    assert_eq!(issues[0].node_id.as_deref(), Some("fetch"));
    // The card carries the issue.
    let first = {
        let canvas = on_board(&s, visual, |b| b.canvas.clone());
        canvas.read_with(visual, |c, _| {
            c.scene()
                .nodes
                .iter()
                .find(|n| n.flow.id == "fetch")
                .map(|n| n.flow.data.issues.clone())
                .unwrap()
        })
    };
    assert!(first[0].contains("inputs.nope"));
    assert!(visual.debug_bounds("wf-board-issues").is_some());
    click(visual, "wf-board-save");
    assert_eq!(stored_first(&s).name, "ClickUp -> Dev -> Review");
    click(visual, "wf-board-issues-paused");
    visual.run_until_parked();
    let w = stored_first(&s);
    assert!(!w.enabled);
    assert_eq!(w.name, "Edited");
}

#[gpui::test]
fn a_session_model_the_catalog_lacks_is_flagged_and_blocks_saving_enabled(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut w = ticket(json!({}));
    for n in &mut w.nodes {
        if let NodeKind::Session { model, .. } = &mut n.kind {
            *model = Some("claude-sonnet-5-5".into());
        }
    }
    s.manager.update(cx, |m, _| {
        m.workflow_live = crate::monos::workflow_runner::Live::new().with_probe(listing)
    });
    let mut visual = open_board(&s, &w, cx);
    let visual = &mut visual;
    visual.run_until_parked();
    let issues = on_board(&s, visual, |b| b.issues.clone());
    assert!(
        issues.iter().any(|i| i.node_id.as_deref() == Some("fetch")
            && i.message
                .contains("Model \"claude-sonnet-5-5\" is not available")),
        "{issues:?}"
    );
    rename(&s, visual, "Edited");
    assert!(!can_save(&s, visual));
    click(visual, "wf-board-issues-paused");
    assert!(!stored_first(&s).enabled);
}

#[gpui::test]
fn saves_a_valid_workflow_keeping_the_fields_the_app_manages(cx: &mut TestAppContext) {
    let s = setup(cx);
    let w = ticket(json!({
        "runRequested": true, "stopRequested": true, "resumeRunId": "run-9", "lastRunId": "run-8",
        "importedFrom": "a.yaml", "nextRunAt": 123,
    }));
    let mut visual = open_board(&s, &w, cx);
    let visual = &mut visual;
    assert!(!can_save(&s, visual));
    rename(&s, visual, "Renamed");
    assert!(can_save(&s, visual));
    click(visual, "wf-board-save");
    visual.run_until_parked();
    let saved = stored_first(&s);
    assert_eq!(saved.name, "Renamed");
    assert!(saved.run_requested && saved.stop_requested);
    assert_eq!(saved.resume_run_id.as_deref(), Some("run-9"));
    assert_eq!(saved.last_run_id.as_deref(), Some("run-8"));
    assert_eq!(saved.imported_from.as_deref(), Some("a.yaml"));
    assert_eq!(saved.created_at, 1);
    assert_eq!(saved.nodes.len(), w.nodes.len());
    // Saved, so it is clean again.
    assert!(!can_save(&s, visual));
}

#[gpui::test]
fn a_flagged_workflow_is_not_saved_enabled_points_to_the_review_and_keeps_its_mark_when_paused(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({"needsReview": true})), cx);
    let visual = &mut visual;
    rename(&s, visual, "Renamed");
    assert!(!can_save(&s, visual));
    assert!(visual.debug_bounds("wf-board-review").is_some());
    click(visual, "wf-board-review-open");
    assert!(matches!(
        route(&s, visual),
        Some(Route::WorkflowReview(_, super::ReviewIntent::Enable))
    ));
    assert!(stored_first(&s).enabled);
    // Back on the board: Save paused keeps the mark.
    s.manager.update(visual, |m, _| {
        m.route = Some(Route::WorkflowBoard(A.into()))
    });
    visual.run_until_parked();
    settle(&s, visual);
    click(visual, "wf-board-save-paused");
    visual.run_until_parked();
    let w = stored_first(&s);
    assert!(w.needs_review && !w.enabled);
    assert_eq!(w.name, "Renamed");
}

#[gpui::test]
fn saving_a_paused_flagged_workflow_keeps_the_mark(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(
        &s,
        &ticket(json!({"enabled": false, "needsReview": true})),
        cx,
    );
    let visual = &mut visual;
    rename(&s, visual, "Renamed");
    assert!(visual.debug_bounds("wf-board-review").is_none());
    click(visual, "wf-board-save");
    visual.run_until_parked();
    let w = stored_first(&s);
    assert!(w.needs_review && !w.enabled);
    assert_eq!(w.name, "Renamed");
}

#[gpui::test]
fn it_refuses_to_enable_a_workflow_flagged_after_the_board_loaded_it(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    rename(&s, visual, "Renamed");
    put(&s, vec![ticket(json!({"needsReview": true}))]);
    click(visual, "wf-board-save");
    visual.run_until_parked();
    let w = stored_first(&s);
    assert_eq!(w.name, "ClickUp -> Dev -> Review");
    assert!(w.needs_review);
    assert!(
        page_notice(&s, visual)
            .unwrap()
            .contains("Review this workflow")
    );
}

#[gpui::test]
fn run_now_and_resume_in_the_menu_open_the_review_of_a_flagged_workflow(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(
        &s,
        &ticket(json!({"enabled": false, "needsReview": true})),
        cx,
    );
    let visual = &mut visual;
    do_with(&s, visual, |m, cx| m.board_run_now(cx));
    assert!(matches!(
        route(&s, visual),
        Some(Route::WorkflowReview(_, super::ReviewIntent::Run))
    ));
    s.manager.update(visual, |m, _| {
        m.route = Some(Route::WorkflowBoard(A.into()))
    });
    visual.run_until_parked();
    do_with(&s, visual, |m, cx| m.board_toggle_enabled(cx));
    assert!(matches!(
        route(&s, visual),
        Some(Route::WorkflowReview(_, super::ReviewIntent::Enable))
    ));
    // Nothing was switched on, in the draft or in the file.
    assert!(!on_board(&s, visual, |b| b.meta.enabled));
    assert!(!on_board(&s, visual, |b| b.changed()));
    let w = stored_first(&s);
    assert!(!w.enabled && w.needs_review && !w.run_requested);
}

#[gpui::test]
fn run_now_asks_for_a_run_of_a_saved_board_and_is_off_while_unsaved(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    do_with(&s, visual, |m, cx| m.board_run_now(cx));
    visual.run_until_parked();
    // Asked for without a refusal; the runner may already have taken the request.
    assert_eq!(page_notice(&s, visual), None);
    let asked = stored_first(&s).run_requested
        || !super::tests::history(&s, A).is_empty()
        || s.manager.read_with(visual, |m, _| m.workflow_running(A));
    assert!(asked);
    // Once there is an edit, the menu says to save first.
    rename(&s, visual, "Edited");
    click(visual, "wf-board-more");
    assert!(s.manager.read_with(visual, |m, _| m.picker.is_some()));
}

#[gpui::test]
fn leaving_with_unsaved_changes_asks_to_save_or_discard(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    // Nothing changed: Back just goes.
    click(visual, "mono-page-back");
    assert_eq!(route(&s, visual), Some(Route::Workflow(A.into())));
    s.manager.update(visual, |m, cx| {
        m.open_workflow_route(Route::WorkflowBoard(A.into()), cx)
    });
    visual.run_until_parked();
    rename(&s, visual, "Changed");
    click(visual, "mono-page-back");
    assert_eq!(route(&s, visual), Some(Route::WorkflowBoard(A.into())));
    assert!(visual.debug_bounds("wf-board-leave").is_some());
    click(visual, "wf-board-keep-editing");
    assert!(visual.debug_bounds("wf-board-leave").is_none());
    click(visual, "mono-page-back");
    click(visual, "wf-board-discard");
    assert_eq!(route(&s, visual), Some(Route::Workflow(A.into())));
    assert_eq!(stored_first(&s).name, "ClickUp -> Dev -> Review");
    assert!(s.manager.read_with(visual, |m, _| m.wf.board.is_none()));
}

#[gpui::test]
fn save_in_the_leave_dialog_saves_then_leaves(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    rename(&s, visual, "Changed");
    click(visual, "mono-page-back");
    click(visual, "wf-board-leave-save");
    visual.run_until_parked();
    assert_eq!(stored_first(&s).name, "Changed");
    assert_eq!(route(&s, visual), Some(Route::Workflow(A.into())));
}

#[gpui::test]
fn a_blocked_workflow_is_saved_paused_from_the_leave_dialog(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({"needsReview": true})), cx);
    let visual = &mut visual;
    rename(&s, visual, "Changed");
    click(visual, "mono-page-back");
    click(visual, "wf-board-leave-save");
    visual.run_until_parked();
    let w = stored_first(&s);
    assert!(!w.enabled && w.needs_review);
    assert_eq!(w.name, "Changed");
}

#[gpui::test]
fn a_new_workflow_starts_with_a_manual_trigger_and_a_report_and_is_saved_paused(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    put(&s, vec![]);
    open_tab(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    click(visual, "mono-workflow-new");
    settle(&s, visual);
    assert_eq!(route(&s, visual), Some(Route::NewWorkflow));
    assert_eq!(node_ids(&s, visual), vec!["report", "trigger"]);
    assert_eq!(
        on_board(&s, visual, |b| b.meta.name.clone()),
        "New workflow"
    );
    rename(&s, visual, "Mine");
    click(visual, "wf-board-save");
    visual.run_until_parked();
    let w = stored_first(&s);
    assert_eq!((w.name.as_str(), w.enabled), ("Mine", false));
    assert!(w.inputs.is_empty() && w.input_values.is_empty());
    assert!(w.id.starts_with("wf-") && w.id.len() == 11);
    let types: Vec<&str> = w.nodes.iter().map(|n| n.kind.type_name()).collect();
    assert_eq!(types, vec!["manual", "report"]);
}

#[gpui::test]
fn the_twenty_first_workflow_is_refused_when_saved(cx: &mut TestAppContext) {
    let s = setup(cx);
    let list: Vec<Workflow> = (0..20)
        .map(|i| {
            let mut w = ticket(json!({}));
            w.id = format!("wf-{i:08}");
            w
        })
        .collect();
    put(&s, list);
    open_tab(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    s.manager.update(visual, |m, cx| {
        m.open_workflow_route(Route::NewWorkflow, cx)
    });
    visual.run_until_parked();
    settle(&s, visual);
    click(visual, "wf-board-save");
    visual.run_until_parked();
    assert_eq!(saved(&s).len(), 20);
    assert_eq!(
        page_notice(&s, visual).as_deref(),
        Some("At most 20 workflows.")
    );
}

#[gpui::test]
fn clicking_a_card_opens_the_inspector_and_back_to_board_closes_it(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    event(&s, visual, GraphEvent::Click("fetch".into()));
    assert_eq!(
        on_board(&s, visual, |b| b.view.clone()),
        Some(InspectorView::Node("fetch".into()))
    );
    assert!(visual.debug_bounds("wf-inspector").is_some());
    let prompt = s.manager.read_with(visual, |m, cx| {
        m.fields[&BoardField::Prompt("fetch".into()).key()]
            .read(cx)
            .content
            .to_string()
    });
    assert!(prompt.contains("List open tickets"));
    assert!(!can_save(&s, visual));
    type_in(
        &s,
        visual,
        BoardField::Prompt("fetch".into()),
        "Fetch everything",
    );
    assert!(can_save(&s, visual));
    click(visual, "wf-inspector-back");
    assert!(visual.debug_bounds("wf-inspector").is_none());
}

#[gpui::test]
fn the_add_menu_adds_a_node_and_opens_it(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    click(visual, "wf-board-add");
    assert!(s.manager.read_with(visual, |m, _| m.picker.is_some()));
    do_with(&s, visual, |m, cx| {
        m.board_menu_picked("Add node", "command".into(), cx)
    });
    assert_eq!(node_ids(&s, visual).len(), 7);
    assert!(can_save(&s, visual));
    assert!(matches!(
        on_board(&s, visual, |b| b.view.clone()),
        Some(InspectorView::Node(id)) if id.starts_with("command")
    ));
}

#[gpui::test]
fn follows_an_edit_made_elsewhere_while_the_board_is_clean(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    put(&s, vec![ticket(json!({"name": "By the Mono"}))]);
    do_with(&s, visual, |m, cx| m.refresh_workflows(cx));
    assert_eq!(on_board(&s, visual, |b| b.meta.name.clone()), "By the Mono");
    assert!(on_board(&s, visual, |b| b.elsewhere.is_none()));
    assert!(!can_save(&s, visual));
}

#[gpui::test]
fn asks_before_replacing_unsaved_work_with_an_edit_made_elsewhere(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    rename(&s, visual, "Mine");
    put(&s, vec![ticket(json!({"name": "By the Mono"}))]);
    do_with(&s, visual, |m, cx| m.refresh_workflows(cx));
    assert!(visual.debug_bounds("wf-board-changed").is_some());
    assert_eq!(on_board(&s, visual, |b| b.meta.name.clone()), "Mine");
    click(visual, "wf-board-reload");
    assert_eq!(on_board(&s, visual, |b| b.meta.name.clone()), "By the Mono");
    assert!(visual.debug_bounds("wf-board-changed").is_none());
    assert!(!can_save(&s, visual));
}

#[gpui::test]
fn keep_mine_dismisses_the_notice_and_keeps_the_draft(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    rename(&s, visual, "Mine");
    put(&s, vec![ticket(json!({"name": "By the Mono"}))]);
    do_with(&s, visual, |m, cx| m.refresh_workflows(cx));
    click(visual, "wf-board-keep");
    assert!(visual.debug_bounds("wf-board-changed").is_none());
    assert_eq!(on_board(&s, visual, |b| b.meta.name.clone()), "Mine");
    // The stored copy was seen, so saving goes over it without asking.
    click(visual, "wf-board-save");
    visual.run_until_parked();
    assert_eq!(stored_first(&s).name, "Mine");
}

#[gpui::test]
fn a_save_over_a_definition_changed_since_loading_is_refused_and_overwrite_is_the_way_past(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    rename(&s, visual, "Mine");
    // No refresh: the board was not told (another window, the CLI).
    put(&s, vec![ticket(json!({"name": "By the Mono"}))]);
    click(visual, "wf-board-save");
    visual.run_until_parked();
    assert_eq!(stored_first(&s).name, "By the Mono");
    assert!(visual.debug_bounds("wf-board-changed").is_some());
    assert!(visual.debug_bounds("wf-board-overwrite").is_some());
    assert_eq!(on_board(&s, visual, |b| b.meta.name.clone()), "Mine");
    // Saving again still refuses.
    click(visual, "wf-board-save");
    visual.run_until_parked();
    assert_eq!(stored_first(&s).name, "By the Mono");
    click(visual, "wf-board-overwrite");
    visual.run_until_parked();
    assert_eq!(stored_first(&s).name, "Mine");
    assert!(visual.debug_bounds("wf-board-changed").is_none());
    assert!(!can_save(&s, visual));
}

#[gpui::test]
fn reload_after_a_refused_save_takes_the_stored_version(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    rename(&s, visual, "Mine");
    put(&s, vec![ticket(json!({"name": "By the Mono"}))]);
    click(visual, "wf-board-save");
    visual.run_until_parked();
    click(visual, "wf-board-reload");
    assert_eq!(on_board(&s, visual, |b| b.meta.name.clone()), "By the Mono");
    assert_eq!(stored_first(&s).name, "By the Mono");
    assert!(!can_save(&s, visual));
}

#[gpui::test]
fn saves_normally_when_only_fields_the_app_manages_changed(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    rename(&s, visual, "Mine");
    put(
        &s,
        vec![ticket(json!({"lastRunId": "run-3", "runRequested": true}))],
    );
    click(visual, "wf-board-save");
    visual.run_until_parked();
    let w = stored_first(&s);
    assert_eq!(w.name, "Mine");
    assert_eq!(w.last_run_id.as_deref(), Some("run-3"));
    assert!(w.run_requested);
    assert!(visual.debug_bounds("wf-board-changed").is_none());
}

#[gpui::test]
fn the_review_banner_follows_an_approval_or_a_flag_made_elsewhere(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({"needsReview": true})), cx);
    let visual = &mut visual;
    assert!(visual.debug_bounds("wf-board-review").is_some());
    put(&s, vec![ticket(json!({}))]);
    do_with(&s, visual, |m, cx| m.refresh_workflows(cx));
    assert!(visual.debug_bounds("wf-board-review").is_none());
    rename(&s, visual, "Renamed");
    assert!(can_save(&s, visual));
    click(visual, "wf-board-save");
    visual.run_until_parked();
    let w = stored_first(&s);
    assert!(w.enabled && !w.needs_review);
    // And the other way.
    put(&s, vec![ticket(json!({"needsReview": true}))]);
    do_with(&s, visual, |m, cx| m.refresh_workflows(cx));
    assert!(visual.debug_bounds("wf-board-review").is_some());
}

#[gpui::test]
fn saving_an_interval_schedule_anchors_it_and_leaves_the_board_clean(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut w = ticket(json!({}));
    if let NodeKind::Schedule { schedule } = &mut w.nodes[0].kind {
        schedule.schedule_kind = "interval".into();
        schedule.interval_hours = Some(4);
        schedule.interval_from = None;
    }
    let mut visual = open_board(&s, &w, cx);
    let visual = &mut visual;
    rename(&s, visual, "Renamed");
    click(visual, "wf-board-save");
    visual.run_until_parked();
    let Some(NodeKind::Schedule { schedule }) = saved(&s)[0].nodes.first().map(|n| n.kind.clone())
    else {
        panic!("a schedule")
    };
    assert!(schedule.interval_from.is_some());
    assert!(!can_save(&s, visual));
    click(visual, "mono-page-back");
    assert!(visual.debug_bounds("wf-board-leave").is_none());
    assert_eq!(route(&s, visual), Some(Route::Workflow(A.into())));
}

#[gpui::test]
fn shows_the_real_reason_a_save_failed(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    // A step too large for the file: the store says so.
    do_with(&s, visual, |m, cx| {
        let huge = "x".repeat(2 * 1024 * 1024 + 10);
        m.board_node_edit(
            "fetch",
            |n| {
                if let NodeKind::Session { prompt, .. } = &mut n.kind {
                    *prompt = huge;
                }
            },
            cx,
        )
    });
    click(visual, "wf-board-save");
    visual.run_until_parked();
    assert_eq!(
        page_notice(&s, visual).as_deref(),
        Some(
            "The workflows are too large to save (over 2 MiB in total). Remove a workflow or shorten its steps."
        )
    );
    assert_eq!(stored_first(&s).name, "ClickUp -> Dev -> Review");
}
