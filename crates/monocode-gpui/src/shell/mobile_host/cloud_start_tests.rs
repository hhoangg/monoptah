//! H-11, H-13, H-14: the start options, the remote cloud start and the moves,
//! from the shell's side. A counting `gh` and host prove a read never calls out.
use super::cloud_start::CloudStartRequest;
use super::*;
use crate::cloud_wake::fakes;
use crate::mobile::cloud::CloudStartPlan;
use crate::shell::cloud::CloudMark;
use gpui::TestAppContext;
use monocode_core::{
    cloud_sessions::{CloudBackendKind, CloudSessions, Host},
    codespaces::{GhOutput, GhRunner},
    mobile::{DeviceKind, Permission, cloud_policy},
    remote::Machine,
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::Instant,
};

const PROJECT: &str = "/work/monocode";

fn device(id: &str, kind: DeviceKind, permission: Permission) -> DeviceContext {
    DeviceContext {
        id: id.into(),
        kind,
        permission,
        projects: None,
        monos: None,
    }
}

fn web(id: &str) -> DeviceContext {
    device(id, DeviceKind::Web, Permission::Control)
}

/// Counts every call that would reach `gh` or a remote host.
#[derive(Clone, Default)]
struct Calls(Arc<AtomicUsize>);
impl Calls {
    fn count(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }
}
struct CountingGh(Calls);
impl GhRunner for CountingGh {
    fn run(&self, _: &[String], _: Option<&str>) -> Result<GhOutput, String> {
        self.0.0.fetch_add(1, Ordering::SeqCst);
        Err("a read must not call gh".into())
    }
}
struct CountingHost(Calls);
impl CountingHost {
    fn hit<T>(&self) -> Result<T, String> {
        self.0.0.fetch_add(1, Ordering::SeqCst);
        Err("a read must not reach a host".into())
    }
}
impl Host for CountingHost {
    fn machines(&self) -> Result<Vec<Machine>, String> {
        self.hit()
    }
    fn connect(
        &self,
        _: &str,
        _: &str,
        _: &AtomicBool,
        _: &mut dyn FnMut(&str),
    ) -> Result<Machine, String> {
        self.hit()
    }
    fn ensure(&self, _: &str, _: &AtomicBool, _: &mut dyn FnMut(&str)) -> Result<(), String> {
        self.hit()
    }
    fn request(&self, _: &str, _: &str, _: Value) -> Result<Value, String> {
        self.hit()
    }
    fn disconnect(&self, _: &str) -> Result<(), String> {
        self.hit()
    }
}

/// The window fixture with a cloud service whose every outside call is counted
/// and refused.
fn counted(cx: &mut TestAppContext) -> (gpui::WindowHandle<Shell>, Calls) {
    let (window, _, _) = crate::shell::tests::window(cx);
    let calls = Calls::default();
    let seen = calls.clone();
    window
        .update(cx, |shell, _, _| {
            shell.cloud = Arc::new(CloudSessions::new(
                Arc::new(CountingGh(seen.clone())),
                Arc::new(CountingHost(seen)),
                shell.store.clone(),
            ));
        })
        .unwrap();
    (window, calls)
}

fn plan(target: Value) -> CloudStartPlan {
    CloudStartPlan {
        target: cloud_policy::validate_start_target(&target).unwrap(),
    }
}

fn request(device: &DeviceContext, target: Value) -> CloudStartRequest {
    CloudStartRequest {
        device: device.clone(),
        command_id: format!("cmd-{}", device.id),
        text: "Fix the build".into(),
        cwd: PROJECT.into(),
        access: None,
        params: json!({"text": "Fix the build", "projectId": project_id(PROJECT)}),
        plan: plan(target),
    }
}

type Answer = Rc<RefCell<Option<Result<Value, Failure>>>>;

fn answer() -> (
    Answer,
    impl FnOnce(&mut Shell, Result<Value, Failure>) + 'static,
) {
    let cell: Answer = Rc::default();
    let sink = cell.clone();
    (cell, move |_, result| *sink.borrow_mut() = Some(result))
}

fn in_flight(device: &str) -> usize {
    cloud_policy::limiter().in_flight(device, Instant::now())
}

fn calls_this_hour(device: &str) -> usize {
    cloud_policy::limiter().calls_this_hour(device, Instant::now())
}

// MARK: H-13

#[gpui::test]
fn the_start_options_check_the_device_and_the_project_and_never_call_out(cx: &mut TestAppContext) {
    let (window, calls) = counted(cx);
    window
        .update(cx, |shell, _, cx| {
            let ask = |shell: &Shell, who: &DeviceContext, project: &str, cx: &App| {
                crate::mobile::cloud::prepare_options(
                    shell,
                    who,
                    &json!({"projectId": project_id(project)}),
                    cx,
                )
                .map(|probe| probe.run().to_value())
            };
            // A read-only device may read the options.
            let reader = device("c13-reader", DeviceKind::Web, Permission::Read);
            let options = ask(shell, &reader, PROJECT, cx).unwrap();
            assert_eq!(options["backends"].as_array().unwrap().len(), 2);
            assert_eq!(options["sizes"].as_array().unwrap().len(), 5);
            // Nothing was created, woken or asked of gh or a host.
            assert_eq!(calls.count(), 0);

            // An unselected project is `not_selected`.
            let mut narrowed = web("c13-narrow");
            narrowed.projects = Some(vec![project_id("/work/another-project")]);
            assert_eq!(
                ask(shell, &narrowed, PROJECT, cx).err(),
                Some(policy::Code::NotSelected.failure())
            );
            // A remote machine is no target of a cloud start.
            assert_eq!(
                ask(shell, &web("c13-remote"), "remote://box/home/me/app", cx).err(),
                Some(policy::Code::UnsupportedTarget.failure())
            );
            // A phone has no cloud.
            assert_eq!(
                ask(
                    shell,
                    &device("c13-phone", DeviceKind::Phone, Permission::Control),
                    PROJECT,
                    cx
                )
                .err(),
                Some(policy::Code::Unsupported.failure())
            );
            assert_eq!(calls.count(), 0);
        })
        .unwrap();
}

#[gpui::test]
fn the_start_options_have_their_own_read_limit(cx: &mut TestAppContext) {
    let (window, _) = counted(cx);
    window
        .update(cx, |shell, _, cx| {
            let who = web("c13-limit");
            let params = json!({"projectId": project_id(PROJECT)});
            for _ in 0..cloud_policy::READ_LIMIT_PER_MINUTE {
                crate::mobile::cloud::prepare_options(shell, &who, &params, cx).unwrap();
            }
            assert_eq!(
                crate::mobile::cloud::prepare_options(shell, &who, &params, cx).err(),
                Some(policy::Code::Busy.failure())
            );
        })
        .unwrap();
}

// MARK: H-11

#[gpui::test]
fn a_cloud_start_is_checked_before_anything_is_made(cx: &mut TestAppContext) {
    let (window, _) = counted(cx);
    window
        .update(cx, |shell, _, cx| {
            shell.cloud_marks.insert(
                9001,
                CloudMark {
                    local_project: PROJECT.into(),
                    codespace: "cs1".into(),
                    kind: CloudBackendKind::Codespaces,
                },
            );
            let who = web("c11-check");
            let start = |shell: &Shell, who: &DeviceContext, params: Value, cx: &App| {
                crate::mobile::cloud::start_target(shell, who, PROJECT, &params, cx)
            };
            let target = json!({"kind": "codespace"});
            // No target: a local start, left alone.
            assert_eq!(start(shell, &who, json!({"text": "hi"}), cx), Ok(None));
            assert!(
                start(shell, &who, json!({"target": target}), cx)
                    .unwrap()
                    .is_some()
            );
            // A phone's target is refused: it keeps today's local start.
            let phone = device("c11-phone", DeviceKind::Phone, Permission::Control);
            assert_eq!(
                start(shell, &phone, json!({"target": target}), cx),
                Err(policy::Code::Unsupported.failure())
            );
            assert_eq!(start(shell, &phone, json!({"text": "hi"}), cx), Ok(None));
            // Fields the client may never set.
            for field in [
                "kit", "template", "image", "cpus", "memory", "ttl", "account",
            ] {
                let mut bad = target.clone();
                bad[field] = json!("x");
                assert_eq!(
                    start(shell, &who, json!({"target": bad}), cx),
                    Err(policy::Code::Forbidden.failure()),
                    "{field}"
                );
            }
            // Claude only, no draft, no worktree, no attachment.
            for extra in [
                json!({"harness": "codex"}),
                json!({"draft": true}),
                json!({"worktreePath": "/tmp/w"}),
                json!({"workspaceMode": "worktree"}),
                json!({"monoId": "m"}),
                json!({"attachments": [{"name": "a"}]}),
            ] {
                let mut params = json!({"target": target});
                params
                    .as_object_mut()
                    .unwrap()
                    .extend(extra.as_object().unwrap().clone());
                assert_eq!(
                    start(shell, &who, params, cx),
                    Err(policy::Code::UnsupportedTarget.failure()),
                    "{extra}"
                );
            }
            let claude = json!({"target": target, "harness": "claude"});
            assert!(start(shell, &who, claude, cx).unwrap().is_some());
            // An existing instance must be one the project uses, of that kind.
            let named =
                |kind: &str, name: &str| json!({"target": {"kind": kind, "instance": name}});
            assert!(
                start(shell, &who, named("codespace", "cs1"), cx)
                    .unwrap()
                    .is_some()
            );
            for (kind, name) in [("codespace", "cs2"), ("sandbox", "cs1")] {
                assert_eq!(
                    start(shell, &who, named(kind, name), cx),
                    Err(policy::Code::UnsupportedTarget.failure()),
                    "{kind} {name}"
                );
            }
        })
        .unwrap();
}

#[gpui::test]
fn a_value_outside_the_options_is_refused_and_costs_no_slot(cx: &mut TestAppContext) {
    let (window, calls) = counted(cx);
    let who = web("c11-values");
    let (got, done) = answer();
    window
        .update(cx, |shell, _, cx| {
            shell.mobile_start_cloud(
                request(&who, json!({"kind": "codespace", "branch": "not-a-branch"})),
                done,
                cx,
            );
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(
        got.borrow().clone(),
        Some(Err(policy::Code::Forbidden.failure()))
    );
    assert_eq!(calls_this_hour(&who.id), 0);
    assert_eq!(in_flight(&who.id), 0);
    assert_eq!(calls.count(), 0);
}

#[gpui::test]
fn a_start_counts_in_the_rate_limit(cx: &mut TestAppContext) {
    let (window, _) = counted(cx);
    let who = web("c11-limit");
    for _ in 0..cloud_policy::WAKE_LIMIT_PER_HOUR {
        let slot = cloud_policy::acquire_wake(&who.id).unwrap();
        cloud_policy::release_wake(&slot);
    }
    let (got, done) = answer();
    window
        .update(cx, |shell, _, cx| {
            shell.mobile_start_cloud(request(&who, json!({"kind": "sandbox"})), done, cx);
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(
        got.borrow().clone(),
        Some(Err(policy::Code::Busy.failure()))
    );
}

#[gpui::test]
fn a_refused_start_frees_its_slot_leaks_nothing_and_makes_no_session(cx: &mut TestAppContext) {
    let _serial = fakes::serial();
    let (window, _, _) = crate::shell::tests::window(cx);
    let who = web("c11-refused");
    let before = window
        .update(cx, |shell, _, _| {
            let (gh, _host) =
                fakes::install(shell.store.clone(), fakes::AUTH_NO_SCOPE, "Available");
            let _ = gh;
            (shell.order.clone(), shell.session_summaries.len())
        })
        .unwrap();
    let (got, done) = answer();
    window
        .update(cx, |shell, _, cx| {
            shell.mobile_start_cloud(request(&who, json!({"kind": "codespace"})), done, cx);
        })
        .unwrap();
    cx.run_until_parked();
    let reply = got.borrow().clone().expect("the start answered");
    let failure = reply.expect_err("the cloud refused the start");
    let text = serde_json::to_string(&failure.error).unwrap();
    // No path, address or command line from the cloud's own error.
    for leak in ["/", "http", "gh auth", "github.com", "="] {
        assert!(!text.contains(leak), "{leak} in {text}");
    }
    assert!(failure.error.chars().count() <= cloud_policy::STARTUP_ERROR_MAX_CHARS);
    // The call counted, and its slot is free again.
    assert_eq!(calls_this_hour(&who.id), 1);
    assert_eq!(in_flight(&who.id), 0);
    window
        .update(cx, |shell, _, _| {
            assert_eq!((shell.order.clone(), shell.session_summaries.len()), before);
        })
        .unwrap();
}

#[gpui::test]
fn a_held_slot_is_freed_when_its_operation_ends(cx: &mut TestAppContext) {
    let (window, _) = counted(cx);
    let who = web("c11-hold");
    let over = Rc::new(Cell::new(false));
    let flag = over.clone();
    window
        .update(cx, |shell, _, cx| {
            let slot = cloud_policy::acquire_wake(&who.id).unwrap();
            shell.hold_wake_slot(slot, move |_| flag.get(), cx);
        })
        .unwrap();
    cx.executor().advance_clock(Duration::from_secs(5));
    cx.run_until_parked();
    assert_eq!(in_flight(&who.id), 1, "still running");
    over.set(true);
    cx.executor().advance_clock(Duration::from_secs(5));
    cx.run_until_parked();
    assert_eq!(in_flight(&who.id), 0, "freed once it ended");
}

/// S5: the start's reply is bounded well inside the host's 25 s wait, and the first answer
/// (the start's own or the `starting` one at the bound) is the only one.
#[gpui::test]
fn a_start_answers_once_and_within_the_hosts_reply_wait(cx: &mut TestAppContext) {
    assert!(super::cloud_start::START_REPLY_BOUND <= Duration::from_secs(20));
    let (window, _, _) = crate::shell::tests::window(cx);
    window
        .update(cx, |shell, _, _| {
            let got: Rc<RefCell<Vec<Result<Value, Failure>>>> = Rc::default();
            let sink = got.clone();
            let done = RefCell::new(Some(move |_: &mut Shell, reply| {
                sink.borrow_mut().push(reply)
            }));
            assert!(super::cloud_start::answer_once(
                &done,
                shell,
                Ok(json!({"accepted": true, "starting": true})),
            ));
            assert!(
                !super::cloud_start::answer_once(&done, shell, Ok(json!({"sessionId": "late"}))),
                "a late answer says it was not delivered, so the caller can show it"
            );
            assert_eq!(
                *got.borrow(),
                [Ok(json!({"accepted": true, "starting": true}))],
                "the late reply is dropped"
            );
        })
        .unwrap();
}

// MARK: H-14

fn seed_move(shell: &mut Shell, id: i64, step: Option<&str>, branch: &str) {
    use crate::shell::cloud_move::MoveRun;
    let mut record: monocode_core::cloud_sessions::move_session::MoveRecord =
        serde_json::from_value(json!({
            "sessionId": id,
            "target": {"kind": "newCodespace", "machine": null},
            "kind": {"kind": "resume"},
            "projectCwd": "/home/me/secret-project",
            "workCwd": "/home/me/secret-project",
            "title": "T",
            "provider": "claude",
            "startedAt": 1,
            "done": ["snapshot", "push"],
            "current": step,
            "failedAt": null,
            "error": "push to git@github.com:me/secret.git failed",
            "branch": branch,
        }))
        .unwrap();
    record.kind = monocode_core::cloud_sessions::move_session::MoveKind::Resume;
    shell.cloud_moves.insert(id, MoveRun::new(Some(record)));
}

#[gpui::test]
fn move_progress_carries_only_a_step_id_and_a_status(cx: &mut TestAppContext) {
    let (window, id, _) = crate::shell::tests::window(cx);
    window
        .update(cx, |shell, _, cx| {
            assert_eq!(shell.mobile_move_progress(id), None);
            seed_move(shell, id, Some("workspace"), "feat/secret-branch");
            assert_eq!(
                shell.mobile_move_progress(id),
                Some(("workspace", "pending"))
            );
            // Moves of a local session add the `move` part only; the field is
            // the cloud's, so a local card has none. Build one from a mark.
            shell.cloud_marks.insert(
                id,
                CloudMark {
                    local_project: PROJECT.into(),
                    codespace: "cs-move".into(),
                    kind: CloudBackendKind::Codespaces,
                },
            );
            let fields = crate::mobile::cloud::frame_fields(shell, id, &web("c14-frame"), cx);
            assert_eq!(
                fields["cloud"]["move"],
                json!({"step": "workspace", "status": "pending"})
            );
            let text = Value::Object(fields).to_string();
            for leak in ["secret", "/home", "git@", "feat/"] {
                assert!(!text.contains(leak), "{leak} in {text}");
            }
            // Every step id is one of the 7.
            for step in monocode_core::cloud_sessions::move_session::MoveStep::ALL {
                assert!(
                    crate::mobile::cloud::MOVE_STEP_IDS
                        .contains(&crate::mobile::cloud::move_step_id(step))
                );
            }
        })
        .unwrap();
}

#[gpui::test]
fn move_calls_check_the_selection_and_the_params(cx: &mut TestAppContext) {
    let (window, id, other) = crate::shell::tests::window(cx);
    window
        .update(cx, |shell, window, cx| {
            let sid = shell.mobile_session_id(id);
            let who = web("c14-params");
            // A device that may not see the project gets `not_selected`.
            let mut narrowed = web("c14-narrow");
            narrowed.projects = Some(vec![project_id(PROJECT)]);
            let params = json!({"sessionId": sid});
            assert_eq!(
                crate::mobile::cloud::dispatch(shell, &narrowed, "cloud.cancelMove", &params, cx),
                Err(policy::Code::NotSelected.failure())
            );
            assert_eq!(
                crate::mobile::cloud::move_to_local(shell, &narrowed, &params, window, cx),
                Err(policy::Code::NotSelected.failure())
            );
            let to_cloud = json!({"sessionId": sid, "target": {"kind": "codespace"}});
            assert_eq!(
                crate::mobile::cloud::dispatch(
                    shell,
                    &narrowed,
                    "cloud.moveToCloud",
                    &to_cloud,
                    cx
                ),
                Err(policy::Code::NotSelected.failure())
            );
            // Nothing but the session id and a start `target` (S4): no kit, deleteWorkspace,
            // image, cpus, template or any other key.
            for (method, params) in [
                (
                    "cloud.cancelMove",
                    json!({"sessionId": sid, "deleteWorkspace": true}),
                ),
                (
                    "cloud.moveToCloud",
                    json!({"sessionId": sid, "target": {"kind": "codespace", "image": "x"}}),
                ),
                (
                    "cloud.moveToCloud",
                    json!({"sessionId": sid, "target": {"kind": "sandbox", "cpus": 64}}),
                ),
                (
                    "cloud.moveToCloud",
                    json!({"sessionId": sid, "target": {"kind": "codespace", "kit": "none"}}),
                ),
                (
                    "cloud.moveToCloud",
                    json!({"sessionId": sid, "kit": "default", "target": {"kind": "codespace"}}),
                ),
            ] {
                assert_eq!(
                    crate::mobile::cloud::dispatch(shell, &who, method, &params, cx),
                    Err(policy::Code::Forbidden.failure()),
                    "{method} {params}"
                );
            }
            // A cancel with no move running is refused and deletes nothing.
            let before = shell.session_summaries.len();
            assert!(
                crate::mobile::cloud::dispatch(shell, &who, "cloud.cancelMove", &params, cx)
                    .is_err()
            );
            assert_eq!(shell.session_summaries.len(), before);
            assert!(shell.session_summaries.contains_key(&other));
            // Nothing was claimed by any of these.
            assert_eq!(in_flight(&who.id), 0);
            assert_eq!(calls_this_hour(&who.id), 0);
        })
        .unwrap();
}

/// S4: a move takes the keys of a start `target` (what the web sends), or none (the
/// desktop's default), and every refusal carries a code.
#[gpui::test]
fn a_move_target_takes_the_start_keys_and_every_refusal_is_coded(cx: &mut TestAppContext) {
    let (window, id, _) = crate::shell::tests::window(cx);
    window
        .update(cx, |shell, _, cx| {
            let sid = shell.mobile_session_id(id);
            let who = web("c14-keys");
            let coded = |result: Result<Value, Failure>| {
                let failure = result.expect_err("the desktop refuses this session");
                failure.code.clone().expect("a code")
            };
            for target in [
                json!({"kind": "codespace", "branch": "x"}),
                json!({"kind": "codespace", "machine": "basicLinux32gb"}),
                json!({"kind": "sandbox", "size": "2cpu-4g"}),
                json!({"kind": "codespace", "instance": "new"}),
            ] {
                let params = json!({"sessionId": sid, "target": target});
                let code = coded(crate::mobile::cloud::dispatch(
                    shell,
                    &who,
                    "cloud.moveToCloud",
                    &params,
                    cx,
                ));
                // Past the shape check: the desktop's own refusal, coded.
                assert_eq!(code, "unsupported_target", "{target}");
            }
            // Wrong types and impossible pairs are `forbidden`.
            for target in [
                json!({"kind": "codespace", "branch": 5}),
                json!({"kind": "codespace", "size": "2cpu-4g"}),
                json!({"kind": "vm"}),
                json!("codespace"),
            ] {
                let params = json!({"sessionId": sid, "target": target});
                assert_eq!(
                    coded(crate::mobile::cloud::dispatch(
                        shell,
                        &who,
                        "cloud.moveToCloud",
                        &params,
                        cx
                    )),
                    "forbidden",
                    "{target}"
                );
            }
            // No target: the desktop's default (the first target its dialog offers).
            // This session offers none, so the refusal is coded too.
            let params = json!({"sessionId": sid});
            assert_eq!(
                coded(crate::mobile::cloud::dispatch(
                    shell,
                    &who,
                    "cloud.moveToCloud",
                    &params,
                    cx
                )),
                "unsupported_target"
            );
            let params = json!({"target": {"kind": "codespace"}});
            assert_eq!(
                coded(crate::mobile::cloud::dispatch(
                    shell,
                    &who,
                    "cloud.moveToCloud",
                    &params,
                    cx
                )),
                "forbidden"
            );
            assert_eq!(in_flight(&who.id), 0, "no refusal holds a slot");
        })
        .unwrap();
}

/// A cancel with a move of the agent's own running reaches the engine, and never asks for
/// the workspace to be deleted (the params take nothing else).
#[gpui::test]
fn a_cancel_with_a_move_running_is_accepted_and_deletes_nothing(cx: &mut TestAppContext) {
    let (window, id, other) = crate::shell::tests::window(cx);
    window
        .update(cx, |shell, _, cx| {
            let sid = shell.mobile_session_id(id);
            let who = web("c14-cancel");
            seed_move(shell, id, Some("upload"), "feat/x");
            shell.cloud_moves.get_mut(&id).unwrap().by_api = true;
            let before = shell.session_summaries.len();
            let reply = crate::mobile::cloud::dispatch(
                shell,
                &who,
                "cloud.cancelMove",
                &json!({"sessionId": sid}),
                cx,
            );
            assert_eq!(reply, Ok(json!({"accepted": true})), "{reply:?}");
            assert_eq!(shell.session_summaries.len(), before);
            assert!(shell.session_summaries.contains_key(&other));
        })
        .unwrap();
}

#[gpui::test]
fn a_move_counts_in_the_rate_limit_and_a_refusal_frees_the_slot(cx: &mut TestAppContext) {
    let (window, id, _) = crate::shell::tests::window(cx);
    window
        .update(cx, |shell, _, cx| {
            let sid = shell.mobile_session_id(id);
            let who = web("c14-limit");
            let params = json!({"sessionId": sid, "target": {"kind": "codespace"}});
            // The desktop refuses (the session has no messages to move): the
            // slot goes back, but the call stays counted.
            let first =
                crate::mobile::cloud::dispatch(shell, &who, "cloud.moveToCloud", &params, cx);
            assert!(first.is_err(), "{first:?}");
            assert_eq!(in_flight(&who.id), 0);
            assert_eq!(calls_this_hour(&who.id), 1);
            // Past the hourly limit the call is `busy` before it asks the desktop.
            for _ in 1..cloud_policy::WAKE_LIMIT_PER_HOUR {
                let slot = cloud_policy::acquire_wake(&who.id).unwrap();
                cloud_policy::release_wake(&slot);
            }
            assert_eq!(
                crate::mobile::cloud::dispatch(shell, &who, "cloud.moveToCloud", &params, cx),
                Err(policy::Code::Busy.failure())
            );
        })
        .unwrap();
}

// MARK: gates

/// The policy table answers before the app: `disabled` per toggle, `read_only`
/// for a reader, and control needed (H-11, H-14).
#[test]
fn the_cloud_methods_are_gated_by_their_toggles_and_the_permission() {
    use policy::{Code, PolicyFlags};
    let flags = |push, cloud| PolicyFlags {
        remote_git_push: push,
        remote_cloud_start: cloud,
        remote_approvals: false,
        show_push_content: true,
    };
    let control = device("gate-control", DeviceKind::Web, Permission::Control);
    let reader = device("gate-reader", DeviceKind::Web, Permission::Read);
    let code = |who: &DeviceContext, method: &str, params: Value, on: PolicyFlags| {
        policy::authorize_with(who, method, &params, &on, false)
            .err()
            .map(|denial| denial.code)
    };
    let start = json!({"target": {"kind": "codespace"}});
    assert_eq!(
        code(&control, "session.start", start.clone(), flags(true, false)),
        Some(Code::Disabled)
    );
    assert_eq!(
        code(&control, "session.start", start.clone(), flags(false, true)),
        None
    );
    assert_eq!(
        code(&reader, "session.start", start, flags(false, true)),
        Some(Code::ReadOnly)
    );
    for method in ["cloud.moveToCloud", "cloud.moveToLocal", "cloud.cancelMove"] {
        let params = json!({});
        // Each toggle on its own is not enough.
        assert_eq!(
            code(&control, method, params.clone(), flags(true, false)),
            Some(Code::Disabled),
            "{method}"
        );
        assert_eq!(
            code(&control, method, params.clone(), flags(false, true)),
            Some(Code::Disabled),
            "{method}"
        );
        assert_eq!(
            code(&control, method, params.clone(), flags(false, false)),
            Some(Code::Disabled),
            "{method}"
        );
        assert_eq!(
            code(&control, method, params.clone(), flags(true, true)),
            None,
            "{method}"
        );
        assert_eq!(
            code(&reader, method, params, flags(true, true)),
            Some(Code::ReadOnly),
            "{method}"
        );
    }
    // The options are a read.
    assert_eq!(
        code(
            &reader,
            "cloud.startOptions",
            json!({"projectId": "p"}),
            flags(false, false)
        ),
        None
    );
}
