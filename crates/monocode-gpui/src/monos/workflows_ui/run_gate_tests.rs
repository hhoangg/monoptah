//! The Run and Resume dialog, what Run and Resume refuse for missing requirements, the badge on
//! the list and the requirements editor of the board. Requirements are checked by a fake that
//! never reaches a real command, shell or MCP server.
use super::actions::{self, Approval, ReviewIntent};
use super::board::{BoardField, BoardPick};
use super::board_tests::{do_with, on_board, open_board, type_in};
use super::run_model_tests::run_of;
use super::tests::{A, build, click, disk, history, open_tab, put, saved, settle};
use super::*;
use crate::monos::tests::{Setup, setup};
use gpui::{TestAppContext, VisualTestContext};
use monocode_core::mono::workflows::{
    model::{
        InputValue, Requirement, RequirementKind, RunStatus, Workflow, definition_fingerprint,
    },
    ordered::OrderedMap,
    preflight::{Preflight, RequirementStatus},
};
use serde_json::{Value, json};

/// Everything is found.
fn all_found(w: &Workflow) -> Preflight {
    judged(w, |_| true)
}

/// Everything but `gh` is found.
fn gh_missing(w: &Workflow) -> Preflight {
    judged(w, |r| r.name != "gh")
}

fn judged(w: &Workflow, found: impl Fn(&Requirement) -> bool) -> Preflight {
    Preflight {
        statuses: w
            .requires
            .iter()
            .map(|r| RequirementStatus {
                requirement: r.clone(),
                met: found(r),
                detail: (!found(r)).then(|| "Not found on PATH".to_string()),
            })
            .collect(),
    }
}

/// A manual workflow with a text input and a requirement on `gh`.
fn needing(extra: Value) -> Workflow {
    let mut all = json!({
        "inputs": {"area": {"type": "string", "description": "Where to look"}},
        "inputValues": {"area": "src"},
        "requires": [{"cli": "gh", "hint": "brew install gh"}],
    });
    for (key, value) in extra.as_object().unwrap() {
        all[key] = value.clone();
    }
    build(
        all,
        json!([{"id": "trigger", "type": "manual"}, {"id": "report", "type": "report"}]),
        json!([{"from": "trigger", "to": "report"}]),
    )
}

fn use_preflight(s: &Setup, visual: &mut VisualTestContext, preflight: fn(&Workflow) -> Preflight) {
    s.manager
        .update(visual, |m, _| m.workflow_live.set_preflight(preflight));
}

/// Looks at the requirements again: what was found is made old enough to be looked at anew.
fn look_again(s: &Setup, visual: &mut VisualTestContext) {
    s.manager.update(visual, |m, cx| {
        for entry in m.wf.preflights.values_mut() {
            entry.1 -= requirements::RECHECK;
        }
        m.refresh_workflows(cx);
    });
    visual.run_until_parked();
}

fn visible(visual: &mut VisualTestContext, selector: &'static str) -> bool {
    visual.debug_bounds(selector).is_some()
}

fn gate_open(s: &Setup, visual: &mut VisualTestContext) -> bool {
    s.manager.read_with(visual, |m, _| m.wf.gate.is_some())
}

fn type_gate(s: &Setup, visual: &mut VisualTestContext, name: &str, text: &str) {
    let key = format!("wf-gate:{name}");
    let text = text.to_string();
    do_with(s, visual, |m, cx| {
        let field = m.fields.get(&key).expect("a gate field").clone();
        field.update(cx, |f, cx| f.set_text(text, cx));
    });
}

fn opened(
    s: &Setup,
    cx: &mut TestAppContext,
    list: Vec<Workflow>,
    preflight: fn(&Workflow) -> Preflight,
) -> VisualTestContext {
    put(s, list);
    open_tab(s, cx);
    super::tests::refresh(s, cx);
    let mut visual = VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    use_preflight(s, &mut visual, preflight);
    visual
}

// ---- when the dialog opens -----------------------------------------------------------------

#[gpui::test]
fn run_opens_the_dialog_for_requirements_or_inputs_and_starts_directly_otherwise(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    let only_requires = needing(json!({"inputs": {}, "inputValues": {}}));
    let only_inputs = needing(json!({"requires": null}));
    for (w, expect_gate) in [(only_requires, true), (only_inputs, true)] {
        let visual = &mut opened(&s, cx, vec![w], all_found);
        click(visual, "mono-workflow-run-wf-aaaaaaaa");
        assert_eq!(gate_open(&s, visual), expect_gate);
        assert!(visible(visual, "wf-run-gate"));
        assert!(
            !saved(&s)[0].run_requested,
            "nothing starts behind the dialog"
        );
        assert!(history(&s, A).is_empty());
        click(visual, "wf-gate-cancel");
        assert!(!gate_open(&s, visual));
    }
    let plain = build(
        json!({}),
        json!([{"id": "trigger", "type": "manual"}, {"id": "report", "type": "report"}]),
        json!([{"from": "trigger", "to": "report"}]),
    );
    let visual = &mut opened(&s, cx, vec![plain], all_found);
    click(visual, "mono-workflow-run-wf-aaaaaaaa");
    assert!(!gate_open(&s, visual));
    assert_eq!(
        history(&s, A).len(),
        1,
        "no inputs and no requires start at once"
    );
}

#[gpui::test]
fn a_flagged_workflow_still_goes_to_the_review_first(cx: &mut TestAppContext) {
    let s = setup(cx);
    let visual = &mut opened(
        &s,
        cx,
        vec![needing(json!({"needsReview": true}))],
        all_found,
    );
    click(visual, "mono-workflow-run-wf-aaaaaaaa");
    assert!(!gate_open(&s, visual));
    assert_eq!(
        s.manager.read_with(visual, |m, _| m.route.clone()),
        Some(Route::WorkflowReview(A.into(), ReviewIntent::Run))
    );
}

#[gpui::test]
fn the_form_is_prefilled_and_a_changed_value_is_not_written_back(cx: &mut TestAppContext) {
    let s = setup(cx);
    let visual = &mut opened(&s, cx, vec![needing(json!({}))], all_found);
    click(visual, "mono-workflow-run-wf-aaaaaaaa");
    visual.run_until_parked();
    assert_eq!(
        s.manager
            .read_with(visual, |m, cx| m.value("wf-gate:area", cx)),
        "src"
    );
    type_gate(&s, visual, "area", "docs");
    click(visual, "wf-gate-submit");
    assert!(!gate_open(&s, visual));
    let kept = saved(&s).remove(0);
    assert_eq!(
        kept.input_values.get("area"),
        Some(&InputValue::String("src".into())),
        "the stored value stays"
    );
    assert!(kept.run_values.is_none(), "the runner consumed the request");
    assert_eq!(history(&s, A).len(), 1, "the run started");
}

#[gpui::test]
fn the_primary_stays_off_until_every_requirement_is_met(cx: &mut TestAppContext) {
    let s = setup(cx);
    let visual = &mut opened(&s, cx, vec![needing(json!({}))], gh_missing);
    click(visual, "mono-workflow-run-wf-aaaaaaaa");
    visual.run_until_parked();
    assert!(visible(visual, "wf-gate-requirement-0"));
    click(visual, "wf-gate-submit");
    assert!(gate_open(&s, visual), "a missing requirement keeps it open");
    assert!(history(&s, A).is_empty());
    let shown = s.manager.read_with(visual, |m, _| {
        m.wf.gate
            .as_ref()
            .and_then(|g| g.preflight.as_ref().map(|p| p.missing().len()))
    });
    assert_eq!(shown, Some(1));
    // Re-checking after the user installed it lets the run go.
    use_preflight(&s, visual, all_found);
    click(visual, "wf-gate-recheck");
    click(visual, "wf-gate-submit");
    assert!(!gate_open(&s, visual));
    assert_eq!(history(&s, A).len(), 1);
}

#[gpui::test]
fn an_answer_to_an_older_check_is_ignored(cx: &mut TestAppContext) {
    let s = setup(cx);
    let visual = &mut opened(&s, cx, vec![needing(json!({}))], all_found);
    click(visual, "mono-workflow-run-wf-aaaaaaaa");
    visual.run_until_parked();
    let stale = s.manager.read_with(visual, |m, _| m.wf.gate_generation);
    click(visual, "wf-gate-recheck");
    let w = needing(json!({}));
    do_with(&s, visual, |m, cx| {
        m.run_gate_checked(stale, gh_missing(&w), cx);
    });
    let missing = s.manager.read_with(visual, |m, _| {
        m.wf.gate
            .as_ref()
            .and_then(|g| g.preflight.as_ref())
            .map(|p| p.missing().len())
    });
    assert_eq!(
        missing,
        Some(0),
        "the older answer did not replace the newer"
    );
}

#[gpui::test]
fn a_number_that_is_not_one_keeps_the_primary_off(cx: &mut TestAppContext) {
    let s = setup(cx);
    let w = needing(json!({
        "inputs": {"count": {"type": "number"}}, "inputValues": {"count": 3}, "requires": null,
    }));
    let visual = &mut opened(&s, cx, vec![w], all_found);
    click(visual, "mono-workflow-run-wf-aaaaaaaa");
    type_gate(&s, visual, "count", "many");
    click(visual, "wf-gate-submit");
    assert!(gate_open(&s, visual));
    assert!(history(&s, A).is_empty());
    type_gate(&s, visual, "count", "4");
    click(visual, "wf-gate-submit");
    assert!(!gate_open(&s, visual));
}

#[gpui::test]
fn the_dialog_belongs_to_the_page_it_was_opened_on(cx: &mut TestAppContext) {
    let s = setup(cx);
    let visual = &mut opened(&s, cx, vec![needing(json!({}))], all_found);
    click(visual, "mono-workflow-run-wf-aaaaaaaa");
    assert!(visible(visual, "wf-run-gate"));
    do_with(&s, visual, |m, cx| {
        m.open_workflow_route(Route::Workflow(A.into()), cx)
    });
    settle(&s, visual);
    assert!(!visible(visual, "wf-run-gate"), "not drawn on another page");
}

// ---- resume -----------------------------------------------------------------------------

fn interrupted(w: &Workflow) -> monocode_core::mono::workflows::model::WorkflowRun {
    let mut run = run_of(w, "interrupted", json!({}), json!([]));
    run.status = RunStatus::Interrupted;
    run
}

#[gpui::test]
fn resume_opens_the_dialog_only_when_something_is_required(cx: &mut TestAppContext) {
    let s = setup(cx);
    let plain = needing(json!({"requires": null, "inputs": {}, "inputValues": {}}));
    let visual = &mut opened(&s, cx, vec![plain.clone()], all_found);
    let mono = s.id.clone();
    let run = interrupted(&plain);
    s.manager.update(visual, |m, _| {
        m.wf.runs.insert((mono.clone(), A.into()), vec![run]);
    });
    let opens = s
        .manager
        .update(visual, |m, cx| m.run_gate_for_resume(A, "run-1", true, cx));
    assert!(!opens, "nothing required: it resumes at once");

    // The run was made with a requirement the workflow no longer asks for.
    let with = needing(json!({"inputs": {}, "inputValues": {}}));
    let run = interrupted(&with);
    s.manager.update(visual, |m, _| {
        m.wf.runs.insert((mono.clone(), A.into()), vec![run]);
    });
    let opens = s
        .manager
        .update(visual, |m, cx| m.run_gate_for_resume(A, "run-1", true, cx));
    visual.run_until_parked();
    assert!(opens);
    s.manager.read_with(visual, |m, _| {
        let gate = m.wf.gate.as_ref().expect("a dialog");
        assert_eq!(gate.subject.requires.len(), 1);
        assert!(gate.inputs.is_empty(), "a resume asks for no values");
    });
}

#[test]
fn resume_is_refused_while_what_the_run_needs_is_missing() {
    let w = needing(json!({"inputs": {}, "inputValues": {}}));
    let mut d = disk(vec![w.clone()]);
    d.host = d.host.with_preflight(gh_missing);
    let run = interrupted(&w);
    monocode_core::mono::workflows::store::update_runs(&d.dir, A, |_| Ok((vec![run.clone()], ())))
        .unwrap();
    let notice = actions::resume_run(&mut d.host, A, "run-1").expect("a notice");
    assert!(notice.starts_with("Can't resume yet"), "{notice}");
    assert!(notice.contains("cli gh"), "{notice}");
    assert!(notice.contains("brew install gh"), "{notice}");
    assert_eq!(d.first().resume_run_id, None);
    d.host = d.host.with_preflight(all_found);
    assert_eq!(actions::resume_run(&mut d.host, A, "run-1"), None);
    assert_eq!(d.first().resume_run_id.as_deref(), Some("run-1"));
}

// ---- the rules of run_now -----------------------------------------------------------------

#[test]
fn run_now_says_what_is_missing_and_saves_nothing() {
    let mut d = disk(vec![needing(json!({}))]);
    d.host = d.host.with_preflight(gh_missing);
    let notice = actions::run_now(&mut d.host, A, None).expect("a notice");
    assert!(notice.starts_with("Can't run yet: cli gh"), "{notice}");
    assert!(!d.first().run_requested);
}

#[test]
fn run_now_keeps_the_values_for_this_run_apart_from_the_stored_ones() {
    let mut d = disk(vec![needing(json!({}))]);
    d.host = d.host.with_preflight(all_found);
    let mut values = OrderedMap::new();
    values.insert("area".to_string(), InputValue::String("docs".into()));
    assert_eq!(actions::run_now(&mut d.host, A, Some(values.clone())), None);
    let w = d.first();
    assert!(w.run_requested);
    assert_eq!(w.run_values, Some(values));
    assert_eq!(
        w.input_values.get("area"),
        Some(&InputValue::String("src".into()))
    );
    // A later request without values must not inherit them.
    assert_eq!(actions::run_now(&mut d.host, A, None), None);
    assert!(d.first().run_values.is_none());
}

#[test]
fn run_now_checks_the_values_like_the_cli_does() {
    let mut d = disk(vec![needing(json!({"requires": null}))]);
    let mut wrong = OrderedMap::new();
    wrong.insert("area".to_string(), InputValue::Number(3.0));
    let notice = actions::run_now(&mut d.host, A, Some(wrong)).expect("a notice");
    assert!(notice.contains("area"), "{notice}");
    let mut unknown = OrderedMap::new();
    unknown.insert("other".to_string(), InputValue::String("x".into()));
    assert!(actions::run_now(&mut d.host, A, Some(unknown)).is_some());
    assert!(!d.first().run_requested);
}

#[test]
fn approving_to_run_needs_the_requirements_but_approving_to_enable_does_not() {
    let w =
        needing(json!({"needsReview": true, "enabled": false, "inputs": {}, "inputValues": {}}));
    let fingerprint = definition_fingerprint(&w);
    let mut d = disk(vec![w]);
    d.host = d.host.with_preflight(gh_missing);
    let result = actions::approve(&mut d.host, A, ReviewIntent::Run, &fingerprint);
    match result {
        Approval::Refused(notice) => assert!(notice.contains("cli gh"), "{notice}"),
        other => panic!("expected a refusal, got {other:?}"),
    }
    assert!(d.first().needs_review, "still waiting for its review");
    assert!(!d.first().run_requested);
    let result = actions::approve(&mut d.host, A, ReviewIntent::Enable, &fingerprint);
    assert_eq!(result, Approval::Done);
}

// ---- the list and the page ----------------------------------------------------------------

#[gpui::test]
fn the_list_flags_a_workflow_whose_requirements_are_missing(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(&s, vec![needing(json!({}))]);
    open_tab(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    use_preflight(&s, visual, gh_missing);
    look_again(&s, visual);
    assert!(visible(visual, "mono-workflow-missing-requirements"));

    // Found again, and looked at again once the old finding has aged.
    use_preflight(&s, visual, all_found);
    look_again(&s, visual);
    assert!(!visible(visual, "mono-workflow-missing-requirements"));
}

#[gpui::test]
fn a_workflow_without_requirements_is_never_looked_at(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(&s, vec![needing(json!({"requires": null}))]);
    open_tab(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    assert!(
        s.manager
            .read_with(visual, |m, _| m.wf.preflights.is_empty())
    );
    assert!(!visible(visual, "mono-workflow-missing-requirements"));
}

#[gpui::test]
fn the_workflow_page_lists_each_missing_requirement_with_its_hint(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(&s, vec![needing(json!({}))]);
    open_tab(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    use_preflight(&s, visual, gh_missing);
    look_again(&s, visual);
    do_with(&s, visual, |m, cx| {
        m.open_workflow_route(Route::Workflow(A.into()), cx)
    });
    settle(&s, visual);
    assert!(visible(visual, "mono-workflow-missing"));
    let hint = s.manager.read_with(visual, |m, _| {
        m.missing_requirements(&m.mono_workflow(A).unwrap().clone())
            .map(|p| p.summary())
    });
    assert!(hint.unwrap().contains("brew install gh"));
}

// ---- the editor of the board ----------------------------------------------------------------

fn settings(s: &Setup, visual: &mut VisualTestContext) {
    do_with(s, visual, |m, cx| {
        m.board_view(Some(super::board::InspectorView::Settings), cx)
    });
    settle(s, visual);
}

fn requires_of(s: &Setup, visual: &mut VisualTestContext) -> Vec<Requirement> {
    on_board(s, visual, |b| b.current().requires)
}

#[gpui::test]
fn requirements_are_added_edited_and_removed_through_the_board(cx: &mut TestAppContext) {
    let s = setup(cx);
    let w = needing(json!({"requires": null}));
    let mut visual = open_board(&s, &w, cx);
    let visual = &mut visual;
    settings(&s, visual);
    assert!(visible(visual, "wf-requires"));
    click(visual, "wf-require-add");
    let added = requires_of(&s, visual);
    assert_eq!(added.len(), 1);
    assert_eq!(added[0].kind, RequirementKind::Cli);
    assert_eq!(added[0].name, "");

    type_in(&s, visual, BoardField::RequireName(0), "gh");
    do_with(&s, visual, |m, cx| {
        m.board_commit(&BoardField::RequireName(0), cx)
    });
    type_in(&s, visual, BoardField::RequireHint(0), "brew install gh");
    do_with(&s, visual, |m, cx| {
        m.board_commit(&BoardField::RequireHint(0), cx)
    });
    do_with(&s, visual, |m, cx| {
        m.board_picked(&BoardPick::RequireKind(0), "env", cx)
    });
    let edited = requires_of(&s, visual);
    assert_eq!(
        edited,
        vec![Requirement {
            kind: RequirementKind::Env,
            name: "gh".into(),
            hint: Some("brew install gh".into()),
        }]
    );

    click(visual, "wf-require-remove-0");
    assert!(requires_of(&s, visual).is_empty());
}

#[gpui::test]
fn a_bad_requirement_name_is_said_inline(cx: &mut TestAppContext) {
    let s = setup(cx);
    let w = needing(json!({"requires": null}));
    let mut visual = open_board(&s, &w, cx);
    let visual = &mut visual;
    settings(&s, visual);
    click(visual, "wf-require-add");
    type_in(&s, visual, BoardField::RequireName(0), "../bin/sh");
    do_with(&s, visual, |m, cx| {
        m.board_commit(&BoardField::RequireName(0), cx)
    });
    settle(&s, visual);
    assert!(visible(visual, "wf-require-problem"));
}

// ---- review and import --------------------------------------------------------------------

#[gpui::test]
fn the_review_and_the_import_show_what_the_workflow_requires(cx: &mut TestAppContext) {
    let s = setup(cx);
    let flagged = needing(json!({"needsReview": true, "enabled": false}));
    put(&s, vec![flagged.clone()]);
    open_tab(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    s.manager.update(visual, |m, cx| {
        m.open_workflow_review(A, ReviewIntent::Enable, None, cx)
    });
    visual.run_until_parked();
    assert!(visible(visual, "mono-workflow-requires"));

    let exported = monocode_core::mono::workflows::yaml::export_workflow(&flagged);
    s.manager.update(visual, |m, cx| m.open_import_page(cx));
    visual.run_until_parked();
    s.manager.update(visual, |m, cx| {
        m.fields["wf-import-text"].update(cx, |f, cx| f.set_text(exported, cx))
    });
    visual.run_until_parked();
    assert!(visible(visual, "mono-workflow-requires"));
    let shown = s.manager.read_with(visual, |m, _| {
        m.wf.import
            .read
            .clone()
            .and_then(Result::ok)
            .map(|p| p.workflow.requires)
    });
    assert_eq!(shown.map(|r| r.len()), Some(1));
}

// ---- fix round: keyboard, drop-downs, scrolling, answers ------------------------------------

#[gpui::test]
fn escape_closes_the_dialog(cx: &mut TestAppContext) {
    let s = setup(cx);
    let visual = &mut opened(&s, cx, vec![needing(json!({}))], all_found);
    click(visual, "mono-workflow-run-wf-aaaaaaaa");
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(gate_open(&s, visual));
    // The shell's Escape reaches the Mono through `dismiss_settings` (the Workspace binding runs
    // before any key handler of the dialog); the shell-level test presses the real key.
    let closed = s
        .manager
        .update_in(visual, |m, window, cx| m.dismiss_settings(window, cx));
    assert!(closed);
    assert!(!gate_open(&s, visual));
}

#[gpui::test]
fn a_drop_down_of_the_dialog_keeps_it_open_and_its_choice_is_an_override(cx: &mut TestAppContext) {
    let s = setup(cx);
    let w = needing(json!({
        "inputs": {"flag": {"type": "boolean"}}, "inputValues": {"flag": false}, "requires": null,
    }));
    let visual = &mut opened(&s, cx, vec![w], all_found);
    click(visual, "mono-workflow-run-wf-aaaaaaaa");
    click(visual, "wf-gate-input-flag");
    assert!(s.manager.read_with(visual, |m, _| m.picker.is_some()));
    assert!(
        gate_open(&s, visual),
        "opening a drop-down keeps the dialog"
    );
    do_with(&s, visual, |m, cx| {
        m.workflow_picked(&Pick::Gate("flag".into()), "on".into(), cx);
        // The menu closes when it answers.
        m.picker = None;
        m.wf.pick = None;
    });
    let overrides = s.manager.read_with(visual, |m, cx| {
        let gate = m.wf.gate.as_ref().expect("still open");
        gate.overrides(&m.run_gate_form(cx))
    });
    let mut expected = OrderedMap::new();
    expected.insert("flag".to_string(), InputValue::Bool(true));
    assert_eq!(overrides, Some(expected));
    click(visual, "wf-gate-submit");
    assert!(!gate_open(&s, visual));
    let kept = saved(&s).remove(0);
    assert_eq!(
        kept.input_values.get("flag"),
        Some(&InputValue::Bool(false))
    );
    assert_eq!(history(&s, A).len(), 1);
}

#[gpui::test]
fn picking_a_project_checks_the_requirements_again(cx: &mut TestAppContext) {
    let s = setup(cx);
    let w = needing(json!({
        "inputs": {"project": {"type": "project"}}, "inputValues": {"project": "/work/api"},
    }));
    let visual = &mut opened(&s, cx, vec![w], all_found);
    click(visual, "mono-workflow-run-wf-aaaaaaaa");
    let before = s.manager.read_with(visual, |m, _| m.wf.gate_generation);
    let project = s.project.clone();
    do_with(&s, visual, |m, cx| {
        m.workflow_picked(&Pick::Gate("project".into()), project, cx)
    });
    assert!(s.manager.read_with(visual, |m, _| m.wf.gate_generation) > before);
}

#[gpui::test]
fn many_requirements_overflow_the_body_and_the_footer_stays_in_the_card(cx: &mut TestAppContext) {
    let s = setup(cx);
    let many: Vec<Value> = (0..120)
        .map(|n| json!({"cli": format!("tool{n}")}))
        .collect();
    let w = needing(json!({"requires": many, "inputs": {}, "inputValues": {}}));
    let visual = &mut opened(&s, cx, vec![w], all_found);
    click(visual, "mono-workflow-run-wf-aaaaaaaa");
    visual.run_until_parked();
    visual.refresh().unwrap();
    let card = visual.debug_bounds("wf-run-gate").unwrap();
    let submit = visual.debug_bounds("wf-gate-submit").unwrap();
    let body = visual.debug_bounds("wf-gate-body").unwrap();
    let last = visual.debug_bounds("wf-gate-requirement-119").unwrap();
    assert!(
        last.bottom() > body.bottom(),
        "the list is longer than the body it sits in ({} vs {})",
        last.bottom(),
        body.bottom()
    );
    assert!(
        submit.bottom() <= card.bottom(),
        "the footer is not clipped"
    );
}

#[gpui::test]
fn a_refusal_stays_in_the_dialog_and_a_start_closes_it(cx: &mut TestAppContext) {
    let s = setup(cx);
    let visual = &mut opened(&s, cx, vec![needing(json!({}))], all_found);
    click(visual, "mono-workflow-run-wf-aaaaaaaa");
    // The machine changed after the dialog looked.
    use_preflight(&s, visual, gh_missing);
    click(visual, "wf-gate-submit");
    assert!(gate_open(&s, visual));
    let (notice, pending) = s.manager.read_with(visual, |m, _| {
        let g = m.wf.gate.as_ref().unwrap();
        (g.notice.clone(), g.submitting)
    });
    assert!(notice.unwrap().contains("cli gh"));
    assert_eq!(pending, None, "the button works again");
    assert!(history(&s, A).is_empty());
    use_preflight(&s, visual, all_found);
    click(visual, "wf-gate-submit");
    assert!(!gate_open(&s, visual));
    assert_eq!(history(&s, A).len(), 1);
}

#[gpui::test]
fn a_pending_start_cannot_be_asked_twice_and_recheck_waits_while_checking(cx: &mut TestAppContext) {
    let s = setup(cx);
    let visual = &mut opened(&s, cx, vec![needing(json!({}))], all_found);
    click(visual, "mono-workflow-run-wf-aaaaaaaa");
    do_with(&s, visual, |m, _| {
        m.wf.gate.as_mut().unwrap().submitting = Some(99);
    });
    click(visual, "wf-gate-submit");
    assert!(
        history(&s, A).is_empty(),
        "a pending start is not asked again"
    );
    do_with(&s, visual, |m, _| {
        let gate = m.wf.gate.as_mut().unwrap();
        gate.submitting = None;
        gate.checking = true;
    });
    let before = s.manager.read_with(visual, |m, _| m.wf.gate_generation);
    click(visual, "wf-gate-recheck");
    assert_eq!(
        s.manager.read_with(visual, |m, _| m.wf.gate_generation),
        before
    );
}

#[gpui::test]
fn the_dialog_goes_when_its_workflow_does(cx: &mut TestAppContext) {
    let s = setup(cx);
    let visual = &mut opened(&s, cx, vec![needing(json!({}))], all_found);
    click(visual, "mono-workflow-run-wf-aaaaaaaa");
    assert!(gate_open(&s, visual));
    put(&s, vec![]);
    s.manager.update(visual, |m, cx| m.refresh_workflows(cx));
    visual.run_until_parked();
    assert!(!gate_open(&s, visual));
}

#[gpui::test]
fn a_changed_project_input_is_looked_at_again(cx: &mut TestAppContext) {
    let s = setup(cx);
    let with = |path: &str| {
        needing(json!({
            "inputs": {"project": {"type": "project"}}, "inputValues": {"project": path},
        }))
    };
    put(&s, vec![with("/work/api")]);
    open_tab(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    use_preflight(&s, visual, gh_missing);
    look_again(&s, visual);
    assert!(visible(visual, "mono-workflow-missing-requirements"));
    use_preflight(&s, visual, all_found);
    // No ageing: the project alone makes the old finding stale.
    put(&s, vec![with("/work/web")]);
    s.manager.update(visual, |m, cx| m.refresh_workflows(cx));
    visual.run_until_parked();
    assert!(!visible(visual, "mono-workflow-missing-requirements"));
}

#[gpui::test]
fn a_hidden_dialog_is_not_shown_for_escape(cx: &mut TestAppContext) {
    let s = setup(cx);
    let visual = &mut opened(&s, cx, vec![needing(json!({}))], all_found);
    click(visual, "mono-workflow-run-wf-aaaaaaaa");
    assert!(s.manager.read_with(visual, |m, _| m.wf_gate_shown()));
    // No Mono in view (a session is): the dialog is kept but must not take Escape.
    s.manager.update(visual, |m, _| m.active = None);
    assert!(!s.manager.read_with(visual, |m, _| m.wf_gate_shown()));
}
