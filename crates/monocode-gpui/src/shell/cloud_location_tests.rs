//! A local session whose location is Cloud is a cloud session in the shell (design 4.2.1):
//! the location drives the sidebar mark, the linked Host record is hidden, and a cloud-born
//! session behaves as before. The scenario: a local session with 10 rows written before the
//! move, location Cloud with `moved_at_row` 10.
use super::cloud::{self, CloudMark};
use super::*;
use gpui::TestAppContext;
use monocode_core::{
    cloud_sessions::{CloudBackendKind, CloudRef, CloudSessions, Host},
    codespaces::{GhOutput, GhRunner},
    remote::Machine,
    store::{CloudLocation, SessionLocation, Store},
};
use serde_json::{Value, json};
use std::sync::atomic::AtomicBool;

const LOCAL: &str = "/work/monocode";

struct Gh;
impl GhRunner for Gh {
    fn run(&self, _: &[String], _: Option<&str>) -> Result<GhOutput, String> {
        Ok(GhOutput {
            success: true,
            stdout: r#"{"state":"Available"}"#.into(),
            stderr: String::new(),
        })
    }
}
struct NoHost;
impl Host for NoHost {
    fn machines(&self) -> Result<Vec<Machine>, String> {
        Ok(vec![])
    }
    fn connect(&self, _: &str, _: &str, _: &mut dyn FnMut(&str)) -> Result<Machine, String> {
        Err("offline".into())
    }
    fn ensure(&self, _: &str, _: &AtomicBool, _: &mut dyn FnMut(&str)) -> Result<(), String> {
        Ok(())
    }
    fn request(&self, _: &str, _: &str, _: Value) -> Result<Value, String> {
        Err("offline".into())
    }
    fn disconnect(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
}

fn service(store: Database) -> CloudSessions {
    CloudSessions::new(Arc::new(Gh), Arc::new(NoHost), store)
}

fn location() -> SessionLocation {
    SessionLocation::Cloud(CloudLocation {
        cloud: CloudRef::codespace("octo-abc"),
        environment_id: "env-octo-abc".into(),
        project_id: "p".into(),
        host_session_id: "host-1".into(),
        remote_cwd: "/workspaces/widgets".into(),
        local_cwd: LOCAL.into(),
        moved_at_row: 10,
        pushed_branch: None,
    })
}

/// A Host record of the codespace `octo-abc`, bound to the local project, with a host id.
fn host_record(store: &Database, host_session: &str) -> i64 {
    let store = store.lock().unwrap();
    let project = store
        .remember_cloud_project(
            "env-octo-abc",
            &json!({"id":"p","cwd":"/workspaces/widgets"}),
            LOCAL,
            "octo-abc",
        )
        .unwrap();
    store
        .register_remote_summary(
            &project,
            &json!({"projectId":"p","revision":1,"status":"idle","updatedAt":5,
            "session":{"id":host_session,"harness":"claude","title":"T","model":"claude:opus",
            "runtimeMode":"auto","blocks":[]}}),
        )
        .unwrap()
        .id
}

/// Moves local session `local` to the cloud as F4-3a will: location, plus the linked record.
fn move_to_cloud(store: &Database, local: i64, host: i64) {
    let store = store.lock().unwrap();
    store.set_cloud_location(local, &location()).unwrap();
    store.link_remote_session(host, Some(local)).unwrap();
}

fn ten_row_session(store: &Database) -> i64 {
    let mut store = store.lock().unwrap();
    let session = store.create("claude", LOCAL).unwrap();
    let rows: Vec<monocode_core::transcript::Row> = (0..10)
        .map(|i| monocode_core::transcript::Row {
            role: if i % 2 == 0 { "user" } else { "assistant" }.into(),
            text: format!("local {i}"),
            first: true,
            sent_at: None,
        })
        .collect();
    store.write_rows(session.id, 0, &rows).unwrap();
    session.id
}

fn memory_store() -> Database {
    Arc::new(Mutex::new(
        Store::open(std::path::Path::new(":memory:")).unwrap(),
    ))
}

#[test]
fn location_flips_drive_the_cloud_mark_and_the_linked_host_record_is_hidden() {
    let store = memory_store();
    let local = ten_row_session(&store);
    let linked = host_record(&store, "host-1");
    // A session born in the cloud: a Host record nothing is linked to.
    let born = host_record(&store, "host-2");
    let svc = service(store.clone());
    let projects = vec![LOCAL.to_string()];

    // Local: the local session has no mark; both Host records are cloud sessions.
    let before = cloud::collect(&svc, &projects, &[]);
    assert!(!before.marks.contains_key(&local));
    assert!(before.marks.contains_key(&linked) && before.marks.contains_key(&born));
    assert!(before.located.is_empty());

    // The location flips to Cloud: the local session is the cloud row, the linked record is
    // hidden and the unlinked cloud-born one is not.
    move_to_cloud(&store, local, linked);
    let after = cloud::collect(&svc, &projects, &[]);
    assert_eq!(
        after.marks.get(&local),
        Some(&CloudMark {
            local_project: LOCAL.into(),
            codespace: "octo-abc".into(),
            kind: CloudBackendKind::Codespaces,
        })
    );
    assert!(!after.marks.contains_key(&linked), "no duplicate row");
    assert!(after.marks.contains_key(&born), "cloud-born is unchanged");
    assert_eq!(after.located[&local].moved_at_row, 10);
    assert_eq!(after.located[&local].sync_error, None);
    assert!(after.states.contains_key("octo-abc"));
    let listed: Vec<i64> = store
        .lock()
        .unwrap()
        .sessions()
        .unwrap()
        .into_iter()
        .map(|s| s.id)
        .collect();
    assert!(listed.contains(&local) && listed.contains(&born));
    assert!(!listed.contains(&linked));

    // The durable sync error rides along.
    store
        .lock()
        .unwrap()
        .record_cloud_sync_error(local, "Host unreachable")
        .unwrap();
    let errored = cloud::collect(&svc, &projects, &[]);
    assert_eq!(
        errored.located[&local].sync_error.as_deref(),
        Some("Host unreachable")
    );

    // And back: the mark goes, the Host record is a session of its own again.
    store
        .lock()
        .unwrap()
        .reset_cloud_location_to_local(local)
        .unwrap();
    let reset = cloud::collect(&svc, &projects, &[]);
    assert!(!reset.marks.contains_key(&local) && reset.located.is_empty());
    assert!(reset.marks.contains_key(&linked) && reset.marks.contains_key(&born));
}

#[gpui::test]
fn the_shell_shows_a_located_session_as_a_cloud_session_and_tells_its_view(
    cx: &mut TestAppContext,
) {
    let (window, _, local) = crate::shell::tests::window(cx);
    let host = window
        .update(cx, |shell, _, _| {
            shell.cloud = Arc::new(service(shell.store.clone()));
            let host = host_record(&shell.store, "host-1");
            // 10 rows before the move: `local` is the project's claude session.
            let mut store = locked(&shell.store).unwrap();
            let rows: Vec<monocode_core::transcript::Row> = (0..10)
                .map(|i| monocode_core::transcript::Row {
                    role: if i % 2 == 0 { "user" } else { "assistant" }.into(),
                    text: format!("local {i}"),
                    first: true,
                    sent_at: None,
                })
                .collect();
            store.write_rows(local, 0, &rows).unwrap();
            drop(store);
            shell
                .session_summaries
                .get_mut(&local)
                .unwrap()
                .has_messages = true;
            host
        })
        .unwrap();
    window
        .update(cx, |shell, _, cx| {
            if let Some(view) = shell.sessions.get(&local) {
                view.update(cx, |view, _| view.summary.has_messages = true);
            }
            assert!(!shell.is_cloud(local));
            move_to_cloud(&shell.store, local, host);
            shell.refresh_cloud(cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            assert!(
                shell.is_cloud(local),
                "the location makes it a cloud session"
            );
            assert!(shell.cloud_badge(local, false).is_some());
            let cwd = shell.session_summaries[&local].cwd.clone();
            assert_eq!(shell.project_of(local, &cwd), LOCAL);
            assert!(!shell.is_cloud(host), "the linked Host record has no row");
            assert!(!shell.session_summaries.contains_key(&host));
            // The open view follows the location (move point drives rewind gating).
            let view = shell.sessions.get(&local).expect("open view");
            assert_eq!(view.read(cx).cloud_move_for_test(), Some(10));
            // The sidebar lists the session once.
            let count = shell
                .sidebar_session_ids(cx)
                .iter()
                .filter(|id| **id == local)
                .count();
            assert_eq!(count, 1);
        })
        .unwrap();
    // Back to Local: the next refresh drops the mark and the view's gate.
    window
        .update(cx, |shell, _, cx| {
            locked(&shell.store)
                .unwrap()
                .reset_cloud_location_to_local(local)
                .unwrap();
            shell.refresh_cloud(cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            assert!(!shell.is_cloud(local));
            let view = shell.sessions.get(&local).expect("open view");
            assert_eq!(view.read(cx).cloud_move_for_test(), None);
        })
        .unwrap();
}

#[gpui::test]
fn deleting_a_cloud_session_says_the_workspace_copy_stays(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_prompt_builder(crate::modal::prompt_builder));
    let (window, other, local) = crate::shell::tests::window(cx);
    window
        .update(cx, |shell, w, cx| {
            locked(&shell.store)
                .unwrap()
                .set_cloud_location(local, &location())
                .unwrap();
            shell.request_delete_sessions(vec![other], w, cx);
            let plain = shell.delete_dialog.take().unwrap();
            assert_eq!(plain.cloud_notice, None, "a local session says nothing");
            shell.request_delete_sessions(vec![local], w, cx);
            let notice = shell
                .delete_dialog
                .as_ref()
                .and_then(|d| d.cloud_notice.clone())
                .expect("the dialog warns");
            assert!(notice.contains("codespace octo-abc"), "{notice}");
            assert!(
                notice.contains("removes only the copy on this computer"),
                "{notice}"
            );
            assert!(
                notice.contains("stays until you delete the codespace"),
                "{notice}"
            );
        })
        .unwrap();
}
