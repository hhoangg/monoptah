//! `NodeInspector.test.ts` and the fields of the inspector.
use super::board::{BoardField, BoardPick};
use super::board_tests::*;
use super::graph::GraphEvent;
use super::tests::{click, listing};
use crate::monos::tests::{Setup, setup};
use gpui::{Focusable, TestAppContext, VisualTestContext};
use monocode_core::mono::workflows::{
    board_model::NodeType,
    model::{Checkout, InputValue, NodeKind},
};
use serde_json::json;

// ---- NodeInspector.test.ts -------------------------------------------------------------------

#[gpui::test]
fn the_in_report_button_stores_the_opposite_of_the_effective_value(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    event(&s, visual, GraphEvent::Click("dev".into()));
    // A string field is in the report, a number field is not.
    click(visual, "wf-output-report-pr_url");
    let node = node_of(&s, visual, "dev");
    let NodeKind::Session { output, .. } = &node.kind else {
        panic!()
    };
    assert_eq!(output.get("pr_url").unwrap().in_report, Some(false));
    click(visual, "wf-output-report-count");
    let node = node_of(&s, visual, "dev");
    let NodeKind::Session { output, .. } = &node.kind else {
        panic!()
    };
    assert_eq!(output.get("count").unwrap().in_report, Some(true));
}

#[gpui::test]
fn the_reuse_switch_shows_on_sessions_in_a_loop_body_only(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    for id in ["dev", "review"] {
        event(&s, visual, GraphEvent::Click(id.into()));
        assert!(visual.debug_bounds("wf-reuse-session").is_some(), "{id}");
    }
    event(&s, visual, GraphEvent::Click("fetch".into()));
    assert!(visual.debug_bounds("wf-reuse-session").is_none());
    event(&s, visual, GraphEvent::Click("dev".into()));
    click(visual, "wf-reuse-session");
    let NodeKind::Session { reuse_session, .. } = node_of(&s, visual, "dev").kind else {
        panic!()
    };
    assert_eq!(reuse_session, Some(false));
}

#[gpui::test]
fn picking_a_model_writes_its_harness_and_model_and_drops_the_effort(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut w = ticket(json!({}));
    if let NodeKind::Session { effort, .. } = &mut w.nodes[1].kind {
        *effort = Some("high".into());
    }
    let mut visual = open_board(&s, &w, cx);
    let visual = &mut visual;
    event(&s, visual, GraphEvent::Click("fetch".into()));
    // The picker's answer, as the list sends it.
    let claude = monocode_core::provider::Kind::Claude;
    do_with(&s, visual, |m, cx| {
        m.board_model_picked("fetch", claude, Some("claude-opus-4-6".into()), cx)
    });
    let NodeKind::Session {
        harness,
        model,
        effort,
        ..
    } = node_of(&s, visual, "fetch").kind
    else {
        panic!()
    };
    assert_eq!(harness.as_deref(), Some("claude"));
    // React's spelling: only the `claude-` prefix comes off.
    assert_eq!(model.as_deref(), Some("claude:opus-4-6"));
    assert_eq!(effort, None);
    do_with(&s, visual, |m, cx| {
        m.board_model_picked("fetch", claude, Some("claude-opus-5-5".into()), cx)
    });
    let NodeKind::Session { model, .. } = node_of(&s, visual, "fetch").kind else {
        panic!()
    };
    assert_eq!(model.as_deref(), Some("claude:opus-5-5"));
}

#[gpui::test]
fn picking_an_effort_fixes_the_model_it_was_chosen_for_and_use_mono_default_clears_them(
    cx: &mut TestAppContext,
) {
    use monocode_core::{catalog, mono::workflows::models::Probe, provider::Kind};
    fn efforts(_: Kind) -> Probe {
        Probe {
            installed: true,
            models: Some(vec![catalog::Model {
                id: "claude-sonnet-5".into(),
                name: "Sonnet 5".into(),
                alias_target: None,
                efforts: vec!["low".into(), "high".into(), "xhigh".into()],
                default_effort: None,
                fast: false,
                thinking: false,
                default: true,
                tiers: Vec::new(),
                controls: Vec::new(),
            }]),
        }
    }
    let s = setup(cx);
    s.manager.update(cx, |m, _| {
        m.workflow_live = crate::monos::workflow_runner::Live::new().with_probe(efforts)
    });
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    event(&s, visual, GraphEvent::Click("fetch".into()));
    visual.run_until_parked();
    pick(&s, visual, BoardPick::Effort("fetch".into()), "xhigh");
    let NodeKind::Session {
        harness,
        model,
        effort,
        ..
    } = node_of(&s, visual, "fetch").kind
    else {
        panic!()
    };
    assert_eq!(harness.as_deref(), Some("claude"));
    assert_eq!(model.as_deref(), Some("claude:sonnet-5"));
    assert_eq!(effort.as_deref(), Some("xhigh"));
    click(visual, "wf-model-default");
    let NodeKind::Session {
        harness,
        model,
        effort,
        ..
    } = node_of(&s, visual, "fetch").kind
    else {
        panic!()
    };
    assert_eq!((harness, model, effort), (None, None, None));
    assert!(visual.debug_bounds("wf-model-default").is_none());
}

#[gpui::test]
fn the_inspector_says_why_a_stored_model_is_not_available(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut w = ticket(json!({}));
    if let NodeKind::Session { model, .. } = &mut w.nodes[1].kind {
        *model = Some("claude-sonnet-5-5".into());
    }
    s.manager.update(cx, |m, _| {
        m.workflow_live = crate::monos::workflow_runner::Live::new().with_probe(listing)
    });
    let mut visual = open_board(&s, &w, cx);
    let visual = &mut visual;
    event(&s, visual, GraphEvent::Click("fetch".into()));
    visual.run_until_parked();
    assert!(visual.debug_bounds("wf-model-issue").is_some());
    assert!(visual.debug_bounds("wf-inspector-issues").is_some());
    // A node that sets nothing has no hint.
    event(&s, visual, GraphEvent::Click("dev".into()));
    assert!(visual.debug_bounds("wf-model-issue").is_none());
}

#[gpui::test]
fn the_checkout_nest_and_if_fields_write_their_values(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    pick(
        &s,
        visual,
        BoardPick::Checkout("dev".into()),
        "worktree:fetch",
    );
    let NodeKind::Session { checkout, .. } = node_of(&s, visual, "dev").kind else {
        panic!()
    };
    assert_eq!(checkout, Checkout::WorktreeOf("fetch".into()));
    pick(&s, visual, BoardPick::NestUnder("review".into()), "");
    let NodeKind::Session { nest_under, .. } = node_of(&s, visual, "review").kind else {
        panic!()
    };
    assert_eq!(nest_under, None);
    do_with(&s, visual, |m, cx| m.board_add(NodeType::If, cx));
    let check = node_ids(&s, visual)
        .into_iter()
        .find(|id| id.starts_with("check"))
        .unwrap();
    pick(&s, visual, BoardPick::IfDecider(check.clone()), "condition");
    let NodeKind::If {
        condition, decide, ..
    } = node_of(&s, visual, &check).kind
    else {
        panic!()
    };
    assert_eq!((condition.as_deref(), decide), (Some(""), None));
    type_in(
        &s,
        visual,
        BoardField::Condition(check.clone()),
        "{{a.b}} == 1",
    );
    let NodeKind::If { condition, .. } = node_of(&s, visual, &check).kind else {
        panic!()
    };
    assert_eq!(condition.as_deref(), Some("{{a.b}} == 1"));
}

#[gpui::test]
fn a_time_limit_and_the_for_each_numbers_follow_what_is_typed(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    event(&s, visual, GraphEvent::Click("fetch".into()));
    type_in(&s, visual, BoardField::Limit("fetch".into()), "45");
    let NodeKind::Session {
        timeout_minutes, ..
    } = node_of(&s, visual, "fetch").kind
    else {
        panic!()
    };
    assert_eq!(timeout_minutes, Some(45.0));
    type_in(&s, visual, BoardField::Limit("fetch".into()), "");
    let NodeKind::Session {
        timeout_minutes, ..
    } = node_of(&s, visual, "fetch").kind
    else {
        panic!()
    };
    assert_eq!(timeout_minutes, None);
    event(&s, visual, GraphEvent::Click("each".into()));
    type_in(&s, visual, BoardField::AtOnce("each".into()), "5");
    let NodeKind::ForEach { max_parallel, .. } = node_of(&s, visual, "each").kind else {
        panic!()
    };
    assert_eq!(max_parallel, Some(5.0));
}

#[gpui::test]
fn a_new_output_field_can_be_renamed_and_removed(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    event(&s, visual, GraphEvent::Click("fetch".into()));
    click(visual, "wf-output-add");
    let NodeKind::Session { output, .. } = node_of(&s, visual, "fetch").kind else {
        panic!()
    };
    assert_eq!(
        output.keys().cloned().collect::<Vec<_>>(),
        vec!["tickets", "field"]
    );
    // A name that is taken or unusable goes back; a usable one renames in place.
    type_in(
        &s,
        visual,
        BoardField::OutputName("fetch".into(), "field".into()),
        "tickets",
    );
    do_with(&s, visual, |m, cx| {
        m.board_graph_event(&GraphEvent::PaneClick, cx);
    });
    event(&s, visual, GraphEvent::Click("fetch".into()));
    click(visual, "wf-output-remove-field");
    let NodeKind::Session { output, .. } = node_of(&s, visual, "fetch").kind else {
        panic!()
    };
    assert_eq!(output.keys().cloned().collect::<Vec<_>>(), vec!["tickets"]);
}

#[gpui::test]
fn a_project_chosen_for_a_step_is_held_by_an_input(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    pick(&s, visual, BoardPick::Project("review".into()), "/work/web");
    let NodeKind::Session { project, .. } = node_of(&s, visual, "review").kind else {
        panic!()
    };
    assert_eq!(project, "{{inputs.web}}");
    let (inputs, values) = on_board(&s, visual, |b| {
        (b.meta.inputs.clone(), b.meta.input_values.clone())
    });
    assert!(inputs.contains_key("web") && inputs.contains_key("project"));
    assert_eq!(
        values.get("web"),
        Some(&InputValue::String("/work/web".into()))
    );
    assert!(on_board(&s, visual, |b| b.changed()));
}

#[gpui::test]
fn the_settings_edit_the_description_and_the_inputs(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    do_with(&s, visual, |m, cx| {
        m.board_menu_picked("Workflow menu", "settings".into(), cx)
    });
    assert!(visual.debug_bounds("wf-inspector").is_some());
    type_in(&s, visual, BoardField::Description, "What it is for.");
    assert_eq!(
        on_board(&s, visual, |b| b.meta.description.clone()).as_deref(),
        Some("What it is for.")
    );
    // Declare a number input, give it a value that does not fit, then one that does.
    pick(&s, visual, BoardPick::NewInputType, "number");
    type_in(&s, visual, BoardField::NewInput, "retries");
    do_with(&s, visual, |m, cx| {
        m.board_add_input("retries", "number", cx)
    });
    assert!(on_board(&s, visual, |b| b
        .meta
        .inputs
        .contains_key("retries")));
    // Missing value: a problem the Save gate shows.
    assert!(on_board(&s, visual, |b| b
        .issues
        .iter()
        .any(|i| i.message.contains("retries"))));
    type_in(&s, visual, BoardField::InputValue("retries".into()), "3");
    do_with(&s, visual, |m, cx| {
        m.board_graph_event(&GraphEvent::PaneClick, cx);
    });
    s.manager.update(visual, |m, cx| {
        let input =
            m.wf.board
                .as_ref()
                .unwrap()
                .meta
                .inputs
                .get("retries")
                .cloned()
                .unwrap();
        m.board_set_input("retries", &input, Some(InputValue::Number(3.0)), cx);
    });
    visual.run_until_parked();
    assert!(!on_board(&s, visual, |b| b
        .issues
        .iter()
        .any(|i| i.message.contains("retries"))));
    // Removing it takes the value too.
    do_with(&s, visual, |m, cx| m.board_remove_input("retries", cx));
    assert!(!on_board(&s, visual, |b| b
        .meta
        .input_values
        .contains_key("retries")));
}

#[gpui::test]
fn the_enabled_switch_and_pause_change_the_draft_only_until_saved(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    do_with(&s, visual, |m, cx| m.board_toggle_enabled(cx));
    assert!(!on_board(&s, visual, |b| b.meta.enabled));
    assert!(stored_first(&s).enabled);
    click(visual, "wf-board-save");
    visual.run_until_parked();
    assert!(!stored_first(&s).enabled);
}

#[gpui::test]
fn a_schedule_trigger_edits_its_kind_hours_and_time(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    event(&s, visual, GraphEvent::Click("trigger".into()));
    pick(
        &s,
        visual,
        BoardPick::ScheduleKind("trigger".into()),
        "interval",
    );
    let NodeKind::Schedule { schedule } = node_of(&s, visual, "trigger").kind else {
        panic!()
    };
    assert_eq!(
        (schedule.schedule_kind.as_str(), schedule.interval_hours),
        ("interval", Some(4))
    );
    pick(&s, visual, BoardPick::ScheduleHours("trigger".into()), "6");
    type_in(&s, visual, BoardField::Time("trigger".into()), "08:30");
    let NodeKind::Schedule { schedule } = node_of(&s, visual, "trigger").kind else {
        panic!()
    };
    assert_eq!(
        (schedule.interval_hours, schedule.time.as_str()),
        (Some(6), "08:30")
    );
    pick(
        &s,
        visual,
        BoardPick::ScheduleKind("trigger".into()),
        "daily",
    );
    let NodeKind::Schedule { schedule } = node_of(&s, visual, "trigger").kind else {
        panic!()
    };
    assert_eq!(schedule.interval_hours, None);
}

fn focus_field(s: &Setup, visual: &mut VisualTestContext, spec: BoardField) {
    let handle = s
        .manager
        .read_with(visual, |m, cx| m.fields[&spec.key()].focus_handle(cx));
    visual.refresh().unwrap();
    // Focus listeners (blur) only hear a window that is the active one.
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    visual.update(|window, cx| window.focus(&handle, cx));
    visual.run_until_parked();
    // Focus events are delivered with the next frame.
    visual.refresh().unwrap();
    assert!(
        visual.update(|window, _| handle.is_focused(window)),
        "{spec:?}"
    );
}

fn open_settings(s: &Setup, visual: &mut VisualTestContext) {
    do_with(s, visual, |m, cx| {
        m.board_menu_picked("Workflow menu", "settings".into(), cx)
    });
    visual.refresh().unwrap();
}

#[gpui::test]
fn a_new_input_is_declared_by_enter_or_plus_and_never_by_leaving_the_field(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    open_settings(&s, visual);
    let declared = |s: &Setup, visual: &mut VisualTestContext, name: &str| {
        on_board(s, visual, |b| b.meta.inputs.get(name).map(|i| i.ty))
    };
    type_in(&s, visual, BoardField::NewInput, "retries");
    focus_field(&s, visual, BoardField::NewInput);
    // Opening the type picker takes the focus off the field: that declares nothing.
    click(visual, "wf-pick:NewInputType");
    assert_eq!(declared(&s, visual, "retries"), None);
    pick(&s, visual, BoardPick::NewInputType, "number");
    s.manager.update(visual, |m, cx| {
        m.picker = None;
        cx.notify();
    });
    visual.refresh().unwrap();
    assert_eq!(declared(&s, visual, "retries"), None);
    assert!(!on_board(&s, visual, |b| b.changed()));
    // Leaving the page does not declare it either.
    click(visual, "wf-inspector-back");
    assert_eq!(declared(&s, visual, "retries"), None);
    open_settings(&s, visual);
    // Enter does, with the type chosen.
    focus_field(&s, visual, BoardField::NewInput);
    visual.simulate_keystrokes("enter");
    assert_eq!(
        declared(&s, visual, "retries"),
        Some(monocode_core::mono::workflows::model::InputType::Number)
    );
    // And so does +.
    visual.refresh().unwrap();
    type_in(&s, visual, BoardField::NewInput, "other");
    click(visual, "wf-input-add");
    assert!(declared(&s, visual, "other").is_some());
}

#[gpui::test]
fn leaving_an_input_value_as_it_was_shown_changes_nothing(cx: &mut TestAppContext) {
    let s = setup(cx);
    let w = ticket(json!({
        "inputs": {"project": {"type": "project"}, "clickupList": {"type": "string"}, "note": {"type": "string"}},
    }));
    let mut visual = open_board(&s, &w, cx);
    let visual = &mut visual;
    open_settings(&s, visual);
    assert!(!on_board(&s, visual, |b| b.changed()));
    // Focus the empty field and leave it: still clean, still no value.
    focus_field(&s, visual, BoardField::InputValue("note".into()));
    click(visual, "wf-board-name");
    assert!(!on_board(&s, visual, |b| b.changed()));
    assert!(!on_board(&s, visual, |b| b
        .meta
        .input_values
        .contains_key("note")));
    // A value that was typed is kept when the field is left.
    type_in(&s, visual, BoardField::InputValue("note".into()), "hello");
    focus_field(&s, visual, BoardField::InputValue("note".into()));
    click(visual, "wf-board-name");
    assert_eq!(
        on_board(&s, visual, |b| b.meta.input_values.get("note").cloned()),
        Some(InputValue::String("hello".into()))
    );
    assert!(on_board(&s, visual, |b| b.changed()));
}
