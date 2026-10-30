//! Behaviour checks ported from `MonoWorkflows.test.ts`, `WorkflowPage.test.ts`,
//! `WorkflowReviewPage.test.ts` and `ImportWorkflowPage.test.ts`: the rules, not the DOM. The
//! action rules run over a Mono's files in a temp dir; the pages run in a real `Monos`.
use super::actions::{self, Approval, ReviewIntent};
use super::import::{Entered, append_prefill, values_of};
use super::list::{active_branches, workflow_subtitle};
use super::*;
use crate::monos::tests::{Setup, chat_rows, setup};
use gpui::{TestAppContext, VisualTestContext};
use monocode_core::catalog;
use monocode_core::mono::workflows::{
    cli::Signals,
    model::{
        InputType, InputValue, RunStatus, WORKFLOWS_MAX, WorkflowInput, definition_fingerprint,
    },
    models::Probe,
    ordered::{Json, OrderedMap},
    parse::parse_workflow,
    store::{load_runs, load_workflows, update_runs, update_workflows},
    yaml::{export_workflow, visible},
};
use monocode_core::provider::Kind;
use serde_json::{Value, json};
use std::sync::Arc;

pub(super) const A: &str = "wf-aaaaaaaa";
const SCHEDULE: &str =
    r#"{"scheduleKind": "weekdays", "time": "09:00", "minute": 0, "dayOfWeek": 1}"#;

pub(super) fn build(extra: Value, nodes: Value, edges: Value) -> Workflow {
    let mut def = json!({
        "id": A, "name": "Nightly checks", "inputs": {}, "inputValues": {},
        "nodes": nodes, "edges": edges, "enabled": true, "createdAt": 1,
    });
    for (key, value) in extra.as_object().unwrap() {
        if value.is_null() {
            def.as_object_mut().unwrap().remove(key);
        } else {
            def[key] = value.clone();
        }
    }
    parse_workflow(&Json::from(&def)).expect("a workflow")
}

/// manual trigger -> report.
pub(super) fn workflow(extra: Value) -> Workflow {
    build(
        extra,
        json!([{"id": "trigger", "type": "manual"}, {"id": "report", "type": "report"}]),
        json!([{"from": "trigger", "to": "report"}]),
    )
}

/// weekdays trigger -> report, next run in an hour.
fn scheduled(extra: Value) -> Workflow {
    let mut all = json!({"nextRunAt": monocode_core::time::now_ms() + 3_600_000});
    for (key, value) in extra.as_object().unwrap() {
        all[key] = value.clone();
    }
    build(
        all,
        json!([
            {"id": "trigger", "type": "schedule", "schedule": serde_json::from_str::<Value>(SCHEDULE).unwrap()},
            {"id": "report", "type": "report"},
        ]),
        json!([{"from": "trigger", "to": "report"}]),
    )
}

/// An empty graph: it has problems.
pub(super) fn broken(extra: Value) -> Workflow {
    build(extra, json!([]), json!([]))
}

pub(super) const MODEL: &str = "claude-sonnet-5-5";

/// A manual workflow whose one session names a model the catalog does not have.
fn typed_model(extra: Value) -> Workflow {
    build(
        {
            let mut all = json!({
                "inputs": {"project": {"type": "project"}},
                "inputValues": {"project": "/work/api"},
            });
            for (key, value) in extra.as_object().unwrap() {
                all[key] = value.clone();
            }
            all
        },
        json!([
            {"id": "trigger", "type": "manual"},
            {"id": "hi", "type": "session", "project": "{{inputs.project}}", "prompt": "hi",
             "checkout": "root", "output": {}, "model": MODEL},
            {"id": "report", "type": "report"},
        ]),
        json!([{"from": "trigger", "to": "hi"}, {"from": "hi", "to": "report"}]),
    )
}

pub(super) fn listing(_: Kind) -> Probe {
    Probe {
        installed: true,
        models: Some(vec![catalog::Model {
            id: "claude-sonnet-5".into(),
            name: "Sonnet 5".into(),
            alias_target: None,
            efforts: Vec::new(),
            default_effort: None,
            fast: false,
            thinking: false,
            default: true,
            tiers: Vec::new(),
            controls: Vec::new(),
        }]),
    }
}

/// A Mono's folder in a temp dir with the host the actions run over.
struct Disk {
    dir: MonoDir,
    host: StoreHost,
    signals: Arc<Signals>,
}

fn disk(list: Vec<Workflow>) -> Disk {
    let root = std::env::temp_dir().join(format!("wf-ui-{}", monocode_core::mono::new_id()));
    std::fs::create_dir_all(&root).unwrap();
    let dir = MonoDir::new(&root, "mono-1");
    let signals = Arc::new(Signals::default());
    let host = StoreHost::new(dir.clone(), signals.clone()).with_probe(listing);
    update_workflows(&dir, |_| Ok((list.clone(), ()))).unwrap();
    Disk { dir, host, signals }
}

impl Disk {
    fn saved(&self) -> Vec<Workflow> {
        load_workflows(&self.dir).unwrap().workflows
    }
    fn first(&self) -> Workflow {
        self.saved().remove(0)
    }
}

// ---- MonoWorkflows.test.ts: the actions ---------------------------------------------------

#[test]
fn run_now_asks_for_a_run_and_wakes_the_runner() {
    let mut d = disk(vec![workflow(json!({}))]);
    assert_eq!(actions::run_now(&mut d.host, A), None);
    assert!(d.first().run_requested);
    assert!(d.signals.take_check_now());
}

#[test]
fn run_now_refuses_a_workflow_with_issues_and_says_why() {
    let mut d = disk(vec![broken(json!({}))]);
    let notice = actions::run_now(&mut d.host, A).expect("a notice");
    assert!(notice.starts_with("Can't run yet: "), "{notice}");
    assert!(notice.ends_with('.'), "{notice}");
    assert!(!d.first().run_requested);
    assert!(!d.signals.take_check_now());
}

#[test]
fn a_refusal_names_the_first_problem_and_counts_the_rest() {
    let issue = |m: &str| monocode_core::mono::workflows::validate::WorkflowIssue {
        node_id: None,
        message: m.into(),
    };
    assert_eq!(
        actions::refusal("run", &[issue("A is wrong.  "), issue("B"), issue("C")]),
        "Can't run yet: A is wrong (and 2 more)."
    );
    assert_eq!(
        actions::refusal("resume", &[issue("Only one...")]),
        "Can't resume yet: Only one."
    );
}

#[test]
fn run_now_refuses_a_session_model_not_in_the_catalog_and_names_it() {
    let mut d = disk(vec![typed_model(json!({}))]);
    let notice = actions::run_now(&mut d.host, A).expect("a notice");
    assert!(
        notice.contains(&format!("Model \"{MODEL}\" is not available")),
        "{notice}"
    );
    assert!(!d.first().run_requested);
}

#[test]
fn resume_and_approve_refuse_a_session_model_not_in_the_catalog() {
    let mut d = disk(vec![typed_model(json!({"enabled": false}))]);
    let notice = actions::toggle(&mut d.host, A).expect("a notice");
    assert!(notice.contains(&format!("Model \"{MODEL}\"")), "{notice}");
    assert!(!d.first().enabled);
    let reviewed = typed_model(json!({"enabled": false, "needsReview": true}));
    update_workflows(&d.dir, |_| Ok((vec![reviewed.clone()], ()))).unwrap();
    let result = actions::approve(
        &mut d.host,
        A,
        ReviewIntent::Enable,
        &definition_fingerprint(&reviewed),
    );
    assert!(matches!(result, Approval::Refused(_)), "{result:?}");
    let now = d.first();
    assert!(!now.enabled && now.needs_review);
}

#[test]
fn resume_schedules_the_next_run_and_refuses_when_the_workflow_has_issues() {
    let mut d = disk(vec![scheduled(
        json!({"enabled": false, "nextRunAt": null}),
    )]);
    assert_eq!(actions::toggle(&mut d.host, A), None);
    let now = monocode_core::time::now_ms();
    let resumed = d.first();
    assert!(resumed.enabled);
    assert!(resumed.next_run_at.unwrap() > now);

    let mut d = disk(vec![broken(json!({"enabled": false}))]);
    assert!(actions::toggle(&mut d.host, A).is_some());
    assert!(!d.first().enabled);
}

#[test]
fn pause_leaves_a_pending_review_alone() {
    let mut d = disk(vec![workflow(json!({"needsReview": true}))]);
    assert_eq!(actions::toggle(&mut d.host, A), None);
    let w = d.first();
    assert!(!w.enabled && w.needs_review);
}

#[test]
fn remove_takes_the_workflow_out_and_empties_its_history() {
    let mut d = disk(vec![workflow(json!({}))]);
    let run = monocode_core::mono::workflows::engine::start_run(
        &workflow(json!({})),
        monocode_core::mono::workflows::model::RunTrigger::Manual,
        1,
        "run-1",
    );
    update_runs(&d.dir, A, |_| Ok((vec![run.clone()], ()))).unwrap();
    assert_eq!(load_runs(&d.dir, A).unwrap().len(), 1);
    assert_eq!(actions::remove(&mut d.host, A), None);
    assert!(d.saved().is_empty());
    assert!(load_runs(&d.dir, A).unwrap().is_empty());
}

#[test]
fn remove_is_refused_while_a_run_is_going() {
    let mut d = disk(vec![workflow(json!({}))]);
    d.signals.set_running(A, true);
    assert_eq!(
        actions::remove(&mut d.host, A).as_deref(),
        Some("Stop the workflow before removing it.")
    );
    assert_eq!(d.saved().len(), 1);
}

#[test]
fn the_actions_refuse_a_flagged_workflow_whoever_calls_them() {
    let mut d = disk(vec![workflow(
        json!({"enabled": false, "needsReview": true}),
    )]);
    let review = Some("Review this workflow before running or enabling it.".to_string());
    assert_eq!(actions::run_now(&mut d.host, A), review);
    assert_eq!(actions::toggle(&mut d.host, A), review);
    let w = d.first();
    assert!(!w.enabled && w.needs_review && !w.run_requested);
}

#[test]
fn approve_clears_the_mark_and_enables_counting_the_next_run_from_now() {
    let w = scheduled(json!({"enabled": false, "nextRunAt": null, "needsReview": true}));
    let mut d = disk(vec![w.clone()]);
    let result = actions::approve(
        &mut d.host,
        A,
        ReviewIntent::Enable,
        &definition_fingerprint(&w),
    );
    assert_eq!(result, Approval::Done);
    let now = d.first();
    assert!(now.enabled && !now.needs_review);
    assert!(now.next_run_at.unwrap() > monocode_core::time::now_ms());
}

#[test]
fn approve_clears_the_mark_and_asks_for_a_run() {
    let w = workflow(json!({"enabled": false, "needsReview": true}));
    let mut d = disk(vec![w.clone()]);
    let result = actions::approve(
        &mut d.host,
        A,
        ReviewIntent::Run,
        &definition_fingerprint(&w),
    );
    assert_eq!(result, Approval::Done);
    let now = d.first();
    assert!(now.run_requested && !now.enabled && !now.needs_review);
    assert!(d.signals.take_check_now());
}

#[test]
fn approve_changes_nothing_when_the_stored_definition_is_not_the_one_reviewed() {
    let command = |text: &str| {
        build(
            json!({"enabled": false, "needsReview": true, "inputs": {"project": {"type": "project"}}, "inputValues": {"project": "/work/api"}}),
            json!([
                {"id": "trigger", "type": "manual"},
                {"id": "build", "type": "command", "project": "{{inputs.project}}", "command": text},
                {"id": "report", "type": "report"},
            ]),
            json!([{"from": "trigger", "to": "build"}, {"from": "build", "to": "report"}]),
        )
    };
    let reviewed = command("npm test");
    let swapped = command("rm -rf ~");
    let mut d = disk(vec![swapped.clone()]);
    let result = actions::approve(
        &mut d.host,
        A,
        ReviewIntent::Enable,
        &definition_fingerprint(&reviewed),
    );
    match result {
        Approval::Changed(stored) => assert!(stored.needs_review),
        other => panic!("{other:?}"),
    }
    assert_eq!(d.first(), swapped);
}

#[test]
fn approve_refuses_a_workflow_with_problems_and_keeps_the_mark() {
    let w = broken(json!({"enabled": false, "needsReview": true}));
    let mut d = disk(vec![w.clone()]);
    let result = actions::approve(
        &mut d.host,
        A,
        ReviewIntent::Enable,
        &definition_fingerprint(&w),
    );
    match result {
        Approval::Refused(notice) => assert!(notice.starts_with("Can't resume"), "{notice}"),
        other => panic!("{other:?}"),
    }
    let now = d.first();
    assert!(!now.enabled && now.needs_review);
}

#[test]
fn approve_does_nothing_for_a_workflow_that_is_gone() {
    let mut d = disk(Vec::new());
    let result = actions::approve(&mut d.host, A, ReviewIntent::Run, "x");
    assert!(matches!(result, Approval::Refused(_)));
    assert!(d.saved().is_empty());
}

// ---- WorkflowPage.test.ts: inputs and export -----------------------------------------------

fn limit_input() -> (WorkflowInput, Workflow) {
    let w = workflow(json!({
        "inputs": {"limit": {"type": "number"}, "repo": {"type": "project"}},
        "inputValues": {"limit": 3, "repo": "/work/api"},
    }));
    (w.inputs.get("limit").unwrap().clone(), w)
}

#[test]
fn an_input_value_is_checked_against_its_type() {
    let (input, _) = limit_input();
    assert_eq!(
        actions::input_problem("limit", &input, None).as_deref(),
        Some("Input \"limit\" needs a value")
    );
    let bad = InputValue::Number(actions::js_number("not a number"));
    assert!(
        actions::input_problem("limit", &input, Some(&bad))
            .unwrap()
            .contains("\"limit\" must be a number")
    );
    assert_eq!(
        actions::input_problem("limit", &input, Some(&InputValue::Number(7.))),
        None
    );
}

#[test]
fn a_saved_input_value_lands_in_the_file_and_the_real_reason_comes_back_on_failure() {
    let (_, w) = limit_input();
    let mut d = disk(vec![w]);
    actions::save_input(&mut d.host, A, "limit", &InputValue::Number(7.)).unwrap();
    assert_eq!(
        d.first().input_values.get("limit"),
        Some(&InputValue::Number(7.))
    );
    let huge = InputValue::String("x".repeat(3 * 1024 * 1024));
    let error = actions::save_input(&mut d.host, A, "repo", &huge).unwrap_err();
    assert!(error.contains("too large to save"), "{error}");
    assert_eq!(
        d.first().input_values.get("limit"),
        Some(&InputValue::Number(7.))
    );
}

#[test]
fn numbers_are_read_the_way_javascript_reads_them() {
    for (typed, want) in [
        ("7", 7.),
        (" 3.5 ", 3.5),
        ("-2", -2.),
        ("1e3", 1000.),
        (".5", 0.5),
        ("0x10", 16.),
        ("0b11", 3.),
    ] {
        assert_eq!(actions::js_number(typed), want, "{typed}");
    }
    for typed in [
        "not a number",
        "inf",
        "nan",
        "1_0",
        "--1",
        "-0x10",
        "",
        "1 2",
    ] {
        assert!(actions::js_number(typed).is_nan(), "{typed}");
    }
    assert_eq!(actions::js_number("Infinity"), f64::INFINITY);
}

#[test]
fn the_export_file_name_is_made_from_the_workflows_name() {
    assert_eq!(
        actions::workflow_file_name("Nightly Checks!"),
        "nightly-checks.monoptah-workflow.yaml"
    );
    assert_eq!(
        actions::workflow_file_name("!!!"),
        "workflow.monoptah-workflow.yaml"
    );
    let long = actions::workflow_file_name(&"a".repeat(100));
    assert_eq!(long.len(), 60 + ".monoptah-workflow.yaml".len());
}

// ---- row text -----------------------------------------------------------------------------

#[test]
fn a_row_says_what_the_workflow_is_doing() {
    let now = 1_000_000_000;
    let w = scheduled(json!({}));
    assert_eq!(
        workflow_subtitle(&w, Some((3, now - 12 * 60_000)), true, now),
        "Running \u{b7} 3 branches \u{b7} 12m"
    );
    assert_eq!(
        workflow_subtitle(&w, Some((1, now - 5_000)), true, now),
        "Running \u{b7} 1 branch \u{b7} 5s"
    );
    assert_eq!(workflow_subtitle(&w, None, true, now), "Running");
    assert_eq!(workflow_subtitle(&w, None, false, now), "Weekdays at 09:00");
    assert_eq!(
        workflow_subtitle(&scheduled(json!({"enabled": false})), None, false, now),
        "Paused \u{b7} Weekdays at 09:00"
    );
    assert_eq!(
        workflow_subtitle(&workflow(json!({})), None, false, now),
        "Manual"
    );
    assert_eq!(
        workflow_subtitle(&workflow(json!({"runRequested": true})), None, false, now),
        "Starting\u{2026}"
    );
}

#[test]
fn branches_count_nodes_at_work_not_the_group_itself() {
    let mut run = monocode_core::mono::workflows::engine::start_run(
        &workflow(json!({})),
        monocode_core::mono::workflows::model::RunTrigger::Manual,
        1,
        "run-live",
    );
    let node = |id: &str, branch: &str, status: &str| {
        serde_json::from_value::<monocode_core::mono::workflows::model::NodeRun>(json!(
            {"nodeId": id, "branch": branch, "status": status, "round": 1}
        ))
        .unwrap()
    };
    run.nodes = vec![
        node("a", "each#0", "running"),
        node("a", "each#1", "running"),
        node("b", "each#2", "waitingApproval"),
        node("c", "each#3", "done"),
    ];
    assert_eq!(active_branches(&run), 3);
    run.nodes = vec![node("c", "root", "done")];
    assert_eq!(active_branches(&run), 1);
}

// ---- ImportWorkflowPage.test.ts: the values form ------------------------------------------

fn declared(types: &[(&str, &str)]) -> OrderedMap<WorkflowInput> {
    let mut inputs = OrderedMap::new();
    for (name, ty) in types {
        let w = workflow(json!({"inputs": {*name: {"type": *ty}}}));
        inputs.insert(name.to_string(), w.inputs.get(name).unwrap().clone());
    }
    inputs
}

#[test]
fn what_was_entered_becomes_values_of_the_declared_type_only() {
    let inputs = declared(&[
        ("p", "project"),
        ("s", "string"),
        ("n", "number"),
        ("b", "boolean"),
    ]);
    let values = values_of(&inputs, |name, _| {
        Some(match name {
            "p" => Entered::Project("/work/web".into()),
            "s" => Entered::Text("the parser".into()),
            "n" => Entered::Text(" 4 ".into()),
            _ => Entered::Flag(true),
        })
    });
    assert_eq!(
        values.get("p"),
        Some(&InputValue::String("/work/web".into()))
    );
    assert_eq!(
        values.get("s"),
        Some(&InputValue::String("the parser".into()))
    );
    assert_eq!(values.get("n"), Some(&InputValue::Number(4.)));
    assert_eq!(values.get("b"), Some(&InputValue::Bool(true)));
    // A blank number is no value; an erased string is an empty text.
    let blank = values_of(&inputs, |name, _| match name {
        "n" => Some(Entered::Text("  ".into())),
        "s" => Some(Entered::Text(String::new())),
        _ => None,
    });
    assert!(blank.get("n").is_none());
    assert_eq!(blank.get("s"), Some(&InputValue::String(String::new())));
    // What was entered for another kind of input is not carried over.
    let wrong = values_of(&inputs, |_, _| Some(Entered::Flag(true)));
    assert_eq!(wrong.len(), 1);
    assert_eq!(InputType::Boolean, inputs.get("b").unwrap().ty);
}

#[test]
fn asking_mono_starts_a_sentence_without_stacking_it() {
    let start = "Help me build a workflow that ";
    assert_eq!(append_prefill("", start), start);
    assert_eq!(
        append_prefill("Earlier note", start),
        format!("Earlier note\n\n{start}")
    );
    assert_eq!(append_prefill("Note\n", start), format!("Note\n\n{start}"));
    assert_eq!(append_prefill(start, start), start);
    assert_eq!(append_prefill("x", "  "), "x");
}

#[test]
fn hidden_characters_are_shown_in_what_the_review_prints() {
    assert_eq!(visible("echo a\u{202e}b"), "echo a<U+202E>b");
}

// ---- the pages, in a real Monos -----------------------------------------------------------

pub(super) fn put(s: &Setup, list: Vec<Workflow>) {
    let dir = MonoDir::new(&s.root, &s.id);
    update_workflows(&dir, |_| Ok((list.clone(), ()))).unwrap();
}

pub(super) fn saved(s: &Setup) -> Vec<Workflow> {
    load_workflows(&MonoDir::new(&s.root, &s.id))
        .unwrap()
        .workflows
}

pub(super) fn history(
    s: &Setup,
    id: &str,
) -> Vec<monocode_core::mono::workflows::model::WorkflowRun> {
    load_runs(&MonoDir::new(&s.root, &s.id), id).unwrap()
}

/// The overview open on the Workflows tab with the Mono's files read.
pub(super) fn open_tab(s: &Setup, cx: &mut TestAppContext) {
    cx.update(|cx| {
        cx.set_reduce_motion(true);
        crate::input::bind(cx);
        crate::popover::bind(cx);
    });
    s.window
        .update(cx, |shell, _, cx| {
            // Wide enough for the tab strip (now with Overview) plus the header buttons.
            shell.workspace.right_panel_width = 520.;
            s.manager.update(cx, |m, cx| {
                m.settings = None;
                m.overview_tab = OverviewTab::Workflows;
                m.wf.import.debounce = Some(Duration::ZERO);
                cx.notify();
            })
        })
        .unwrap();
    cx.run_until_parked();
}

pub(super) fn refresh(s: &Setup, cx: &mut TestAppContext) {
    s.manager.update(cx, |m, cx| m.refresh_workflows(cx));
    cx.run_until_parked();
}

pub(super) fn click(visual: &mut VisualTestContext, selector: &'static str) {
    let bounds = visual
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is not on screen"));
    visual.simulate_click(bounds.center(), Default::default());
    visual.run_until_parked();
}

/// Lets the page slide finish, so what is clicked is where it is drawn.
pub(super) fn settle(s: &Setup, visual: &mut VisualTestContext) {
    let details = s.manager.read_with(visual, |m, _| m.details.clone());
    details.update(visual, |d, cx| d.elapsed(Duration::from_secs(2), cx));
    visual.run_until_parked();
    visual.refresh().unwrap();
}

pub(super) fn notice(s: &Setup, visual: &mut VisualTestContext) -> Option<String> {
    s.manager.read_with(visual, |m, _| m.wf.notice.clone())
}

pub(super) fn page_notice(s: &Setup, visual: &mut VisualTestContext) -> Option<String> {
    s.manager.read_with(visual, |m, _| m.wf.page_notice.clone())
}

pub(super) fn route(s: &Setup, visual: &mut VisualTestContext) -> Option<Route> {
    s.manager.read_with(visual, |m, _| m.route.clone())
}

#[gpui::test]
fn the_tab_lists_the_workflows_in_order_with_a_count(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(
        &s,
        vec![
            scheduled(json!({"id": "wf-schedule", "name": "Morning checks"})),
            workflow(json!({"id": "wf-manual", "name": "Release train"})),
        ],
    );
    open_tab(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    let first = visual.debug_bounds("mono-workflow-wf-schedule").unwrap();
    let second = visual.debug_bounds("mono-workflow-wf-manual").unwrap();
    assert!(first.top() < second.top());
    assert_eq!(
        s.manager.read_with(visual, |m, _| m.workflow_count()),
        Some(2)
    );
    assert!(visual.debug_bounds("mono-overview-Workflows").is_some());
    assert!(visual.debug_bounds("mono-workflow-needs-review").is_none());
}

#[gpui::test]
fn an_empty_tab_says_how_to_fill_it(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(&s, Vec::new());
    open_tab(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    assert!(visual.debug_bounds("mono-workflows-list").is_some());
    assert!(visual.debug_bounds("mono-workflow-wf-aaaaaaaa").is_none());
    assert_eq!(
        super::list::empty_message("Broski"),
        "Ask Broski to chain sessions into a workflow, or create one with +."
    );
}

#[gpui::test]
fn run_now_on_a_row_starts_a_manual_run(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(&s, vec![workflow(json!({}))]);
    open_tab(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    click(visual, "mono-workflow-run-wf-aaaaaaaa");
    let runs = history(&s, A);
    assert_eq!(runs.len(), 1, "the runner took the request");
    assert_eq!(runs[0].status, RunStatus::Done);
    assert!(
        chat_rows(&s)
            .iter()
            .any(|r| r.starts_with("## Nightly checks"))
    );
    assert_eq!(notice(&s, visual), None);
}

#[gpui::test]
fn run_now_on_a_workflow_with_issues_says_why_under_the_list(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(&s, vec![broken(json!({}))]);
    open_tab(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    click(visual, "mono-workflow-run-wf-aaaaaaaa");
    assert!(notice(&s, visual).unwrap().starts_with("Can't run yet"));
    assert!(visual.debug_bounds("mono-workflow-notice").is_some());
    assert!(!saved(&s)[0].run_requested);
    assert!(history(&s, A).is_empty());
}

#[gpui::test]
fn resume_and_run_now_on_a_flagged_workflow_open_the_review_instead(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(
        &s,
        vec![scheduled(
            json!({"enabled": false, "nextRunAt": null, "needsReview": true}),
        )],
    );
    open_tab(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    assert!(visual.debug_bounds("mono-workflow-needs-review").is_some());
    click(visual, "mono-workflow-toggle-wf-aaaaaaaa");
    assert_eq!(
        route(&s, visual),
        Some(Route::WorkflowReview(A.into(), ReviewIntent::Enable))
    );
    let w = &saved(&s)[0];
    assert!(!w.enabled && w.needs_review && w.next_run_at.is_none());
    s.manager.update(visual, |m, _| m.route = None);
    visual.run_until_parked();
    settle(&s, visual);
    click(visual, "mono-workflow-run-wf-aaaaaaaa");
    assert_eq!(
        route(&s, visual),
        Some(Route::WorkflowReview(A.into(), ReviewIntent::Run))
    );
    assert!(!saved(&s)[0].run_requested);
}

#[gpui::test]
fn resume_and_pause_flip_the_workflow_and_remove_takes_it_out(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(
        &s,
        vec![
            scheduled(json!({"enabled": false, "nextRunAt": null})),
            workflow(json!({"id": "wf-other001", "name": "Other"})),
        ],
    );
    open_tab(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    click(visual, "mono-workflow-toggle-wf-aaaaaaaa");
    assert!(saved(&s)[0].enabled);
    assert!(saved(&s)[0].next_run_at.unwrap() > monocode_core::time::now_ms());
    click(visual, "mono-workflow-toggle-wf-aaaaaaaa");
    assert!(!saved(&s)[0].enabled);
    click(visual, "mono-workflow-delete-wf-other001");
    assert_eq!(saved(&s).len(), 1);
    assert_eq!(
        s.manager.read_with(visual, |m, _| m.workflow_count()),
        Some(1)
    );
    assert!(visual.debug_bounds("mono-workflow-wf-other001").is_none());
}

#[gpui::test]
fn remove_on_a_row_is_refused_while_a_run_is_going(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(&s, vec![workflow(json!({}))]);
    open_tab(&s, cx);
    s.manager
        .read_with(cx, |m, _| m.workflow_signals.set_running(A, true));
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    click(visual, "mono-workflow-delete-wf-aaaaaaaa");
    assert_eq!(
        notice(&s, visual).as_deref(),
        Some("Stop the workflow before removing it.")
    );
    assert_eq!(saved(&s).len(), 1);
    s.manager
        .read_with(visual, |m, _| m.workflow_signals.set_running(A, false));
}

fn waiting(extra: Value) -> Workflow {
    build(
        extra,
        json!([
            {"id": "trigger", "type": "manual"},
            {"id": "ask", "type": "askMono", "question": "Ship it?", "answers": ["yes", "no"]},
            {"id": "report", "type": "report"},
        ]),
        json!([{"from": "trigger", "to": "ask"}, {"from": "ask", "to": "report"}]),
    )
}

#[gpui::test]
fn a_running_row_offers_stop_and_stop_ends_the_run(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(&s, vec![waiting(json!({"runRequested": true}))]);
    s.manager.update(cx, |m, cx| m.check_workflows(cx));
    cx.run_until_parked();
    assert!(s.manager.read_with(cx, |m, _| m.workflows.is_running(A)));
    open_tab(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds("mono-workflow-stop-wf-aaaaaaaa")
            .is_some()
    );
    let glance = s.manager.read_with(visual, |m, _| m.workflow_glance(A));
    assert_eq!(glance.map(|(branches, _)| branches), Some(1));
    click(visual, "mono-workflow-stop-wf-aaaaaaaa");
    visual.run_until_parked();
    assert!(
        !s.manager
            .read_with(visual, |m, _| m.workflows.is_running(A))
    );
    refresh(&s, visual);
    assert!(
        visual
            .debug_bounds("mono-workflow-stop-wf-aaaaaaaa")
            .is_none()
    );
    assert_eq!(history(&s, A)[0].status, RunStatus::Stopped);
}

#[gpui::test]
fn the_list_follows_a_run_that_starts_and_ends_in_the_runner(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(&s, vec![waiting(json!({}))]);
    open_tab(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds("mono-workflow-stop-wf-aaaaaaaa")
            .is_none()
    );
    let dir = MonoDir::new(&s.root, &s.id);
    update_workflows(&dir, |mut list| {
        list[0].run_requested = true;
        Ok((list, ()))
    })
    .unwrap();
    s.manager.update(visual, |m, cx| m.check_workflows(cx));
    visual.run_until_parked();
    // The clock notices the runner started a run.
    s.manager.update(visual, |m, cx| {
        assert!(m.workflows_poll(cx));
    });
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds("mono-workflow-stop-wf-aaaaaaaa")
            .is_some()
    );
    s.manager.update(visual, |m, _| {
        m.stop_workflow_run(A);
    });
    visual.run_until_parked();
    s.manager.update(visual, |m, cx| {
        assert!(m.workflows_poll(cx));
    });
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds("mono-workflow-stop-wf-aaaaaaaa")
            .is_none()
    );
}

#[gpui::test]
fn import_and_new_are_off_at_the_limit_and_ask_mono_starts_a_sentence(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(
        &s,
        (0..20)
            .map(|i| workflow(json!({"id": format!("wf-full{i:04}")})))
            .collect(),
    );
    open_tab(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    click(visual, "mono-workflow-import");
    click(visual, "mono-workflow-new");
    assert_eq!(route(&s, visual), None, "the limit refuses both");
    assert_eq!(WORKFLOWS_MAX, 20);
    // Ask Mono fills the composer and never sends.
    let before = chat_rows(&s).len();
    click(visual, "mono-workflow-ask");
    click(visual, "mono-workflow-ask");
    let draft = s.manager.read_with(visual, |m, cx| {
        m.chat()
            .unwrap()
            .read(cx)
            .input
            .read(cx)
            .content
            .to_string()
    });
    assert_eq!(draft, "Help me build a workflow that ");
    assert_eq!(chat_rows(&s).len(), before);
}

#[gpui::test]
fn import_and_new_open_their_pages_below_the_limit(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(&s, vec![workflow(json!({}))]);
    open_tab(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    click(visual, "mono-workflow-new");
    assert_eq!(route(&s, visual), Some(Route::NewWorkflow));
    assert!(visual.debug_bounds("wf-board").is_some());
    s.manager.update(visual, |m, _| m.route = None);
    visual.run_until_parked();
    click(visual, "mono-workflow-import");
    assert_eq!(route(&s, visual), Some(Route::ImportWorkflow));
    assert!(visual.debug_bounds("mono-import-page").is_some());
}

pub(super) fn open_page(s: &Setup, visual: &mut VisualTestContext) {
    s.manager.update(visual, |m, cx| {
        m.open_workflow_route(Route::Workflow(A.into()), cx)
    });
    visual.run_until_parked();
}

#[gpui::test]
fn the_page_lists_recent_runs_newest_first_and_opens_one(cx: &mut TestAppContext) {
    let s = setup(cx);
    let w = workflow(json!({}));
    put(&s, vec![w.clone()]);
    let mut done = monocode_core::mono::workflows::engine::start_run(
        &w,
        monocode_core::mono::workflows::model::RunTrigger::Manual,
        1_000,
        "run-1",
    );
    done.status = RunStatus::Done;
    done.ended_at = Some(61_000);
    let mut failed = done.clone();
    failed.id = "run-2".into();
    failed.status = RunStatus::Failed;
    update_runs(&MonoDir::new(&s.root, &s.id), A, |_| {
        Ok((vec![failed.clone(), done.clone()], ()))
    })
    .unwrap();
    open_tab(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    open_page(&s, visual);
    let newest = visual.debug_bounds("mono-workflow-run-run-2").unwrap();
    let older = visual.debug_bounds("mono-workflow-run-run-1").unwrap();
    assert!(newest.top() < older.top());
    click(visual, "mono-workflow-run-run-1");
    assert_eq!(
        route(&s, visual),
        Some(Route::WorkflowRun(A.into(), "run-1".into()))
    );
}

#[gpui::test]
fn export_refuses_on_a_path_problem_and_never_asks_for_a_file(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(
        &s,
        vec![workflow(json!({"description": "Runs in /Users/me/secret"}))],
    );
    open_tab(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    open_page(&s, visual);
    click(visual, "mono-workflow-export");
    assert!(!visual.did_prompt_for_new_path());
    assert!(page_notice(&s, visual).unwrap().contains("Can't export"));
}

#[gpui::test]
fn export_writes_the_definition_where_the_user_picks(cx: &mut TestAppContext) {
    let s = setup(cx);
    let w = workflow(json!({"name": "Nightly Checks!"}));
    put(&s, vec![w.clone()]);
    open_tab(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    open_page(&s, visual);
    click(visual, "mono-workflow-export");
    assert!(visual.did_prompt_for_new_path());
    let target =
        std::env::temp_dir().join(format!("wf-export-{}.yaml", monocode_core::mono::new_id()));
    let file = target.clone();
    visual.simulate_new_path_selection(move |_| Some(file));
    visual.run_until_parked();
    let text = std::fs::read_to_string(&target).unwrap();
    assert!(text.contains("monoptahWorkflow"));
    assert_eq!(text, export_workflow(&w));
    assert_eq!(page_notice(&s, visual), None);
    // A cancelled dialog writes nothing.
    click(visual, "mono-workflow-export");
    visual.simulate_new_path_selection(|_| None);
    visual.run_until_parked();
    assert_eq!(page_notice(&s, visual), None);
}

#[gpui::test]
fn an_input_value_is_saved_after_its_type_is_checked(cx: &mut TestAppContext) {
    let s = setup(cx);
    let (_, w) = limit_input();
    put(&s, vec![w]);
    open_tab(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    open_page(&s, visual);
    let key = format!("wf-input:{A}:limit");
    let type_in = |visual: &mut VisualTestContext, text: &str| {
        s.manager.update(visual, |m, cx| {
            m.fields[&key].update(cx, |f, cx| f.set_text(text.into(), cx));
            m.workflow_commit_input(A, "limit", cx);
        });
        visual.run_until_parked();
    };
    type_in(visual, "not a number");
    assert_eq!(
        saved(&s)[0].input_values.get("limit"),
        Some(&InputValue::Number(3.))
    );
    assert!(
        page_notice(&s, visual)
            .unwrap()
            .contains("\"limit\" must be a number")
    );
    type_in(visual, "7");
    assert_eq!(
        saved(&s)[0].input_values.get("limit"),
        Some(&InputValue::Number(7.))
    );
    assert_eq!(page_notice(&s, visual), None);
    // A project is chosen from the Mono's own.
    s.manager.update(visual, |m, cx| {
        m.workflow_choose_input(A, "repo", &s.project, cx)
    });
    visual.run_until_parked();
    assert_eq!(
        saved(&s)[0].input_values.get("repo"),
        Some(&InputValue::String(s.project.clone()))
    );
}

#[gpui::test]
fn the_real_reason_an_input_could_not_be_saved_is_shown(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(
        &s,
        vec![workflow(
            json!({"inputs": {"note": {"type": "string"}}, "inputValues": {"note": "a"}}),
        )],
    );
    open_tab(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    open_page(&s, visual);
    let key = format!("wf-input:{A}:note");
    s.manager.update(visual, |m, cx| {
        m.fields[&key].update(cx, |f, cx| f.set_text("x".repeat(3 * 1024 * 1024), cx));
        m.workflow_commit_input(A, "note", cx);
    });
    visual.run_until_parked();
    assert!(
        page_notice(&s, visual)
            .unwrap()
            .contains("too large to save")
    );
    assert_eq!(
        saved(&s)[0].input_values.get("note"),
        Some(&InputValue::String("a".into()))
    );
}

#[gpui::test]
fn remove_on_the_page_closes_it_and_is_refused_while_a_run_is_going(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(&s, vec![workflow(json!({}))]);
    open_tab(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    open_page(&s, visual);
    s.manager
        .read_with(visual, |m, _| m.workflow_signals.set_running(A, true));
    click(visual, "mono-workflow-delete");
    assert!(
        page_notice(&s, visual)
            .unwrap()
            .contains("Stop the workflow before removing it")
    );
    assert_eq!(saved(&s).len(), 1);
    assert_eq!(route(&s, visual), Some(Route::Workflow(A.into())));
    s.manager
        .read_with(visual, |m, _| m.workflow_signals.set_running(A, false));
    click(visual, "mono-workflow-delete");
    assert!(saved(&s).is_empty());
    assert_eq!(route(&s, visual), None);
}

#[gpui::test]
fn a_page_shows_the_needs_review_badge_only_when_needed_and_sends_run_and_resume_to_the_review(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    put(&s, vec![workflow(json!({}))]);
    open_tab(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    open_page(&s, visual);
    assert!(visual.debug_bounds("mono-workflow-needs-review").is_none());
    put(
        &s,
        vec![workflow(json!({"enabled": false, "needsReview": true}))],
    );
    refresh(&s, visual);
    assert!(visual.debug_bounds("mono-workflow-needs-review").is_some());
    click(visual, "mono-workflow-run");
    assert_eq!(
        route(&s, visual),
        Some(Route::WorkflowReview(A.into(), ReviewIntent::Run))
    );
    // Back from the review returns to the page it came from.
    click(visual, "mono-page-back");
    assert_eq!(route(&s, visual), Some(Route::Workflow(A.into())));
    click(visual, "mono-workflow-toggle");
    assert_eq!(
        route(&s, visual),
        Some(Route::WorkflowReview(A.into(), ReviewIntent::Enable))
    );
    let w = &saved(&s)[0];
    assert!(!w.enabled && w.needs_review && !w.run_requested);
}

#[gpui::test]
fn a_removed_workflow_closes_its_pages(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(&s, vec![workflow(json!({}))]);
    open_tab(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    open_page(&s, visual);
    assert_eq!(route(&s, visual), Some(Route::Workflow(A.into())));
    put(&s, Vec::new());
    refresh(&s, visual);
    assert_eq!(route(&s, visual), None);
}

#[test]
fn a_step_named_by_its_id_reads_it_as_words() {
    let node = |id: &str| {
        let w = workflow(json!({}));
        let mut n = w.nodes[1].clone();
        n.id = id.into();
        n
    };
    assert_eq!(
        super::review::node_title(&node("fetch_big__tickets")),
        "Fetch big tickets"
    );
    assert_eq!(super::review::node_title(&node("_a___b_")), "A b");
    let mut titled = node("x");
    titled.title = Some("  ".into());
    assert_eq!(super::review::node_title(&titled), "X");
}

#[gpui::test]
fn ask_mono_focuses_the_composer(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(&s, vec![workflow(json!({}))]);
    open_tab(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    click(visual, "mono-workflow-ask");
    let focused = s
        .window
        .update(visual, |_, window, cx| {
            let chat = s.manager.read(cx).chat().unwrap();
            let handle = chat.read(cx).input.focus_handle(cx);
            handle.is_focused(window)
        })
        .unwrap();
    assert!(focused);
}

#[gpui::test]
fn the_tab_reads_the_files_again_when_it_is_shown_again(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(&s, vec![workflow(json!({}))]);
    open_tab(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    assert_eq!(
        s.manager.read_with(visual, |m, _| m.workflow_count()),
        Some(1)
    );
    // The clock stopped while nothing showed workflows; the files changed meanwhile.
    put(
        &s,
        vec![workflow(json!({})), workflow(json!({"id": "wf-bbbbbbbb"}))],
    );
    s.manager.update(visual, |m, cx| {
        m.wf.poll = None;
        cx.notify();
    });
    visual.refresh().unwrap();
    visual.run_until_parked();
    assert_eq!(
        s.manager.read_with(visual, |m, _| m.workflow_count()),
        Some(2)
    );
    assert!(s.manager.read_with(visual, |m, _| m.wf.poll.is_some()));
}

#[gpui::test]
fn a_notice_goes_with_the_tab_and_the_page_it_was_shown_on(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(&s, vec![workflow(json!({}))]);
    open_tab(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    s.manager.update(visual, |m, cx| {
        m.wf.notice = Some("Can't run yet: x.".into());
        m.select_overview(OverviewTab::Workflows, cx);
        assert!(m.wf.notice.is_some(), "the same tab keeps it");
        m.select_overview(OverviewTab::Sessions, cx);
        assert!(m.wf.notice.is_none());
    });
    open_page(&s, visual);
    s.manager.update(visual, |m, _| {
        m.wf.page_notice = Some("Can't export.".into())
    });
    s.manager.update(visual, |m, cx| {
        m.select_overview(OverviewTab::Workflows, cx)
    });
    assert_eq!(page_notice(&s, visual), None);
}

#[gpui::test]
fn the_board_and_run_pages_go_back_to_the_workflow(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(&s, vec![workflow(json!({}))]);
    open_tab(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    for page in [
        Route::WorkflowBoard(A.into()),
        Route::WorkflowRun(A.into(), "run-1".into()),
    ] {
        s.manager
            .update(visual, |m, cx| m.open_workflow_route(page.clone(), cx));
        visual.run_until_parked();
        settle(&s, visual);
        click(visual, "mono-page-back");
        assert_eq!(
            route(&s, visual),
            Some(Route::Workflow(A.into())),
            "{page:?}"
        );
    }
}
