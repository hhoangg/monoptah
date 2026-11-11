//! Mono dispatch of the `cloud.*` actions and of cloud sessions, for codespaces and sandboxes.
use super::tests::{Setup, call, call_from, fake_run, launches_of, setup, setup_with};
use crate::cloud_wake::fakes::{
    AUTH_OK, FakeHost, Gh, SANDBOX, Sbx, install, install_with_sandbox, sandbox_session, serial,
};
use crate::runtime::locked;
use gpui::TestAppContext;
use serde_json::{Value, json};
use std::sync::Arc;

type CloudSetup = (
    Setup,
    Arc<Gh>,
    Arc<FakeHost>,
    std::sync::MutexGuard<'static, ()>,
);
fn git_project(s: &Setup) {
    for args in [
        vec!["init", "-q", "-b", "trunk"],
        vec!["remote", "add", "origin", "https://github.com/octo/widgets"],
    ] {
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(&s.project)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }
}
/// A Mono whose project is a git repo with a GitHub origin, over fake gh and host.
fn cloud_setup(cx: &mut TestAppContext, state: &'static str) -> CloudSetup {
    let guard = serial();
    let s = setup(cx);
    git_project(&s);
    let (gh, host) = install(s.store.clone(), AUTH_OK, state);
    host.0.lock().unwrap().clear();
    (s, gh, host, guard)
}
/// [`cloud_setup`] with the fake Docker sandbox `box-1` (in `sandbox_state`) registered too.
fn sandbox_setup(
    cx: &mut TestAppContext,
    state: &'static str,
    sandbox_state: &str,
) -> (CloudSetup, Arc<Sbx>) {
    let guard = serial();
    let s = setup(cx);
    git_project(&s);
    let (gh, host, sbx, _) = install_with_sandbox(s.store.clone(), AUTH_OK, state, sandbox_state);
    host.0.lock().unwrap().clear();
    ((s, gh, host, guard), sbx)
}
fn run(
    s: &Setup,
    request: &str,
    action: &str,
    input: Value,
    cx: &mut TestAppContext,
) -> Result<Value, String> {
    call(s, request, action, input, cx).try_recv().unwrap()
}
/// Shows a stored session in the shell, as the sidebar does for a cloud session.
fn show(s: &Setup, summary: monocode_core::store::Session, cx: &mut TestAppContext) -> i64 {
    let id = summary.id;
    cx.update(|cx| {
        s.manager
            .read(cx)
            .owner
            .upgrade()
            .unwrap()
            .update(cx, |owner, cx| owner.mono_add_session(summary, cx))
    });
    cx.run_until_parked();
    id
}
/// A cloud session of the Mono's project on the fake codespace, shown in the shell.
fn cloud_session(s: &Setup, cx: &mut TestAppContext) -> i64 {
    use monocode_core::cloud_sessions::CloudBackendKind;
    use monocode_core::mono::cloud_api::{CloudTarget, InstanceChoice};
    let api = monocode_core::mono::cloud_api::CloudApi::shared(&s.store).unwrap();
    let binding = api
        .prepare(
            &s.project,
            CloudTarget::new(
                CloudBackendKind::Codespaces,
                InstanceChoice::Named("octo-run".into()),
            ),
            &mut |_| {},
        )
        .unwrap();
    let summary = api.create_session(&binding, "claude").unwrap();
    show(s, summary, cx)
}
/// A session of the Mono's project on the fake sandbox `box-1`, shown in the shell.
fn sandbox_cloud_session(s: &Setup, cx: &mut TestAppContext) -> i64 {
    show(s, sandbox_session(&s.store, &s.project), cx)
}
/// Marks the remote session as running a turn (a busy session on its instance).
fn set_running(s: &Setup, id: i64) {
    let store = locked(&s.store).unwrap();
    let mut remote = store.remote_session(id).unwrap().unwrap();
    remote.running = true;
    store.save_remote_session(&remote).unwrap();
}

#[gpui::test]
fn a_mono_lists_the_instances_of_both_backends_and_reads_their_status(cx: &mut TestAppContext) {
    let ((s, _, _, _guard), sbx) = sandbox_setup(cx, "Shutdown", "running");
    let codespace = cloud_session(&s, cx);
    let sandbox = sandbox_cloud_session(&s, cx);
    let list = run(&s, "c1", "cloud.list", json!({}), cx).unwrap();
    assert_eq!(list["backends"]["codespace"]["ok"], true, "{list}");
    assert_eq!(list["backends"]["sandbox"]["ok"], true, "{list}");
    let rows = list["instances"].as_array().unwrap();
    let find = |kind: &str| {
        rows.iter()
            .find(|row| row["kind"] == kind)
            .unwrap_or_else(|| panic!("{kind} row in {list}"))
    };
    let cs = find("codespace");
    assert_eq!(cs["instance"], "octo-run");
    assert_eq!(cs["state"], "stopped");
    assert_eq!(cs["sessions"][0]["sessionId"], codespace);
    let sb = find("sandbox");
    assert_eq!(sb["instance"], SANDBOX);
    assert_eq!(sb["state"], "available");
    assert_eq!(sb["sessions"][0]["sessionId"], sandbox);
    // One backend only, and an unknown field is refused before anything runs.
    let only = run(&s, "c2", "cloud.list", json!({"kind":"sandbox"}), cx).unwrap();
    assert!(
        only["instances"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["kind"] == "sandbox")
    );
    assert!(run(&s, "c3", "cloud.list", json!({"name":"x"}), cx).is_err());
    // Status never carries a secret value, and the codespace secrets are listed by key.
    let status = run(&s, "c4", "cloud.status", json!({}), cx).unwrap();
    assert_eq!(
        status["backends"]["codespace"]["secrets"][0]["key"], "CLAUDE_CODE_OAUTH_TOKEN",
        "{status}"
    );
    assert_eq!(status["backends"]["sandbox"]["installed"], true, "{status}");
    assert!(
        !sbx.calls
            .lock()
            .unwrap()
            .iter()
            .any(|c| c.contains(" run ")),
        "listing starts nothing"
    );
}

#[gpui::test]
fn a_mono_stops_resumes_and_deletes_a_codespace(cx: &mut TestAppContext) {
    let (s, gh, host, _guard) = cloud_setup(cx, "Shutdown");
    let by_name = json!({"kind":"codespace","instance":"octo-run"});
    let stopped = run(&s, "k1", "cloud.stop", by_name.clone(), cx).unwrap();
    assert_eq!(
        stopped,
        json!({"kind":"codespace","instance":"octo-run","stopped":true,"sessions":[]})
    );
    // A codespace of another repository is refused before gh is asked to delete it.
    let error = run(
        &s,
        "k2",
        "cloud.delete",
        json!({"kind":"codespace","instance":"elsewhere-1"}),
        cx,
    )
    .unwrap_err();
    assert!(error.contains("not a codespace of octo/widgets"), "{error}");
    // Resuming goes through a session of the project on it.
    let id = cloud_session(&s, cx);
    host.0.lock().unwrap().clear();
    let resumed = run(&s, "k3", "cloud.resume", json!({"sessionId":id}), cx).unwrap();
    assert_eq!(resumed["resumed"], true, "{resumed}");
    assert_eq!(resumed["kind"], "codespace");
    assert!(
        host.0
            .lock()
            .unwrap()
            .iter()
            .any(|l| l == "ensure m-octo-run"),
        "{:?}",
        host.0.lock().unwrap()
    );
    // Delete is irreversible but allowed for a Mono once nothing is busy.
    let deleted = run(&s, "k4", "cloud.delete", by_name, cx).unwrap();
    assert_eq!(deleted["deleted"], true, "{deleted}");
    let calls = gh.calls.lock().unwrap();
    assert!(
        calls
            .iter()
            .any(|c| c.contains("codespace delete -c octo-run"))
    );
    assert!(
        !calls
            .iter()
            .any(|c| c.contains("elsewhere-1") && c.contains("delete"))
    );
}

#[gpui::test]
fn a_mono_stops_resumes_and_deletes_a_sandbox_through_its_session(cx: &mut TestAppContext) {
    let ((s, _, host, _guard), sbx) = sandbox_setup(cx, "Shutdown", "running");
    let id = sandbox_cloud_session(&s, cx);
    let stopped = run(&s, "b1", "cloud.stop", json!({"sessionId":id}), cx).unwrap();
    assert_eq!(stopped["kind"], "sandbox", "{stopped}");
    assert_eq!(stopped["instance"], SANDBOX);
    assert_eq!(stopped["stopped"], true);
    assert_eq!(stopped["sessions"][0]["sessionId"], id);
    assert!(
        sbx.calls
            .lock()
            .unwrap()
            .iter()
            .any(|c| c.contains(" stop ")),
        "{:?}",
        sbx.calls.lock().unwrap()
    );
    *sbx.state.lock().unwrap() = "stopped".into();
    host.0.lock().unwrap().clear();
    let resumed = run(&s, "b2", "cloud.resume", json!({"sessionId":id}), cx).unwrap();
    assert_eq!(resumed["resumed"], true, "{resumed}");
    assert!(
        host.0
            .lock()
            .unwrap()
            .iter()
            .any(|l| l == "request m-sbx-box-1 environment.describe"),
        "the sandbox was reconnected: {:?}",
        host.0.lock().unwrap()
    );
    // By kind and instance, and refused for an instance no session of the project uses.
    let unknown = run(
        &s,
        "b3",
        "cloud.delete",
        json!({"kind":"sandbox","instance":"other-box"}),
        cx,
    )
    .unwrap_err();
    assert!(unknown.contains("not used by a session"), "{unknown}");
    let deleted = run(
        &s,
        "b4",
        "cloud.delete",
        json!({"kind":"sandbox","instance":SANDBOX}),
        cx,
    );
    let deleted = deleted.unwrap();
    assert_eq!(deleted["deleted"], true, "{deleted}");
    assert_eq!(deleted["instance"], SANDBOX);
    assert!(
        sbx.calls.lock().unwrap().iter().any(|c| c.contains(" rm ")),
        "{:?}",
        sbx.calls.lock().unwrap()
    );
}

#[gpui::test]
fn cloud_delete_waits_while_a_session_is_busy(cx: &mut TestAppContext) {
    let ((s, gh, _, _guard), sbx) = sandbox_setup(cx, "Available", "running");
    let codespace = cloud_session(&s, cx);
    let sandbox = sandbox_cloud_session(&s, cx);
    // A session working on a running instance blocks delete for either backend (on a stopped
    // instance the stored flag is stale and ignored).
    set_running(&s, codespace);
    set_running(&s, sandbox);
    for (n, input) in [
        json!({"sessionId":codespace}),
        json!({"kind":"sandbox","instance":SANDBOX}),
    ]
    .into_iter()
    .enumerate()
    {
        let error = run(&s, &format!("busy-{n}"), "cloud.delete", input, cx).unwrap_err();
        assert!(error.contains("sessions are working on it"), "{error}");
    }
    assert!(
        !gh.calls
            .lock()
            .unwrap()
            .iter()
            .any(|c| c.contains("codespace delete")),
        "nothing was deleted"
    );
    assert!(!sbx.calls.lock().unwrap().iter().any(|c| c.contains(" rm ")));
}

#[gpui::test]
fn an_archive_in_flight_blocks_cloud_delete_and_refuses_stop_on_both_backends(
    cx: &mut TestAppContext,
) {
    use monocode_core::cloud_sessions::CloudRef;
    let ((s, gh, _, _guard), sbx) = sandbox_setup(cx, "Shutdown", "running");
    let sandbox = sandbox_cloud_session(&s, cx);
    for cloud in [CloudRef::codespace("octo-run"), CloudRef::sandbox(SANDBOX)] {
        s.window
            .update(cx, |shell, _, _| shell.mono_test_busy_cloud(&cloud))
            .unwrap();
    }
    // However the instance is addressed, the snapshot of the shell's busy keys decides.
    for (n, input) in [
        json!({"kind":"codespace","instance":"octo-run"}),
        json!({"sessionId":sandbox}),
    ]
    .into_iter()
    .enumerate()
    {
        let error = run(&s, &format!("a-{n}"), "cloud.delete", input, cx).unwrap_err();
        assert!(error.contains("archiving or converting"), "{error}");
    }
    assert!(
        !gh.calls
            .lock()
            .unwrap()
            .iter()
            .any(|c| c.contains("codespace delete")),
        "nothing was deleted"
    );
    assert!(!sbx.calls.lock().unwrap().iter().any(|c| c.contains(" rm ")));
    // Stopping is futile while an archive works on the instance (it would resume it): refused,
    // and nothing is stopped, for either backend.
    for (n, input) in [
        json!({"kind":"codespace","instance":"octo-run"}),
        json!({"sessionId":sandbox}),
    ]
    .into_iter()
    .enumerate()
    {
        let error = run(&s, &format!("a-stop-{n}"), "cloud.stop", input, cx).unwrap_err();
        assert!(error.starts_with("Cannot stop "), "{error}");
        assert!(error.contains("archiving or converting"), "{error}");
    }
    assert!(
        !gh.calls
            .lock()
            .unwrap()
            .iter()
            .any(|c| c.contains("codespace stop")),
        "nothing was stopped"
    );
    assert!(
        !sbx.calls
            .lock()
            .unwrap()
            .iter()
            .any(|c| c.contains(" stop "))
    );
}

#[gpui::test]
fn a_mono_lists_reads_and_stops_cloud_sessions_by_their_negative_ids(cx: &mut TestAppContext) {
    let _clis = crate::shell::surface_tests::NoClis::new();
    let (s, _, host, _guard) = cloud_setup(cx, "Shutdown");
    let id = cloud_session(&s, cx);
    assert!(
        id < 0,
        "cloud sessions are remote, so their ids are negative"
    );
    host.0.lock().unwrap().clear();
    let list = run(&s, "l1", "sessions.list", json!({}), cx).unwrap();
    let row = list["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == id)
        .expect("the project's cloud session is listed")
        .clone();
    assert_eq!(
        row["target"],
        json!({"kind":"codespace","instance":"octo-run","state":"stopped"})
    );
    assert_eq!(row["cloudState"], "stopped");
    assert!(row.get("codespaceState").is_none());
    let read = run(&s, "r1", "sessions.read", json!({"sessionId":id}), cx).unwrap();
    assert_eq!(read["target"]["instance"], "octo-run");
    assert_eq!(read["cloudState"], "stopped");
    assert_eq!(
        run(&s, "s1", "sessions.stop", json!({"sessionId":id}), cx).unwrap(),
        json!({"accepted":true})
    );
    assert!(
        host.0.lock().unwrap().is_empty(),
        "reading never wakes the instance"
    );
}

#[gpui::test]
fn a_send_to_a_cloud_session_wakes_a_stopped_codespace_and_can_notify(cx: &mut TestAppContext) {
    let _clis = crate::shell::surface_tests::NoClis::new();
    let (s, _, host, _guard) = cloud_setup(cx, "Shutdown");
    let id = cloud_session(&s, cx);
    host.0.lock().unwrap().clear();
    let sent = run(
        &s,
        "m1",
        "sessions.send",
        json!({"sessionId":id,"prompt":"go","notifyOnComplete":true}),
        cx,
    );
    sent.unwrap();
    assert!(
        host.0
            .lock()
            .unwrap()
            .iter()
            .any(|l| l == "ensure m-octo-run"),
        "{:?}",
        host.0.lock().unwrap()
    );
}

/// gh says Available but the host has no live tunnel: the send is queued and still reconnects.
/// An Unavailable codespace is left alone.
#[gpui::test]
fn a_send_reconnects_an_available_codespace_and_skips_an_unavailable_one(cx: &mut TestAppContext) {
    for (state, woken) in [("Available", true), ("Failed", false)] {
        let _clis = crate::shell::surface_tests::NoClis::new();
        let (s, _, host, _guard) = cloud_setup(cx, state);
        let id = cloud_session(&s, cx);
        host.0.lock().unwrap().clear();
        run(
            &s,
            "m1",
            "sessions.send",
            json!({"sessionId":id,"prompt":"go"}),
            cx,
        )
        .unwrap();
        assert_eq!(
            host.0
                .lock()
                .unwrap()
                .iter()
                .any(|l| l == "ensure m-octo-run"),
            woken,
            "{state}: {:?}",
            host.0.lock().unwrap()
        );
    }
}

#[gpui::test]
fn a_mono_notified_send_to_a_cloud_session_is_tracked_as_a_launch(cx: &mut TestAppContext) {
    let _clis = crate::shell::surface_tests::NoClis::new();
    let (s, _, _, _guard) = cloud_setup(cx, "Available");
    let id = cloud_session(&s, cx);
    run(
        &s,
        "n1",
        "sessions.send",
        json!({"sessionId":id,"prompt":"go","notifyOnComplete":true}),
        cx,
    )
    .unwrap();
    let launches = launches_of(&s, cx);
    let launch = launches
        .iter()
        .find(|l| l.session_id == id)
        .expect("tracked");
    assert!(launch.notify_on_complete && !launch.completed);
    assert_eq!(launch.cwd, s.project);
}

#[gpui::test]
fn a_mono_start_with_a_target_fails_before_creating_anything_when_validation_fails(
    cx: &mut TestAppContext,
) {
    let (s, gh, _, _guard) = cloud_setup(cx, "Shutdown");
    for (request, input) in [
        (
            "v1",
            json!({"prompt":"x","harness":"codex","target":{"kind":"codespace","instance":"new"}}),
        ),
        (
            "v2",
            json!({"prompt":"x","target":{"kind":"codespace","instance":"new"},"parentSessionId":999}),
        ),
        (
            "v3",
            json!({"prompt":"x","target":{"kind":"codespace","instance":"new"},"folderId":"nope"}),
        ),
    ] {
        assert!(
            run(&s, request, "sessions.start", input, cx).is_err(),
            "{request}"
        );
    }
    assert!(
        !gh.calls
            .lock()
            .unwrap()
            .iter()
            .any(|c| c.contains("codespace create") || c.contains("--method POST")),
        "no codespace may be created for a start that cannot succeed"
    );
}

#[gpui::test]
fn a_mono_start_with_an_account_and_a_cloud_target_is_refused_before_any_cloud_call(
    cx: &mut TestAppContext,
) {
    let ((s, gh, host, _guard), sbx) = sandbox_setup(cx, "Shutdown", "running");
    let (gh_before, sbx_before) = (
        gh.calls.lock().unwrap().len(),
        sbx.calls.lock().unwrap().len(),
    );
    let sessions = super::tests::session_count(&s);
    for (request, target) in [
        ("a1", json!({"kind":"codespace","instance":"new"})),
        ("a2", json!({"kind":"codespace","instance":"octo-run"})),
        ("a3", json!({"kind":"sandbox","instance":"new"})),
        ("a4", json!({"kind":"sandbox","instance":"box-1"})),
        ("a5", json!({"kind":"sandbox","instance":{"session":1}})),
    ] {
        let error = run(
            &s,
            request,
            "sessions.start",
            json!({"prompt":"x","account":"Work","target":target}),
            cx,
        )
        .unwrap_err();
        assert_eq!(
            error,
            monocode_core::mono::api::ACCOUNT_LOCAL_ONLY,
            "{request}"
        );
    }
    assert_eq!(gh.calls.lock().unwrap().len(), gh_before, "no gh call");
    assert_eq!(sbx.calls.lock().unwrap().len(), sbx_before, "no sbx call");
    assert!(host.0.lock().unwrap().is_empty());
    assert_eq!(super::tests::session_count(&s), sessions, "no session row");
}

#[gpui::test]
fn a_draft_for_a_cloud_session_never_wakes_its_codespace(cx: &mut TestAppContext) {
    let _clis = crate::shell::surface_tests::NoClis::new();
    let (s, _, host, _guard) = cloud_setup(cx, "Shutdown");
    let id = cloud_session(&s, cx);
    host.0.lock().unwrap().clear();
    run(
        &s,
        "d1",
        "sessions.draft",
        json!({"sessionId":id,"prompt":"later"}),
        cx,
    )
    .unwrap();
    assert!(
        host.0.lock().unwrap().is_empty(),
        "{:?}",
        host.0.lock().unwrap()
    );
}

/// A cloud session a Mono starts belongs to the local project: the sidebar lists it there at once
/// (not after the next refresh), and the launch and its folder are filed under that project, not
/// under the codespace's remote key, which no sidebar shows.
#[gpui::test]
fn a_cloud_session_a_mono_starts_is_listed_and_filed_under_the_local_project(
    cx: &mut TestAppContext,
) {
    let _clis = crate::shell::surface_tests::NoClis::new();
    let (s, _, _, _guard) = cloud_setup(cx, "Available");
    s.manager
        .update(cx, |m, cx| m.sidebar_preferences(&s.id, true, true, cx));
    cx.run_until_parked();
    let started = run(
        &s,
        "cs1",
        "sessions.start",
        json!({"prompt":"go","title":"Codespace test","target":{"kind":"codespace","instance":"octo-run"}}),
        cx,
    )
    .unwrap();
    let id = started["sessionId"].as_i64().unwrap();
    assert!(
        id < 0,
        "cloud sessions are remote, so their ids are negative"
    );
    let owner = cx.update(|cx| s.manager.read(cx).owner.upgrade().unwrap());
    cx.update(|cx| {
        let owner = owner.read(cx);
        assert_eq!(
            owner.mono_listed_project(id).as_deref(),
            Some(s.project.as_str()),
            "listed under the local project right away"
        );
        let groups = owner.mono_groups(&s.project);
        assert!(
            groups.folders.iter().any(|f| f.session_ids.contains(&id)),
            "filed in a folder of the local project: {groups:?}"
        );
        assert!(!s.manager.read(cx).hidden_session(id));
    });
    let launches = launches_of(&s, cx);
    let launch = launches.iter().find(|l| l.session_id == id).unwrap();
    assert_eq!(launch.cwd, s.project);
}

#[gpui::test]
fn a_mono_returns_a_pending_cloud_session_before_provisioning_finishes(cx: &mut TestAppContext) {
    let _clis = crate::shell::surface_tests::NoClis::new();
    let (setup, gh, host, _guard) = cloud_setup(cx, "Failed");
    setup.manager.update(cx, |manager, cx| {
        manager.sidebar_preferences(&setup.id, true, true, cx)
    });
    let input = json!({"prompt":"inspect the environment","title":"Starting codespace","target":{"kind":"codespace","instance":"new","branch":"trunk"}});
    let started = run(&setup, "pending-cloud", "sessions.start", input.clone(), cx).unwrap();
    let session_id = started["sessionId"].as_i64().unwrap();
    assert_eq!(started["cloudState"], "starting");
    assert_eq!(started["target"]["kind"], "codespace");
    assert_eq!(started["target"]["state"], "starting");
    assert!(started["startup"].get("step").is_some());
    let owner = cx.update(|cx| setup.manager.read(cx).owner.upgrade().unwrap());
    cx.update(|cx| {
        let shell = owner.read(cx);
        assert_eq!(
            shell.mono_listed_project(session_id).as_deref(),
            Some(setup.project.as_str())
        );
        let conversation = shell.mono_session_entity(session_id).unwrap();
        assert!(conversation.read(cx).cloud_start_blocks());
        assert!(
            conversation
                .read(cx)
                .remote_debug()
                .contains("waking=false")
        );
        assert!(
            shell
                .mono_session_views(cx)
                .iter()
                .find(|session| session.id == session_id)
                .unwrap()
                .busy
        );
        assert_eq!(
            conversation.read(cx).cloud_queued(),
            Some("inspect the environment")
        );
        assert!(
            shell
                .mono_groups(&setup.project)
                .folders
                .iter()
                .any(|folder| folder.session_ids.contains(&session_id))
        );
    });
    assert!(host.0.lock().unwrap().is_empty());
    assert!(
        !gh.calls
            .lock()
            .unwrap()
            .iter()
            .any(|call| call.contains("codespace view"))
    );
    assert_eq!(
        run(&setup, "pending-cloud", "sessions.start", input, cx).unwrap(),
        started
    );
    assert_eq!(
        gh.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| call.contains("--method POST"))
            .count(),
        1
    );
    let listed = run(&setup, "pending-list", "sessions.list", json!({}), cx).unwrap();
    let row = listed["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["sessionId"] == session_id)
        .unwrap();
    assert_eq!(row["cloudState"], "starting");
    assert!(row.get("startup").is_some());
    let page = run(
        &setup,
        "pending-read",
        "sessions.read",
        json!({"sessionId":session_id}),
        cx,
    )
    .unwrap();
    assert_eq!(page["cloudState"], "starting");
    let error = run(
        &setup,
        "pending-send",
        "sessions.send",
        json!({"sessionId":session_id,"prompt":"another prompt"}),
        cx,
    )
    .unwrap_err();
    assert!(error.contains("still starting"), "{error}");
    assert!(host.0.lock().unwrap().is_empty());
}

const DESCRIBE: &str = "request m-sbx-box-1 environment.describe";

#[gpui::test]
fn a_mono_lists_and_reads_a_sandbox_session_with_the_same_target_shape(cx: &mut TestAppContext) {
    let _clis = crate::shell::surface_tests::NoClis::new();
    let ((s, _, host, _guard), _) = sandbox_setup(cx, "Shutdown", "stopped");
    let id = sandbox_cloud_session(&s, cx);
    host.0.lock().unwrap().clear();
    let list = run(&s, "l1", "sessions.list", json!({}), cx).unwrap();
    let row = list["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == id)
        .expect("the sandbox session is listed")
        .clone();
    assert_eq!(
        row["target"],
        json!({"kind":"sandbox","instance":SANDBOX,"state":"stopped"})
    );
    assert_eq!(row["cloudState"], "stopped");
    let read = run(&s, "r1", "sessions.read", json!({"sessionId":id}), cx).unwrap();
    assert_eq!(read["target"]["kind"], "sandbox");
    assert_eq!(read["cloudState"], "stopped");
    assert!(
        host.0.lock().unwrap().is_empty(),
        "reading never wakes the sandbox: {:?}",
        host.0.lock().unwrap()
    );
}

/// A Mono's send is explicit enough to resume a sandbox, like a codespace.
#[gpui::test]
fn a_send_to_a_session_on_a_stopped_sandbox_wakes_it(cx: &mut TestAppContext) {
    let _clis = crate::shell::surface_tests::NoClis::new();
    let ((s, _, host, _guard), _) = sandbox_setup(cx, "Shutdown", "stopped");
    let id = sandbox_cloud_session(&s, cx);
    host.0.lock().unwrap().clear();
    run(
        &s,
        "m1",
        "sessions.send",
        json!({"sessionId":id,"prompt":"go","notifyOnComplete":true}),
        cx,
    )
    .unwrap();
    assert!(
        host.0.lock().unwrap().iter().any(|l| l == DESCRIBE),
        "{:?}",
        host.0.lock().unwrap()
    );
}

#[gpui::test]
fn a_draft_for_a_session_on_a_sandbox_never_wakes_it(cx: &mut TestAppContext) {
    let _clis = crate::shell::surface_tests::NoClis::new();
    let ((s, _, host, _guard), _) = sandbox_setup(cx, "Shutdown", "stopped");
    let id = sandbox_cloud_session(&s, cx);
    host.0.lock().unwrap().clear();
    run(
        &s,
        "d1",
        "sessions.draft",
        json!({"sessionId":id,"prompt":"later"}),
        cx,
    )
    .unwrap();
    assert!(
        host.0.lock().unwrap().is_empty(),
        "{:?}",
        host.0.lock().unwrap()
    );
}

#[gpui::test]
fn a_mono_starts_a_session_on_an_existing_sandbox_without_a_startup(cx: &mut TestAppContext) {
    let _clis = crate::shell::surface_tests::NoClis::new();
    let ((s, _, host, _guard), _) = sandbox_setup(cx, "Shutdown", "stopped");
    let existing = sandbox_cloud_session(&s, cx);
    s.manager
        .update(cx, |m, cx| m.sidebar_preferences(&s.id, true, true, cx));
    host.0.lock().unwrap().clear();
    for (request, instance) in [
        ("sb-session", json!({"session":existing})),
        ("sb-name", json!(SANDBOX)),
    ] {
        let started = run(
            &s,
            request,
            "sessions.start",
            json!({"prompt":"go","target":{"kind":"sandbox","instance":instance}}),
            cx,
        )
        .unwrap();
        let id = started["sessionId"].as_i64().unwrap();
        assert!(id < 0 && id != existing, "{started}");
        assert_eq!(
            started["target"],
            json!({"kind":"sandbox","instance":SANDBOX,"state":"stopped"})
        );
        assert_eq!(started["cloudState"], "stopped");
        assert!(started.get("startup").is_none(), "{started}");
        let owner = cx.update(|cx| s.manager.read(cx).owner.upgrade().unwrap());
        cx.update(|cx| {
            assert_eq!(
                owner.read(cx).mono_listed_project(id).as_deref(),
                Some(s.project.as_str())
            );
            let conversation = owner.read(cx).mono_session_entity(id).unwrap();
            assert!(!conversation.read(cx).cloud_start_blocks());
        });
        let record = locked(&s.store)
            .unwrap()
            .remote_session(id)
            .unwrap()
            .unwrap();
        assert_eq!(
            record.project.cloud(),
            Some(monocode_core::cloud_sessions::CloudRef::sandbox(SANDBOX))
        );
        assert!(
            monocode_core::cloud_sessions::start_record(&locked(&s.store).unwrap(), id).is_none(),
            "nothing to provision"
        );
    }
    // The first prompt of an attached session wakes the stopped sandbox.
    assert!(
        host.0.lock().unwrap().iter().any(|l| l == DESCRIBE),
        "{:?}",
        host.0.lock().unwrap()
    );
    // Options only apply to a new sandbox, and a session of another kind is refused.
    let error = run(
        &s,
        "sb-bad",
        "sessions.start",
        json!({"prompt":"go","target":{"kind":"codespace","instance":{"session":existing}}}),
        cx,
    )
    .unwrap_err();
    assert!(error.contains("runs in a"), "{error}");
}

#[gpui::test]
fn a_mono_pulls_evidence_of_a_sandbox_session_through_the_core(cx: &mut TestAppContext) {
    let _clis = crate::shell::surface_tests::NoClis::new();
    let ((s, gh, _, _guard), _) = sandbox_setup(cx, "Shutdown", "running");
    let id = sandbox_cloud_session(&s, cx);
    let calls = gh.calls.lock().unwrap().len();
    let error = run(
        &s,
        "pe1",
        "sessions.pullEvidence",
        json!({"sessionId":id,"path":"../x"}),
        cx,
    )
    .unwrap_err();
    assert!(error.contains(".."), "{error}");
    // The sandbox session is accepted and reaches the core (whose own tests cover the result).
    let reached = run(
        &s,
        "pe2",
        "sessions.pullEvidence",
        json!({"sessionId":id}),
        cx,
    );
    if let Err(error) = &reached {
        assert!(!error.contains("not a cloud session"), "{error}");
    }
    assert_eq!(gh.calls.lock().unwrap().len(), calls, "no gh call was made");
}

/// LIVE: creates a real codespace of hhoangg/monoptah through the same Mono `sessions.start` path the
/// app uses (real gh, real Connections) and watches the first prompt. Run by hand only:
/// `cargo test live_cloud_start -- --ignored --nocapture --test-threads=1`. The codespace is kept;
/// the caller deletes it.
#[gpui::test]
#[ignore]
fn live_cloud_start(cx: &mut TestAppContext) {
    let _guard = serial();
    let s = setup_with(cx, std::env::var("LIVE_SHARED").is_ok());
    for args in [
        vec!["init", "-q", "-b", "main"],
        vec![
            "remote",
            "add",
            "origin",
            "https://github.com/hhoangg/monoptah",
        ],
    ] {
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(&s.project)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }
    s.manager
        .update(cx, |m, cx| m.sidebar_preferences(&s.id, true, true, cx));
    cx.run_until_parked();
    let rx = call(
        &s,
        "live1",
        "sessions.start",
        json!({"prompt":"Reply with OK and nothing else.","title":"Live debug","target":{"kind":"codespace","instance":std::env::var("LIVE_CS").unwrap_or("new".into())}}),
        cx,
    );
    let mut id = None;
    for tick in 0..900 {
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(1));
        cx.run_until_parked();
        if id.is_none()
            && let Ok(result) = rx.try_recv()
        {
            eprintln!("LIVE start result: {result:?}");
            id = result.ok().and_then(|v| v["sessionId"].as_i64());
        }
        if let Some(id) = id {
            let owner = cx.update(|cx| s.manager.read(cx).owner.upgrade().unwrap());
            let (debug, listed) = cx.update(|cx| {
                let o = owner.read(cx);
                (
                    o.mono_session_entity(id).map(|v| v.read(cx).remote_debug()),
                    o.mono_listed_project(id),
                )
            });
            let store = locked(&s.store).unwrap();
            let remote = store.remote_session(id).unwrap();
            eprintln!(
                "LIVE t={tick} listed={listed:?} view={debug:?} session_id={:?} outbox={} draft={}",
                remote.as_ref().and_then(|r| r.session_id.clone()),
                remote.as_ref().map_or(0, |r| r.outbox.len()),
                store.follow_ups(id).unwrap().draft.is_some()
            );
            drop(store);
            if tick > 120 && debug.is_some() {
                // keep going a while to see the turn
            }
            if tick > 300 {
                break;
            }
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

#[gpui::test]
fn a_mono_move_call_is_validated_before_anything_is_created(cx: &mut TestAppContext) {
    let (s, gh, host, _guard) = cloud_setup(cx, "Available");
    let before = gh.calls.lock().unwrap().len();
    let target = json!({"kind":"codespace","instance":"new"});
    for (n, (action, input, needle)) in [
        (
            "sessions.moveToCloud",
            json!({"session":1,"target":target,"force":true}),
            "does not take force",
        ),
        (
            "sessions.moveToCloud",
            json!({"session":1,"target":{"kind":"codespace","instance":"new","branch":"main"}}),
            "does not take branch",
        ),
        (
            "sessions.moveToCloud",
            json!({"session":1,"target":target,"kit":{"skills":["x"]}}),
            "must be text",
        ),
        (
            "sessions.moveToCloud",
            json!({"target":target}),
            "Provide session",
        ),
        (
            "sessions.moveToLocal",
            json!({"session":1,"deleteWorkspace":true}),
            "does not take deleteWorkspace",
        ),
        (
            "sessions.cancelMove",
            json!({"session":1,"deleteWorkspace":true}),
            "does not take deleteWorkspace",
        ),
        // A session of no project of this Mono.
        (
            "sessions.moveStatus",
            json!({"session":424242}),
            "outside the assigned project",
        ),
        (
            "sessions.moveToCloud",
            json!({"session":424242,"target":target}),
            "outside the assigned project",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let error = run(&s, &format!("mv-{n}"), action, input, cx).unwrap_err();
        assert!(error.contains(needle), "{action}: {error}");
    }
    // No codespace was created and no workspace was woken.
    assert_eq!(gh.calls.lock().unwrap().len(), before);
    assert!(host.0.lock().unwrap().is_empty());
}

const MOVE_ACTIONS: [&str; 3] = [
    "sessions.moveToCloud",
    "sessions.moveToLocal",
    "sessions.cancelMove",
];

fn move_input(action: &str, session: i64) -> Value {
    match action {
        "sessions.moveToCloud" => {
            json!({"session":session,"target":{"kind":"codespace","instance":"new"}})
        }
        _ => json!({"session":session}),
    }
}

fn plain_session(s: &Setup, cx: &mut TestAppContext) -> i64 {
    let summary = locked(&s.store)
        .unwrap()
        .create("claude", &s.project)
        .unwrap();
    show(s, summary, cx)
}

#[gpui::test]
fn a_habit_run_or_heartbeat_cannot_start_or_cancel_a_move_but_can_read_one(
    cx: &mut TestAppContext,
) {
    let (s, gh, host, _guard) = cloud_setup(cx, "Available");
    let target = plain_session(&s, cx);
    let habit = fake_run(&s, cx);
    let beat = fake_run(&s, cx);
    cx.update(|cx| {
        s.manager.update(cx, |m, _| {
            m.running.get_mut(&beat).unwrap().habit = monocode_core::mono::heartbeat::RUN_ID.into();
        })
    });
    let before = gh.calls.lock().unwrap().len();
    for (caller, label) in [(habit, "habit"), (beat, "heartbeat")] {
        for action in MOVE_ACTIONS {
            let error = call_from(
                &s,
                caller,
                &format!("{label}-{action}"),
                action,
                move_input(action, target),
                cx,
            )
            .try_recv()
            .unwrap()
            .unwrap_err();
            assert!(
                error.contains("cannot move sessions"),
                "{label} {action}: {error}"
            );
        }
        // Reading a move is fine.
        let status = call_from(
            &s,
            caller,
            &format!("{label}-status"),
            "sessions.moveStatus",
            json!({"session":target}),
            cx,
        )
        .try_recv()
        .unwrap()
        .unwrap();
        assert_eq!(status["state"], "none");
    }
    assert_eq!(gh.calls.lock().unwrap().len(), before);
    assert!(host.0.lock().unwrap().is_empty());
}

#[gpui::test]
fn the_move_actions_skip_the_sessions_a_mono_must_not_manage(cx: &mut TestAppContext) {
    let (s, _gh, _host, _guard) = cloud_setup(cx, "Available");
    let hidden = plain_session(&s, cx);
    cx.update(|cx| {
        s.manager
            .update(cx, |m, _| m.roster.hidden_sessions.push(hidden))
    });
    let running = fake_run(&s, cx);
    let managed: Vec<(&str, i64)> = vec![
        // The caller's own chat.
        ("own", s.session),
        // A habit run session and a hidden (Mono-owned) session.
        ("run", running),
        ("hidden", hidden),
    ];
    for (label, session) in managed {
        for action in MOVE_ACTIONS {
            let error = call(
                &s,
                &format!("{label}-{action}"),
                action,
                move_input(action, session),
                cx,
            )
            .try_recv()
            .unwrap()
            .unwrap_err();
            assert!(
                error.contains("Only other regular project sessions"),
                "{label} {action}: {error}"
            );
        }
    }
}

#[gpui::test]
fn a_move_call_for_a_regular_session_reaches_the_shell(cx: &mut TestAppContext) {
    let (s, gh, host, _guard) = cloud_setup(cx, "Available");
    let session = plain_session(&s, cx);
    let before = gh.calls.lock().unwrap().len();
    // The shell's own refusal (a conversation with no messages), so the call got through the
    // dispatcher and validation to `Shell::api_move_to_cloud`, which created nothing.
    let error = call(
        &s,
        "reach-1",
        "sessions.moveToCloud",
        move_input("sessions.moveToCloud", session),
        cx,
    )
    .try_recv()
    .unwrap()
    .unwrap_err();
    assert!(error.contains("no messages yet"), "{error}");
    let status = call(
        &s,
        "reach-2",
        "sessions.moveStatus",
        json!({"session":session}),
        cx,
    )
    .try_recv()
    .unwrap()
    .unwrap();
    assert_eq!(status["state"], "none");
    assert_eq!(status["location"], "local");
    assert_eq!(gh.calls.lock().unwrap().len(), before);
    assert!(host.0.lock().unwrap().is_empty());
}
