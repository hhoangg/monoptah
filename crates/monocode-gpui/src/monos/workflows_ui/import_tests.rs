//! The import page (`ImportWorkflowPage.test.ts`).
use super::tests::*;
use super::*;
use crate::monos::tests::{Setup, call_from, setup};
use gpui::{TestAppContext, VisualTestContext};
use monocode_core::mono::workflows::{
    model::{InputValue, NodeKind, Workflow},
    yaml::export_workflow,
};
use serde_json::{Value, json};

// ---- ImportWorkflowPage.test.ts ------------------------------------------------------------

fn source(extra: Value) -> Workflow {
    let mut all = json!({
        "id": "wf-sourceaaa", "name": "Build and fix",
        "description": "Builds the project and repairs it.",
        "inputs": {"project": {"type": "project"}, "area": {"type": "string", "description": "Where to look"}},
        "inputValues": {},
    });
    for (key, value) in extra.as_object().unwrap() {
        all[key] = value.clone();
    }
    build(
        all,
        json!([
            {"id": "trigger", "type": "manual", "title": "Start by hand"},
            {"id": "build", "type": "command", "title": "Build it", "project": "{{inputs.project}}",
             "command": "npm run build && echo 'done' | tee out.txt"},
            {"id": "fix", "type": "session", "title": "Fix failures", "project": "{{inputs.project}}",
             "checkout": "root", "prompt": "Fix what broke in {{inputs.area}}.",
             "output": {"summary": {"type": "string", "description": "One line on what changed"}}},
            {"id": "report", "type": "report", "title": "Tell the Mono"},
        ]),
        json!([
            {"from": "trigger", "to": "build"},
            {"from": "build", "to": "fix"},
            {"from": "fix", "to": "report"},
            {"from": "fix", "to": "build", "loop": {"maxRounds": 3, "decide": "{{fix.summary}} != \"clean\""}},
        ]),
    )
}

fn plain_source() -> Workflow {
    build(
        json!({"id": "wf-sourceaaa", "name": "Tiny"}),
        json!([{"id": "trigger", "type": "manual"}, {"id": "report", "type": "report"}]),
        json!([{"from": "trigger", "to": "report"}]),
    )
}

fn open_import(s: &Setup, cx: &mut TestAppContext) {
    open_tab(s, cx);
    s.manager.update(cx, |m, cx| m.open_import_page(cx));
    cx.run_until_parked();
}

fn paste(s: &Setup, text: &str, visual: &mut VisualTestContext) {
    s.manager.update(visual, |m, cx| {
        m.fields["wf-import-text"].update(cx, |f, cx| f.set_text(text.into(), cx))
    });
    visual.run_until_parked();
}

fn ready(s: &Setup, visual: &mut VisualTestContext) -> bool {
    // The button is dimmed and inert unless the import may be saved: ask by clicking.
    let before = saved(s).len();
    click(visual, "mono-import-paused");
    saved(s).len() != before
}

fn import_error(s: &Setup, visual: &mut VisualTestContext) -> Option<String> {
    s.manager
        .read_with(visual, |m, _| m.wf.import.error.clone())
}

fn read_error(s: &Setup, visual: &mut VisualTestContext) -> Option<String> {
    s.manager.read_with(visual, |m, _| match &m.wf.import.read {
        Some(Err(e)) => Some(e.clone()),
        _ => None,
    })
}

fn type_input(s: &Setup, ty: &str, name: &str, text: &str, visual: &mut VisualTestContext) {
    let key = format!("wf-import-in:{ty}:{name}");
    s.manager.update(visual, |m, cx| {
        m.fields[&key].update(cx, |f, cx| f.set_text(text.into(), cx))
    });
    visual.run_until_parked();
}

fn choose_project(s: &Setup, name: &str, visual: &mut VisualTestContext) {
    let project = s.project.clone();
    s.manager.update(visual, |m, cx| {
        m.import_choose(name, "project", &project, cx)
    });
    visual.run_until_parked();
}

fn preview_of(
    s: &Setup,
    visual: &mut VisualTestContext,
) -> Option<monocode_core::mono::workflows::yaml::ImportPreview> {
    s.manager
        .read_with(visual, |m, _| m.wf.import.read.clone().and_then(Result::ok))
}

#[gpui::test]
fn a_pasted_file_is_previewed_with_its_commands_prompts_and_loops(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(&s, Vec::new());
    open_import(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    paste(&s, &export_workflow(&source(json!({}))), visual);
    let preview = preview_of(&s, visual).expect("a preview");
    assert_eq!(
        preview.commands[0].command,
        "npm run build && echo 'done' | tee out.txt"
    );
    let prompts: String = preview.prompts.iter().map(|p| p.text.clone()).collect();
    assert!(prompts.contains("Fix what broke in {{inputs.area}}."));
    assert!(prompts.contains("One line on what changed"));
    assert_eq!(preview.workflow.name, "Build and fix");
    for selector in [
        "mono-workflow-commands",
        "mono-workflow-steps",
        "mono-workflow-prompts",
        "mono-import-inputs",
    ] {
        assert!(visual.debug_bounds(selector).is_some(), "{selector}");
    }
}

#[gpui::test]
fn import_waits_for_every_input_and_saves_paused_and_flagged(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(&s, Vec::new());
    open_import(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    paste(
        &s,
        &export_workflow(&source(json!({"enabled": true}))),
        visual,
    );
    assert!(!ready(&s, visual));
    choose_project(&s, "project", visual);
    assert!(!ready(&s, visual));
    type_input(&s, "string", "area", "the parser", visual);
    assert!(ready(&s, visual));
    let w = &saved(&s)[0];
    assert!(!w.enabled);
    assert!(w.id.starts_with("wf-") && w.id != "wf-sourceaaa");
    assert_eq!(
        w.input_values.get("project"),
        Some(&InputValue::String(s.project.clone()))
    );
    assert_eq!(
        w.input_values.get("area"),
        Some(&InputValue::String("the parser".into()))
    );
    assert!(w.next_run_at.is_none());
    // What came from a file is read again, and approved, before it can run.
    assert!(w.needs_review);
    // The import page is replaced by the new workflow's page.
    assert_eq!(route(&s, visual), Some(Route::Workflow(w.id.clone())));
}

#[gpui::test]
fn validation_issues_are_shown_without_blocking_the_import(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(&s, Vec::new());
    open_import(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    let broken_file = build(
        json!({"id": "wf-sourceaaa", "name": "Odd", "inputs": {"project": {"type": "project"}}}),
        json!([
            {"id": "trigger", "type": "manual"},
            {"id": "build", "type": "command", "project": "{{inputs.project}}", "command": "make"},
            {"id": "report", "type": "report"},
        ]),
        json!([{"from": "trigger", "to": "build"}]),
    );
    paste(&s, &export_workflow(&broken_file), visual);
    let preview = preview_of(&s, visual).unwrap();
    assert!(!preview.issues.is_empty());
    assert!(visual.debug_bounds("mono-workflow-problems").is_some());
    choose_project(&s, "project", visual);
    assert!(ready(&s, visual));
}

#[gpui::test]
fn a_file_the_model_refuses_shows_its_message_and_nothing_else(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(&s, Vec::new());
    open_import(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    paste(
        &s,
        &export_workflow(&source(json!({}))).replace("monoptahWorkflow: 1", "monoptahWorkflow: 2"),
        visual,
    );
    assert!(
        read_error(&s, visual)
            .unwrap()
            .contains("Unsupported workflow file version: 2")
    );
    assert!(visual.debug_bounds("mono-workflow-commands").is_none());
    assert!(visual.debug_bounds("mono-workflow-prompts").is_none());
    assert!(!ready(&s, visual));
}

#[gpui::test]
fn the_limit_disables_the_import_and_is_checked_again_over_the_disk(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(&s, vec![workflow(json!({}))]);
    open_import(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    paste(&s, &export_workflow(&plain_source()), visual);
    // The page believes there is room; the disk is full.
    put(
        &s,
        (0..20)
            .map(|i| workflow(json!({"id": format!("wf-full{i:04}")})))
            .collect(),
    );
    assert!(!ready(&s, visual), "the disk is full");
    assert_eq!(saved(&s).len(), 20);
    assert!(
        import_error(&s, visual)
            .unwrap()
            .contains("at most 20 workflows")
    );
    assert_eq!(route(&s, visual), Some(Route::ImportWorkflow));
    // Once the page knows, the button is off.
    refresh(&s, visual);
    assert!(!ready(&s, visual));
    assert_eq!(saved(&s).len(), 20);
}

#[gpui::test]
fn a_picked_file_goes_into_the_same_preview(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(&s, Vec::new());
    open_import(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    let file = std::env::temp_dir().join(format!("wf-pick-{}.yaml", monocode_core::mono::new_id()));
    let text = export_workflow(&source(json!({})));
    std::fs::write(&file, &text).unwrap();
    click(visual, "mono-import-choose-file");
    assert!(visual.did_prompt_for_paths());
    let picked = file.clone();
    visual.simulate_path_prompt_response(move |options| {
        assert!(options.files && !options.directories && !options.multiple);
        Some(vec![picked.clone()])
    });
    visual.run_until_parked();
    assert_eq!(
        s.manager
            .read_with(visual, |m, cx| m.value("wf-import-text", cx)),
        text
    );
    assert!(preview_of(&s, visual).is_some());
}

#[gpui::test]
fn a_cancelled_dialog_keeps_what_is_shown(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(&s, Vec::new());
    open_import(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    paste(&s, &export_workflow(&plain_source()), visual);
    click(visual, "mono-import-choose-file");
    visual.simulate_path_prompt_response(|_| None);
    visual.run_until_parked();
    assert!(preview_of(&s, visual).is_some());
}

#[gpui::test]
fn a_file_that_cannot_be_read_says_why(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(&s, Vec::new());
    open_import(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    let missing =
        std::env::temp_dir().join(format!("wf-missing-{}.yaml", monocode_core::mono::new_id()));
    click(visual, "mono-import-choose-file");
    visual.simulate_path_prompt_response(move |_| Some(vec![missing.clone()]));
    visual.run_until_parked();
    let error = s
        .manager
        .read_with(visual, |m, _| m.wf.import.file_error.clone());
    assert!(error.unwrap().starts_with("Could not read the file: "));
}

#[gpui::test]
fn a_picked_file_over_the_limit_never_reaches_the_page(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(&s, Vec::new());
    open_import(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    paste(&s, &export_workflow(&plain_source()), visual);
    let file = std::env::temp_dir().join(format!("wf-big-{}.yaml", monocode_core::mono::new_id()));
    std::fs::write(&file, "a: 1\n".repeat(120_000)).unwrap();
    click(visual, "mono-import-choose-file");
    let picked = file.clone();
    visual.simulate_path_prompt_response(move |_| Some(vec![picked.clone()]));
    visual.run_until_parked();
    let error = s
        .manager
        .read_with(visual, |m, _| m.wf.import.file_error.clone());
    assert!(error.unwrap().contains("too large (over 512 KiB)"));
    assert_eq!(
        s.manager
            .read_with(visual, |m, cx| m.value("wf-import-text", cx)),
        ""
    );
    assert!(preview_of(&s, visual).is_none());
    assert!(!ready(&s, visual));
}

#[gpui::test]
fn an_interval_schedule_is_anchored_like_the_cli_import(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(&s, Vec::new());
    open_import(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    let timed = build(
        json!({"id": "wf-sourceaaa", "name": "Timed"}),
        json!([
            {"id": "trigger", "type": "schedule", "schedule": {"scheduleKind": "interval", "time": "09:00", "minute": 0, "dayOfWeek": 1, "intervalHours": 4}},
            {"id": "report", "type": "report"},
        ]),
        json!([{"from": "trigger", "to": "report"}]),
    );
    paste(&s, &export_workflow(&timed), visual);
    assert!(ready(&s, visual));
    let w = &saved(&s)[0];
    match &w.nodes[0].kind {
        NodeKind::Schedule { schedule } => {
            let from = schedule.interval_from.clone().unwrap();
            assert_eq!(from.len(), 10, "{from}");
        }
        other => panic!("{other:?}"),
    }
    assert!(!w.enabled);
}

#[gpui::test]
fn an_input_value_is_not_carried_to_a_file_that_declares_it_differently(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(&s, Vec::new());
    open_import(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    let file = |ty: &str| {
        export_workflow(&build(
            json!({"id": "wf-sourceaaa", "name": "One input", "inputs": {"area": {"type": ty}}}),
            json!([{"id": "trigger", "type": "manual"}, {"id": "report", "type": "report"}]),
            json!([{"from": "trigger", "to": "report"}]),
        ))
    };
    paste(&s, &file("string"), visual);
    type_input(&s, "string", "area", "/work/api", visual);
    let inputs = preview_of(&s, visual).unwrap().workflow.inputs;
    let values = s
        .manager
        .read_with(visual, |m, cx| m.import_values(&inputs, cx));
    assert_eq!(
        values.get("area"),
        Some(&InputValue::String("/work/api".into()))
    );
    paste(&s, &file("project"), visual);
    let inputs = preview_of(&s, visual).unwrap().workflow.inputs;
    let values = s
        .manager
        .read_with(visual, |m, cx| m.import_values(&inputs, cx));
    assert!(values.get("area").is_none());
    assert!(!ready(&s, visual));
}

#[gpui::test]
fn the_real_reason_an_import_could_not_be_saved_is_shown(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(&s, Vec::new());
    open_import(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    paste(&s, &export_workflow(&plain_source()), visual);
    // The workflows file cannot be read as a file any more.
    let file = s.root.join(&s.id).join("workflows.json");
    let _ = std::fs::remove_file(&file);
    std::fs::create_dir_all(&file).unwrap();
    click(visual, "mono-import-paused");
    let error = import_error(&s, visual).expect("an error");
    assert_ne!(error, "Could not save the workflow.");
    assert!(!error.is_empty());
    assert_eq!(route(&s, visual), Some(Route::ImportWorkflow));
}

#[gpui::test]
fn a_workflow_the_mono_adds_shows_up_in_the_open_tab(cx: &mut TestAppContext) {
    // The pages go through the same store as the CLI, so a CLI add shows up in the open tab.
    let s = setup(cx);
    put(&s, Vec::new());
    open_tab(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    assert_eq!(
        s.manager.read_with(visual, |m, _| m.workflow_count()),
        Some(0)
    );
    let rx = call_from(
        &s,
        s.session,
        "wf-add-1",
        "workflows.add",
        json!({"workflow": {
            "name": "From the Mono",
            "nodes": [{"id": "trigger", "type": "manual"}, {"id": "report", "type": "report"}],
            "edges": [{"from": "trigger", "to": "report"}],
        }}),
        visual,
    );
    visual.run_until_parked();
    assert!(rx.try_recv().unwrap().is_ok());
    assert_eq!(
        s.manager.read_with(visual, |m, _| m.workflow_count()),
        Some(1)
    );
}

#[gpui::test]
fn a_file_that_cannot_be_read_keeps_what_was_pasted(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(&s, Vec::new());
    open_import(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    let text = export_workflow(&plain_source());
    paste(&s, &text, visual);
    let missing =
        std::env::temp_dir().join(format!("wf-missing-{}.yaml", monocode_core::mono::new_id()));
    click(visual, "mono-import-choose-file");
    visual.simulate_path_prompt_response(move |_| Some(vec![missing.clone()]));
    visual.run_until_parked();
    assert_eq!(
        s.manager
            .read_with(visual, |m, cx| m.value("wf-import-text", cx)),
        text
    );
    assert!(preview_of(&s, visual).is_some());
    let error = s
        .manager
        .read_with(visual, |m, _| m.wf.import.file_error.clone());
    assert!(error.unwrap().starts_with("Could not read the file: "));
}
