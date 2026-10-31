//! The scheduling cases of `useMonoWorkflows.test.ts` over a real `Monos` (real files, the real
//! host, no provider): what a look starts, the waker, requests in the file, the startup sweep,
//! quitting and deleting a Mono. The file-level cases are in core (`tick_tests`).
use super::*;
use crate::monos::tests::{Setup, call, chat_rows, setup};
use gpui::TestAppContext;
use monocode_core::mono::workflows::{
    engine::{mark_interrupted, start_run},
    model::{RunStatus, RunTrigger, Workflow, WorkflowRun},
    ordered::Json,
    parse::parse_workflow,
    store::{load_runs, load_workflows, update_runs, update_workflows},
};
use serde_json::{Value, json};

fn definition(id: &str, nodes: Vec<Value>, edges: Vec<Value>, extra: Value) -> Workflow {
    let mut def = json!({
        "id": id, "name": "Ship it", "inputs": {}, "inputValues": {},
        "nodes": nodes, "edges": edges, "enabled": false, "createdAt": 0,
    });
    for (key, value) in extra.as_object().unwrap() {
        def[key] = value.clone();
    }
    parse_workflow(&Json::from(&def)).expect("a workflow")
}

/// manual trigger -> report: runs to the end by itself.
fn quick(id: &str, extra: Value) -> Workflow {
    definition(
        id,
        vec![
            json!({"id": "trigger", "type": "manual"}),
            json!({"id": "report", "type": "report"}),
        ],
        vec![json!({"from": "trigger", "to": "report"})],
        extra,
    )
}

/// manual trigger -> a question to the Mono -> report: stays running until stopped, because no
/// provider answers the hidden session in a test.
fn waiting(id: &str, extra: Value) -> Workflow {
    definition(
        id,
        vec![
            json!({"id": "trigger", "type": "manual"}),
            json!({"id": "ask", "type": "askMono", "question": "Ship it?", "answers": ["yes", "no"]}),
            json!({"id": "report", "type": "report"}),
        ],
        vec![
            json!({"from": "trigger", "to": "ask"}),
            json!({"from": "ask", "to": "report"}),
        ],
        extra,
    )
}

fn dir(s: &Setup) -> MonoDir {
    MonoDir::new(&s.root, &s.id)
}

fn put(s: &Setup, workflows: Vec<Workflow>) {
    update_workflows(&dir(s), |_| Ok((workflows.clone(), ()))).unwrap();
}

fn saved(s: &Setup) -> Vec<Workflow> {
    load_workflows(&dir(s)).unwrap().workflows
}

fn runs(s: &Setup, workflow: &str) -> Vec<WorkflowRun> {
    load_runs(&dir(s), workflow).unwrap()
}

fn put_runs(s: &Setup, workflow: &str, list: Vec<WorkflowRun>) {
    update_runs(&dir(s), workflow, |_| Ok((list.clone(), ()))).unwrap();
}

/// A look, as the clock or the waker makes it, with everything it starts run to the end.
fn look(s: &Setup, cx: &mut TestAppContext) {
    cx.update(|cx| s.manager.update(cx, |m, cx| m.check_workflows(cx)));
    cx.run_until_parked();
}

fn running(s: &Setup, workflow: &str, cx: &mut TestAppContext) -> bool {
    cx.update(|cx| s.manager.read(cx).workflows.is_running(workflow))
}

fn forget_startup(s: &Setup, cx: &mut TestAppContext) {
    cx.update(|cx| {
        s.manager.update(cx, |m, _| {
            m.workflow_checks.swept = false;
            m.workflow_checks.asks_swept = false;
        })
    });
}

fn reported(s: &Setup, heading: &str) -> bool {
    chat_rows(s).iter().any(|row| row.starts_with(heading))
}

const A: &str = "wf-aaaaaaaa";
const B: &str = "wf-bbbbbbbb";

#[gpui::test]
fn a_requested_run_starts_on_the_look_and_its_report_reaches_the_chat(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(&s, vec![quick(A, json!({"runRequested": true}))]);
    look(&s, cx);
    let history = runs(&s, A);
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].status, RunStatus::Done);
    assert_eq!(history[0].trigger, RunTrigger::Manual);
    let after = &saved(&s)[0];
    assert!(!after.run_requested);
    assert_eq!(after.last_run_id.as_deref(), Some(history[0].id.as_str()));
    // A manual workflow stays paused; only the request ran it.
    assert!(!after.enabled);
    assert!(
        reported(&s, "## Ship it \u{b7} completed"),
        "{:?}",
        chat_rows(&s)
    );
    assert!(!running(&s, A, cx));
}

#[gpui::test]
fn a_due_schedule_is_claimed_and_run_once(cx: &mut TestAppContext) {
    let s = setup(cx);
    let schedule = json!({"id": "trigger", "type": "schedule", "schedule": {"scheduleKind": "daily", "time": "09:00", "minute": 0, "dayOfWeek": 1}});
    let due = definition(
        A,
        vec![schedule, json!({"id": "report", "type": "report"})],
        vec![json!({"from": "trigger", "to": "report"})],
        json!({"enabled": true, "nextRunAt": monocode_core::time::now_ms() - 1_000}),
    );
    put(&s, vec![due]);
    look(&s, cx);
    look(&s, cx);
    let history = runs(&s, A);
    assert_eq!(history.len(), 1, "the second look finds it already claimed");
    assert_eq!(history[0].trigger, RunTrigger::Schedule);
    assert_eq!(history[0].status, RunStatus::Done);
    assert!(saved(&s)[0].next_run_at.unwrap() > monocode_core::time::now_ms());
}

#[gpui::test]
fn a_workflow_that_cannot_start_gets_a_failed_run_and_a_message_in_the_chat(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    let mut broken = quick(A, json!({"runRequested": true}));
    broken.nodes.retain(|n| n.id != "report");
    broken.edges.clear();
    put(&s, vec![broken]);
    look(&s, cx);
    let history = runs(&s, A);
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].status, RunStatus::Failed);
    assert!(
        history[0]
            .error
            .as_deref()
            .unwrap()
            .starts_with("The workflow is not valid: ")
    );
    assert!(
        reported(
            &s,
            "## Ship it \u{b7} failed\n\nThe workflow could not start. "
        ),
        "{:?}",
        chat_rows(&s)
    );
    assert!(!running(&s, A, cx));
}

#[gpui::test]
fn the_waker_makes_a_request_act_at_once(cx: &mut TestAppContext) {
    let s = setup(cx);
    // Written the way another window would write it; nothing looks until the waker rings.
    put(&s, vec![quick(A, json!({"runRequested": true}))]);
    cx.run_until_parked();
    assert!(runs(&s, A).is_empty());
    let signals = cx.update(|cx| s.manager.read(cx).workflow_signals.clone());
    signals.request_check_now();
    cx.run_until_parked();
    assert_eq!(runs(&s, A)[0].status, RunStatus::Done);
    assert!(!signals.take_check_now(), "the look answered the request");
}

#[gpui::test]
fn the_clock_looks_every_minute(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(&s, vec![quick(A, json!({"runRequested": true}))]);
    cx.executor().advance_clock(Duration::from_secs(30));
    cx.run_until_parked();
    assert!(runs(&s, A).is_empty(), "not yet");
    cx.executor().advance_clock(Duration::from_secs(31));
    cx.run_until_parked();
    assert_eq!(runs(&s, A).len(), 1);
}

#[gpui::test]
fn a_second_run_of_a_workflow_waits_for_the_first(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(&s, vec![waiting(A, json!({"runRequested": true}))]);
    look(&s, cx);
    assert!(running(&s, A, cx));
    assert_eq!(runs(&s, A).len(), 1);
    // Asked again while it goes: the request waits, nothing else starts.
    update_workflows(&dir(&s), |mut list| {
        list[0].run_requested = true;
        Ok((list, ()))
    })
    .unwrap();
    look(&s, cx);
    assert_eq!(runs(&s, A).len(), 1);
    assert!(saved(&s)[0].run_requested);
    // Another workflow goes side by side.
    put(
        &s,
        vec![saved(&s).remove(0), quick(B, json!({"runRequested": true}))],
    );
    look(&s, cx);
    assert_eq!(runs(&s, B)[0].status, RunStatus::Done);
    assert!(running(&s, A, cx));
    // Once the first ends the waiting request runs.
    cx.update(|cx| s.manager.update(cx, |m, _| m.stop_workflow_run(A)));
    cx.run_until_parked();
    assert_eq!(runs(&s, A)[0].status, RunStatus::Stopped);
    look(&s, cx);
    assert_eq!(runs(&s, A).len(), 2);
}

#[gpui::test]
fn a_stop_written_in_the_file_stops_the_run_that_is_going(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(&s, vec![waiting(A, json!({"runRequested": true}))]);
    look(&s, cx);
    assert!(running(&s, A, cx));
    update_workflows(&dir(&s), |mut list| {
        list[0].stop_requested = true;
        Ok((list, ()))
    })
    .unwrap();
    look(&s, cx);
    assert_eq!(runs(&s, A)[0].status, RunStatus::Stopped);
    assert!(!saved(&s)[0].stop_requested);
    assert!(!running(&s, A, cx));
    assert!(reported(&s, "## Ship it \u{b7} stopped"));
}

#[gpui::test]
fn a_resume_written_in_the_file_continues_an_interrupted_run(cx: &mut TestAppContext) {
    let s = setup(cx);
    let def = quick(A, json!({}));
    let interrupted = mark_interrupted(&start_run(&def, RunTrigger::Manual, 1_000, "run-old"));
    assert_eq!(interrupted.status, RunStatus::Interrupted);
    put(&s, vec![quick(A, json!({"resumeRunId": "run-old"}))]);
    put_runs(&s, A, vec![interrupted]);
    look(&s, cx);
    let history = runs(&s, A);
    assert_eq!(history.len(), 1, "the same run continues");
    assert_eq!(history[0].id, "run-old");
    assert_eq!(history[0].status, RunStatus::Done);
    assert_eq!(saved(&s)[0].resume_run_id, None);
    assert!(reported(&s, "## Ship it \u{b7} completed"));
}

#[gpui::test]
fn the_first_look_interrupts_runs_a_previous_session_left_but_not_the_ones_it_drives(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    put(
        &s,
        vec![
            waiting(A, json!({"runRequested": true})),
            quick(B, json!({})),
        ],
    );
    look(&s, cx);
    assert!(running(&s, A, cx));
    let live_id = runs(&s, A)[0].id.clone();
    let stale = start_run(&quick(B, json!({})), RunTrigger::Manual, 1_000, "run-stale");
    assert_eq!(stale.status, RunStatus::Running);
    put_runs(&s, B, vec![stale]);
    forget_startup(&s, cx);
    look(&s, cx);
    assert_eq!(runs(&s, B)[0].status, RunStatus::Interrupted);
    // An interrupted run says nothing until it is resumed.
    assert!(!reported(&s, "## Ship it \u{b7} interrupted"));
    let driven = runs(&s, A);
    assert_eq!(
        (driven[0].id.as_str(), driven[0].status),
        (live_id.as_str(), RunStatus::Running)
    );
    // It is done once: a later look does not sweep again.
    put_runs(
        &s,
        B,
        vec![start_run(
            &quick(B, json!({})),
            RunTrigger::Manual,
            2_000,
            "run-new",
        )],
    );
    look(&s, cx);
    assert_eq!(runs(&s, B)[0].status, RunStatus::Running);
}

#[gpui::test]
fn the_first_look_removes_question_sessions_a_quit_left_behind(cx: &mut TestAppContext) {
    let s = setup(cx);
    let (leftover, lookalike) = {
        let db = locked(&s.store).unwrap();
        let leftover = db.create("codex", &s.project).unwrap();
        db.title(leftover.id, "Mono \u{b7} workflow question")
            .unwrap();
        // A hidden session whose title only looks like one: the roster never recorded it.
        let lookalike = db.create("codex", &s.project).unwrap();
        db.title(lookalike.id, "Mono \u{b7} workflow question")
            .unwrap();
        (leftover.id, lookalike.id)
    };
    cx.update(|cx| {
        s.manager.update(cx, |m, _| {
            m.roster.hidden_sessions.extend([leftover, lookalike]);
            m.roster.ask_sessions.push(leftover);
        })
    });
    forget_startup(&s, cx);
    look(&s, cx);
    cx.run_until_parked();
    let (hidden, asks, sessions): (Vec<i64>, Vec<i64>, Vec<i64>) = (
        cx.update(|cx| s.manager.read(cx).roster.hidden_sessions.clone()),
        cx.update(|cx| s.manager.read(cx).roster.ask_sessions.clone()),
        locked(&s.store)
            .unwrap()
            .sessions()
            .unwrap()
            .into_iter()
            .map(|session| session.id)
            .collect(),
    );
    assert!(!hidden.contains(&leftover) && !sessions.contains(&leftover));
    assert!(!asks.contains(&leftover));
    assert!(hidden.contains(&lookalike) && sessions.contains(&lookalike));
    // The Mono's own chat is never taken for one.
    assert!(hidden.contains(&s.session) && sessions.contains(&s.session));
}

#[gpui::test]
fn a_question_is_recorded_while_it_waits_and_forgotten_when_it_ends(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(&s, vec![waiting(A, json!({"runRequested": true}))]);
    look(&s, cx);
    let recorded = cx.update(|cx| s.manager.read(cx).roster.ask_sessions.clone());
    assert_eq!(recorded.len(), 1);
    assert!(cx.update(|cx| {
        s.manager
            .read(cx)
            .roster
            .hidden_sessions
            .contains(&recorded[0])
    }));
    cx.update(|cx| s.manager.update(cx, |m, _| m.stop_workflow_run(A)));
    cx.executor().advance_clock(Duration::from_secs(3));
    cx.run_until_parked();
    // Stopping abandons the question; its session goes at its own timer, and is forgotten then.
    cx.executor().advance_clock(Duration::from_secs(600));
    cx.run_until_parked();
    assert!(cx.update(|cx| s.manager.read(cx).roster.ask_sessions.is_empty()));
}

#[gpui::test]
fn a_look_asked_for_during_a_look_follows_it_and_never_overlaps(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(&s, vec![quick(A, json!({"runRequested": true}))]);
    cx.update(|cx| {
        s.manager.update(cx, |m, cx| {
            m.check_workflows(cx);
            assert!(m.workflow_checks.checking);
            m.check_workflows(cx);
            assert!(m.workflow_checks.again);
        })
    });
    cx.run_until_parked();
    cx.update(|cx| {
        let checks = &s.manager.read(cx).workflow_checks;
        assert!(!checks.checking && !checks.again);
    });
    assert_eq!(runs(&s, A).len(), 1);
}

#[gpui::test]
fn quitting_leaves_a_run_running_in_its_file_and_the_next_start_interrupts_it(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    put(&s, vec![waiting(A, json!({"runRequested": true}))]);
    look(&s, cx);
    assert!(running(&s, A, cx));
    let before = chat_rows(&s).len();
    cx.update(begin_quit);
    cx.run_until_parked();
    assert!(!running(&s, A, cx));
    // No stop: the file says `running`, no report was posted.
    assert_eq!(runs(&s, A)[0].status, RunStatus::Running);
    assert_eq!(chat_rows(&s).len(), before);
    // The next start (another process: the free function stands in for its first look) marks it,
    // and it can be resumed.
    let signals = monocode_core::mono::workflows::cli::Signals::default();
    let probe = |_: Kind| Probe {
        installed: true,
        models: None,
    };
    let looked = look_at(
        &s.root,
        &[(s.id.clone(), true)],
        &[],
        true,
        &signals,
        probes(probe),
        monocode_core::time::now_ms(),
    );
    assert!(looked.swept);
    assert_eq!(runs(&s, A)[0].status, RunStatus::Interrupted);
}

#[gpui::test]
fn the_quit_path_reaches_the_manager_through_the_app(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(&s, vec![waiting(A, json!({"runRequested": true}))]);
    look(&s, cx);
    assert!(running(&s, A, cx));
    cx.update(begin_quit);
    cx.run_until_parked();
    assert!(!running(&s, A, cx));
    assert_eq!(runs(&s, A)[0].status, RunStatus::Running);
}

#[gpui::test]
fn deleting_a_mono_stops_its_workflow_runs(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(&s, vec![waiting(A, json!({"runRequested": true}))]);
    look(&s, cx);
    assert!(running(&s, A, cx));
    assert_eq!(
        cx.update(|cx| s.manager.read(cx).workflow_live.asks.len()),
        1
    );
    let handle: gpui::AnyWindowHandle = s.window.into();
    let id = s.id.clone();
    handle
        .update(cx, |_, window, cx| {
            s.manager.update(cx, |m, cx| m.delete_mono(&id, window, cx))
        })
        .unwrap();
    cx.simulate_prompt_answer("Delete");
    cx.run_until_parked();
    cx.update(|cx| {
        let monos = s.manager.read(cx);
        assert!(monos.roster.get(&s.id).is_none());
        assert!(!monos.workflows.is_running(A));
        assert!(!monos.workflow_signals.is_running(A));
        assert!(
            monos.workflow_live.asks.is_empty(),
            "its question ended too"
        );
    });
    assert_eq!(runs(&s, A)[0].status, RunStatus::Stopped);
}

#[test]
fn a_look_over_missing_monos_is_empty_and_counts_as_swept() {
    let root = std::env::temp_dir().join(format!("wf-look-{}", mono::new_id()));
    let signals = monocode_core::mono::workflows::cli::Signals::default();
    let probe = |_: Kind| Probe {
        installed: true,
        models: None,
    };
    let looked = look_at(
        &root,
        &[("nobody".into(), true)],
        &[],
        true,
        &signals,
        probes(probe),
        1,
    );
    assert!(looked.swept && looked.monos.is_empty() && looked.errors.is_empty());
    let looked = look_at(&root, &[], &[], false, &signals, probes(probe), 1);
    assert!(!looked.swept);
}

#[gpui::test]
fn the_quit_does_not_need_the_manager_to_be_free(cx: &mut TestAppContext) {
    // The tray's Quit runs inside an update of the manager.
    let s = setup(cx);
    cx.update(|cx| s.manager.update(cx, |_, cx| begin_quit(cx)));
    cx.update(|cx| assert!(s.manager.read(cx).workflows.is_quitting()));
}

#[gpui::test]
fn after_the_quit_a_look_starts_nothing_and_writes_nothing(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(&s, vec![quick(A, json!({"runRequested": true}))]);
    let before = std::fs::read_to_string(s.root.join("monos").join(&s.id).join("workflows.json"))
        .unwrap_or_default();
    cx.update(begin_quit);
    look(&s, cx);
    let signals = cx.update(|cx| s.manager.read(cx).workflow_signals.clone());
    signals.request_check_now();
    cx.executor().advance_clock(Duration::from_secs(61));
    cx.run_until_parked();
    assert!(runs(&s, A).is_empty());
    assert!(saved(&s)[0].run_requested, "the request was not claimed");
    let after = std::fs::read_to_string(s.root.join("monos").join(&s.id).join("workflows.json"))
        .unwrap_or_default();
    assert_eq!(before, after);
}

#[gpui::test]
fn a_look_that_was_going_when_the_app_quit_starts_nothing(cx: &mut TestAppContext) {
    let s = setup(cx);
    // One that would be failed and reported, and one that would run.
    let mut broken = quick(B, json!({"runRequested": true}));
    broken.nodes.retain(|n| n.id != "report");
    broken.edges.clear();
    put(&s, vec![quick(A, json!({"runRequested": true})), broken]);
    // The look is under way (its claim may already be written) when the quit begins.
    cx.update(|cx| s.manager.update(cx, |m, cx| m.check_workflows(cx)));
    cx.update(begin_quit);
    cx.run_until_parked();
    assert!(
        runs(&s, A).is_empty() && runs(&s, B).is_empty(),
        "no run was written"
    );
    cx.update(|cx| {
        let monos = s.manager.read(cx);
        assert!(!monos.workflows.is_running(A) && !monos.workflow_signals.is_running(A));
    });
    let rows = chat_rows(&s);
    assert!(
        rows.iter().all(|row| !row.starts_with("## Ship it")),
        "{rows:?}"
    );
}

#[gpui::test]
fn a_leftover_question_that_is_already_gone_does_not_hold_up_the_others(cx: &mut TestAppContext) {
    let s = setup(cx);
    let real = locked(&s.store)
        .unwrap()
        .create("codex", &s.project)
        .unwrap()
        .id;
    let gone = real + 1000;
    // Nothing failed: the missing one counts as deleted, the real one is deleted.
    assert_eq!(
        delete_asks(&s.store, &[gone, real]).unwrap(),
        Vec::<i64>::new()
    );
    let remaining: Vec<i64> = locked(&s.store)
        .unwrap()
        .sessions()
        .unwrap()
        .into_iter()
        .map(|session| session.id)
        .collect();
    assert!(!remaining.contains(&real));
    // And the same ids again: nothing left, nothing fails.
    assert_eq!(
        delete_asks(&s.store, &[gone, real]).unwrap(),
        Vec::<i64>::new()
    );
}

fn probes(models: fn(Kind) -> Probe) -> Probes {
    Probes {
        models,
        requirements: preflight_met,
    }
}

/// Nothing a workflow requires is ever missing.
fn preflight_met(_: &Workflow) -> monocode_core::mono::workflows::preflight::Preflight {
    Default::default()
}

fn probe_nothing(_: Kind) -> Probe {
    Probe {
        installed: true,
        models: None,
    }
}

/// A look whose background half is done but whose runs are not started yet (what the model
/// probing of a slow catalog leaves open), as the manager would be in the middle of it.
fn half_a_look(s: &Setup, cx: &mut TestAppContext) -> Looked {
    cx.update(|cx| {
        s.manager
            .update(cx, |m, _| m.workflow_checks.checking = true)
    });
    let signals = cx.update(|cx| s.manager.read(cx).workflow_signals.clone());
    look_at(
        &s.root,
        &[(s.id.clone(), true)],
        &[],
        false,
        &signals,
        probes(probe_nothing),
        monocode_core::time::now_ms(),
    )
}

#[gpui::test]
fn a_workflow_claimed_but_not_started_yet_counts_as_running(cx: &mut TestAppContext) {
    let s = setup(cx);
    let signals = cx.update(|cx| s.manager.read(cx).workflow_signals.clone());
    put(
        &s,
        vec![waiting(A, json!({"runRequested": true, "enabled": true}))],
    );
    let looked = half_a_look(&s, cx);
    assert!(signals.is_running(A), "marked at the claim");
    assert!(runs(&s, A).is_empty());
    let ask = |request: &str, action: &str, cx: &mut TestAppContext| {
        call(&s, request, action, json!({"id": A}), cx)
            .try_recv()
            .unwrap()
    };
    // It cannot be removed from under the run, run twice, or called not running.
    assert_eq!(
        ask("rm-1", "workflows.remove", cx).unwrap_err(),
        "Stop the workflow before removing it"
    );
    assert!(
        ask("run-1", "workflows.run", cx)
            .unwrap_err()
            .contains("already running")
    );
    ask("stop-1", "workflows.stop", cx).unwrap();
    // The look in progress ends: the run starts, and the stop asked for meanwhile follows it.
    cx.update(|cx| s.manager.update(cx, |m, cx| m.workflows_looked(looked, cx)));
    cx.run_until_parked();
    let history = runs(&s, A);
    assert_eq!(history.len(), 1, "one run, not two");
    assert_eq!(history[0].status, RunStatus::Stopped);
    assert!(!saved(&s).is_empty(), "still there");
    assert!(!saved(&s)[0].stop_requested);
    assert!(!signals.is_running(A));
}

#[gpui::test]
fn a_claim_that_is_not_started_gives_the_mark_back(cx: &mut TestAppContext) {
    let s = setup(cx);
    let signals = cx.update(|cx| s.manager.read(cx).workflow_signals.clone());
    put(&s, vec![waiting(A, json!({"runRequested": true}))]);
    let looked = half_a_look(&s, cx);
    assert!(signals.is_running(A));
    // The Mono is gone by the time the look ends.
    cx.update(|cx| {
        s.manager.update(cx, |m, cx| {
            m.roster.monos.clear();
            m.workflows_looked(looked, cx);
        })
    });
    assert!(!signals.is_running(A));
    assert!(runs(&s, A).is_empty());
}

#[gpui::test]
fn forgetting_a_question_session_never_writes_over_a_newer_roster(cx: &mut TestAppContext) {
    let s = setup(cx);
    let id = locked(&s.store)
        .unwrap()
        .create("codex", &s.project)
        .unwrap()
        .id;
    cx.update(|cx| {
        s.manager.update(cx, |m, cx| {
            m.roster.ask_sessions.push(id);
            m.forget_asks(vec![id], cx);
            // A change made right after, saved through the manager's ordered writes.
            m.roster.hidden_sessions.push(424_242);
            m.save(cx);
        })
    });
    cx.run_until_parked();
    let roster = monocode_core::mono::Roster::load(&locked(&s.store).unwrap()).unwrap();
    assert!(
        roster.hidden_sessions.contains(&424_242),
        "the newer write is the last"
    );
    assert!(!roster.ask_sessions.contains(&id));
}
