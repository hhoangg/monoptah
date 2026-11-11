//! The Continue locally dialog: nothing runs before the confirmation, success adopts the
//! result, and a failure leaves the session in the cloud with a retry.
use super::cloud_continue::{Kind, confirm_text};
use super::cloud_location_tests::{LOCAL, host_record, move_to_cloud, service};
use super::*;
use gpui::TestAppContext;
use monocode_core::cloud_sessions::{ConvertError, Converted, Resume};
use std::sync::atomic::{AtomicUsize, Ordering};

fn labels(shell: &Shell, id: i64, cx: &App) -> Vec<String> {
    shell
        .session_menu_items(id, cx)
        .into_iter()
        .filter_map(|item| match item {
            crate::popover::MenuItem::Action(a) => Some(a.label.to_string()),
            _ => None,
        })
        .collect()
}

/// A fake worker: counts calls, optionally fails, and on success does what the core does for
/// a moved session (the location goes back to Local).
fn fake(
    calls: Arc<AtomicUsize>,
    fail: Option<&'static str>,
    store: Database,
) -> cloud_continue::ContinueFn {
    Arc::new(move |_, id, progress| {
        calls.fetch_add(1, Ordering::SeqCst);
        progress("Waking the codespace");
        if let Some(error) = fail {
            return Err(ConvertError::Failed(error.into()));
        }
        let s = store.lock().unwrap();
        if s.cloud_location(id).unwrap().is_cloud() {
            s.leave_cloud(id).unwrap();
        }
        let summary = s
            .sessions()
            .unwrap()
            .into_iter()
            .find(|x| x.id == id)
            .unwrap();
        Ok(Converted {
            cloud_id: id,
            local_id: id,
            title: summary.title.clone(),
            summary,
            codespace: "octo-abc".into(),
            rows: 0,
            resume: Resume::Missing("test".into()),
            evidence: vec![],
            code_dir: None,
            ignored: vec![],
            ignored_truncated: false,
            evidence_partial: false,
            warnings: vec!["Applied.".into()],
            history_saved: true,
        })
    })
}

fn moved_window(cx: &mut TestAppContext) -> (gpui::WindowHandle<Shell>, i64) {
    let (window, _, local) = crate::shell::tests::window(cx);
    window
        .update(cx, |shell, _, cx| {
            shell.cloud = Arc::new(service(shell.store.clone()));
            let host = host_record(&shell.store, "host-1");
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
            move_to_cloud(&shell.store, local, host);
            shell.refresh_cloud(cx);
        })
        .unwrap();
    cx.run_until_parked();
    (window, local)
}

#[gpui::test]
fn the_menu_offers_it_for_cloud_sessions_only(cx: &mut TestAppContext) {
    let (window, local) = moved_window(cx);
    window
        .update(cx, |shell, _, cx| {
            assert!(shell.cloud_located.contains_key(&local));
            assert!(labels(shell, local, cx).contains(&"Continue locally…".to_string()));
            let plain = shell
                .session_summaries
                .keys()
                .copied()
                .find(|id| !shell.cloud_marks.contains_key(id))
                .expect("a plain local session");
            assert!(!labels(shell, plain, cx).contains(&"Continue locally…".to_string()));
        })
        .unwrap();
}

#[gpui::test]
fn nothing_runs_before_the_confirmation_and_cancel_changes_nothing(cx: &mut TestAppContext) {
    let (window, local) = moved_window(cx);
    let calls = Arc::new(AtomicUsize::new(0));
    window
        .update(cx, |shell, window, cx| {
            shell.cloud_continue.run = fake(calls.clone(), None, shell.store.clone());
            shell.run_session_menu(local, "continue-locally", window, cx);
            let dialog = shell
                .cloud_continue
                .dialog
                .as_ref()
                .expect("the confirmation");
            let _ = dialog;
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, window, cx| {
            assert_eq!(calls.load(Ordering::SeqCst), 0, "asked, not run");
            shell.close_continue_locally(window, cx);
            assert!(shell.cloud_continue.dialog.is_none());
            assert!(
                locked(&shell.store)
                    .unwrap()
                    .cloud_location(local)
                    .unwrap()
                    .is_cloud(),
                "still in the cloud"
            );
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[gpui::test]
fn confirming_brings_the_same_session_back_and_a_failure_can_retry(cx: &mut TestAppContext) {
    let (window, local) = moved_window(cx);
    let calls = Arc::new(AtomicUsize::new(0));
    window
        .update(cx, |shell, window, cx| {
            shell.cloud_continue.run =
                fake(calls.clone(), Some("ssh dropped"), shell.store.clone());
            shell.open_continue_locally(local, window, cx);
            shell.confirm_continue_locally(window, cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, window, cx| {
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            // The failure keeps the dialog for a retry and the session in the cloud.
            assert!(shell.cloud_continue.dialog.is_some());
            assert!(
                locked(&shell.store)
                    .unwrap()
                    .cloud_location(local)
                    .unwrap()
                    .is_cloud()
            );
            assert!(!shell.cloud_busy.contains(&local));
            shell.cloud_continue.run = fake(calls.clone(), None, shell.store.clone());
            shell.confirm_continue_locally(window, cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, _| {
            assert_eq!(calls.load(Ordering::SeqCst), 2);
            assert!(shell.cloud_continue.dialog.is_none());
            assert!(
                !locked(&shell.store)
                    .unwrap()
                    .cloud_location(local)
                    .unwrap()
                    .is_cloud()
            );
            assert!(
                !shell.cloud_marks.contains_key(&local),
                "no longer a cloud row"
            );
            assert!(
                shell.session_summaries.contains_key(&local),
                "the same session"
            );
            assert!(
                shell
                    .error
                    .as_deref()
                    .is_some_and(|e| e.contains("Back on this computer")),
                "{:?}",
                shell.error
            );
        })
        .unwrap();
}

#[test]
fn the_confirmation_says_what_will_happen_and_never_uses_dashes() {
    let moved = confirm_text(Kind::Moved { unsynced: false }, "Fix login", "codespace");
    for needle in [
        "wake the codespace",
        "uncommitted changes are never overwritten",
        "stays in the cloud",
        "same conversation",
    ] {
        assert!(moved.contains(needle), "{needle}: {moved}");
    }
    let unsynced = confirm_text(Kind::Moved { unsynced: true }, "Fix login", "codespace");
    assert!(unsynced.contains("copy the cloud turns that are not here yet"));
    let lost = confirm_text(Kind::Lost, "Fix login", "sandbox");
    assert!(
        lost.contains("deleted") && lost.contains("summary"),
        "{lost}"
    );
    let born = confirm_text(Kind::Born, "Fix login", "codespace");
    assert!(
        born.contains("not archived") && born.contains("new worktree"),
        "{born}"
    );
    for text in [moved, unsynced, lost, born] {
        assert!(!text.contains('\u{2014}') && !text.contains('\u{2013}'));
    }
    let _ = LOCAL;
}

use serde_json::json;

fn scratch_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "f45-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn broken_marker(
    window: &gpui::WindowHandle<Shell>,
    cx: &mut TestAppContext,
    local: i64,
    backup: &std::path::Path,
) {
    window
        .update(cx, |shell, _, _| {
            locked(&shell.store)
                .unwrap()
                .set_cloud_continue_broken(local, &json!({"backup": backup}).to_string())
                .unwrap();
        })
        .unwrap();
}

fn fake_backup(
    calls: Arc<AtomicUsize>,
    fail: Option<&'static str>,
    store: Database,
) -> cloud_continue::BackupFn {
    Arc::new(move |_, id, _restore| {
        calls.fetch_add(1, Ordering::SeqCst);
        if let Some(error) = fail {
            return Err(error.into());
        }
        locked(&store)
            .unwrap()
            .set_cloud_continue_broken(id, "")
            .unwrap();
        Ok("Done.".into())
    })
}

#[gpui::test]
fn restore_and_discard_are_offered_and_nothing_runs_before_the_confirmation(
    cx: &mut TestAppContext,
) {
    let (window, local) = moved_window(cx);
    let tmp = scratch_dir();
    broken_marker(&window, cx, local, tmp.as_path());
    let calls = Arc::new(AtomicUsize::new(0));
    window
        .update(cx, |shell, window, cx| {
            shell.cloud_continue.backup = fake_backup(calls.clone(), None, shell.store.clone());
            let items = labels(shell, local, cx);
            assert!(
                items.contains(&"Restore my backup…".to_string()),
                "{items:?}"
            );
            assert!(items.contains(&"Discard backup…".to_string()));
            shell.run_session_menu(local, "restore-continue-backup", window, cx);
            assert_eq!(shell.cloud_continue.phase_name(), "confirm-backup");
            // Cancel runs nothing and keeps the marker.
            shell.close_continue_locally(window, cx);
            assert_eq!(shell.cloud_continue.phase_name(), "closed");
            assert!(
                locked(&shell.store)
                    .unwrap()
                    .cloud_continue_broken(local)
                    .is_some()
            );
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    window
        .update(cx, |shell, window, cx| {
            shell.run_session_menu(local, "restore-continue-backup", window, cx);
            shell.confirm_backup_action(window, cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, _| {
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            assert_eq!(shell.cloud_continue.phase_name(), "closed");
            assert!(
                locked(&shell.store)
                    .unwrap()
                    .cloud_continue_broken(local)
                    .is_none()
            );
            assert_eq!(shell.error.as_deref(), Some("Done."));
        })
        .unwrap();
}

#[gpui::test]
fn a_failed_restore_keeps_the_marker_and_can_be_retried(cx: &mut TestAppContext) {
    let (window, local) = moved_window(cx);
    let tmp = scratch_dir();
    broken_marker(&window, cx, local, tmp.as_path());
    let calls = Arc::new(AtomicUsize::new(0));
    window
        .update(cx, |shell, window, cx| {
            shell.cloud_continue.backup = fake_backup(
                calls.clone(),
                Some("a file is in the way"),
                shell.store.clone(),
            );
            shell.open_backup_action(local, cloud_continue::Op::Restore, window, cx);
            shell.confirm_backup_action(window, cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, window, cx| {
            assert_eq!(shell.cloud_continue.phase_name(), "failed");
            assert!(
                locked(&shell.store)
                    .unwrap()
                    .cloud_continue_broken(local)
                    .is_some()
            );
            shell.cloud_continue.backup = fake_backup(calls.clone(), None, shell.store.clone());
            shell.confirm_backup_action(window, cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, _| {
            assert_eq!(calls.load(Ordering::SeqCst), 2);
            assert!(
                locked(&shell.store)
                    .unwrap()
                    .cloud_continue_broken(local)
                    .is_none()
            );
        })
        .unwrap();
}

#[gpui::test]
fn a_blocked_continue_offers_the_backup_actions_and_discard_unblocks_delete(
    cx: &mut TestAppContext,
) {
    let (window, local) = moved_window(cx);
    let tmp = scratch_dir();
    let backup = tmp.join("pre-continue-1");
    std::fs::create_dir_all(&backup).unwrap();
    std::fs::write(backup.join("changes.patch"), "x").unwrap();
    broken_marker(&window, cx, local, &backup);
    let calls = Arc::new(AtomicUsize::new(0));
    window
        .update(cx, |shell, window, cx| {
            shell.cloud_continue.run = fake(
                calls.clone(),
                Some("An earlier return was interrupted"),
                shell.store.clone(),
            );
            shell.open_continue_locally(local, window, cx);
            shell.confirm_continue_locally(window, cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, window, cx| {
            assert_eq!(shell.cloud_continue.phase_name(), "failed");
            // From the failed dialog, Discard asks first.
            shell.ask_backup_action(cloud_continue::Op::Discard, cx);
            assert_eq!(shell.cloud_continue.phase_name(), "confirm-backup");
            assert!(
                locked(&shell.store).unwrap().delete(local).is_err(),
                "delete is blocked"
            );
            shell.confirm_backup_action(window, cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, _| {
            let mut store = locked(&shell.store).unwrap();
            assert!(store.cloud_continue_broken(local).is_none());
            // This store keeps no session folders, so the path cannot be verified as one of ours
            // and nothing is deleted (the core test covers the real deletion).
            assert!(backup.exists());
            store.delete(local).unwrap();
        })
        .unwrap();
}

#[test]
fn the_backup_confirmations_name_the_path_and_the_consequence() {
    let restore = cloud_continue::backup_text(cloud_continue::Op::Restore, "/tmp/b");
    assert!(restore.contains("/tmp/b") && restore.contains("fresh backup"));
    let discard = cloud_continue::backup_text(cloud_continue::Op::Discard, "/tmp/b");
    let keep = cloud_continue::backup_text(cloud_continue::Op::Keep, "/tmp/b");
    assert!(keep.contains("index") && keep.contains("/tmp/b"));
    assert!(discard.contains("/tmp/b") && discard.contains("cannot be undone"));
    for text in [restore, discard, keep] {
        assert!(!text.contains('\u{2014}') && !text.contains('\u{2013}'));
    }
}

#[gpui::test]
fn keep_the_cloud_state_is_offered_only_at_the_cloud_commit_and_runs_after_the_confirmation(
    cx: &mut TestAppContext,
) {
    let (window, local) = moved_window(cx);
    let tmp = scratch_dir();
    let repo = tmp.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .current_dir(&repo)
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success());
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    git(&["init", "-q"]);
    git(&["commit", "-q", "--allow-empty", "-m", "one"]);
    let cloud_head = git(&["rev-parse", "HEAD"]);
    let marker = |head: &str| {
        json!({"backup": tmp.join("pre-continue-1"), "root": repo, "cloudHead": head}).to_string()
    };
    let calls = Arc::new(AtomicUsize::new(0));
    window
        .update(cx, |shell, window, cx| {
            let store = shell.store.clone();
            locked(&store)
                .unwrap()
                .set_cloud_continue_broken(local, &marker("0".repeat(40).as_str()))
                .unwrap();
            // Not at the cloud commit: no Keep.
            assert!(!labels(shell, local, cx).contains(&"Keep the cloud state…".to_string()));
            locked(&store)
                .unwrap()
                .set_cloud_continue_broken(local, &marker(&cloud_head))
                .unwrap();
            shell.cloud_continue.backup = fake_backup(calls.clone(), None, store);
            assert!(labels(shell, local, cx).contains(&"Keep the cloud state…".to_string()));
            shell.run_session_menu(local, "keep-continue-backup", window, cx);
            assert_eq!(shell.cloud_continue.phase_name(), "confirm-backup");
            assert_eq!(calls.load(Ordering::SeqCst), 0);
            shell.confirm_backup_action(window, cx);
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

// ------------------------------------------------------------ app API (F4-3b)

fn project_of(window: gpui::WindowHandle<Shell>, id: i64, cx: &mut TestAppContext) -> String {
    window
        .update(cx, |shell, _, _| {
            shell.cloud_marks[&id].local_project.clone()
        })
        .unwrap()
}

#[gpui::test]
fn an_agent_return_runs_the_same_continue_locally_and_reports_through_the_status(
    cx: &mut TestAppContext,
) {
    let (window, local) = moved_window(cx);
    let project = project_of(window, local, cx);
    let calls = Arc::new(AtomicUsize::new(0));
    let started = window
        .update(cx, |shell, window, cx| {
            shell.cloud_continue.run = fake(calls.clone(), None, shell.store.clone());
            shell.api_move_to_local(&project, local, window, cx)
        })
        .unwrap()
        .unwrap();
    // The call only opens the confirmation: nothing runs before the user presses Continue.
    assert_eq!(started["state"], "awaitingConfirmation");
    cx.run_until_parked();
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let waiting = window
        .update(cx, |shell, _, _| shell.api_move_status(&project, local))
        .unwrap()
        .unwrap();
    assert_eq!(waiting["state"], "awaitingConfirmation");
    assert_eq!(waiting["direction"], "toLocal");
    window
        .update(cx, |shell, window, cx| {
            assert_eq!(shell.cloud_continue.phase_name(), "confirm");
            shell.confirm_continue_locally(window, cx)
        })
        .unwrap();
    cx.run_until_parked();
    // The seam of the dialog ran once, and the session is local again.
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let status = window
        .update(cx, |shell, _, _| shell.api_move_status(&project, local))
        .unwrap()
        .unwrap();
    assert_eq!(status["state"], "none");
    assert_eq!(status["location"], "local");
    assert!(
        !locked(&window.update(cx, |s, _, _| s.store.clone()).unwrap())
            .unwrap()
            .cloud_location(local)
            .unwrap()
            .is_cloud()
    );
}

#[gpui::test]
fn a_refusal_of_continue_locally_reaches_the_agent_and_the_session_stays_in_the_cloud(
    cx: &mut TestAppContext,
) {
    let (window, local) = moved_window(cx);
    let project = project_of(window, local, cx);
    let calls = Arc::new(AtomicUsize::new(0));
    window
        .update(cx, |shell, window, cx| {
            shell.cloud_continue.run = fake(
                calls.clone(),
                Some("A turn is running in the cloud. Wait for it to finish."),
                shell.store.clone(),
            );
            shell
                .api_move_to_local(&project, local, window, cx)
                .unwrap();
            shell.confirm_continue_locally(window, cx);
        })
        .unwrap();
    cx.run_until_parked();
    let status = window
        .update(cx, |shell, _, _| shell.api_move_status(&project, local))
        .unwrap()
        .unwrap();
    assert_eq!(status["state"], "failed");
    assert_eq!(status["direction"], "toLocal");
    assert!(
        status["error"]
            .as_str()
            .unwrap()
            .contains("turn is running"),
        "{status}"
    );
    window
        .update(cx, |shell, _, _| {
            assert!(
                locked(&shell.store)
                    .unwrap()
                    .cloud_location(local)
                    .unwrap()
                    .is_cloud()
            );
            assert_eq!(shell.cloud_continue.phase_name(), "failed");
        })
        .unwrap();
}

#[gpui::test]
fn the_real_continue_locally_is_the_default_and_its_failure_leaves_the_cloud_in_place(
    cx: &mut TestAppContext,
) {
    // No seam replaced: this is `CloudSessions::continue_locally` against a Host that is
    // offline, so it refuses and nothing changes.
    let (window, local) = moved_window(cx);
    let project = project_of(window, local, cx);
    window
        .update(cx, |shell, window, cx| {
            shell
                .api_move_to_local(&project, local, window, cx)
                .unwrap();
            shell.confirm_continue_locally(window, cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, _| {
            let status = shell.api_move_status(&project, local).unwrap();
            assert_eq!(status["state"], "failed", "{status}");
            assert!(
                locked(&shell.store)
                    .unwrap()
                    .cloud_location(local)
                    .unwrap()
                    .is_cloud()
            );
        })
        .unwrap();
}

#[gpui::test]
fn an_agent_return_is_refused_for_the_wrong_session_a_running_turn_or_another_dialog(
    cx: &mut TestAppContext,
) {
    let (window, local) = moved_window(cx);
    let project = project_of(window, local, cx);
    let calls = Arc::new(AtomicUsize::new(0));
    window
        .update(cx, |shell, window, cx| {
            shell.cloud_continue.run = fake(calls.clone(), None, shell.store.clone());
            // Another project, an unknown session, a session that is not in the cloud.
            let error = shell
                .api_move_to_local("/somewhere/else", local, window, cx)
                .unwrap_err();
            assert!(error.contains("outside the assigned project"), "{error}");
            assert!(shell.api_move_to_local(&project, 9999, window, cx).is_err());
            let plain = shell
                .session_summaries
                .keys()
                .copied()
                .find(|id| !shell.cloud_marks.contains_key(id))
                .expect("a plain local session");
            let own = shell.session_summaries[&plain].cwd.clone();
            let error = shell
                .api_move_to_local(&own, plain, window, cx)
                .unwrap_err();
            assert!(error.contains("does not live in the cloud"), "{error}");
            // A turn running here.
            shell.sessions[&local].update(cx, |c, _| c.busy = true);
            let error = shell
                .api_move_to_local(&project, local, window, cx)
                .unwrap_err();
            assert!(error.contains("turn is running"), "{error}");
            shell.sessions[&local].update(cx, |c, _| c.busy = false);
            // The user's own dialog is open.
            shell.run_session_menu(local, "continue-locally", window, cx);
            let error = shell
                .api_move_to_local(&project, local, window, cx)
                .unwrap_err();
            assert!(error.contains("Another return"), "{error}");
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(calls.load(Ordering::SeqCst), 0, "nothing ran");
}

#[gpui::test]
fn another_projects_session_gets_the_same_refusal_whether_local_or_in_the_cloud(
    cx: &mut TestAppContext,
) {
    let (window, local) = moved_window(cx);
    window
        .update(cx, |shell, window, cx| {
            let plain = shell
                .session_summaries
                .keys()
                .copied()
                .find(|id| !shell.cloud_marks.contains_key(id))
                .expect("a plain local session");
            let one = shell
                .api_move_to_local("/somewhere/else", local, window, cx)
                .unwrap_err();
            let two = shell
                .api_move_to_local("/somewhere/else", plain, window, cx)
                .unwrap_err();
            assert_eq!(one, two);
        })
        .unwrap();
}

#[gpui::test]
fn a_session_born_in_the_cloud_still_answers_under_its_old_id_once_it_is_local(
    cx: &mut TestAppContext,
) {
    let (window, local) = moved_window(cx);
    let project = project_of(window, local, cx);
    window
        .update(cx, |shell, _, _| {
            // The old (negative) id was forgotten by the adoption; its new local id is kept.
            shell.remember_returned(local, 4242);
            let done = shell.api_move_status(&project, local).unwrap();
            assert_eq!(done["state"], "done");
            assert_eq!(done["newSession"], 4242);
            // Only for the project that owned it.
            assert!(shell.api_move_status("/somewhere/else", local).is_err());
            // The memory is bounded.
            for n in 0..100 {
                shell.remember_returned(local, 5000 + n);
            }
            assert!(shell.api_returned.len() <= 32);
        })
        .unwrap();
}

/// Helpers shared by the tests of the blocking progress dialogs.
pub(super) mod blocking {
    use super::*;
    use gpui::{Bounds, Pixels, Point, VisualTestContext};

    /// The sidebar row the tests hover and click, and the button its hover reveals.
    pub(in crate::shell) const ROW: &str = "sidebar-session-2";
    pub(in crate::shell) const ARCHIVE: &str = "archive-session-2";
    pub(in crate::shell) const PROBED: i64 = 2;
    pub(in crate::shell) const OTHER: i64 = 1;
    pub(in crate::shell) const MOD: &str = if cfg!(target_os = "macos") {
        "cmd"
    } else {
        "ctrl"
    };

    /// Binds the keymap the Workspace shortcuts come from, before the window opens.
    pub(in crate::shell) fn bind(cx: &mut TestAppContext) {
        cx.update(|cx| {
            crate::input::bind(cx);
            super::super::bind_keys(cx);
            cx.set_reduce_motion(true);
        });
    }

    /// Makes `PROBED` a visible, inactive sidebar row and returns its centre.
    pub(in crate::shell) fn arm(
        window: gpui::WindowHandle<Shell>,
        cx: &mut TestAppContext,
    ) -> Point<Pixels> {
        window
            .update(cx, |shell, _, cx| {
                shell.sessions[&PROBED]
                    .update(cx, |session, _| session.summary.has_messages = true);
                shell.workspace.active = Some(OTHER);
                shell.invalidate_chrome(cx);
            })
            .unwrap();
        cx.refresh().unwrap();
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        visual.refresh().unwrap();
        visual.debug_bounds(ROW).expect("the probed row").center()
    }

    /// Whether hovering the row at `at` reveals its Archive button.
    pub(in crate::shell) fn hover_reveals(
        visual: &mut VisualTestContext,
        at: Point<Pixels>,
    ) -> bool {
        visual.simulate_mouse_move(
            gpui::point(gpui::px(900.), gpui::px(700.)),
            None,
            Default::default(),
        );
        visual.run_until_parked();
        visual.refresh().unwrap();
        visual.simulate_mouse_move(at, None, Default::default());
        visual.run_until_parked();
        visual.refresh().unwrap();
        visual.debug_bounds(ARCHIVE).is_some()
    }

    pub(in crate::shell) fn sidebar_visible(
        window: gpui::WindowHandle<Shell>,
        visual: &mut VisualTestContext,
    ) -> bool {
        window
            .update(visual, |shell, _, _| shell.workspace.sidebar_visible)
            .unwrap()
    }

    /// Dispatches the View > Toggle Sidebar menu action to the focused node, as AppKit does.
    pub(in crate::shell) fn menu_toggle_sidebar(
        window: gpui::WindowHandle<Shell>,
        visual: &mut VisualTestContext,
    ) {
        use gpui::Action;
        window
            .update(visual, |_, window, cx| {
                window.dispatch_action(super::super::ToggleSidebar.boxed_clone(), cx)
            })
            .unwrap();
        visual.run_until_parked();
    }

    pub(in crate::shell) fn active(
        window: gpui::WindowHandle<Shell>,
        visual: &mut VisualTestContext,
    ) -> Option<i64> {
        window
            .update(visual, |shell, _, _| shell.workspace.active)
            .unwrap()
    }

    /// The window behind a dialog gets no hover, workspace shortcut or Tab.
    pub(in crate::shell) fn assert_blocked(
        window: gpui::WindowHandle<Shell>,
        visual: &mut VisualTestContext,
        at: Point<Pixels>,
        card: Bounds<Pixels>,
        dialog_focus: impl Fn(&Shell, &Window, &App) -> bool,
    ) {
        assert!(
            !card.contains(&at),
            "the probed row must be outside the card"
        );
        assert!(!hover_reveals(visual, at), "hover must not reach the row");
        let visible = sidebar_visible(window, visual);
        visual.simulate_keystrokes(&format!("{MOD}-b"));
        assert_eq!(
            sidebar_visible(window, visual),
            visible,
            "no workspace shortcut"
        );
        menu_toggle_sidebar(window, visual);
        assert_eq!(
            sidebar_visible(window, visual),
            visible,
            "no menu action reaches the workspace"
        );
        visual.simulate_keystrokes("tab");
        assert!(
            window
                .update(visual, |shell, window, cx| dialog_focus(shell, window, cx))
                .unwrap(),
            "Tab must keep focus inside the dialog"
        );
    }

    /// The window works again: hover, the sidebar shortcut and focus inside the Shell.
    pub(in crate::shell) fn assert_restored(
        window: gpui::WindowHandle<Shell>,
        visual: &mut VisualTestContext,
        at: Point<Pixels>,
    ) {
        assert!(hover_reveals(visual, at), "hover works again");
        let visible = sidebar_visible(window, visual);
        visual.simulate_keystrokes(&format!("{MOD}-b"));
        assert_eq!(
            sidebar_visible(window, visual),
            !visible,
            "shortcut works again"
        );
        visual.simulate_keystrokes(&format!("{MOD}-b"));
        assert_eq!(sidebar_visible(window, visual), visible);
        menu_toggle_sidebar(window, visual);
        assert_eq!(
            sidebar_visible(window, visual),
            !visible,
            "menu works again"
        );
        menu_toggle_sidebar(window, visual);
        assert_eq!(sidebar_visible(window, visual), visible);
        assert!(
            window
                .update(visual, |shell, window, cx| shell
                    .focus
                    .contains_focused(window, cx))
                .unwrap(),
            "focus is back inside the Shell"
        );
    }
}

fn continue_focused(shell: &Shell, window: &Window, cx: &App) -> bool {
    shell
        .cloud_continue
        .dialog
        .as_ref()
        .is_some_and(|d| d.focus_handle().contains_focused(window, cx))
}

#[gpui::test]
fn a_running_continue_blocks_the_window_behind_it_and_ignores_escape(cx: &mut TestAppContext) {
    use blocking::*;
    bind(cx);
    let (window, local) = moved_window(cx);
    let at = arm(window, cx);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    assert!(
        hover_reveals(visual, at),
        "the hover probe works without a dialog"
    );
    window
        .update(visual, |shell, window, cx| {
            shell.open_continue_locally(local, window, cx);
            shell.cloud_continue.hold_working("Saving the code state");
        })
        .unwrap();
    visual.run_until_parked();
    visual.refresh().unwrap();
    let card = visual
        .debug_bounds("cloud-continue-dialog")
        .expect("the dialog");
    assert_blocked(window, visual, at, card, continue_focused);
    visual.simulate_keystrokes("escape");
    window
        .update(visual, |shell, _, _| {
            assert_eq!(shell.cloud_continue.phase_name(), "working");
        })
        .unwrap();
    // A press on the backdrop does not close it either.
    visual.simulate_click(
        gpui::point(gpui::px(900.), gpui::px(700.)),
        Default::default(),
    );
    window
        .update(visual, |shell, _, _| {
            assert_eq!(shell.cloud_continue.phase_name(), "working");
        })
        .unwrap();
}

#[gpui::test]
fn the_continue_confirmation_closes_with_escape_or_cancel_and_frees_the_window(
    cx: &mut TestAppContext,
) {
    use blocking::*;
    bind(cx);
    let (window, local) = moved_window(cx);
    let at = arm(window, cx);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    window
        .update(visual, |shell, window, cx| {
            shell.open_continue_locally(local, window, cx)
        })
        .unwrap();
    visual.run_until_parked();
    visual.refresh().unwrap();
    let card = visual
        .debug_bounds("cloud-continue-dialog")
        .expect("the dialog");
    assert_blocked(window, visual, at, card, continue_focused);
    window
        .update(visual, |shell, _, _| {
            assert_eq!(shell.cloud_continue.phase_name(), "confirm");
        })
        .unwrap();
    visual.simulate_keystrokes("escape");
    window
        .update(visual, |shell, _, _| {
            assert_eq!(shell.cloud_continue.phase_name(), "closed");
        })
        .unwrap();
    visual.refresh().unwrap();
    assert_restored(window, visual, at);

    // The Cancel button still works through the barrier.
    window
        .update(visual, |shell, window, cx| {
            shell.open_continue_locally(local, window, cx)
        })
        .unwrap();
    visual.run_until_parked();
    visual.refresh().unwrap();
    let cancel = visual
        .debug_bounds("cloud-continue-cancel")
        .expect("Cancel");
    visual.simulate_click(cancel.center(), Default::default());
    window
        .update(visual, |shell, _, _| {
            assert_eq!(shell.cloud_continue.phase_name(), "closed");
        })
        .unwrap();
    visual.refresh().unwrap();
    assert_restored(window, visual, at);
}

#[gpui::test]
fn a_finished_continue_returns_focus_and_the_window_to_the_shell(cx: &mut TestAppContext) {
    use blocking::*;
    bind(cx);
    let (window, local) = moved_window(cx);
    let at = arm(window, cx);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    let calls = Arc::new(AtomicUsize::new(0));
    window
        .update(visual, |shell, window, cx| {
            shell.cloud_continue.run = fake(calls.clone(), None, shell.store.clone());
            shell.open_continue_locally(local, window, cx);
            shell.confirm_continue_locally(window, cx);
        })
        .unwrap();
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    window
        .update(visual, |shell, _, _| {
            assert_eq!(shell.cloud_continue.phase_name(), "closed");
        })
        .unwrap();
    assert_restored(window, visual, at);
}
