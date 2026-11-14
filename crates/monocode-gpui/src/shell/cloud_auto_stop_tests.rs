//! The shell side of cloud auto-stop: what it feeds the policy, and that a decision becomes
//! exactly one stop call, a notice and a Stopped workspace that Resume wakes again. A fake `gh`
//! stands in for the backend; the policy itself is covered in `cloud_sessions/auto_stop_tests.rs`.
use super::cloud::CloudMark;
use super::*;
use gpui::TestAppContext;
use monocode_core::{
    cloud_sessions::{CloudBackendKind, CloudRef, CloudSessions, CloudState, Host},
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
    fail_stop: std::sync::atomic::AtomicBool,
}
impl Gh {
    fn new(state: &'static str) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(state),
            calls: Mutex::new(vec![]),
            fail_stop: Default::default(),
        })
    }
    fn fail_stops(&self, fail: bool) {
        self.fail_stop
            .store(fail, std::sync::atomic::Ordering::SeqCst);
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
        if line.starts_with("codespace stop")
            && self.fail_stop.load(std::sync::atomic::Ordering::SeqCst)
        {
            return Ok(GhOutput {
                success: false,
                stdout: String::new(),
                stderr: "gh: HTTP 500 (the token expired)".into(),
            });
        }
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

/// A conversation of the window put on the workspace of `world`, so there is a live view to lock.
fn live_view(window: gpui::WindowHandle<Shell>, cx: &mut TestAppContext) -> i64 {
    window
        .update(cx, |shell, _, cx| {
            let live = *shell.sessions.keys().next().unwrap();
            shell.sessions[&live].update(cx, |view, _| view.loaded_for_test());
            shell.cloud_marks.insert(
                live,
                CloudMark {
                    local_project: "/work/monocode".into(),
                    codespace: CODESPACE.into(),
                    kind: CloudBackendKind::Codespaces,
                },
            );
            live
        })
        .unwrap()
}

#[gpui::test]
fn nothing_can_start_on_the_workspace_while_the_worker_stops_it(cx: &mut TestAppContext) {
    let (window, _, gh) = world(cx, "Available", Some(10));
    let live = live_view(window, cx);
    check_at(window, 0, cx);
    // The decision is made and the worker is spawned, but has not run yet: this is the window
    // in which a send used to slip through.
    window
        .update(cx, |shell, window, cx| {
            shell.auto_stop_epoch = std::time::Instant::now() - Duration::from_secs(11 * 60);
            shell.check_auto_stop(cx);
            assert!(shell.auto_stop_running.contains(CODESPACE));
            // Archive, delete, continue and the cloud.* actions see the workspace as held.
            assert!(shell.cloud_workspaces_busy.contains(CODESPACE));
            // A send from the composer is refused, with the reason and the text kept.
            let view = shell.sessions[&live].clone();
            assert!(view.update(cx, |v, cx| v.refuse_while_saving_copy(cx)));
            // So is a Mono / app turn.
            let sent = view.update(cx, |v, cx| v.mono_external_send("hi".into(), false, cx));
            assert_eq!(sent, Err(crate::conversation::AUTO_STOPPING.to_owned()));
            // And a new terminal on it, which would be cut off or wake it again.
            let id = shell.cloud_marks.keys().copied().min().unwrap();
            shell.error = None;
            shell.new_cloud_terminal(id, window, cx);
            assert_eq!(shell.error.as_deref(), Some(super::cloud::WORKSPACE_BUSY));
            assert!(shell.cloud_terminals.is_empty());
        })
        .unwrap();
    assert_eq!(gh.stops(), 0, "the worker has not run yet");
    cx.run_until_parked();
    assert_eq!(gh.stops(), 1);
    // Everything is released when the stop is over; the next send resumes the workspace.
    window
        .update(cx, |shell, _, cx| {
            assert!(!shell.cloud_workspaces_busy.contains(CODESPACE));
            assert!(!shell.auto_stop_running.contains(CODESPACE));
            let view = shell.sessions[&live].clone();
            assert!(!view.update(cx, |v, cx| v.refuse_while_saving_copy(cx)));
            assert!(!view.read(cx).auto_stopping);
        })
        .unwrap();
}

#[gpui::test]
fn a_turn_the_stored_flags_do_not_show_yet_stops_nothing(cx: &mut TestAppContext) {
    // The decision is made on a tick; the live view shows a turn that the stored flags (polled
    // every 30 s) do not have. The worker gets the live snapshot and leaves the workspace.
    let (window, id, gh) = world(cx, "Available", Some(10));
    check_at(window, 0, cx);
    window
        .update(cx, |shell, _, cx| {
            let view = shell.ensure_session(id, cx).unwrap();
            view.update(cx, |view, _| {
                view.loaded_for_test();
                view.busy = true;
            });
            assert!(shell.cloud_busy_snapshot(cx).sessions.contains(&id));
            // Pretend the facts were gathered a moment earlier, when it was still idle.
            let decision = monocode_core::cloud_sessions::auto_stop::Decision {
                cloud: monocode_core::cloud_sessions::CloudRef::codespace(CODESPACE),
                reason: monocode_core::cloud_sessions::auto_stop::Reason::Idle { minutes: 10 },
            };
            shell.run_auto_stop(decision, cx);
            assert_eq!(shell.remote_marks.get(&id), None);
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(gh.stops(), 0);
    window
        .update(cx, |shell, _, _| {
            assert!(!shell.cloud_workspaces_busy.contains(CODESPACE));
            assert!(shell.notice.is_none());
        })
        .unwrap();
}

#[gpui::test]
fn an_open_cloud_terminal_keeps_the_workspace_running_until_it_is_closed(cx: &mut TestAppContext) {
    let (window, _, gh) = world(cx, "Available", Some(10));
    check_at(window, 0, cx);
    let terminal = window
        .update(cx, |shell, _, cx| {
            shell.spawn_codespace_terminal(CODESPACE, true, cx)
        })
        .unwrap();
    // Hours of a dev server, with nobody typing in the composer.
    for minute in [11, 60, 600] {
        check_at(window, minute, cx);
    }
    assert_eq!(gh.stops(), 0);
    // The shell ends: the window starts from there, as for any activity.
    terminal.update(cx, |t, cx| t.fake_exit(Some(0), cx));
    check_at(window, 601, cx);
    check_at(window, 605, cx);
    assert_eq!(gh.stops(), 0);
    check_at(window, 612, cx);
    assert_eq!(gh.stops(), 1);
}

#[gpui::test]
fn a_closed_cloud_terminal_no_longer_counts(cx: &mut TestAppContext) {
    let (window, _, gh) = world(cx, "Available", Some(10));
    check_at(window, 0, cx);
    let terminal = window
        .update(cx, |shell, _, cx| {
            let terminal = shell.spawn_codespace_terminal(CODESPACE, true, cx);
            assert_eq!(shell.open_cloud_terminals(cx).len(), 1);
            terminal
        })
        .unwrap();
    // Closed from the dock: the last handle goes away and the entity is released.
    drop(terminal);
    window
        .update(cx, |shell, _, cx| {
            assert!(shell.open_cloud_terminals(cx).is_empty())
        })
        .unwrap();
    check_at(window, 1, cx);
    check_at(window, 12, cx);
    assert_eq!(gh.stops(), 1);
}

#[gpui::test]
fn an_open_terminal_holds_a_workspace_whose_sessions_are_gone(cx: &mut TestAppContext) {
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
    let terminal = window
        .update(cx, |shell, _, cx| {
            let terminal = shell.spawn_codespace_terminal(CODESPACE, true, cx);
            locked(&shell.store).unwrap().delete(id).unwrap();
            shell.cloud_marks.remove(&id);
            shell.session_summaries.remove(&id);
            shell.order.retain(|other| *other != id);
            terminal
        })
        .unwrap();
    for minute in [2, 10, 60, 300] {
        check_at(window, minute, cx);
    }
    assert_eq!(gh.stops(), 0, "a shell is still open on it");
    terminal.update(cx, |t, cx| t.fake_exit(Some(0), cx));
    check_at(window, 301, cx);
    check_at(window, 304, cx);
    assert_eq!(gh.stops(), 0, "inside the grace");
    check_at(window, 307, cx);
    assert_eq!(gh.stops(), 1);
}

#[gpui::test]
fn keep_running_in_the_archive_dialog_is_not_overridden_by_the_unused_rule(
    cx: &mut TestAppContext,
) {
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
    window
        .update(cx, |shell, window, cx| {
            shell.open_cloud_archive(vec![id], window, cx);
            shell.confirm_cloud_archive(super::cloud::ArchiveChoice::Keep, window, cx);
        })
        .unwrap();
    cx.run_until_parked();
    for minute in [1, 6, 12, 60, 600] {
        check_at(window, minute, cx);
    }
    assert_eq!(gh.stops(), 0, "the user chose to keep it running");
}

#[gpui::test]
fn without_a_keep_choice_the_unused_rule_still_stops_an_archived_workspace(
    cx: &mut TestAppContext,
) {
    // Control for the test above: without the Keep choice the same flow does stop it.
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
    window
        .update(cx, |shell, _, cx| {
            shell.toggle_archive_now(id, cx);
        })
        .unwrap();
    cx.run_until_parked();
    for minute in [1, 6, 8] {
        check_at(window, minute, cx);
    }
    assert_eq!(gh.stops(), 1);
}

#[gpui::test]
fn removing_a_session_from_monoptah_leaves_its_workspace_running(cx: &mut TestAppContext) {
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
    window
        .update(cx, |shell, window, cx| {
            shell.remove_cloud_session_locally(id, window, cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, _| {
            assert!(!shell.cloud_marks.contains_key(&id))
        })
        .unwrap();
    for minute in [1, 6, 12, 60, 600] {
        check_at(window, minute, cx);
    }
    assert_eq!(gh.stops(), 0, "its confirmation says it keeps running");
}

#[gpui::test]
fn a_failed_stop_is_shown_once_per_workspace_and_logged_every_time(cx: &mut TestAppContext) {
    let (window, _, gh) = world(cx, "Available", Some(10));
    gh.fail_stops(true);
    check_at(window, 0, cx);
    check_at(window, 11, cx);
    assert_eq!(gh.stops(), 1);
    window
        .update(cx, |shell, _, _| {
            let error = shell.error.clone().unwrap();
            assert!(
                error.contains(&format!("Could not stop codespace {CODESPACE}")),
                "{error}"
            );
            assert!(error.contains("token expired"), "{error}");
            assert!(error.contains("still running"), "{error}");
            assert_eq!(shell.cloud_states[CODESPACE], CloudState::Available);
            // Released: the user can work and the workspace is not held.
            assert!(!shell.cloud_workspaces_busy.contains(CODESPACE));
            // The user dismisses it.
            shell.error = None;
        })
        .unwrap();
    // The retry a window later fails again: no second banner.
    check_at(window, 22, cx);
    assert_eq!(gh.stops(), 2);
    window
        .update(cx, |shell, _, _| assert!(shell.error.is_none()))
        .unwrap();
    // It works at last: the notice, and a later failure is shown again.
    gh.fail_stops(false);
    check_at(window, 33, cx);
    assert_eq!(gh.stops(), 3);
    window
        .update(cx, |shell, _, _| {
            assert!(
                shell
                    .notice
                    .as_deref()
                    .is_some_and(|n| n.starts_with("Stopped codespace"))
            );
            assert!(shell.auto_stop_failed.is_empty());
        })
        .unwrap();
}

#[gpui::test]
fn typing_counts_as_input_but_other_notifies_of_the_composer_do_not(cx: &mut TestAppContext) {
    let (window, _, _) = world(cx, "Available", Some(10));
    let live = live_view(window, cx);
    let input = window
        .update(cx, |shell, _, cx| {
            shell.sessions[&live].read(cx).input.clone()
        })
        .unwrap();
    let last = |window: gpui::WindowHandle<Shell>, cx: &mut TestAppContext| {
        window
            .update(cx, |shell, _, cx| {
                shell.sessions[&live]
                    .read(cx)
                    .auto_stop_facts(shell.auto_stop_epoch)
                    .last_input
            })
            .unwrap()
    };
    // Programmatic notifies: a placeholder, `set_text`, a plain notify.
    window
        .update(cx, |_, window, cx| {
            input.update(cx, |input, cx| {
                input.set_text("restored draft".into(), cx);
                cx.notify();
            });
            let _ = window;
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(last(window, cx), None, "not the user");
    // A keystroke: an edit through the input handler.
    window
        .update(cx, |_, window, cx| {
            input.update(cx, |input, cx| {
                use gpui::EntityInputHandler;
                input.replace_text_in_range(None, "x", window, cx)
            });
        })
        .unwrap();
    cx.run_until_parked();
    assert!(last(window, cx).is_some(), "typing counts");
}

#[gpui::test]
fn a_panel_shell_the_app_opened_holds_the_workspace_only_once_it_is_typed_in(
    cx: &mut TestAppContext,
) {
    let (window, _, gh) = world(cx, "Available", Some(10));
    check_at(window, 0, cx);
    let terminal = window
        .update(cx, |shell, _, cx| {
            // What `ensure_panel_terminal` does by itself when a codespace session is shown.
            let terminal = shell.spawn_codespace_terminal(CODESPACE, false, cx);
            assert!(shell.open_cloud_terminals(cx).is_empty());
            terminal
        })
        .unwrap();
    // Nobody touched it: it is a prompt, not work. The idle rule still works.
    check_at(window, 5, cx);
    terminal.update(cx, |t, cx| t.run("npm run dev", cx));
    window
        .update(cx, |shell, _, cx| {
            assert_eq!(shell.open_cloud_terminals(cx).len(), 1)
        })
        .unwrap();
    for minute in [11, 120] {
        check_at(window, minute, cx);
    }
    assert_eq!(gh.stops(), 0);
}

#[gpui::test]
fn a_conversation_opened_while_the_workspace_is_being_stopped_is_locked_too(
    cx: &mut TestAppContext,
) {
    let (window, _, gh) = world(cx, "Available", Some(10));
    let live = live_view(window, cx);
    check_at(window, 0, cx);
    window
        .update(cx, |shell, _, cx| {
            // The conversation was not open when the stop began (history, a tab restored later).
            shell.sessions.remove(&live);
            shell.auto_stop_epoch = std::time::Instant::now() - Duration::from_secs(11 * 60);
            shell.check_auto_stop(cx);
            assert!(shell.auto_stop_running.contains(CODESPACE));
            // Opened now, a few seconds into the stop.
            let view = shell.ensure_session(live, cx).expect("opens");
            assert!(view.read(cx).auto_stopping, "locked as it opens");
            assert!(view.update(cx, |v, cx| v.refuse_while_saving_copy(cx)));
            let sent = view.update(cx, |v, cx| v.mono_external_send("hi".into(), false, cx));
            assert_eq!(sent, Err(crate::conversation::AUTO_STOPPING.to_owned()));
        })
        .unwrap();
    assert_eq!(gh.stops(), 0, "the worker has not run yet");
    cx.run_until_parked();
    assert_eq!(gh.stops(), 1);
    window
        .update(cx, |shell, _, cx| {
            let view = shell.sessions[&live].clone();
            assert!(!view.read(cx).auto_stopping, "released with the others");
            assert!(!view.update(cx, |v, cx| v.refuse_while_saving_copy(cx)));
        })
        .unwrap();
}

#[gpui::test]
fn a_session_marked_as_cloud_during_the_stop_is_locked_when_the_marks_are_applied(
    cx: &mut TestAppContext,
) {
    let (window, _, _) = world(cx, "Available", Some(10));
    let live = live_view(window, cx);
    check_at(window, 0, cx);
    window
        .update(cx, |shell, _, cx| {
            shell.auto_stop_epoch = std::time::Instant::now() - Duration::from_secs(11 * 60);
            shell.check_auto_stop(cx);
            assert!(shell.sessions[&live].read(cx).auto_stopping);
            // An open conversation the marks did not know as cloud yet: not locked...
            let mark = shell.cloud_marks.remove(&live).unwrap();
            shell.sync_auto_stopping(cx);
            assert!(!shell.sessions[&live].read(cx).auto_stopping);
            // ...and locked as soon as a refresh marks it as a session of the stopping workspace.
            shell.cloud_marks.insert(live, mark);
            shell.sync_auto_stopping(cx);
            assert!(shell.sessions[&live].read(cx).auto_stopping);
        })
        .unwrap();
    cx.run_until_parked();
}

#[gpui::test]
fn a_new_session_on_the_workspace_being_stopped_is_refused_and_nothing_is_created(
    cx: &mut TestAppContext,
) {
    use monocode_core::mono::cloud_api::{CloudTarget, InstanceChoice};
    let (window, _, gh) = world(cx, "Available", Some(10));
    let live = live_view(window, cx);
    let named = |name: &str| {
        CloudTarget::new(
            CloudBackendKind::Codespaces,
            InstanceChoice::Named(name.into()),
        )
    };
    check_at(window, 0, cx);
    window
        .update(cx, |shell, window, cx| {
            let session = CloudTarget::new(
                CloudBackendKind::Codespaces,
                InstanceChoice::OfSession(live),
            );
            let new = CloudTarget::new(CloudBackendKind::Codespaces, InstanceChoice::New);
            // Before the stop: everything is allowed.
            assert_eq!(shell.refuse_new_session_on(&named(CODESPACE)), Ok(()));
            shell.auto_stop_epoch = std::time::Instant::now() - Duration::from_secs(11 * 60);
            shell.check_auto_stop(cx);
            let refused = Err(super::cloud_auto_stop::STOPPING_NEW_SESSION.to_owned());
            // `sessions.start` of an app or a Mono, by name or by a session of the workspace.
            assert_eq!(shell.refuse_new_session_on(&named(CODESPACE)), refused);
            assert_eq!(shell.refuse_new_session_on(&session), refused);
            // A new workspace and another workspace are not held.
            assert_eq!(shell.refuse_new_session_on(&new), Ok(()));
            assert_eq!(shell.refuse_new_session_on(&named("octo-other")), Ok(()));
            // The codespace picker refuses the click, before anything is created.
            let sessions = shell.session_summaries.len();
            shell.open_codespace_picker_for("/work/monocode".into(), window, cx);
            assert_eq!(
                shell.pick_codespace_for_test(CODESPACE, cx).as_deref(),
                Some(super::cloud_auto_stop::STOPPING_NEW_SESSION)
            );
            assert!(!shell.codespace_picker_busy(cx), "no worker was started");
            assert_eq!(shell.session_summaries.len(), sessions);
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(gh.stops(), 1);
    // Over: the same requests are allowed again.
    window
        .update(cx, |shell, _, _| {
            assert_eq!(shell.refuse_new_session_on(&named(CODESPACE)), Ok(()));
        })
        .unwrap();
}

#[gpui::test]
fn the_sandbox_picker_refuses_a_sandbox_that_is_being_stopped(cx: &mut TestAppContext) {
    let (window, _, _) = crate::shell::tests::window(cx);
    window
        .update(cx, |shell, window, cx| {
            let key = CloudRef::sandbox("sbx-held").key();
            shell.auto_stop_running.insert(key.clone());
            shell.open_sandbox_picker("/work/widgets".into(), window, cx);
            assert_eq!(
                shell.pick_sandbox_for_test("sbx-held", cx).as_deref(),
                Some(super::cloud_auto_stop::STOPPING_NEW_SESSION)
            );
            assert!(!shell.sandbox_picker_busy(cx), "no worker was started");
            shell.auto_stop_running.remove(&key);
        })
        .unwrap();
}
