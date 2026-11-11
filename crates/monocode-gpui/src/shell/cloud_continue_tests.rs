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
            shell.open_backup_action(local, true, window, cx);
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
    let backup = tmp.as_path().join("pre-continue-1");
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
            shell.ask_backup_action(false, cx);
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
            assert!(!backup.exists(), "the backup is deleted");
            store.delete(local).unwrap();
        })
        .unwrap();
}

#[test]
fn the_backup_confirmations_name_the_path_and_the_consequence() {
    let restore = cloud_continue::backup_text(true, "/tmp/b");
    assert!(restore.contains("/tmp/b") && restore.contains("fresh backup"));
    let discard = cloud_continue::backup_text(false, "/tmp/b");
    assert!(discard.contains("/tmp/b") && discard.contains("cannot be undone"));
    for text in [restore, discard] {
        assert!(!text.contains('\u{2014}') && !text.contains('\u{2013}'));
    }
}
