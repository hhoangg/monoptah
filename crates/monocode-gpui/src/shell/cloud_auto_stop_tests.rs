//! The shell side of cloud auto-stop: what it feeds the policy, and that a decision becomes
//! exactly one stop call, a notice and a Stopped workspace that Resume wakes again. A fake `gh`
//! stands in for the backend; the policy itself is covered in `cloud_sessions/auto_stop_tests.rs`.
use super::cloud::CloudMark;
use super::*;
use gpui::TestAppContext;
use monocode_core::{
    cloud_sessions::{CloudBackendKind, CloudSessions, CloudState, Host},
    codespaces::{GhOutput, GhRunner},
    remote::Machine,
    store::Session,
};
use serde_json::{Value, json};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

const CODESPACE: &str = "octo-abc";

/// Answers `codespace view` from a state it changes on `codespace stop`, and records every call.
struct Gh {
    state: Mutex<&'static str>,
    calls: Mutex<Vec<String>>,
}
impl Gh {
    fn new(state: &'static str) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(state),
            calls: Mutex::new(vec![]),
        })
    }
    fn stops(&self) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.starts_with("codespace stop"))
            .count()
    }
    fn set(&self, state: &'static str) {
        *self.state.lock().unwrap() = state;
    }
}
impl GhRunner for Gh {
    fn run(&self, args: &[String], _: Option<&str>) -> Result<GhOutput, String> {
        let line = args.join(" ");
        self.calls.lock().unwrap().push(line.clone());
        let stdout = if line.starts_with("codespace view") {
            format!(r#"{{"state":"{}"}}"#, self.state.lock().unwrap())
        } else {
            if line.starts_with("codespace stop") {
                *self.state.lock().unwrap() = "Shutdown";
            }
            String::new()
        };
        Ok(GhOutput {
            success: true,
            stdout,
            stderr: String::new(),
        })
    }
}

#[derive(Default)]
struct NoHost;
impl Host for NoHost {
    fn machines(&self) -> Result<Vec<Machine>, String> {
        Ok(vec![])
    }
    fn connect(
        &self,
        _: &str,
        _: &str,
        _: &AtomicBool,
        _: &mut dyn FnMut(&str),
    ) -> Result<Machine, String> {
        Err("unused".into())
    }
    fn ensure(&self, _: &str, _: &AtomicBool, _: &mut dyn FnMut(&str)) -> Result<(), String> {
        Err("auto-stop must never wake a workspace".into())
    }
    fn request(&self, _: &str, _: &str, _: Value) -> Result<Value, String> {
        Err("unused".into())
    }
    fn disconnect(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
}

/// A window with one cloud session on `CODESPACE`, cached as `Available`, the codespace policy
/// switched to `idle` minutes (`None` keeps the default: off).
fn world(
    cx: &mut TestAppContext,
    state: &'static str,
    idle: Option<u32>,
) -> (gpui::WindowHandle<Shell>, i64, Arc<Gh>) {
    let (window, _, _) = crate::shell::tests::window(cx);
    let gh = Gh::new(state);
    let id = window
        .update(cx, |shell, _, _| {
            let id = {
                let store = shell.store.lock().unwrap();
                let project = store
                    .remember_cloud_project(
                        &format!("env-{CODESPACE}"),
                        &json!({"id":"p","cwd":"/workspaces/widgets"}),
                        "/work/monocode",
                        CODESPACE,
                    )
                    .unwrap();
                store.create("claude", &project.key).unwrap().id
            };
            let summary: Session = locked(&shell.store)
                .unwrap()
                .sessions()
                .unwrap()
                .into_iter()
                .find(|s| s.id == id)
                .unwrap();
            shell.cloud = Arc::new(CloudSessions::new(
                gh.clone(),
                Arc::new(NoHost),
                shell.store.clone(),
            ));
            shell.session_summaries.insert(
                id,
                Session {
                    has_messages: true,
                    ..summary
                },
            );
            shell.order.push(id);
            shell.cloud_marks.insert(
                id,
                CloudMark {
                    local_project: "/work/monocode".into(),
                    codespace: CODESPACE.into(),
                    kind: CloudBackendKind::Codespaces,
                },
            );
            shell
                .cloud_states
                .insert(CODESPACE.into(), CloudState::Available);
            if let Some(minutes) = idle {
                shell
                    .workspace
                    .preferences
                    .cloud_auto_stop
                    .codespace
                    .idle_minutes = Some(minutes);
            }
            id
        })
        .unwrap();
    (window, id, gh)
}

/// Runs the minute check `minutes` after the app started, then lets a stop worker finish.
fn check_at(window: gpui::WindowHandle<Shell>, minutes: u64, cx: &mut TestAppContext) {
    window
        .update(cx, |shell, _, cx| {
            shell.auto_stop_epoch = std::time::Instant::now() - Duration::from_secs(minutes * 60);
            shell.check_auto_stop(cx);
        })
        .unwrap();
    cx.run_until_parked();
}

#[gpui::test]
fn an_idle_workspace_is_stopped_once_with_a_notice_and_resume_starts_a_new_window(
    cx: &mut TestAppContext,
) {
    let (window, id, gh) = world(cx, "Available", Some(10));
    check_at(window, 0, cx);
    check_at(window, 9, cx);
    assert_eq!(gh.stops(), 0, "not idle long enough yet");
    check_at(window, 11, cx);
    assert_eq!(gh.stops(), 1);
    window
        .update(cx, |shell, _, _| {
            assert_eq!(shell.cloud_states[CODESPACE], CloudState::Stopped);
            let notice = shell.notice.clone().unwrap();
            assert_eq!(
                notice,
                format!(
                    "Stopped codespace {CODESPACE} after 10 min idle; send a message to resume."
                )
            );
        })
        .unwrap();
    // It stays stopped, and nothing asks again.
    for minute in [12, 30, 120] {
        check_at(window, minute, cx);
    }
    assert_eq!(gh.stops(), 1);
    // The user resumes it (the Resume pill / next send): a fresh window starts there.
    gh.set("Available");
    window
        .update(cx, |shell, _, cx| {
            shell.cloud_resumed(id, cx);
            // The first check after the resume starts its clock; later ones count from there.
        })
        .unwrap();
    cx.run_until_parked();
    check_at(window, 125, cx);
    assert_eq!(gh.stops(), 1);
    check_at(window, 140, cx);
    assert_eq!(gh.stops(), 2);
}

#[gpui::test]
fn a_working_session_a_queue_or_an_unknown_state_keeps_the_workspace(cx: &mut TestAppContext) {
    let (window, id, gh) = world(cx, "Available", Some(10));
    check_at(window, 0, cx);
    // The stored flags say a turn runs.
    window
        .update(cx, |shell, _, _| {
            shell.remote_marks.insert(id, (true, false));
        })
        .unwrap();
    check_at(window, 60, cx);
    // A turn that waits for input counts too.
    window
        .update(cx, |shell, _, _| {
            shell.remote_marks.insert(id, (false, true));
        })
        .unwrap();
    check_at(window, 120, cx);
    // Idle again, but the app does not know the workspace state.
    window
        .update(cx, |shell, _, _| {
            shell.remote_marks.insert(id, (false, false));
            shell
                .cloud_states
                .insert(CODESPACE.into(), CloudState::Unknown);
        })
        .unwrap();
    check_at(window, 180, cx);
    // A workspace the app is archiving.
    window
        .update(cx, |shell, _, _| {
            shell
                .cloud_states
                .insert(CODESPACE.into(), CloudState::Available);
            shell.cloud_workspaces_busy.insert(CODESPACE.to_owned());
        })
        .unwrap();
    check_at(window, 240, cx);
    assert_eq!(gh.stops(), 0);
}

#[gpui::test]
fn the_default_leaves_codespaces_to_github(cx: &mut TestAppContext) {
    let (window, _, gh) = world(cx, "Available", None);
    for minute in [0, 11, 120, 600] {
        check_at(window, minute, cx);
    }
    assert_eq!(gh.stops(), 0);
    window
        .update(cx, |shell, _, _| assert!(shell.notice.is_none()))
        .unwrap();
}

#[gpui::test]
fn an_already_stopped_workspace_is_never_stopped_again(cx: &mut TestAppContext) {
    let (window, _, gh) = world(cx, "Shutdown", Some(10));
    window
        .update(cx, |shell, _, _| {
            shell
                .cloud_states
                .insert(CODESPACE.into(), CloudState::Stopped);
        })
        .unwrap();
    for minute in [0, 11, 120] {
        check_at(window, minute, cx);
    }
    assert_eq!(gh.stops(), 0);
}

#[gpui::test]
fn a_stale_cache_does_not_stop_a_workspace_that_is_already_down(cx: &mut TestAppContext) {
    // The cache says Available, but gh already reports it stopped (idled out on GitHub's side).
    let (window, _, gh) = world(cx, "Shutdown", Some(10));
    check_at(window, 0, cx);
    check_at(window, 11, cx);
    assert_eq!(gh.stops(), 0, "the fresh read found it stopped");
}

#[gpui::test]
fn the_last_session_going_away_stops_the_workspace_after_the_grace(cx: &mut TestAppContext) {
    let (window, id, gh) = world(cx, "Available", None);
    window
        .update(cx, |shell, _, _| {
            shell
                .workspace
                .preferences
                .cloud_auto_stop
                .codespace
                .stop_when_unused = true;
        })
        .unwrap();
    check_at(window, 0, cx);
    // The session is deleted: the store forgets it and the window drops its mark.
    window
        .update(cx, |shell, _, _| {
            locked(&shell.store).unwrap().delete(id).unwrap();
            shell.cloud_marks.remove(&id);
            shell.session_summaries.remove(&id);
            shell.order.retain(|other| *other != id);
        })
        .unwrap();
    check_at(window, 2, cx);
    assert_eq!(gh.stops(), 0, "inside the grace");
    check_at(window, 6, cx);
    assert_eq!(gh.stops(), 0, "still inside the 5 minute grace");
    check_at(window, 8, cx);
    assert_eq!(gh.stops(), 1);
    window
        .update(cx, |shell, _, _| {
            assert!(
                shell
                    .notice
                    .as_deref()
                    .is_some_and(|n| n.contains("last session was archived or deleted"))
            );
        })
        .unwrap();
    check_at(window, 60, cx);
    assert_eq!(gh.stops(), 1);
}

#[gpui::test]
fn a_session_of_a_closed_project_keeps_a_workspace_the_window_forgot(cx: &mut TestAppContext) {
    let (window, id, gh) = world(cx, "Available", None);
    window
        .update(cx, |shell, _, _| {
            shell
                .workspace
                .preferences
                .cloud_auto_stop
                .codespace
                .stop_when_unused = true;
        })
        .unwrap();
    check_at(window, 0, cx);
    // The project was closed: no mark in the window, but the store still has the session.
    window
        .update(cx, |shell, _, _| {
            shell.cloud_marks.remove(&id);
        })
        .unwrap();
    check_at(window, 6, cx);
    check_at(window, 12, cx);
    assert_eq!(gh.stops(), 0);
}

#[gpui::test]
fn the_facts_leave_out_archived_sessions_and_mark_missing_data_unknown(cx: &mut TestAppContext) {
    let (window, id, _) = world(cx, "Available", Some(10));
    window
        .update(cx, |shell, _, cx| {
            let facts = shell.auto_stop_facts(cx);
            assert_eq!(facts.len(), 1);
            assert_eq!(facts[0].state, CloudState::Available);
            assert_eq!(facts[0].sessions.len(), 1);
            assert!(!facts[0].sessions[0].busy && !facts[0].sessions[0].unknown);
            // Stored activity flags are busy.
            shell.remote_marks.insert(id, (true, false));
            assert!(shell.auto_stop_facts(cx)[0].sessions[0].busy);
            shell.remote_marks.insert(id, (false, false));
            // A move in progress is not idle.
            // No summary: the app cannot tell.
            let summary = shell.session_summaries.remove(&id).unwrap();
            assert!(shell.auto_stop_facts(cx)[0].sessions[0].unknown);
            // Archived: the workspace has no session that needs it.
            shell.session_summaries.insert(
                id,
                Session {
                    archived: true,
                    ..summary
                },
            );
            assert!(shell.auto_stop_facts(cx).is_empty());
        })
        .unwrap();
}
