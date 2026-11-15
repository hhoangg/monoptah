//! Cloud steps through the real host, over the fake `gh` and machine host of the cloud tests: a
//! step starts through the same path as `sessions.start` with a target, waits through the start,
//! and a stop leaves the instance alone. No codespace or sandbox is ever created for real.
use super::*;
use crate::monos::cloud_tests::cloud_setup;
use monocode_core::cloud_sessions::{
    StepKind, queued_start_message, start_record, startup::save_start_record,
};

fn target(instance: CloudInstance, options: Value) -> Option<CloudRequest> {
    let mut options = options.as_object().cloned().unwrap_or_default();
    // The test project has no commits, so a new instance needs its branch named.
    if instance == CloudInstance::New {
        options.entry("branch").or_insert(json!("trunk"));
    }
    Some(CloudRequest {
        kind: "codespace".into(),
        instance,
        options,
    })
}

fn cloud_request(s: &Setup, instance: CloudInstance, options: Value) -> SessionRequest {
    let mut req = request(s);
    req.target = target(instance, options);
    req
}

/// Starts a cloud step through the host path, as a run does (the prompt is queued for the
/// instance, not left in a composer).
fn begin_cloud(
    s: &Setup,
    cx: &mut TestAppContext,
    req: SessionRequest,
) -> Result<StartedSession, String> {
    let (tx, rx) = async_channel::bounded(1);
    let asked = cx.update(|cx| {
        s.manager
            .update(cx, |m, cx| m.workflow_session(&s.id, req, false, tx, cx))
    });
    assert!(asked?, "the Mono's chat is loaded");
    cx.run_until_parked();
    rx.try_recv().expect("the start answered")
}

fn creates(gh: &crate::cloud_wake::fakes::Gh) -> usize {
    gh.calls
        .lock()
        .unwrap()
        .iter()
        .filter(|call| call.contains("--method POST"))
        .count()
}

fn instance_untouched_gh(gh: &crate::cloud_wake::fakes::Gh) {
    let calls = gh.calls.lock().unwrap();
    assert!(
        !calls
            .iter()
            .any(|call| call.contains("codespace stop") || call.contains("codespace delete")),
        "{calls:?}"
    );
}

/// Nothing that stops or deletes an instance was asked of `gh`, and the machine host was not
/// asked anything.
fn instance_untouched(
    gh: &crate::cloud_wake::fakes::Gh,
    host: &crate::cloud_wake::fakes::FakeHost,
) {
    instance_untouched_gh(gh);
    assert!(host.0.lock().unwrap().is_empty());
}

/// The newest launch of the Mono: the local id and the harness of the session a start made.
fn newest(s: &Setup, cx: &mut TestAppContext) -> (i64, String) {
    let launch = launches_of(s, cx).pop().expect("a launch");
    (launch.session_id, launch.harness)
}

fn project_worktrees(s: &Setup) -> usize {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(&s.project)
        .args(["worktree", "list"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).lines().count()
}

#[gpui::test]
fn a_cloud_step_starts_through_the_sessions_start_path_in_claude_without_a_worktree(
    cx: &mut TestAppContext,
) {
    let (s, gh, host, _guard) = cloud_setup(cx, "Failed");
    let started = begin_cloud(
        &s,
        cx,
        cloud_request(&s, CloudInstance::New, json!({"branch": "trunk"})),
    )
    .unwrap();
    assert_eq!(creates(&gh), 1, "one codespace is created");
    let (id, harness) = newest(&s, cx);
    assert!(id < 0, "a cloud session has a negative local id");
    assert_eq!(harness, "claude");
    assert_eq!(project_worktrees(&s), 1, "no worktree was made");
    // The first prompt waits in the store for the instance, like any API launch.
    let store = locked(&s.store).unwrap();
    assert_eq!(queued_start_message(&store, id).as_deref(), Some("Do it"));
    assert!(start_record(&store, id).is_some(), "the start is under way");
    drop(store);
    // The report can name the instance it runs on.
    let info = host_of(&s, cx).cloud_instance(&started.session_id).unwrap();
    assert_eq!(info.kind, "codespace");
    assert!(info.name.is_some());
    instance_untouched(&gh, &host);
}

#[gpui::test]
fn a_same_as_step_starts_on_the_instance_of_an_earlier_session_without_creating_another(
    cx: &mut TestAppContext,
) {
    // The fake `gh` knows one codespace, `octo-run`: the earlier session already runs on it.
    let (s, gh, host, _guard) = cloud_setup(cx, "Failed");
    let first = crate::monos::cloud_tests::cloud_session(&s, cx);
    let persisted = locked(&s.store)
        .unwrap()
        .persisted_session_id(first)
        .unwrap();
    let second = begin_cloud(
        &s,
        cx,
        cloud_request(&s, CloudInstance::OfSession(persisted.clone()), json!({})),
    )
    .unwrap();
    assert_eq!(creates(&gh), 0, "no create");
    assert_ne!(persisted, second.session_id);
    let (id, harness) = newest(&s, cx);
    assert_eq!(harness, "claude");
    let store = locked(&s.store).unwrap();
    let on = |id: i64| store.remote_session(id).unwrap().unwrap().project.cloud();
    assert_eq!(on(id), on(first));
    assert!(on(id).is_some());
    drop(store);
    assert_eq!(project_worktrees(&s), 1);
    // Connecting to the existing instance is all that was asked of the machine host.
    assert!(
        !host
            .0
            .lock()
            .unwrap()
            .iter()
            .any(|c| c.contains("stop") || c.contains("delete"))
    );
    instance_untouched_gh(&gh);
}

#[gpui::test]
fn a_target_the_mono_api_would_refuse_is_refused_before_anything_is_created(
    cx: &mut TestAppContext,
) {
    let (s, gh, host, _guard) = cloud_setup(cx, "Failed");
    let local = begin(&s, cx, request(&s)).unwrap();
    let before = session_count(&s);

    let mut codex = cloud_request(&s, CloudInstance::New, json!({}));
    codex.harness = Some("codex".into());
    let error = begin_cloud(&s, cx, codex).err().unwrap();
    assert!(error.contains("Claude only"), "{error}");

    let on_local = cloud_request(&s, CloudInstance::OfSession(local.session_id), json!({}));
    let error = begin_cloud(&s, cx, on_local).err().unwrap();
    assert!(error.contains("not a cloud session"), "{error}");

    let options_on_existing = {
        let first = crate::monos::cloud_tests::cloud_session(&s, cx);
        let persisted = locked(&s.store)
            .unwrap()
            .persisted_session_id(first)
            .unwrap();
        cloud_request(
            &s,
            CloudInstance::OfSession(persisted),
            json!({"branch": "trunk"}),
        )
    };
    let error = begin_cloud(&s, cx, options_on_existing).err().unwrap();
    assert!(error.contains("only applies when creating"), "{error}");

    // A draft is a prompt left in the composer, which a cloud session has no use for.
    let (tx, _rx) = async_channel::bounded(1);
    let draft = cx.update(|cx| {
        s.manager.update(cx, |m, cx| {
            m.workflow_session(
                &s.id,
                cloud_request(&s, CloudInstance::New, json!({})),
                true,
                tx,
                cx,
            )
        })
    });
    assert!(draft.unwrap_err().contains("draft"));

    // Only the one session that was made on purpose exists on top of the local one.
    assert_eq!(session_count(&s), before + 1);
    assert_eq!(creates(&gh), 0);
    instance_untouched_gh(&gh);
    let _ = &host;
}

#[gpui::test]
fn a_session_whose_instance_is_still_starting_is_waited_for_and_never_given_up(
    cx: &mut TestAppContext,
) {
    let (s, gh, host, _guard) = cloud_setup(cx, "Failed");
    let started = begin_cloud(&s, cx, cloud_request(&s, CloudInstance::New, json!({}))).unwrap();
    let waiting = collect(cx, started.settled);
    cx.run_until_parked();
    // Far longer than a turn that never begins is waited for (ten looks of a second).
    for _ in 0..30 {
        cx.executor().advance_clock(Duration::from_secs(10));
        cx.run_until_parked();
    }
    assert!(
        waiting.borrow().is_none(),
        "still waiting: {:?}",
        waiting.borrow()
    );
    instance_untouched(&gh, &host);
}

#[gpui::test]
fn a_start_that_fails_ends_the_wait_with_its_error(cx: &mut TestAppContext) {
    let (s, gh, host, _guard) = cloud_setup(cx, "Failed");
    let started = begin_cloud(&s, cx, cloud_request(&s, CloudInstance::New, json!({}))).unwrap();
    let (id, _) = newest(&s, cx);
    let waiting = collect(cx, started.settled);
    cx.run_until_parked();
    assert!(waiting.borrow().is_none());
    {
        let store = locked(&s.store).unwrap();
        let mut record = start_record(&store, id).unwrap();
        record.error = Some("SSH setup failed: no route".into());
        record.failed_at = Some(StepKind::Create);
        save_start_record(&store, &record).unwrap();
    }
    cx.executor().advance_clock(Duration::from_secs(2));
    cx.run_until_parked();
    let outcome = waiting.borrow().clone().expect("the wait ended");
    assert_eq!(outcome.status, Status::Failed);
    assert_eq!(
        outcome.error.as_deref(),
        Some("The cloud instance could not start: SSH setup failed: no route")
    );
    instance_untouched(&gh, &host);
}

#[gpui::test]
fn stopping_a_session_that_is_still_starting_drops_its_queued_prompt_and_leaves_the_instance(
    cx: &mut TestAppContext,
) {
    let (s, gh, host, _guard) = cloud_setup(cx, "Failed");
    let started = begin_cloud(&s, cx, cloud_request(&s, CloudInstance::New, json!({}))).unwrap();
    let (id, _) = newest(&s, cx);
    let owner = cx.update(|cx| s.manager.read(cx).owner.upgrade().unwrap());
    let view = cx
        .update(|cx| owner.update(cx, |o, _| o.mono_session_entity(id)))
        .unwrap();
    cx.update(|cx| {
        assert_eq!(view.read(cx).cloud_queued(), Some("Do it"));
    });
    let stop = host_of(&s, cx).stop(&started.session_id);
    let stopped = collect(cx, stop);
    cx.run_until_parked();
    assert_eq!(stopped.borrow().clone(), Some(Ok(())));
    assert_eq!(
        queued_start_message(&locked(&s.store).unwrap(), id),
        None,
        "the first prompt is not sent once the instance is ready"
    );
    cx.update(|cx| {
        assert_eq!(view.read(cx).cloud_queued(), None);
        assert!(view.read(cx).remote_debug().contains("waiting=false"));
    });
    // The start itself goes on (it is not ours to cancel) and the instance is left as it is.
    assert!(start_record(&locked(&s.store).unwrap(), id).is_some());
    instance_untouched(&gh, &host);
    // Stopping again changes nothing.
    let stop = host_of(&s, cx).stop(&started.session_id);
    let again = collect(cx, stop);
    cx.run_until_parked();
    assert_eq!(again.borrow().clone(), Some(Ok(())));
}

#[gpui::test]
fn stopping_a_local_session_is_never_a_cloud_stop(cx: &mut TestAppContext) {
    // A local session has no start and no instance: stopping it only stops its turn.
    let (s, _gh, _host, _guard) = cloud_setup(cx, "Failed");
    let started = begin(&s, cx, request(&s)).unwrap();
    let id = local(&started);
    let stop = host_of(&s, cx).stop(&started.session_id);
    let stopped = collect(cx, stop);
    cx.run_until_parked();
    assert_eq!(stopped.borrow().clone(), Some(Ok(())));
    let owner = cx.update(|cx| s.manager.read(cx).owner.upgrade().unwrap());
    cx.update(|cx| {
        let view = owner.read(cx).mono_session_entity(id).unwrap();
        assert!(!view.read(cx).is_cloud_session());
    });
}

// --- whole runs ------------------------------------------------------------------------------

fn cloud_workflow(s: &Setup) -> Workflow {
    workflow_with(
        vec![
            trigger(),
            session(
                "build",
                json!({}),
                json!({"target": {"kind": "codespace", "branch": "trunk"}}),
            ),
            session(
                "check",
                json!({}),
                json!({"target": {"kind": "codespace", "instance": {"sameAs": "build"}}}),
            ),
            report(),
        ],
        vec![
            edge("trigger", "build"),
            edge("build", "check"),
            edge("check", "report"),
        ],
        json!({"repo": s.project, "lib": s.project}),
    )
}

fn start_cloud_run(s: &Setup, cx: &mut TestAppContext) {
    let run = start_run(&cloud_workflow(s), RunTrigger::Manual, 1_000, "run-1");
    let started = cx.update(|cx| {
        s.manager
            .update(cx, |m, cx| m.start_workflow_run(&s.id, run, cx))
    });
    assert!(started);
    cx.run_until_parked();
}

fn stored_run(s: &Setup) -> WorkflowRun {
    load_runs(&MonoDir::new(&s.root, &s.id), WF)
        .unwrap()
        .remove(0)
}

#[gpui::test]
fn a_run_waits_through_the_start_of_its_instance_and_a_stop_leaves_the_instance_alone(
    cx: &mut TestAppContext,
) {
    let (s, gh, host, _guard) = cloud_setup(cx, "Failed");
    start_cloud_run(&s, cx);
    assert_eq!(creates(&gh), 1, "the first step created its codespace");
    let (id, harness) = newest(&s, cx);
    assert_eq!(harness, "claude");
    assert_eq!(project_worktrees(&s), 1);
    // Minutes pass while the instance starts: the node goes on waiting.
    for _ in 0..30 {
        cx.executor().advance_clock(Duration::from_secs(10));
        cx.run_until_parked();
    }
    let run = stored_run(&s);
    assert_eq!(run.status, RunStatus::Running);
    assert_eq!(node(&run, "build").status, NodeStatus::Running);
    assert_eq!(
        launches_of(&s, cx).len(),
        1,
        "the second step waits for the first"
    );

    let stopped = cx.update(|cx| s.manager.update(cx, |m, _| m.stop_workflow_run(WF)));
    assert!(stopped);
    cx.run_until_parked();
    assert_eq!(stored_run(&s).status, RunStatus::Stopped);
    assert_eq!(queued_start_message(&locked(&s.store).unwrap(), id), None);
    instance_untouched(&gh, &host);
    assert_eq!(creates(&gh), 1);
    // The report in the Mono's chat lists the instance and says it was left alone.
    let rows = chat_rows(&s);
    let report = rows
        .iter()
        .find(|r| r.starts_with("## Ship it · stopped"))
        .unwrap_or_else(|| panic!("{rows:?}"));
    assert!(report.contains("Cloud instances this run used"), "{report}");
    assert!(report.contains("Codespace"), "{report}");
    assert!(
        report.contains("created by this run; step build"),
        "{report}"
    );
    assert!(report.contains("Stop or delete them from Cloud workspaces"));
}

#[gpui::test]
fn a_run_whose_instance_fails_to_start_fails_the_node_with_the_error(cx: &mut TestAppContext) {
    let (s, gh, host, _guard) = cloud_setup(cx, "Failed");
    start_cloud_run(&s, cx);
    let (id, _) = newest(&s, cx);
    {
        let store = locked(&s.store).unwrap();
        let mut record = start_record(&store, id).unwrap();
        record.error = Some("no capacity".into());
        record.failed_at = Some(StepKind::Provision);
        save_start_record(&store, &record).unwrap();
    }
    cx.executor().advance_clock(Duration::from_secs(4));
    cx.run_until_parked();
    let run = stored_run(&s);
    assert_eq!(run.status, RunStatus::Failed);
    let build = node(&run, "build");
    assert_eq!(build.status, NodeStatus::Failed);
    assert_eq!(
        build.error.as_deref(),
        Some("The cloud instance could not start: no capacity")
    );
    assert_ne!(node(&run, "check").status, NodeStatus::Done);
    // The step's first prompt is dropped with the node: a later Retry of the start in the
    // window would otherwise run it outside any run.
    assert_eq!(queued_start_message(&locked(&s.store).unwrap(), id), None);
    assert_eq!(creates(&gh), 1);
    instance_untouched(&gh, &host);
    // Not stopped or deleted at the end of a failed run either, and the report says where it is.
    let rows = chat_rows(&s);
    assert!(
        rows.iter()
            .any(|r| r.contains("Cloud instances this run used")),
        "{rows:?}"
    );
}
