//! H-10 and H-12: instance lifecycle for web devices (stop, resume, retry,
//! keep running, wake on send, archive, delete, auto-stop notices) over a
//! shell with the shared cloud service installed over counting fakes.
use super::*;
use crate::cloud_wake::fakes::{AUTH_OK, Dir, FakeHost, Gh, github_project, install};
use crate::shell::cloud::CloudMark;
use gpui::TestAppContext;
use monocode_core::{
    cloud_sessions::{CloudBackendKind, CloudRef, CloudState},
    mobile::{DeviceKind, Permission, cloud_policy},
};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

const CS: &str = "octo-run";

fn device_of(id: &str, kind: DeviceKind, projects: Option<Vec<String>>) -> DeviceContext {
    DeviceContext {
        id: id.into(),
        kind,
        permission: Permission::Control,
        projects,
        monos: None,
    }
}

/// The limiter is process-wide and tests run side by side: every fixture has its own devices.
fn fresh_id(kind: &str) -> String {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    format!("{kind}-{}", NEXT.fetch_add(1, Ordering::Relaxed))
}

struct Fx {
    /// Id of this fixture's web device.
    id: String,
    window: gpui::WindowHandle<Shell>,
    /// Holds the lock of the process-wide cloud service and removes the project.
    repo: Dir,
    gh: Arc<Gh>,
    host: Arc<FakeHost>,
    /// The codespace session and a local one, both in the project.
    cs: i64,
    local: i64,
}

impl Fx {
    fn web(&self) -> DeviceContext {
        device_of(&self.id, DeviceKind::Web, None)
    }
    fn phone(&self) -> DeviceContext {
        device_of(&format!("{}-phone", self.id), DeviceKind::Phone, None)
    }
    fn project(&self) -> String {
        self.repo.0.to_string_lossy().into_owned()
    }
    fn key() -> String {
        CloudRef::codespace(CS).key()
    }
}

/// A shell whose rail has a GitHub project with one codespace session on the
/// fake codespace `octo-run` (cached state `cache`; `gh codespace view` says
/// `view`) and one local session.
fn fixture(cx: &mut TestAppContext, view: &'static str, cache: CloudState) -> Fx {
    crate::conversation::Conversation::reset_wake_sends();
    let (window, _, _) = crate::shell::tests::window(cx);
    let repo = github_project();
    let project = repo.0.to_string_lossy().into_owned();
    let (gh, host, cs, local) = window
        .update(cx, |shell, _, cx| {
            let (gh, host) = install(shell.store.clone(), AUTH_OK, view);
            shell.workspace.add_project(&project);
            let (cs, local) = {
                let store = locked(&shell.store).unwrap();
                let remote = store
                    .remember_cloud_project(
                        &format!("env-{CS}"),
                        &json!({"id":"p","cwd":"/workspaces/widgets"}),
                        &project,
                        CS,
                    )
                    .unwrap();
                let mut cs = store.create("claude", &remote.key).unwrap();
                cs.has_messages = true;
                let mut local = store.create("claude", &project).unwrap();
                local.has_messages = true;
                (cs, local)
            };
            let (cs_id, local_id) = (cs.id, local.id);
            for summary in [cs, local] {
                shell.order.push(summary.id);
                shell.session_summaries.insert(summary.id, summary);
            }
            let mark = CloudMark {
                local_project: project.clone(),
                codespace: CS.into(),
                kind: CloudBackendKind::Codespaces,
            };
            shell.cloud_states.insert(mark.key(), cache);
            shell.cloud_marks.insert(cs_id, mark);
            shell.ensure_session(cs_id, cx).expect("conversation");
            (gh, host, cs_id, local_id)
        })
        .unwrap();
    gh.calls.lock().unwrap().clear();
    host.0.lock().unwrap().clear();
    Fx {
        id: fresh_id("web"),
        window,
        repo,
        gh,
        host,
        cs,
        local,
    }
}

/// Runs `method` through the deferred dispatch and returns its answer.
fn call(
    fx: &Fx,
    cx: &mut TestAppContext,
    device: &DeviceContext,
    method: &str,
    params: Value,
) -> Result<Value, Failure> {
    let answer: Rc<RefCell<Option<Result<Value, Failure>>>> = Rc::default();
    let slot = answer.clone();
    fx.window
        .update(cx, |shell, _, cx| {
            cloud::dispatch_deferred(
                shell,
                device,
                method,
                &params,
                move |_, result| *slot.borrow_mut() = Some(result),
                cx,
            )
        })
        .unwrap();
    cx.run_until_parked();
    answer.take().expect("the call was answered")
}

fn persisted(fx: &Fx, cx: &mut TestAppContext, id: i64) -> String {
    fx.window
        .update(cx, |shell, _, _| shell.mobile_session_id(id))
        .unwrap()
}

fn code_of(failure: &Failure) -> Option<&str> {
    failure.code.as_deref()
}

fn count(gh: &Gh, needle: &str) -> usize {
    gh.calls
        .lock()
        .unwrap()
        .iter()
        .filter(|call| call.contains(needle))
        .count()
}

/// Any call that starts, stops or creates something.
const CHANGES: [&str; 3] = ["codespace stop", "codespace ssh", "api --method POST"];

fn changes(gh: &Gh) -> usize {
    CHANGES.iter().map(|needle| count(gh, needle)).sum()
}

fn before_send(
    fx: &Fx,
    cx: &mut TestAppContext,
    device: &DeviceContext,
    params: Value,
) -> Result<cloud::SendPlan, Failure> {
    let id = fx.cs;
    fx.window
        .update(cx, |shell, _, cx| {
            cloud::before_send(shell, device, Some(id), &params, cx)
        })
        .unwrap()
}

/// `conversation.send` for `device` through the host's own path (`mobile_send_for`): the
/// checks, the staging, the wake and the submit, answered like the wire reply.
fn send(
    fx: &Fx,
    cx: &mut TestAppContext,
    device: &DeviceContext,
    session: i64,
    params: Value,
) -> Result<Value, Failure> {
    let wire = persisted(fx, cx, session);
    let mut params = params;
    params["conversationId"] = json!(wire);
    if params.get("text").is_none() {
        params["text"] = json!("hello");
    }
    let answer: Rc<RefCell<Option<Result<Value, Failure>>>> = Rc::default();
    let slot = answer.clone();
    fx.window
        .update(cx, |shell, _, cx| {
            shell.mobile_send_for(
                device.clone(),
                params,
                move |_, result| *slot.borrow_mut() = Some(result),
                cx,
            )
        })
        .unwrap();
    cx.run_until_parked();
    answer.take().expect("the send was answered")
}

/// Marks the codespace session's view as loaded, with its link down.
fn load_view(fx: &Fx, cx: &mut TestAppContext) {
    let cs = fx.cs;
    fx.window
        .update(cx, |shell, _, cx| {
            shell.sessions[&cs].update(cx, |chat, _| chat.loaded_for_test());
        })
        .unwrap();
}

/// Brings the codespace session's link up or down.
fn connect(fx: &Fx, cx: &mut TestAppContext, online: bool) {
    let cs = fx.cs;
    fx.window
        .update(cx, |shell, _, cx| {
            shell.sessions[&cs].update(cx, |chat, _| chat.set_online_for_test(online));
        })
        .unwrap();
}

/// The prompts the session's view sent to its workspace.
fn launched(fx: &Fx, cx: &mut TestAppContext) -> Vec<String> {
    let cs = fx.cs;
    fx.window
        .update(cx, |shell, _, cx| {
            shell.sessions[&cs].read(cx).launched_for_test()
        })
        .unwrap()
}

fn before_send_of(
    fx: &Fx,
    cx: &mut TestAppContext,
    device: &DeviceContext,
    id: i64,
    params: Value,
) -> Result<cloud::SendPlan, Failure> {
    fx.window
        .update(cx, |shell, _, cx| {
            cloud::before_send(shell, device, Some(id), &params, cx)
        })
        .unwrap()
}

fn set_state(fx: &Fx, cx: &mut TestAppContext, state: CloudState) {
    fx.window
        .update(cx, |shell, _, _| {
            shell.cloud_states.insert(Fx::key(), state);
        })
        .unwrap();
}

/// What the desktop leaves on a record when its start failed.
fn fail(record: &mut monocode_core::cloud_sessions::StartRecord, error: String) {
    record.error = Some(error);
    record.failed_at = Some(monocode_core::cloud_sessions::StepKind::Create);
    record.finished_at = Some(5_000);
}

/// A start record of the codespace session, running or failed.
fn save_start(fx: &Fx, cx: &mut TestAppContext, failed: bool) {
    let id = fx.cs;
    let project = fx.project();
    fx.window
        .update(cx, |shell, _, _| {
            let mut record = crate::cloud_start_view::fixtures::record(2);
            record.session_id = id;
            record.local_project = project;
            record.codespace = CS.into();
            if failed {
                fail(&mut record, "The create step failed".into());
            }
            monocode_core::cloud_sessions::startup::save_start_record(
                &locked(&shell.store).unwrap(),
                &record,
            )
            .unwrap();
        })
        .unwrap();
}

#[gpui::test]
fn instances_are_served_from_the_cache_and_a_miss_loads_in_the_background(cx: &mut TestAppContext) {
    let fx = fixture(cx, "Shutdown", CloudState::Stopped);
    // S5: the first read finds nothing cached. It answers at once, without the
    // instances yet, and says it is loading; the load runs after the answer.
    let answer: Rc<RefCell<Option<Result<Value, Failure>>>> = Rc::default();
    let slot = answer.clone();
    let device = fx.web();
    fx.window
        .update(cx, |shell, _, cx| {
            cloud::dispatch_deferred(
                shell,
                &device,
                "cloud.instances",
                &json!({}),
                move |_, result| *slot.borrow_mut() = Some(result),
                cx,
            )
        })
        .unwrap();
    cx.executor().run_until_parked();
    let first = answer.take().expect("answered").unwrap();
    assert_eq!(first["loading"], true);
    assert_eq!(first["instances"], json!([]));
    let after_load = fx.gh.calls.lock().unwrap().len();
    assert!(
        after_load > 0,
        "the overview was loaded once, in the background"
    );
    // The next read is answered from the cache: no outside call at all.
    let second = call(&fx, cx, &fx.web(), "cloud.instances", json!({})).unwrap();
    assert!(second.get("loading").is_none(), "{second}");
    for _ in 0..5 {
        let again = call(&fx, cx, &fx.web(), "cloud.instances", json!({})).unwrap();
        assert_eq!(again, second);
    }
    assert_eq!(
        fx.gh.calls.lock().unwrap().len(),
        after_load,
        "{:?}",
        fx.gh.calls.lock().unwrap()
    );
    assert_eq!(changes(&fx.gh), 0, "listing never wakes anything");
    assert!(fx.host.0.lock().unwrap().is_empty());

    let instance = &second["instances"][0];
    assert_eq!(instance["kind"], "codespace");
    assert_eq!(instance["instance"], CS);
    assert_eq!(instance["state"], "stopped");
    assert!(instance["displayName"].is_string());
    assert_eq!(
        instance["sessions"],
        json!([{
            "sessionId": persisted(&fx, cx, fx.cs),
            "title": instance["sessions"][0]["title"],
            "busy": false,
        }])
    );
    assert!(second["backends"].as_array().unwrap().len() == 2);
    assert!(second.get("otherCount").is_none());
}

#[gpui::test]
fn reads_are_rate_limited_and_a_phone_has_no_cloud_methods(cx: &mut TestAppContext) {
    let fx = fixture(cx, "Shutdown", CloudState::Stopped);
    for _ in 0..cloud_policy::READ_LIMIT_PER_MINUTE {
        call(&fx, cx, &fx.web(), "cloud.instances", json!({})).unwrap();
    }
    let refused = call(&fx, cx, &fx.web(), "cloud.instances", json!({})).unwrap_err();
    assert_eq!(code_of(&refused), Some("busy"));
    // Another device has its own budget.
    let other = device_of(&format!("{}-2", fx.id), DeviceKind::Web, None);
    call(&fx, cx, &other, "cloud.instances", json!({})).unwrap();
    for method in ["cloud.instances", "cloud.stop", "cloud.resume"] {
        let refused = call(&fx, cx, &fx.phone(), method, json!({})).unwrap_err();
        assert_eq!(code_of(&refused), Some("unsupported"), "{method}");
    }
    // The synchronous dispatch answers neither of the two worker methods.
    fx.window
        .update(cx, |shell, _, cx| {
            for method in ["cloud.instances", "cloud.stop"] {
                assert_eq!(
                    cloud::dispatch(shell, &fx.web(), method, &json!({}), cx),
                    Err(policy::Code::Unsupported.failure())
                );
            }
        })
        .unwrap();
}

#[gpui::test]
fn a_device_with_a_selection_lists_only_its_projects_instances(cx: &mut TestAppContext) {
    let fx = fixture(cx, "Shutdown", CloudState::Stopped);
    // A first read loads the overview in the background; the checks read the cache.
    call(&fx, cx, &fx.web(), "cloud.instances", json!({})).unwrap();
    let elsewhere = device_of(
        &format!("{}-3", fx.id),
        DeviceKind::Web,
        Some(vec![project_id("/work/another-project")]),
    );
    let none = call(&fx, cx, &elsewhere, "cloud.instances", json!({})).unwrap();
    assert_eq!(none["instances"], json!([]));
    let selected = device_of(
        &format!("{}-4", fx.id),
        DeviceKind::Web,
        Some(vec![project_id(&fx.project())]),
    );
    let some = call(&fx, cx, &selected, "cloud.instances", json!({})).unwrap();
    assert_eq!(some["instances"].as_array().unwrap().len(), 1);
}

#[gpui::test]
fn stop_asks_the_instance_to_stop_and_names_the_sessions_it_interrupted(cx: &mut TestAppContext) {
    let fx = fixture(cx, "Available", CloudState::Available);
    // A turn is running on the instance.
    let cs = fx.cs;
    fx.window
        .update(cx, |shell, _, _| {
            let store = locked(&shell.store).unwrap();
            let mut record = store.remote_session(cs).unwrap().unwrap();
            record.running = true;
            record.flags_at = monocode_core::time::now_ms();
            store.save_remote_session(&record).unwrap();
        })
        .unwrap();
    let id = persisted(&fx, cx, cs);
    let reply = call(&fx, cx, &fx.web(), "cloud.stop", json!({"sessionId": id})).unwrap();
    assert_eq!(reply["stopped"], true);
    assert_eq!(reply["kind"], "codespace");
    assert_eq!(reply["instance"], CS);
    assert_eq!(reply["sessions"][0]["sessionId"], id.as_str());
    assert_eq!(reply["sessions"][0]["busy"], true);
    let warning = reply["warning"].as_str().expect("a warning");
    assert!(
        warning.contains("were interrupted")
            && warning.contains(&format!(
                "\"{}\"",
                reply["sessions"][0]["title"].as_str().unwrap()
            )),
        "{warning}"
    );
    assert_eq!(
        count(&fx.gh, "codespace stop"),
        1,
        "{:?}",
        fx.gh.calls.lock().unwrap()
    );
    // Stop is only lightly limited: a loop of them is not.
    for _ in 0..8 {
        call(&fx, cx, &fx.web(), "cloud.stop", json!({"sessionId": id})).unwrap();
    }
}

/// L3: a stop by name needs a name the app knows, and a device may stop only so often.
#[gpui::test]
fn a_stop_by_name_needs_a_known_instance_and_is_limited_per_device(cx: &mut TestAppContext) {
    let fx = fixture(cx, "Available", CloudState::Available);
    // A codespace the app never listed and no session uses: not ours to stop.
    let refused = call(
        &fx,
        cx,
        &fx.web(),
        "cloud.stop",
        json!({"kind": "codespace", "instance": "somebody-elses"}),
    )
    .unwrap_err();
    assert_eq!(code_of(&refused), Some("unsupported_target"));
    assert_eq!(changes(&fx.gh), 0, "nothing was called for a stranger");
    // A listed one is known (an orphan billing instance is listed, not marked).
    fx.window
        .update(cx, |shell, _, _| {
            shell.cloud_marks.clear();
        })
        .unwrap();
    call(&fx, cx, &fx.web(), "cloud.instances", json!({})).unwrap();
    let listed = call(&fx, cx, &fx.web(), "cloud.instances", json!({})).unwrap();
    assert_eq!(listed["instances"][0]["instance"], CS, "{listed}");
    let reply = call(
        &fx,
        cx,
        &fx.web(),
        "cloud.stop",
        json!({"kind": "codespace", "instance": CS}),
    )
    .unwrap();
    assert_eq!(reply["stopped"], true);
    // The eleventh stop in a minute is busy; another device has its own budget.
    for _ in 1..cloud_policy::STOP_LIMIT_PER_MINUTE {
        call(
            &fx,
            cx,
            &fx.web(),
            "cloud.stop",
            json!({"kind": "codespace", "instance": CS}),
        )
        .unwrap();
    }
    let refused = call(
        &fx,
        cx,
        &fx.web(),
        "cloud.stop",
        json!({"kind": "codespace", "instance": CS}),
    )
    .unwrap_err();
    assert_eq!(code_of(&refused), Some("busy"));
    let other = device_of(&format!("{}-6", fx.id), DeviceKind::Web, None);
    call(
        &fx,
        cx,
        &other,
        "cloud.stop",
        json!({"kind": "codespace", "instance": CS}),
    )
    .unwrap();
}

/// S5: the stop answers at once, whatever the backend takes; the stop runs after the answer.
#[gpui::test]
fn a_stop_is_answered_before_the_worker_stops_anything(cx: &mut TestAppContext) {
    let fx = fixture(cx, "Available", CloudState::Available);
    let id = persisted(&fx, cx, fx.cs);
    let answer: Rc<RefCell<Option<Result<Value, Failure>>>> = Rc::default();
    let slot = answer.clone();
    let device = fx.web();
    fx.window
        .update(cx, |shell, _, cx| {
            cloud::dispatch_deferred(
                shell,
                &device,
                "cloud.stop",
                &json!({"sessionId": id}),
                move |_, result| *slot.borrow_mut() = Some(result),
                cx,
            );
            assert_eq!(
                shell.cloud_states.get(&Fx::key()),
                Some(&CloudState::Available)
            );
        })
        .unwrap();
    // Answered in the same turn, before any worker ran: nothing was called yet.
    let reply = answer.take().expect("answered at once").unwrap();
    assert_eq!(reply["stopped"], true);
    assert_eq!(count(&fx.gh, "codespace stop"), 0);
    cx.run_until_parked();
    assert_eq!(
        count(&fx.gh, "codespace stop"),
        1,
        "the worker stops it after"
    );
}

#[gpui::test]
fn a_kept_instance_can_be_stopped_by_its_name(cx: &mut TestAppContext) {
    let fx = fixture(cx, "Available", CloudState::Available);
    save_start(&fx, cx, true);
    let cs = fx.cs;
    fx.window
        .update(cx, |shell, _, cx| {
            let fields = cloud::read_fields(shell, cs, &fx.web(), cx);
            assert_eq!(fields["cloud"]["kept"], true);
            assert!(fields["cloud"]["startup"]["error"].is_string());
        })
        .unwrap();
    let reply = call(
        &fx,
        cx,
        &fx.web(),
        "cloud.stop",
        json!({"kind": "codespace", "instance": CS}),
    )
    .unwrap();
    assert_eq!(reply["stopped"], true);
    assert_eq!(count(&fx.gh, "codespace stop"), 1);
    // A start that is only running is not kept.
    save_start(&fx, cx, false);
    fx.window
        .update(cx, |shell, _, cx| {
            let fields = cloud::read_fields(shell, cs, &fx.web(), cx);
            assert!(fields["cloud"].get("kept").is_none());
        })
        .unwrap();
}

#[gpui::test]
fn a_stop_is_refused_while_the_desktop_works_on_the_instance_or_it_is_not_selected(
    cx: &mut TestAppContext,
) {
    let fx = fixture(cx, "Available", CloudState::Available);
    let id = persisted(&fx, cx, fx.cs);
    // Archiving or converting on the instance.
    fx.window
        .update(cx, |shell, _, _| {
            shell.cloud_workspaces_busy.insert(Fx::key());
        })
        .unwrap();
    for params in [
        json!({"sessionId": id}),
        json!({"kind": "codespace", "instance": CS}),
    ] {
        let refused = call(&fx, cx, &fx.web(), "cloud.stop", params).unwrap_err();
        assert_eq!(code_of(&refused), Some("instance_busy"));
    }
    // The desktop stops it on its own.
    fx.window
        .update(cx, |shell, _, _| {
            shell.cloud_workspaces_busy.remove(&Fx::key());
            shell.auto_stop_running.insert(Fx::key());
        })
        .unwrap();
    let refused = call(&fx, cx, &fx.web(), "cloud.stop", json!({"sessionId": id})).unwrap_err();
    assert_eq!(code_of(&refused), Some("auto_stopping"));
    fx.window
        .update(cx, |shell, _, _| {
            shell.auto_stop_running.remove(&Fx::key());
        })
        .unwrap();
    assert_eq!(count(&fx.gh, "codespace stop"), 0);

    // An instance no selected project uses is not selected, by session or by name.
    let elsewhere = device_of(
        &format!("{}-5", fx.id),
        DeviceKind::Web,
        Some(vec![project_id("/work/another-project")]),
    );
    for params in [
        json!({"sessionId": id}),
        json!({"kind": "codespace", "instance": CS}),
    ] {
        let refused = call(&fx, cx, &elsewhere, "cloud.stop", params).unwrap_err();
        assert_eq!(code_of(&refused), Some("not_selected"));
    }
    // A local session is not an instance.
    let local = persisted(&fx, cx, fx.local);
    let refused = call(
        &fx,
        cx,
        &fx.web(),
        "cloud.stop",
        json!({"sessionId": local}),
    )
    .unwrap_err();
    assert_eq!(code_of(&refused), Some("unsupported_target"));
    assert_eq!(count(&fx.gh, "codespace stop"), 0);
}

#[gpui::test]
fn a_read_only_device_cannot_use_any_control_method(cx: &mut TestAppContext) {
    let _ = cx;
    let reader = DeviceContext {
        permission: Permission::Read,
        ..device_of("reader", DeviceKind::Web, None)
    };
    for method in [
        "cloud.stop",
        "cloud.resume",
        "cloud.retryStart",
        "cloud.keepRunning",
        "conversation.send",
        "session.archive",
        "session.delete",
    ] {
        let denied = policy::authorize(&reader, method, &json!({})).unwrap_err();
        assert_eq!(denied.code, policy::Code::ReadOnly, "{method}");
    }
    assert!(policy::authorize(&reader, "cloud.instances", &json!({})).is_ok());
}

#[gpui::test]
fn resume_answers_at_once_and_wakes_on_a_worker_within_the_rate_limit(cx: &mut TestAppContext) {
    let fx = fixture(cx, "Shutdown", CloudState::Stopped);
    let id = persisted(&fx, cx, fx.cs);
    let device = fx.web();
    let answered: Rc<RefCell<Option<Result<Value, Failure>>>> = Rc::default();
    let slot = answered.clone();
    fx.window
        .update(cx, |shell, _, cx| {
            cloud::dispatch_deferred(
                shell,
                &device,
                "cloud.resume",
                &json!({"sessionId": id}),
                move |_, result| *slot.borrow_mut() = Some(result),
                cx,
            )
        })
        .unwrap();
    // Answered before any worker ran.
    assert_eq!(answered.take().unwrap().unwrap(), json!({"accepted": true}));
    assert_eq!(
        cloud_policy::limiter().in_flight(&fx.id, std::time::Instant::now()),
        1
    );
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_secs(5));
    cx.run_until_parked();
    assert!(
        fx.host
            .0
            .lock()
            .unwrap()
            .iter()
            .any(|call| call.starts_with("ensure")),
        "{:?} {:?}",
        fx.host.0.lock().unwrap(),
        fx.gh.calls.lock().unwrap()
    );
    assert_eq!(
        cloud_policy::limiter().in_flight(&fx.id, std::time::Instant::now()),
        0,
        "the slot is released when the wake ends"
    );
}

#[gpui::test]
fn resume_and_retry_count_in_the_rate_limit_the_seventh_call_and_third_slot_are_busy(
    cx: &mut TestAppContext,
) {
    let fx = fixture(cx, "Shutdown", CloudState::Stopped);
    let id = persisted(&fx, cx, fx.cs);
    // Two in flight: a third wake is busy, whatever kind it is.
    let first = cloud_policy::acquire_wake(&fx.id).unwrap();
    let second = cloud_policy::acquire_wake(&fx.id).unwrap();
    let refused = call(&fx, cx, &fx.web(), "cloud.resume", json!({"sessionId": id})).unwrap_err();
    assert_eq!(code_of(&refused), Some("busy"));
    cloud_policy::release_wake(&first);
    cloud_policy::release_wake(&second);
    assert_eq!(changes(&fx.gh), 0, "a refused call wakes nothing");
    // Six in the hour: the seventh is busy.
    cloud_policy::limiter().forget(&fx.id);
    for _ in 0..cloud_policy::WAKE_LIMIT_PER_HOUR {
        let slot = cloud_policy::acquire_wake(&fx.id).unwrap();
        cloud_policy::release_wake(&slot);
    }
    let refused = call(&fx, cx, &fx.web(), "cloud.resume", json!({"sessionId": id})).unwrap_err();
    assert_eq!(code_of(&refused), Some("busy"));
    save_start(&fx, cx, true);
    let refused = call(
        &fx,
        cx,
        &fx.web(),
        "cloud.retryStart",
        json!({"sessionId": id}),
    )
    .unwrap_err();
    assert_eq!(code_of(&refused), Some("busy"));
    assert!(fx.host.0.lock().unwrap().is_empty());
}

#[gpui::test]
fn retry_start_runs_a_failed_start_again_and_only_a_failed_one(cx: &mut TestAppContext) {
    let fx = fixture(cx, "Shutdown", CloudState::Stopped);
    let id = persisted(&fx, cx, fx.cs);
    let cs = fx.cs;
    // No start at all: not applicable.
    let refused = call(
        &fx,
        cx,
        &fx.web(),
        "cloud.retryStart",
        json!({"sessionId": id}),
    )
    .unwrap_err();
    assert_eq!(code_of(&refused), Some("unsupported_target"));
    // A local session: not applicable either.
    let local = persisted(&fx, cx, fx.local);
    let refused = call(
        &fx,
        cx,
        &fx.web(),
        "cloud.retryStart",
        json!({"sessionId": local}),
    )
    .unwrap_err();
    assert_eq!(code_of(&refused), Some("unsupported_target"));
    assert_eq!(
        cloud_policy::limiter().calls_this_hour(&fx.id, std::time::Instant::now()),
        0
    );

    save_start(&fx, cx, true);
    let accepted = call(
        &fx,
        cx,
        &fx.web(),
        "cloud.retryStart",
        json!({"sessionId": id}),
    )
    .unwrap();
    assert_eq!(accepted, json!({"accepted": true}));
    fx.window
        .update(cx, |shell, _, _| {
            assert!(shell.mobile_start_running(cs), "the start follows again");
        })
        .unwrap();
    assert_eq!(
        cloud_policy::limiter().in_flight(&fx.id, std::time::Instant::now()),
        1
    );
    // While it runs, the instance is starting: a second retry is refused.
    let refused = call(
        &fx,
        cx,
        &fx.web(),
        "cloud.retryStart",
        json!({"sessionId": id}),
    )
    .unwrap_err();
    assert_eq!(code_of(&refused), Some("starting"));
    // The slot is released when the start's worker is over.
    fx.window
        .update(cx, |shell, _, _| shell.drop_cloud_start(cs))
        .unwrap();
    cx.executor().advance_clock(Duration::from_secs(3));
    cx.run_until_parked();
    assert_eq!(
        cloud_policy::limiter().in_flight(&fx.id, std::time::Instant::now()),
        0
    );
}

/// L4: a retry that has to create the instance is a cloud start (needs the toggle);
/// a retry on a kept instance is not.
#[gpui::test]
fn a_retry_that_creates_an_instance_needs_remote_cloud_start(cx: &mut TestAppContext) {
    let fx = fixture(cx, "Shutdown", CloudState::Stopped);
    let id = persisted(&fx, cx, fx.cs);
    let before = policy::flags();
    policy::set_flags(policy::PolicyFlags::default());
    // The failed start never created the instance (`created` is false).
    save_start(&fx, cx, true);
    let cs = fx.cs;
    fx.window
        .update(cx, |shell, _, _| {
            let store = locked(&shell.store).unwrap();
            let mut record = monocode_core::cloud_sessions::start_record(&store, cs).unwrap();
            record.created = false;
            monocode_core::cloud_sessions::startup::save_start_record(&store, &record).unwrap();
        })
        .unwrap();
    let refused = call(
        &fx,
        cx,
        &fx.web(),
        "cloud.retryStart",
        json!({"sessionId": id}),
    )
    .unwrap_err();
    assert_eq!(code_of(&refused), Some("disabled"));
    assert_eq!(
        cloud_policy::limiter().calls_this_hour(&fx.id, std::time::Instant::now()),
        0,
        "a refused retry costs no slot"
    );
    // With the toggle on, it runs.
    policy::set_flags(policy::PolicyFlags {
        remote_cloud_start: true,
        ..Default::default()
    });
    assert_eq!(
        call(
            &fx,
            cx,
            &fx.web(),
            "cloud.retryStart",
            json!({"sessionId": id})
        )
        .unwrap(),
        json!({"accepted": true})
    );
    // A kept instance (the failed start created it) is retried in place, toggle off.
    policy::set_flags(policy::PolicyFlags::default());
    fx.window
        .update(cx, |shell, _, _| {
            shell.drop_cloud_start(cs);
            let store = locked(&shell.store).unwrap();
            let mut record = monocode_core::cloud_sessions::start_record(&store, cs).unwrap();
            record.created = true;
            monocode_core::cloud_sessions::startup::save_start_record(&store, &record).unwrap();
        })
        .unwrap();
    cx.executor().advance_clock(Duration::from_secs(3));
    cx.run_until_parked();
    assert_eq!(
        call(
            &fx,
            cx,
            &fx.web(),
            "cloud.retryStart",
            json!({"sessionId": id})
        )
        .unwrap(),
        json!({"accepted": true})
    );
    policy::set_flags(before);
}

#[gpui::test]
fn a_send_while_the_instance_starts_is_refused_with_the_desktops_own_text(cx: &mut TestAppContext) {
    let fx = fixture(cx, "Shutdown", CloudState::Stopped);
    save_start(&fx, cx, false);
    let cs = fx.cs;
    let expected = fx
        .window
        .update(cx, |shell, _, _| shell.mobile_start_refusal(cs))
        .unwrap()
        .expect("a start record refuses");
    assert!(expected.contains("still starting"), "{expected}");
    let refused = before_send(&fx, cx, &fx.web(), json!({"wake": true})).unwrap_err();
    assert_eq!(code_of(&refused), Some("starting"));
    // The desktop's text leaves through the same sanitizer as `startup.error`.
    assert_eq!(
        refused.error,
        cloud_policy::sanitize_startup_error(&expected)
    );
    // A failed start refuses with its own text and the Retry hint.
    save_start(&fx, cx, true);
    let refused = before_send(&fx, cx, &fx.web(), json!({"wake": true})).unwrap_err();
    assert_eq!(code_of(&refused), Some("starting"));
    assert!(refused.error.contains("Retry"), "{}", refused.error);
    assert_eq!(
        cloud_policy::limiter().calls_this_hour(&fx.id, std::time::Instant::now()),
        0
    );
}

#[gpui::test]
fn a_send_needs_consent_to_wake_and_an_available_instance_sends_as_today(cx: &mut TestAppContext) {
    let fx = fixture(cx, "Shutdown", CloudState::Stopped);
    for state in [
        CloudState::Stopped,
        CloudState::Stopping,
        CloudState::Unknown,
    ] {
        set_state(&fx, cx, state);
        for params in [json!({}), json!({"wake": false}), json!({"wake": "yes"})] {
            let refused = before_send(&fx, cx, &fx.web(), params).unwrap_err();
            assert_eq!(code_of(&refused), Some("wake_required"), "{state:?}");
            assert_eq!(refused.error, policy::Code::WakeRequired.message());
        }
    }
    set_state(&fx, cx, CloudState::Unavailable);
    let refused = before_send(&fx, cx, &fx.web(), json!({"wake": true})).unwrap_err();
    assert_eq!(code_of(&refused), Some("unsupported_target"));
    set_state(&fx, cx, CloudState::Available);
    // N1: a cached `available` is not enough: the view must be loaded and connected.
    let refused = before_send(&fx, cx, &fx.web(), json!({})).unwrap_err();
    assert_eq!(code_of(&refused), Some("wake_required"));
    load_view(&fx, cx);
    connect(&fx, cx, true);
    assert_eq!(
        before_send(&fx, cx, &fx.web(), json!({})),
        Ok(cloud::SendPlan { wake: false })
    );
    assert_eq!(
        before_send(&fx, cx, &fx.web(), json!({"wake": true})),
        Ok(cloud::SendPlan { wake: false }),
        "a connected instance needs no wake"
    );
    assert_eq!(
        cloud_policy::limiter().calls_this_hour(&fx.id, std::time::Instant::now()),
        0
    );
    assert_eq!(changes(&fx.gh), 0);
    // A local session is never looked at.
    let local = fx.local;
    fx.window
        .update(cx, |shell, _, cx| {
            assert_eq!(
                cloud::before_send(shell, &fx.web(), Some(local), &json!({}), cx),
                Ok(cloud::SendPlan::default())
            );
        })
        .unwrap();
}

#[gpui::test]
fn a_send_during_an_auto_stop_or_an_archive_is_refused(cx: &mut TestAppContext) {
    let fx = fixture(cx, "Available", CloudState::Available);
    fx.window
        .update(cx, |shell, _, _| {
            shell.auto_stop_running.insert(Fx::key());
        })
        .unwrap();
    let refused = before_send(&fx, cx, &fx.web(), json!({"wake": true})).unwrap_err();
    assert_eq!(code_of(&refused), Some("auto_stopping"));
    assert_eq!(refused.error, crate::conversation::AUTO_STOPPING);
    fx.window
        .update(cx, |shell, _, _| {
            shell.auto_stop_running.remove(&Fx::key());
            shell.cloud_workspaces_busy.insert(Fx::key());
        })
        .unwrap();
    let refused = before_send(&fx, cx, &fx.web(), json!({"wake": true})).unwrap_err();
    assert_eq!(code_of(&refused), Some("instance_busy"));
}

#[gpui::test]
fn a_wake_send_wakes_once_queues_the_message_and_a_second_send_is_busy(cx: &mut TestAppContext) {
    let fx = fixture(cx, "Shutdown", CloudState::Stopped);
    let cs = fx.cs;
    // The check alone claims nothing: no slot, no flag, no wake.
    let plan = before_send(&fx, cx, &fx.web(), json!({"wake": true})).unwrap();
    assert!(plan.wake);
    assert_eq!(
        cloud_policy::limiter().in_flight(&fx.id, std::time::Instant::now()),
        0
    );
    assert!(!crate::conversation::Conversation::wake_send_pending(cs));
    // S10: the wire reply of the host's own send path.
    let reply = send(
        &fx,
        cx,
        &fx.web(),
        cs,
        json!({"wake": true, "text": "hello"}),
    )
    .unwrap();
    assert_eq!(
        reply,
        json!({"sessionId": persisted(&fx, cx, cs), "wake": true, "queued": true})
    );
    assert_eq!(
        cloud_policy::limiter().in_flight(&fx.id, std::time::Instant::now()),
        1
    );
    assert!(crate::conversation::Conversation::wake_send_pending(cs));
    // The message waits in its own slot, not in the history deferral (N2).
    // A second send while one is pending is busy and takes no slot.
    let refused = send(&fx, cx, &fx.web(), cs, json!({"wake": true})).unwrap_err();
    assert_eq!(code_of(&refused), Some("busy"));
    assert_eq!(
        cloud_policy::limiter().in_flight(&fx.id, std::time::Instant::now()),
        1
    );
    // Another device is refused for the same reason: one pending send per session.
    let other = device_of(&format!("{}-9", fx.id), DeviceKind::Web, None);
    let refused = before_send(&fx, cx, &other, json!({"wake": true})).unwrap_err();
    assert_eq!(code_of(&refused), Some("busy"));
    // The wake runs and the instance comes back.
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_secs(5));
    cx.run_until_parked();
    assert!(
        fx.host
            .0
            .lock()
            .unwrap()
            .iter()
            .any(|call| call.starts_with("ensure")),
        "the wake reached the host: {:?}",
        fx.host.0.lock().unwrap()
    );
    assert_eq!(
        cloud_policy::limiter().in_flight(&fx.id, std::time::Instant::now()),
        0
    );
}

/// S6: with the history already loaded, submitting the wake-send does not clear the busy
/// flag; only its flush does, so a second send is still busy while the first waits.
#[gpui::test]
fn the_busy_flag_survives_the_submit_of_a_loaded_session_and_ends_at_the_flush(
    cx: &mut TestAppContext,
) {
    let fx = fixture(cx, "Shutdown", CloudState::Stopped);
    let cs = fx.cs;
    load_view(&fx, cx);
    let reply = send(
        &fx,
        cx,
        &fx.web(),
        cs,
        json!({"wake": true, "text": "queued one"}),
    )
    .unwrap();
    assert_eq!(reply["wake"], true);
    assert!(
        crate::conversation::Conversation::wake_send_pending(cs),
        "the submit that created the flag must not clear it"
    );
    let refused = before_send(&fx, cx, &fx.web(), json!({"wake": true})).unwrap_err();
    assert_eq!(code_of(&refused), Some("busy"));
    assert!(
        launched(&fx, cx).is_empty(),
        "nothing is sent while the instance is down"
    );
    // The link comes up and the reload flushes the deferral: the message goes out and the flag ends.
    fx.window
        .update(cx, |shell, _, cx| {
            shell.sessions[&cs].update(cx, |chat, cx| {
                chat.set_online_for_test(true);
                chat.flush_history_for_test(cx);
            });
        })
        .unwrap();
    assert_eq!(launched(&fx, cx), ["queued one"]);
    assert!(
        !crate::conversation::Conversation::wake_send_pending(cs),
        "the flush clears the flag"
    );
}

#[gpui::test]
fn a_failed_wake_releases_the_slot_and_drops_only_the_queued_send(cx: &mut TestAppContext) {
    let fx = fixture(cx, "Shutdown", CloudState::Stopped);
    let cs = fx.cs;
    load_view(&fx, cx);
    fx.host.2.store(true, std::sync::atomic::Ordering::Relaxed);
    // S7: an action the desktop itself waits to run on the same history must survive.
    let desktop_ran = Rc::new(std::cell::Cell::new(false));
    let ran = desktop_ran.clone();
    fx.window
        .update(cx, |shell, _, cx| {
            shell.sessions[&cs].update(cx, |chat, cx| {
                chat.defer_history_action(move |_, _| ran.set(true), cx)
            });
        })
        .unwrap();
    assert_eq!(
        send(
            &fx,
            cx,
            &fx.web(),
            cs,
            json!({"wake": true, "text": "doomed"})
        )
        .unwrap()["wake"],
        true
    );
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_secs(5));
    cx.run_until_parked();
    assert_eq!(
        cloud_policy::limiter().in_flight(&fx.id, std::time::Instant::now()),
        0
    );
    assert!(
        !crate::conversation::Conversation::wake_send_pending(cs),
        "a failed wake clears the busy flag"
    );
    // The link is brought up by hand later: the doomed message must not go out, the
    // desktop's own action still runs.
    fx.window
        .update(cx, |shell, _, cx| {
            shell.sessions[&cs].update(cx, |chat, cx| {
                chat.set_online_for_test(true);
                chat.flush_history_for_test(cx);
            });
        })
        .unwrap();
    assert!(
        launched(&fx, cx).is_empty(),
        "the dropped send never goes out"
    );
    assert!(
        desktop_ran.get(),
        "the desktop's own deferred action was not dropped"
    );
    // The device may try again.
    fx.host.2.store(false, std::sync::atomic::Ordering::Relaxed);
    assert!(
        before_send(&fx, cx, &fx.web(), json!({"wake": true}))
            .unwrap()
            .wake
    );
}

/// L1 and S3: a message that is refused (no text, too long) wakes nothing, takes no slot and
/// leaves no flag, so the device is not locked out for 15 minutes.
#[gpui::test]
fn a_refused_message_never_wakes_or_leaves_a_busy_flag(cx: &mut TestAppContext) {
    let fx = fixture(cx, "Shutdown", CloudState::Stopped);
    let cs = fx.cs;
    for params in [
        json!({"wake": true, "text": ""}),
        json!({"wake": true, "text": "   "}),
        json!({"wake": true, "text": "x".repeat(100_001)}),
    ] {
        let refused = send(&fx, cx, &fx.web(), cs, params).unwrap_err();
        assert!(
            refused.error.contains("Enter a message"),
            "{}",
            refused.error
        );
    }
    // Without consent the refusal is the free one.
    let refused = send(&fx, cx, &fx.web(), cs, json!({"text": "hi"})).unwrap_err();
    assert_eq!(code_of(&refused), Some("wake_required"));
    cx.executor().advance_clock(Duration::from_secs(5));
    cx.run_until_parked();
    assert_eq!(
        cloud_policy::limiter().calls_this_hour(&fx.id, std::time::Instant::now()),
        0
    );
    assert_eq!(changes(&fx.gh), 0);
    assert!(
        fx.host.0.lock().unwrap().is_empty(),
        "{:?}",
        fx.host.0.lock().unwrap()
    );
    assert!(!crate::conversation::Conversation::wake_send_pending(cs));
    // So the next good send is not busy.
    assert_eq!(
        send(&fx, cx, &fx.web(), cs, json!({"wake": true, "text": "hi"})).unwrap()["wake"],
        true
    );
}

/// H1 and L5: the cache can say `available` after the instance went down. A loaded view that
/// is not connected is not available: without consent the send is `wake_required`, with it
/// the wake is claimed and counted. A send never reconnects an instance on its own.
#[gpui::test]
fn a_stale_available_cache_does_not_let_a_send_wake_the_instance(cx: &mut TestAppContext) {
    let fx = fixture(cx, "Shutdown", CloudState::Available);
    let cs = fx.cs;
    // N1: a view that is not loaded (after a restart of the desktop) is not connected either.
    let refused = send(&fx, cx, &fx.web(), cs, json!({"text": "hi"})).unwrap_err();
    assert_eq!(code_of(&refused), Some("wake_required"));
    assert!(launched(&fx, cx).is_empty());
    load_view(&fx, cx);
    let refused = send(&fx, cx, &fx.web(), cs, json!({"text": "hi"})).unwrap_err();
    assert_eq!(code_of(&refused), Some("wake_required"));
    cx.executor().advance_clock(Duration::from_secs(5));
    cx.run_until_parked();
    assert_eq!(changes(&fx.gh), 0);
    assert!(
        fx.host.0.lock().unwrap().is_empty(),
        "{:?}",
        fx.host.0.lock().unwrap()
    );
    assert_eq!(
        cloud_policy::limiter().calls_this_hour(&fx.id, std::time::Instant::now()),
        0
    );
    // A connected view is available: a plain send, as today.
    fx.window
        .update(cx, |shell, _, cx| {
            shell.sessions[&cs].update(cx, |chat, _| chat.set_online_for_test(true));
        })
        .unwrap();
    assert_eq!(
        before_send(&fx, cx, &fx.web(), json!({})),
        Ok(cloud::SendPlan { wake: false })
    );
}

/// H1: a remote send without the wake plan (a phone, or any send the host did not check
/// for a wake) never reconnects a stopped cloud workspace, whatever state the view is in.
#[gpui::test]
fn a_send_without_the_wake_plan_never_reconnects_the_workspace(cx: &mut TestAppContext) {
    let fx = fixture(cx, "Shutdown", CloudState::Stopped);
    let cs = fx.cs;
    load_view(&fx, cx);
    fx.window
        .update(cx, |shell, _, cx| {
            shell.sessions[&cs].update(cx, |chat, cx| {
                chat.mobile_submit("sneaky".into(), vec![], cx);
                assert!(!chat.is_waking(), "no reconnect was started");
            });
        })
        .unwrap();
    cx.executor().advance_clock(Duration::from_secs(5));
    cx.run_until_parked();
    assert!(
        fx.host.0.lock().unwrap().is_empty(),
        "{:?}",
        fx.host.0.lock().unwrap()
    );
    assert_eq!(changes(&fx.gh), 0);
    assert!(launched(&fx, cx).is_empty());
    fx.window
        .update(cx, |shell, _, cx| {
            let chat = shell.sessions[&cs].read(cx);
            assert!(!chat.is_waking());
            assert!(
                !chat.input_has_text_for_test(cx),
                "the desktop composer is untouched"
            );
        })
        .unwrap();
}

/// N2: running the deferred history actions while the link is still down neither spins nor
/// loses the wake-send; it goes out once, when the link is up.
#[gpui::test]
fn history_actions_run_while_the_link_is_down_do_not_loop_or_lose_the_wake_send(
    cx: &mut TestAppContext,
) {
    let fx = fixture(cx, "Shutdown", CloudState::Stopped);
    let cs = fx.cs;
    load_view(&fx, cx);
    send(
        &fx,
        cx,
        &fx.web(),
        cs,
        json!({"wake": true, "text": "later"}),
    )
    .unwrap();
    for _ in 0..3 {
        fx.window
            .update(cx, |shell, _, cx| {
                shell.sessions[&cs].update(cx, |chat, cx| chat.flush_history_for_test(cx));
            })
            .unwrap();
    }
    assert!(launched(&fx, cx).is_empty());
    assert!(crate::conversation::Conversation::wake_send_pending(cs));
    connect(&fx, cx, true);
    fx.window
        .update(cx, |shell, _, cx| {
            shell.sessions[&cs].update(cx, |chat, cx| chat.flush_history_for_test(cx));
        })
        .unwrap();
    assert_eq!(launched(&fx, cx), ["later"]);
    assert!(!crate::conversation::Conversation::wake_send_pending(cs));
}

/// N3 and L5: a phone that reaches a cloud conversation (its remote project is on the rail)
/// gets a coded failure while the link is down, never ok and a dropped message; with the
/// link up the message is delivered as today.
#[gpui::test]
fn a_phone_send_to_a_cloud_conversation_fails_coded_unless_connected(cx: &mut TestAppContext) {
    let fx = fixture(cx, "Shutdown", CloudState::Stopped);
    let cs = fx.cs;
    fx.window
        .update(cx, |shell, _, _| {
            let cwd = shell.session_summaries[&cs].cwd.clone();
            shell.workspace.add_project(&cwd);
        })
        .unwrap();
    let phone = fx.phone();
    let wire = persisted(&fx, cx, cs);
    for loaded in [false, true] {
        if loaded {
            load_view(&fx, cx);
        }
        let refused = send(&fx, cx, &phone, cs, json!({"text": "hi"})).unwrap_err();
        assert_eq!(code_of(&refused), Some("unsupported_target"), "{loaded}");
        assert!(refused.error.contains("not connected"), "{}", refused.error);
    }
    cx.executor().advance_clock(Duration::from_secs(5));
    cx.run_until_parked();
    assert!(fx.host.0.lock().unwrap().is_empty(), "nothing woke");
    assert_eq!(changes(&fx.gh), 0);
    assert!(launched(&fx, cx).is_empty());
    connect(&fx, cx, true);
    let reply = send(&fx, cx, &phone, cs, json!({"text": "hi"})).unwrap();
    assert_eq!(reply, json!({"sessionId": wire}));
}

/// Makes the fixture's local session a moved one: the local folder is kept, its store
/// location is Cloud (its turns run on the linked Host) and it is bound to the instance.
fn move_local_to_cloud(fx: &Fx, cx: &mut TestAppContext) {
    let (cs, local, project) = (fx.cs, fx.local, fx.project());
    fx.window
        .update(cx, |shell, _, _| {
            let mark = shell.cloud_marks[&cs].clone();
            shell.cloud_marks.insert(local, mark);
            let location = monocode_core::store::CloudLocation {
                cloud: CloudRef::codespace(CS),
                environment_id: format!("env-{CS}"),
                project_id: "p".into(),
                host_session_id: "host-1".into(),
                remote_cwd: "/workspaces/widgets".into(),
                local_cwd: project,
                moved_at_row: 0,
                pushed_branch: None,
            };
            locked(&shell.store)
                .unwrap()
                .set_cloud_location(
                    local,
                    &monocode_core::store::SessionLocation::Cloud(location),
                )
                .unwrap();
            assert!(shell.mobile_is_cloud_conversation(local));
        })
        .unwrap();
}

/// The blocker: a moved session (local folder, store location Cloud) would wake its stopped
/// instance through `ensure_before_send` with no consent. It is a cloud conversation like the
/// others: a web device needs the wake plan, a phone is refused while the instance is down,
/// and with the instance live a phone's message is delivered as today.
#[gpui::test]
fn a_moved_session_never_wakes_without_consent_for_web_or_phone(cx: &mut TestAppContext) {
    let fx = fixture(cx, "Shutdown", CloudState::Stopped);
    let local = fx.local;
    move_local_to_cloud(&fx, cx);
    let no_calls = |fx: &Fx| {
        assert!(
            fx.host.0.lock().unwrap().is_empty(),
            "{:?}",
            fx.host.0.lock().unwrap()
        );
        assert_eq!(changes(&fx.gh), 0);
    };
    // Web without consent: free refusal, nothing called.
    let refused = send(&fx, cx, &fx.web(), local, json!({"text": "hi"})).unwrap_err();
    assert_eq!(code_of(&refused), Some("wake_required"));
    no_calls(&fx);
    assert_eq!(
        cloud_policy::limiter().calls_this_hour(&fx.id, std::time::Instant::now()),
        0
    );
    // Phone while the instance is down: a coded failure, nothing called.
    let phone = fx.phone();
    let refused = send(&fx, cx, &phone, local, json!({"text": "hi"})).unwrap_err();
    assert_eq!(code_of(&refused), Some("unsupported_target"));
    assert!(refused.error.contains("not connected"), "{}", refused.error);
    cx.executor().advance_clock(Duration::from_secs(5));
    cx.run_until_parked();
    no_calls(&fx);
    // Web with the consent: the wake plan, counted in the rate limit.
    let plan = before_send_of(&fx, cx, &fx.web(), local, json!({"wake": true})).unwrap();
    assert!(plan.wake);
    let reply = send(
        &fx,
        cx,
        &fx.web(),
        local,
        json!({"wake": true, "text": "hi"}),
    )
    .unwrap();
    assert_eq!(reply["wake"], true);
    assert_eq!(
        cloud_policy::limiter().calls_this_hour(&fx.id, std::time::Instant::now()),
        1
    );
    crate::conversation::Conversation::reset_wake_sends();
    // Phone with the instance live: delivered as today.
    set_state(&fx, cx, CloudState::Available);
    let reply = send(&fx, cx, &phone, local, json!({"text": "hi"})).unwrap();
    assert_eq!(reply, json!({"sessionId": persisted(&fx, cx, local)}));
}

/// N7: a wake that cannot open its conversation is a coded failure and holds nothing.
#[gpui::test]
fn a_wake_that_cannot_open_the_session_fails_coded_and_holds_nothing(cx: &mut TestAppContext) {
    let fx = fixture(cx, "Shutdown", CloudState::Stopped);
    let web = fx.web();
    let refused = fx
        .window
        .update(cx, |shell, _, cx| {
            cloud::start_wake_send(shell, &web, 9_999_999, cx).unwrap_err()
        })
        .unwrap();
    assert_eq!(code_of(&refused), Some("unsupported_target"));
    assert!(!crate::conversation::Conversation::wake_send_pending(
        9_999_999
    ));
    assert_eq!(
        cloud_policy::limiter().in_flight(&fx.id, std::time::Instant::now()),
        0
    );
}

/// N4: a narrowed device may stop an instance it was shown (listed under its project), and
/// one listed under another project is not selected.
#[gpui::test]
fn a_narrowed_device_stops_by_name_only_what_was_listed_under_its_project(cx: &mut TestAppContext) {
    let fx = fixture(cx, "Available", CloudState::Available);
    call(&fx, cx, &fx.web(), "cloud.instances", json!({})).unwrap();
    call(&fx, cx, &fx.web(), "cloud.instances", json!({})).unwrap();
    fx.window
        .update(cx, |shell, _, _| shell.cloud_marks.clear())
        .unwrap();
    let mine = device_of(
        &format!("{}-7", fx.id),
        DeviceKind::Web,
        Some(vec![project_id(&fx.project())]),
    );
    let reply = call(
        &fx,
        cx,
        &mine,
        "cloud.stop",
        json!({"kind": "codespace", "instance": CS}),
    )
    .unwrap();
    assert_eq!(reply["stopped"], true);
    let other = device_of(
        &format!("{}-8", fx.id),
        DeviceKind::Web,
        Some(vec![project_id("/work/another-project")]),
    );
    let refused = call(
        &fx,
        cx,
        &other,
        "cloud.stop",
        json!({"kind": "codespace", "instance": CS}),
    )
    .unwrap_err();
    assert_eq!(code_of(&refused), Some("not_selected"));
    assert!(cloud::instance_listed_project(&CloudRef::codespace(CS)).is_some());
}

/// S10: a phone's send goes through the host's own path and gets today's reply, with no
/// wake call. A moved session (a local row bound to an instance) is the cloud session a
/// phone may still reach; a cloud-born one is refused (H2).
#[gpui::test]
fn a_phone_send_never_wakes_and_keeps_its_reply(cx: &mut TestAppContext) {
    let fx = fixture(cx, "Shutdown", CloudState::Stopped);
    let (cs, local) = (fx.cs, fx.local);
    // The local session is moved: it is bound to the stopped instance and keeps its folder.
    fx.window
        .update(cx, |shell, _, _| {
            let mark = shell.cloud_marks[&cs].clone();
            shell.cloud_marks.insert(local, mark);
        })
        .unwrap();
    let phone = fx.phone();
    for params in [json!({"text": "hi"}), json!({"text": "hi", "wake": true})] {
        // Today's reply: the session id and nothing else.
        let reply = send(&fx, cx, &phone, local, params).unwrap();
        assert_eq!(reply, json!({"sessionId": persisted(&fx, cx, local)}));
    }
    cx.executor().advance_clock(Duration::from_secs(5));
    cx.run_until_parked();
    assert_eq!(
        cloud_policy::limiter().calls_this_hour(&phone.id, std::time::Instant::now()),
        0
    );
    assert!(!crate::conversation::Conversation::wake_send_pending(local));
    assert_eq!(changes(&fx.gh), 0);
    assert!(
        fx.host.0.lock().unwrap().is_empty(),
        "{:?}",
        fx.host.0.lock().unwrap()
    );
    // The cloud-born session is not a phone's business.
    let refused = send(&fx, cx, &phone, cs, json!({"text": "hi"})).unwrap_err();
    assert_eq!(code_of(&refused), Some("unsupported_target"));
    assert!(fx.host.0.lock().unwrap().is_empty());
}

#[gpui::test]
fn a_remote_machine_session_never_needs_a_wake(cx: &mut TestAppContext) {
    let fx = fixture(cx, "Shutdown", CloudState::Stopped);
    let remote = fx
        .window
        .update(cx, |shell, _, _| {
            let id = {
                let store = locked(&shell.store).unwrap();
                let project = json!({"id": "p", "cwd": "/home/me/app"});
                let key = store.remember_remote_project("box", &project).unwrap().key;
                let mut summary = store.create("claude", &key).unwrap();
                summary.has_messages = true;
                shell.session_summaries.insert(summary.id, summary.clone());
                summary.id
            };
            shell.order.push(id);
            id
        })
        .unwrap();
    fx.window
        .update(cx, |shell, _, cx| {
            assert_eq!(
                cloud::before_send(shell, &fx.web(), Some(remote), &json!({}), cx),
                Ok(cloud::SendPlan::default())
            );
        })
        .unwrap();
}

#[gpui::test]
fn a_web_send_counts_as_activity_and_watching_does_not(cx: &mut TestAppContext) {
    let fx = fixture(cx, "Available", CloudState::Available);
    let cs = fx.cs;
    fx.window
        .update(cx, |shell, _, cx| {
            let epoch = std::time::Instant::now();
            let last = |shell: &Shell, cx: &App| {
                shell.sessions[&cs]
                    .read(cx)
                    .auto_stop_facts(epoch)
                    .last_input
            };
            // Reads and watch frames build the cloud field and touch nothing.
            for _ in 0..3 {
                let _ = cloud::read_fields(shell, cs, &fx.web(), cx);
                let _ = cloud::frame_fields(shell, cs, &fx.web(), cx);
                let _ = shell.mobile_frame_fields(&Target::Session(cs), &fx.web(), cx);
            }
            assert_eq!(last(shell, cx), None, "watching is not activity");
            let view = shell.sessions[&cs].clone();
            view.update(cx, |chat, cx| chat.mobile_submit("hi".into(), vec![], cx));
            assert!(last(shell, cx).is_some(), "a send is activity");
        })
        .unwrap();
}

#[gpui::test]
fn an_auto_stop_is_announced_near_its_time_cancelled_by_keep_running_and_refuses_a_send(
    cx: &mut TestAppContext,
) {
    let fx = fixture(cx, "Available", CloudState::Available);
    let (cs, local) = (fx.cs, fx.local);
    let _ = local;
    fx.window
        .update(cx, |shell, _, cx| {
            shell
                .workspace
                .preferences
                .cloud_auto_stop
                .policy_mut(CloudBackendKind::Codespaces)
                .idle_minutes = Some(10);
            shell.sessions[&cs].update(cx, |chat, _| chat.loaded_for_test());
            // The idle window began at second 0 and the clock is at second 6 minutes.
            let prefs = shell.workspace.preferences.cloud_auto_stop;
            let facts = shell.auto_stop_facts(cx);
            shell.auto_stop.check(0, &prefs, &facts);
            shell.auto_stop_epoch = std::time::Instant::now() - Duration::from_secs(6 * 60);
            let announced = cloud::read_fields(shell, cs, &fx.web(), cx);
            let at = announced["cloud"]["autoStop"]["at"]
                .as_u64()
                .expect("announced");
            let now = monocode_core::time::now_ms();
            let in_ms = at.saturating_sub(now);
            assert!((200_000..=245_000).contains(&in_ms), "{in_ms}");
            assert_eq!(announced["cloud"]["autoStop"]["reason"], "idle");
            // A frame says the same (milliseconds since the epoch, D2).
            assert!(
                cloud::frame_fields(shell, cs, &fx.web(), cx)["cloud"]["autoStop"]["at"].is_u64()
            );
        })
        .unwrap();
    let id = persisted(&fx, cx, cs);
    assert_eq!(
        call(
            &fx,
            cx,
            &fx.web(),
            "cloud.keepRunning",
            json!({"sessionId": id})
        )
        .unwrap(),
        json!({"keepRunning": true})
    );
    fx.window
        .update(cx, |shell, _, cx| {
            assert!(
                cloud::read_fields(shell, cs, &fx.web(), cx)["cloud"]
                    .get("autoStop")
                    .is_none(),
                "the notice is gone"
            );
            // The stop runs now: announced, and a send is refused; it cannot be cancelled.
            shell.auto_stop_running.insert(Fx::key());
            let running = cloud::read_fields(shell, cs, &fx.web(), cx);
            assert!(running["cloud"]["autoStop"]["at"].is_u64());
        })
        .unwrap();
    let refused = before_send(&fx, cx, &fx.web(), json!({"wake": true})).unwrap_err();
    assert_eq!(code_of(&refused), Some("auto_stopping"));
    let refused = call(
        &fx,
        cx,
        &fx.web(),
        "cloud.keepRunning",
        json!({"sessionId": id}),
    )
    .unwrap_err();
    assert_eq!(code_of(&refused), Some("auto_stopping"));
    // Phones and local sessions have no such method.
    let local = persisted(&fx, cx, fx.local);
    fx.window
        .update(cx, |shell, _, _| {
            shell.auto_stop_running.remove(&Fx::key());
        })
        .unwrap();
    let refused = call(
        &fx,
        cx,
        &fx.web(),
        "cloud.keepRunning",
        json!({"sessionId": local}),
    )
    .unwrap_err();
    assert_eq!(code_of(&refused), Some("unsupported_target"));
}

/// S8: `autoStop.at` is a fixed deadline. Two frames a second apart carry the same value, so
/// the watch (which sends the field when it changed) is not woken every second.
#[gpui::test]
fn the_announced_auto_stop_time_does_not_move_between_frames(cx: &mut TestAppContext) {
    let fx = fixture(cx, "Available", CloudState::Available);
    let cs = fx.cs;
    let at = |fx: &Fx, cx: &mut TestAppContext| {
        fx.window
            .update(cx, |shell, _, cx| {
                cloud::frame_fields(shell, cs, &fx.web(), cx)["cloud"]["autoStop"]["at"].as_u64()
            })
            .unwrap()
    };
    fx.window
        .update(cx, |shell, _, cx| {
            shell
                .workspace
                .preferences
                .cloud_auto_stop
                .policy_mut(CloudBackendKind::Codespaces)
                .idle_minutes = Some(10);
            shell.sessions[&cs].update(cx, |chat, _| chat.loaded_for_test());
            let prefs = shell.workspace.preferences.cloud_auto_stop;
            let facts = shell.auto_stop_facts(cx);
            shell.auto_stop.check(0, &prefs, &facts);
            shell.auto_stop_epoch = std::time::Instant::now() - Duration::from_secs(6 * 60);
        })
        .unwrap();
    let first = at(&fx, cx).expect("announced");
    std::thread::sleep(Duration::from_millis(1100));
    assert_eq!(at(&fx, cx), Some(first), "the idle deadline is fixed");
    // While the stop runs the value is the moment it was first seen, also fixed.
    fx.window
        .update(cx, |shell, _, _| {
            shell.auto_stop_running.insert(Fx::key());
        })
        .unwrap();
    let running = at(&fx, cx).expect("announced");
    std::thread::sleep(Duration::from_millis(1100));
    assert_eq!(at(&fx, cx), Some(running));
}

#[gpui::test]
fn archiving_a_cloud_session_needs_the_cloud_choice_and_goes_through_the_cloud_archive(
    cx: &mut TestAppContext,
) {
    let fx = fixture(cx, "Available", CloudState::Available);
    let (cs, local) = (fx.cs, fx.local);
    fx.window
        .update(cx, |shell, window, cx| {
            let text = "Choose whether the cloud instance keeps running or stops";
            for params in [json!({}), json!({"cloud": "delete"}), json!({"cloud": "x"})] {
                let refused =
                    cloud::before_archive(shell, &fx.web(), cs, &params, window, cx).unwrap_err();
                assert_eq!(code_of(&refused), Some("unsupported_target"));
                assert_eq!(refused.error, text);
            }
            assert!(shell.cloud_busy.is_empty(), "nothing started");
            // A local session and a phone keep today's archive.
            assert_eq!(
                cloud::before_archive(shell, &fx.web(), local, &json!({}), window, cx),
                Ok(cloud::Archive::Local)
            );
            // A phone cannot choose keep or stop, so it may not archive a session bound to
            // an instance at all (H2); a plain session is archived as always.
            let refused =
                cloud::before_archive(shell, &fx.phone(), cs, &json!({}), window, cx).unwrap_err();
            assert_eq!(code_of(&refused), Some("unsupported_target"));
            assert_eq!(
                cloud::before_archive(shell, &fx.phone(), local, &json!({}), window, cx),
                Ok(cloud::Archive::Local)
            );
            // With the choice the cloud archive takes it, and the instance is held.
            assert_eq!(
                cloud::before_archive(shell, &fx.web(), cs, &json!({"cloud": "keep"}), window, cx),
                Ok(cloud::Archive::Handled)
            );
            assert!(shell.cloud_busy.contains(&cs));
            assert!(shell.cloud_workspaces_busy.contains(&Fx::key()));
            // While it works, another archive, a stop and a send are instance_busy.
            let refused =
                cloud::before_archive(shell, &fx.web(), cs, &json!({"cloud": "stop"}), window, cx)
                    .unwrap_err();
            assert_eq!(code_of(&refused), Some("instance_busy"));
            let refused =
                cloud::before_send(shell, &fx.web(), Some(cs), &json!({"wake": true}), cx)
                    .unwrap_err();
            assert_eq!(code_of(&refused), Some("instance_busy"));
            let answer: Rc<RefCell<Option<Result<Value, Failure>>>> = Rc::default();
            let slot = answer.clone();
            let id = shell.mobile_session_id(cs);
            cloud::dispatch_deferred(
                shell,
                &fx.web(),
                "cloud.stop",
                &json!({"sessionId": id}),
                move |_, result| *slot.borrow_mut() = Some(result),
                cx,
            );
            // Refused before any worker starts.
            let refused = answer.take().expect("answered at once").unwrap_err();
            assert_eq!(code_of(&refused), Some("instance_busy"));
        })
        .unwrap();
    assert_eq!(count(&fx.gh, "codespace stop"), 0);
}

#[gpui::test]
fn a_cloud_or_remote_session_is_never_deleted_by_a_web_device(cx: &mut TestAppContext) {
    let fx = fixture(cx, "Available", CloudState::Available);
    let (cs, local) = (fx.cs, fx.local);
    fx.window
        .update(cx, |shell, _, cx| {
            let refused = cloud::before_delete(shell, &fx.web(), cs, cx).unwrap_err();
            assert_eq!(code_of(&refused), Some("unsupported_target"));
            assert_eq!(cloud::before_delete(shell, &fx.web(), local, cx), Ok(()));
            // A phone's delete of a plain session only removes the local row, as it always
            // did; of one bound to an instance it would orphan a billing instance (H2).
            assert_eq!(cloud::before_delete(shell, &fx.phone(), local, cx), Ok(()));
            let refused = cloud::before_delete(shell, &fx.phone(), cs, cx).unwrap_err();
            assert_eq!(code_of(&refused), Some("unsupported_target"));
        })
        .unwrap();
}

#[gpui::test]
fn lifecycle_replies_never_carry_secrets_paths_or_logs(cx: &mut TestAppContext) {
    let fx = fixture(cx, "Available", CloudState::Available);
    let id = persisted(&fx, cx, fx.cs);
    let token = concat!("ghp_", "abcdefghijklmnopqrstuvwxyz0123456789");
    // A failed start whose record is full of things that must stay home.
    let (cs, project) = (fx.cs, fx.project());
    fx.window
        .update(cx, |shell, _, _| {
            let mut record = crate::cloud_start_view::fixtures::record(2);
            record.session_id = cs;
            record.local_project = project;
            record.codespace = CS.into();
            record.web_url = Some("https://leak-web-url.example.test/codespace".into());
            record.remote_cwd = "/leak/remote/cwd".into();
            record.log_tail = vec![format!("leak-log-line {token}")];
            record.gh_state = Some("leak-gh-state".into());
            for step in &mut record.steps {
                step.detail = Some("leak-step-detail".into());
            }
            fail(
                &mut record,
                format!("gh codespace ssh failed with {token} at /leak/path ssh://leak-host"),
            );
            monocode_core::cloud_sessions::startup::save_start_record(
                &locked(&shell.store).unwrap(),
                &record,
            )
            .unwrap();
        })
        .unwrap();
    let mut seen = vec![
        call(&fx, cx, &fx.web(), "cloud.instances", json!({})).unwrap(),
        call(&fx, cx, &fx.web(), "cloud.stop", json!({"sessionId": id})).unwrap(),
        json!(
            call(&fx, cx, &fx.web(), "cloud.resume", json!({"sessionId": id}))
                .unwrap_err()
                .error
        ),
        json!(
            before_send(&fx, cx, &fx.web(), json!({"wake": true}))
                .unwrap_err()
                .error
        ),
        call(
            &fx,
            cx,
            &fx.web(),
            "cloud.keepRunning",
            json!({"sessionId": id}),
        )
        .unwrap(),
    ];
    fx.window
        .update(cx, |shell, _, cx| {
            seen.push(Value::Object(cloud::read_fields(shell, cs, &fx.web(), cx)));
        })
        .unwrap();
    let text = serde_json::to_string(&seen).unwrap();
    for needle in [
        "leak-",
        "ghp_",
        "https://",
        "ssh://",
        "/leak",
        "log_tail",
        "logTail",
        "web_url",
        "webUrl",
        "remote_cwd",
        "remoteCwd",
        "gh_state",
        "ghState",
        "detail",
        "token",
        "ssh",
    ] {
        assert!(!text.contains(needle), "leaked {needle:?} in {text}");
    }
}
