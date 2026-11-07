//! Mono dispatch of the codespace actions and of cloud sessions.
use super::tests::{Setup, call, launches_of, setup, setup_with};
use crate::cloud_wake::fakes::{AUTH_OK, FakeHost, Gh, install, serial};
use crate::runtime::locked;
use gpui::TestAppContext;
use serde_json::{Value, json};
use std::sync::Arc;

/// A Mono whose project is a git repo with a GitHub origin, over fake gh and host.
fn cloud_setup(
    cx: &mut TestAppContext,
    state: &'static str,
) -> (
    Setup,
    Arc<Gh>,
    Arc<FakeHost>,
    std::sync::MutexGuard<'static, ()>,
) {
    let guard = serial();
    let s = setup(cx);
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
    let (gh, host) = install(s.store.clone(), AUTH_OK, state);
    host.0.lock().unwrap().clear();
    (s, gh, host, guard)
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
/// A cloud session of the Mono's project on the fake codespace, shown in the shell.
fn cloud_session(s: &Setup, cx: &mut TestAppContext) -> i64 {
    use monocode_core::cloud_sessions::{CodespaceChoice, Target};
    let api = monocode_core::mono::cloud_api::CloudApi::shared(&s.store).unwrap();
    let binding = api
        .prepare(
            &s.project,
            Target {
                codespace: CodespaceChoice::Named("octo-run".into()),
                branch: None,
                machine: None,
            },
            &mut |_| {},
        )
        .unwrap();
    let summary = api.create_session(&binding, "claude").unwrap();
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

#[gpui::test]
fn a_mono_manages_codespaces_including_delete(cx: &mut TestAppContext) {
    let (s, gh, _, _guard) = cloud_setup(cx, "Shutdown");
    let list = run(&s, "c1", "codespaces.list", json!({}), cx).unwrap();
    assert_eq!(list["codespaces"][0]["name"], "octo-run");
    let secrets = run(&s, "c2", "codespaces.secretsStatus", json!({}), cx).unwrap();
    assert_eq!(secrets["secrets"][0]["key"], "CLAUDE_CODE_OAUTH_TOKEN");
    assert_eq!(
        run(&s, "c3", "codespaces.stop", json!({"name":"octo-run"}), cx).unwrap(),
        json!({"codespace":"octo-run","stopped":true})
    );
    // Another repository's codespace is refused before gh is asked to delete it.
    let error = run(
        &s,
        "c4",
        "codespaces.delete",
        json!({"name":"elsewhere-1"}),
        cx,
    )
    .unwrap_err();
    assert!(error.contains("not a codespace of octo/widgets"), "{error}");
    assert_eq!(
        run(
            &s,
            "c5",
            "codespaces.delete",
            json!({"name":"octo-run"}),
            cx
        )
        .unwrap(),
        json!({"codespace":"octo-run","deleted":true})
    );
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
        json!({"kind":"codespace","codespace":"octo-run","state":"stopped"})
    );
    assert_eq!(row["codespaceState"], "stopped");
    let read = run(&s, "r1", "sessions.read", json!({"sessionId":id}), cx).unwrap();
    assert_eq!(read["target"]["codespace"], "octo-run");
    assert_eq!(
        run(&s, "s1", "sessions.stop", json!({"sessionId":id}), cx).unwrap(),
        json!({"accepted":true})
    );
    assert!(
        host.0.lock().unwrap().is_empty(),
        "reading never wakes the codespace"
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
            json!({"prompt":"x","harness":"codex","target":{"codespace":"new"}}),
        ),
        (
            "v2",
            json!({"prompt":"x","target":{"codespace":"new"},"parentSessionId":999}),
        ),
        (
            "v3",
            json!({"prompt":"x","target":{"codespace":"new"},"folderId":"nope"}),
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
        json!({"prompt":"go","title":"Codespace test","target":{"codespace":"octo-run"}}),
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
    let input = json!({"prompt":"inspect the environment","title":"Starting codespace","target":{"codespace":"new","branch":"trunk"}});
    let started = run(&setup, "pending-cloud", "sessions.start", input.clone(), cx).unwrap();
    let session_id = started["sessionId"].as_i64().unwrap();
    assert_eq!(started["codespaceState"], "starting");
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
    assert_eq!(row["codespaceState"], "starting");
    assert!(row.get("startup").is_some());
    let page = run(
        &setup,
        "pending-read",
        "sessions.read",
        json!({"sessionId":session_id}),
        cx,
    )
    .unwrap();
    assert_eq!(page["codespaceState"], "starting");
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
        json!({"prompt":"Reply with OK and nothing else.","title":"Live debug","target":{"codespace":std::env::var("LIVE_CS").unwrap_or("new".into())}}),
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
