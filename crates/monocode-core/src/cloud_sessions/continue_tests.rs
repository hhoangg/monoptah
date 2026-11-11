//! "Continue locally" against the same real-git fixtures the archive tests use.
use super::convert_tests::{git, world, World, SESSION};
use super::*;
use crate::store::{CloudLocation, MovePoint, SessionLocation};
use std::path::{Path, PathBuf};
use std::process::Command;

fn encoded(claude: &Path, cwd: &Path) -> PathBuf {
    let real = std::fs::canonicalize(cwd).unwrap();
    claude
        .join("projects")
        .join(claude_project_dir_name(&real.to_string_lossy()))
}

fn row(role: &str, text: &str) -> crate::transcript::Row {
    crate::transcript::Row {
        role: role.into(),
        text: text.into(),
        first: true,
        sent_at: None,
    }
}

/// The rows the Host snapshot projects to (the cloud turns of the fixture).
fn cloud_rows(w: &World) -> Vec<String> {
    let mut value = crate::remote::protocol::apply_session_sync(
        None,
        &json!({"kind":"snapshot","value":w.host.snapshot}),
    )
    .unwrap();
    crate::remote::attachments::qualify_snapshot(&mut value, "env-1", "host-1");
    let (history, _) = Store::project_remote_history(&value).unwrap();
    history.rows.into_iter().map(|r| r.text).collect()
}

fn tree_of(dir: &Path) -> String {
    let staging = tempfile::tempdir().unwrap();
    move_out::snapshot(dir, staging.path()).unwrap().tree
}

/// A local session that moved to the cloud: two local rows before the move, the Host record
/// linked, the location Cloud, the move point recorded, and the pre-move Claude file on disk.
/// The cloud's turns are NOT mirrored yet (the Host holds them).
fn moved(w: &World) -> i64 {
    moved_in(w, &w.project)
}

/// [`moved`] for a session that works in `work` (maybe a subfolder of the repository).
fn moved_in(w: &World, work: &Path) -> i64 {
    let local = {
        let mut store = w.svc.store().unwrap();
        let session = store.create("claude", w.project.to_str().unwrap()).unwrap();
        store
            .write_rows(
                session.id,
                0,
                &[
                    row("user", "pre-move question"),
                    row("assistant", "pre-move answer"),
                ],
            )
            .unwrap();
        store.provider_id(session.id, SESSION).unwrap();
        session.id
    };
    let head = git(&w.project, &["rev-parse", "HEAD"]);
    let point = MovePoint {
        host_session_id: "host-1".into(),
        head,
        tree: tree_of(work),
        work_cwd: work.to_str().unwrap().into(),
        branch: {
            let name = git(work, &["rev-parse", "--symbolic-full-name", "HEAD"]);
            if name == "HEAD" {
                String::new()
            } else {
                name
            }
        },
    };
    let store = w.svc.store().unwrap();
    store
        .set_cloud_location(
            local,
            &SessionLocation::Cloud(CloudLocation {
                cloud: CloudRef::codespace("cs-1"),
                environment_id: "env-1".into(),
                project_id: "p".into(),
                host_session_id: "host-1".into(),
                remote_cwd: w.ws.to_str().unwrap().into(),
                local_cwd: w.project.to_str().unwrap().into(),
                moved_at_row: 2,
                pushed_branch: Some("feat/x".into()),
            }),
        )
        .unwrap();
    store.link_remote_session(w.cloud_id, Some(local)).unwrap();
    store.set_cloud_move_point(local, &point).unwrap();
    drop(store);
    // The local file as the move left it: the first entry of the cloud chain.
    let dir = encoded(&w.claude, &w.project);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(format!("{SESSION}.jsonl")),
        format!(
            "{{\"type\":\"user\",\"uuid\":\"u1\",\"cwd\":\"{}\",\"sessionId\":\"{SESSION}\"}}\n",
            w.project.display()
        ),
    )
    .unwrap();
    local
}

fn run(w: &World, id: i64) -> Result<Converted, ConvertError> {
    w.svc.continue_locally(id, &mut |_| {})
}

#[test]
fn a_round_trip_keeps_one_session_one_id_and_the_cloud_turns_in_the_local_file() {
    let w = world(true);
    let local = moved(&w);
    let before =
        std::fs::read_to_string(encoded(&w.claude, &w.project).join(format!("{SESSION}.jsonl")))
            .unwrap();
    assert!(!before.contains("\"u2\""));
    let done = run(&w, local).unwrap();
    assert_eq!(done.local_id, local, "the same session continues");
    let store = w.svc.store().unwrap();
    assert_eq!(store.cloud_location(local).unwrap(), SessionLocation::Local);
    let sessions = store.sessions().unwrap();
    assert!(sessions.iter().all(|s| !s.archived), "nothing is archived");
    assert_eq!(
        sessions
            .iter()
            .filter(|s| s.cwd == w.project.to_str().unwrap())
            .count(),
        1,
        "one session, not two"
    );
    assert!(
        store.remote_session(w.cloud_id).unwrap().is_none(),
        "the cloud row is gone"
    );
    // The Host copy is tombstoned like Reset to local does.
    assert!(store.linked_remote_session(local).unwrap().is_none());
    // One id, and the local file now holds the cloud turn (R1 fixed).
    assert_eq!(summary_provider(&store, local), Some(SESSION.to_string()));
    drop(store);
    let dir = encoded(&w.claude, &w.project);
    let after = std::fs::read_to_string(dir.join(format!("{SESSION}.jsonl"))).unwrap();
    assert!(
        after.contains("\"u1\"") && after.contains("\"u2\""),
        "{after}"
    );
    assert!(
        after.contains(&w.project.display().to_string()),
        "cwd rewritten"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join(format!("{SESSION}.jsonl.pre-cloud"))).unwrap(),
        before,
        "the old file is kept"
    );
    // The two cloud rows were synced before the restore, after the two local ones, once.
    let rows = w.svc.store().unwrap().load(local).unwrap().rows;
    let texts: Vec<String> = rows.iter().map(|r| r.text.clone()).collect();
    let mut expected = vec![
        "pre-move question".to_string(),
        "pre-move answer".to_string(),
    ];
    expected.extend(cloud_rows(&w));
    assert_eq!(texts, expected);
    // The folder was unchanged since the move, so the cloud work is applied in place.
    assert_eq!(
        git(&w.project, &["log", "-1", "--format=%s"]),
        "claude commit"
    );
    assert_eq!(
        std::fs::read_to_string(w.project.join("a.txt")).unwrap(),
        "one\ntwo\n"
    );
    assert_eq!(
        std::fs::read_to_string(w.project.join("untracked.txt")).unwrap(),
        "new file\n"
    );
    assert!(
        w.svc
            .store()
            .unwrap()
            .agent_settings(local)
            .unwrap()
            .unwrap_or_default()
            .worktree_cwd
            .is_none(),
        "no worktree"
    );
    assert!(
        done.warnings.iter().any(|n| n.contains("applied to")),
        "{:?}",
        done.warnings
    );
    // The move point is spent.
    assert!(w.svc.store().unwrap().cloud_move_point(local).is_none());
}

fn summary_provider(store: &Store, id: i64) -> Option<String> {
    store
        .sessions()
        .unwrap()
        .into_iter()
        .find(|s| s.id == id)
        .and_then(|s| s.provider_id)
}

#[test]
fn rows_already_mirrored_are_not_duplicated() {
    let w = world(true);
    let local = moved(&w);
    // The F4-L append already copied the cloud turns.
    let mut snapshot = w.host.snapshot.clone();
    crate::remote::attachments::qualify_snapshot(&mut snapshot, "env-1", "host-1");
    w.svc
        .store()
        .unwrap()
        .append_remote_rows(local, &snapshot)
        .unwrap();
    let total = 2 + cloud_rows(&w).len();
    assert_eq!(
        w.svc.store().unwrap().load(local).unwrap().rows.len(),
        total
    );
    run(&w, local).unwrap();
    assert_eq!(
        w.svc.store().unwrap().load(local).unwrap().rows.len(),
        total
    );
}

#[test]
fn local_changes_force_a_new_worktree_and_leave_the_files_alone() {
    let w = world(true);
    let local = moved(&w);
    // The user edited a file after the move.
    std::fs::write(w.project.join("a.txt"), "my own edit\n").unwrap();
    let head_before = git(&w.project, &["rev-parse", "HEAD"]);
    let done = run(&w, local).unwrap();
    assert_eq!(
        std::fs::read_to_string(w.project.join("a.txt")).unwrap(),
        "my own edit\n"
    );
    assert_eq!(git(&w.project, &["rev-parse", "HEAD"]), head_before);
    let settings = w
        .svc
        .store()
        .unwrap()
        .agent_settings(local)
        .unwrap()
        .unwrap();
    let tree = PathBuf::from(
        settings
            .worktree_cwd
            .expect("the session moves to the worktree"),
    );
    assert!(
        tree.join("made.txt").is_file(),
        "the cloud commit is in the worktree"
    );
    assert_eq!(
        std::fs::read_to_string(tree.join("a.txt")).unwrap(),
        "one\ntwo\n"
    );
    // Claude finds the session by the folder it runs in.
    let installed = encoded(&w.claude, &tree).join(format!("{SESSION}.jsonl"));
    assert!(std::fs::read_to_string(installed)
        .unwrap()
        .contains("\"u2\""));
    assert!(
        done.warnings.iter().any(|n| n.contains("new worktree")),
        "{:?}",
        done.warnings
    );
    assert_eq!(
        w.svc.store().unwrap().cloud_location(local).unwrap(),
        SessionLocation::Local
    );
}

#[test]
fn a_moved_branch_forces_a_worktree_too() {
    let w = world(true);
    let local = moved(&w);
    git(
        &w.project,
        &["commit", "-q", "--allow-empty", "-m", "local work"],
    );
    run(&w, local).unwrap();
    let settings = w
        .svc
        .store()
        .unwrap()
        .agent_settings(local)
        .unwrap()
        .unwrap();
    assert!(settings.worktree_cwd.is_some());
    assert_eq!(git(&w.project, &["log", "-1", "--format=%s"]), "local work");
}

#[test]
fn a_diverged_local_file_forks_with_a_message_and_is_never_overwritten() {
    let w = world(true);
    let local = moved(&w);
    let file = encoded(&w.claude, &w.project).join(format!("{SESSION}.jsonl"));
    let mine = format!(
        "{{\"type\":\"user\",\"uuid\":\"zz\",\"cwd\":\"{}\",\"sessionId\":\"{SESSION}\"}}\n",
        w.project.display()
    );
    std::fs::write(&file, &mine).unwrap();
    let done = run(&w, local).unwrap();
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        mine,
        "never overwritten"
    );
    let Resume::Forked(new_id) = &done.resume else {
        panic!("{:?}", done.resume)
    };
    assert_ne!(new_id, SESSION);
    let store = w.svc.store().unwrap();
    assert_eq!(
        summary_provider(&store, local),
        Some(new_id.clone()),
        "continues on the fork"
    );
    assert!(
        done.warnings.iter().any(|n| n.contains("diverged")),
        "{:?}",
        done.warnings
    );
    drop(store);
    let fork = encoded(&w.claude, &w.project).join(format!("{new_id}.jsonl"));
    assert!(std::fs::read_to_string(fork).unwrap().contains("\"u2\""));
}

#[test]
fn a_deleted_workspace_continues_on_a_new_id_with_a_summary_and_never_the_stale_file() {
    let w = world(true);
    let local = moved(&w);
    // The workspace was deleted: its Host record went with it.
    w.svc
        .store()
        .unwrap()
        .forget_cloud_session(w.cloud_id)
        .unwrap();
    let stale =
        std::fs::read_to_string(encoded(&w.claude, &w.project).join(format!("{SESSION}.jsonl")))
            .unwrap();
    let done = run(&w, local).unwrap();
    assert!(
        w.host.log.lock().unwrap().is_empty(),
        "nothing was woken or asked"
    );
    let store = w.svc.store().unwrap();
    assert_eq!(store.cloud_location(local).unwrap(), SessionLocation::Local);
    assert_eq!(
        summary_provider(&store, local),
        None,
        "no id: a new one starts"
    );
    let settings = store.agent_settings(local).unwrap().unwrap();
    let brief = settings.handoff.expect("a summary of the rows").brief;
    assert!(brief.contains("pre-move question"), "{brief}");
    assert_eq!(store.load(local).unwrap().rows.len(), 2, "rows are kept");
    drop(store);
    assert_eq!(
        std::fs::read_to_string(encoded(&w.claude, &w.project).join(format!("{SESSION}.jsonl")))
            .unwrap(),
        stale,
        "the stale file is left alone"
    );
    assert!(
        done.warnings.iter().any(|n| n.contains("gone")),
        "{:?}",
        done.warnings
    );
}

#[test]
fn a_failure_midway_leaves_the_session_in_the_cloud_and_a_retry_works() {
    let w = world(true);
    let local = moved(&w);
    *w.gh.fail_snapshot.lock().unwrap() = Some("ssh dropped".into());
    let error = run(&w, local).unwrap_err().to_string();
    assert!(error.contains("ssh dropped"), "{error}");
    let store = w.svc.store().unwrap();
    assert!(
        store.cloud_location(local).unwrap().is_cloud(),
        "still Cloud"
    );
    assert!(
        store.linked_remote_session(local).unwrap().is_some(),
        "Host copy untouched"
    );
    assert!(store.cloud_move_point(local).is_some());
    assert_eq!(summary_provider(&store, local), Some(SESSION.to_string()));
    drop(store);
    let file = encoded(&w.claude, &w.project).join(format!("{SESSION}.jsonl"));
    assert!(
        !std::fs::read_to_string(&file).unwrap().contains("\"u2\""),
        "file untouched"
    );
    assert_eq!(
        git(&w.project, &["log", "-1", "--format=%s"]),
        "base",
        "code untouched"
    );
    // Retry once the problem is gone: one session, one id.
    *w.gh.fail_snapshot.lock().unwrap() = None;
    run(&w, local).unwrap();
    let store = w.svc.store().unwrap();
    assert_eq!(store.cloud_location(local).unwrap(), SessionLocation::Local);
    let total = 2 + cloud_rows(&w).len();
    assert_eq!(store.load(local).unwrap().rows.len(), total);
    drop(store);
    // A second call after success changes nothing and says why.
    assert!(run(&w, local).is_err());
    assert_eq!(
        w.svc.store().unwrap().load(local).unwrap().rows.len(),
        total
    );
}

#[test]
fn a_turn_running_in_the_cloud_blocks_it_before_anything_is_woken() {
    let w = world(true);
    let local = moved(&w);
    {
        let store = w.svc.store().unwrap();
        let (_, mut record) = store.linked_remote_session(local).unwrap().unwrap();
        record.running = true;
        record.flags_at = crate::time::now_ms();
        store.save_remote_session(&record).unwrap();
    }
    let error = run(&w, local).unwrap_err().to_string();
    assert_eq!(error, crate::store::HOST_TURN_RUNNING);
    assert!(w.host.log.lock().unwrap().is_empty());
    assert!(w
        .svc
        .store()
        .unwrap()
        .cloud_location(local)
        .unwrap()
        .is_cloud());
}

#[test]
fn a_session_born_in_the_cloud_gets_a_live_local_session_that_replaces_the_cloud_row() {
    let w = world(true);
    let done = run(&w, w.cloud_id).unwrap();
    assert_ne!(done.local_id, w.cloud_id);
    let store = w.svc.store().unwrap();
    let local = summary_provider(&store, done.local_id);
    assert_eq!(local, Some(SESSION.to_string()), "the cloud id comes back");
    assert!(
        !store
            .sessions()
            .unwrap()
            .iter()
            .find(|s| s.id == done.local_id)
            .unwrap()
            .archived
    );
    assert!(
        store.remote_session(w.cloud_id).unwrap().is_none(),
        "the cloud row is replaced"
    );
    assert_eq!(
        store.load(done.local_id).unwrap().rows.len(),
        cloud_rows(&w).len()
    );
    let settings = store.agent_settings(done.local_id).unwrap().unwrap();
    let tree = PathBuf::from(settings.worktree_cwd.expect("code lands in a new worktree"));
    drop(store);
    assert!(tree.join("made.txt").is_file());
    assert!(encoded(&w.claude, &tree)
        .join(format!("{SESSION}.jsonl"))
        .is_file());
}

#[test]
fn the_archive_flow_still_archives() {
    let w = world(true);
    let done = w.svc.convert_session(w.cloud_id, &mut |_| {}).unwrap();
    assert!(done.summary.archived);
    let store = w.svc.store().unwrap();
    assert!(
        store
            .sessions()
            .unwrap()
            .iter()
            .find(|s| s.id == done.local_id)
            .unwrap()
            .archived
    );
    assert!(store.remote_session(w.cloud_id).unwrap().is_none());
}

#[test]
fn a_session_in_a_subfolder_gets_the_whole_cloud_state_and_loses_nothing() {
    let w = world(true);
    // The local project gets a subfolder commit that the cloud also has.
    std::fs::create_dir_all(w.project.join("sub")).unwrap();
    std::fs::write(w.project.join("sub/x.txt"), "x\n").unwrap();
    git(&w.project, &["add", "."]);
    git(&w.project, &["commit", "-q", "-m", "sub"]);
    git(&w.project, &["push", "-q", "origin", "main"]);
    git(&w.ws, &["fetch", "-q", "origin"]);
    git(&w.ws, &["merge", "-q", "--no-edit", "origin/main"]);
    let local = moved_in(&w, &w.project.join("sub"));
    let done = run(&w, local).unwrap();
    assert!(
        done.warnings.iter().any(|n| n.contains("applied to")),
        "{:?}",
        done.warnings
    );
    // Files OUTSIDE the subfolder carry the cloud state too (the bug applied from sub/ only).
    assert_eq!(
        std::fs::read_to_string(w.project.join("a.txt")).unwrap(),
        "one\ntwo\n"
    );
    assert_eq!(
        std::fs::read_to_string(w.project.join("untracked.txt")).unwrap(),
        "new file\n"
    );
    assert!(w.project.join("made.txt").is_file());
    assert_eq!(
        git(
            &w.project,
            &["status", "--porcelain", "--untracked-files=no"]
        )
        .lines()
        .count(),
        2
    );
}

#[test]
fn an_ignored_local_file_in_the_way_falls_back_to_a_worktree_and_is_kept() {
    let w = world(true);
    let local = moved(&w);
    // Ignored, so the tree check passes; the cloud patch adds a file of the same name.
    std::fs::write(w.project.join(".git/info/exclude"), "untracked.txt\n").unwrap();
    std::fs::write(w.project.join("untracked.txt"), "mine\n").unwrap();
    run(&w, local).unwrap();
    assert_eq!(
        std::fs::read_to_string(w.project.join("untracked.txt")).unwrap(),
        "mine\n"
    );
    assert_eq!(
        git(&w.project, &["log", "-1", "--format=%s"]),
        "base",
        "rolled back and verified"
    );
    let settings = w
        .svc
        .store()
        .unwrap()
        .agent_settings(local)
        .unwrap()
        .unwrap();
    assert!(settings.worktree_cwd.is_some());
    // No backup is left behind after a verified rollback.
    let thread = w.svc.store().unwrap().thread_dir(local).unwrap().unwrap();
    let leftovers = std::fs::read_dir(&thread)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("pre-continue"))
        .count();
    assert_eq!(leftovers, 0);
}

#[test]
fn files_of_an_earlier_return_are_never_reused_and_the_restore_offer_is_consumed() {
    let w = world(true);
    let local = moved(&w);
    let thread = w.svc.store().unwrap().thread_dir(local).unwrap().unwrap();
    let stale = thread.join(CODESPACE_DIR);
    std::fs::create_dir_all(stale.join(PATCHES)).unwrap();
    std::fs::write(stale.join(RESTORED), r#"{"status":"done"}"#).unwrap();
    std::fs::write(stale.join(PATCH), "garbage that must never be applied").unwrap();
    std::fs::write(stale.join(PATCHES).join("0001.patch"), "garbage").unwrap();
    run(&w, local).unwrap();
    assert_eq!(
        std::fs::read_to_string(w.project.join("a.txt")).unwrap(),
        "one\ntwo\n"
    );
    let store = w.svc.store().unwrap();
    assert!(
        pending_restore_for(&store, local).is_none(),
        "no second restore is offered"
    );
}

#[test]
fn a_crash_after_the_apply_is_detected_on_retry_instead_of_forking_a_worktree() {
    let w = world(true);
    let local = moved(&w);
    // The cloud state is already here (the previous attempt applied it, then died).
    git(
        &w.project,
        &["fetch", "-q", w.ws.to_str().unwrap(), "feat/x"],
    );
    git(&w.project, &["merge", "-q", "--ff-only", "FETCH_HEAD"]);
    std::fs::write(w.project.join("a.txt"), "one\ntwo\n").unwrap();
    std::fs::write(w.project.join("bin.dat"), [9u8, 8, 7, 0]).unwrap();
    std::fs::write(w.project.join("untracked.txt"), "new file\n").unwrap();
    let done = run(&w, local).unwrap();
    assert!(
        done.warnings.iter().any(|n| n.contains("already applied")),
        "{:?}",
        done.warnings
    );
    let settings = w
        .svc
        .store()
        .unwrap()
        .agent_settings(local)
        .unwrap()
        .unwrap_or_default();
    assert!(settings.worktree_cwd.is_none());
}

#[test]
fn a_broken_earlier_attempt_blocks_a_retry_and_keeps_the_backup() {
    let w = world(true);
    let local = moved(&w);
    let backup = w.root.join("kept-backup");
    std::fs::create_dir_all(&backup).unwrap();
    std::fs::write(backup.join("changes.patch"), "x").unwrap();
    let marker = serde_json::json!({
        "root": w.project, "backup": backup, "head": git(&w.project, &["rev-parse", "HEAD"]),
        "tree": "0000000000000000000000000000000000000000", "patch": null
    });
    w.svc
        .store()
        .unwrap()
        .set_cloud_continue_broken(local, &marker.to_string())
        .unwrap();
    let error = run(&w, local).unwrap_err().to_string();
    assert!(error.contains("kept-backup"), "{error}");
    assert!(
        backup.join("changes.patch").is_file(),
        "the backup is never deleted"
    );
    let store = w.svc.store().unwrap();
    assert!(store.cloud_location(local).unwrap().is_cloud());
    assert!(store.cloud_continue_broken(local).is_some());
}

#[test]
fn a_sync_error_does_not_hide_a_running_cloud_turn() {
    let w = world(true);
    let local = moved(&w);
    {
        let store = w.svc.store().unwrap();
        store
            .record_cloud_sync_error(local, "Host unreachable")
            .unwrap();
        let (_, mut record) = store.linked_remote_session(local).unwrap().unwrap();
        record.running = true;
        record.flags_at = crate::time::now_ms();
        store.save_remote_session(&record).unwrap();
    }
    assert_eq!(
        run(&w, local).unwrap_err().to_string(),
        crate::store::HOST_TURN_RUNNING
    );
    assert!(w.host.log.lock().unwrap().is_empty());
}

#[test]
fn reset_with_cloud_turns_starts_a_new_claude_session_instead_of_resuming_the_stale_file() {
    let w = world(true);
    let local = moved(&w);
    let snapshot = {
        let mut s = w.host.snapshot.clone();
        crate::remote::attachments::qualify_snapshot(&mut s, "env-1", "host-1");
        s
    };
    w.svc
        .store()
        .unwrap()
        .append_remote_rows(local, &snapshot)
        .unwrap();
    w.svc.store().unwrap().leave_cloud(local).unwrap();
    let store = w.svc.store().unwrap();
    assert_eq!(
        summary_provider(&store, local),
        None,
        "no stale id to resume"
    );
    assert!(store
        .agent_settings(local)
        .unwrap()
        .unwrap()
        .handoff
        .is_some());
    assert_eq!(
        store.returned_gate(local),
        Some(store.load(local).unwrap().rows.len())
    );
}

#[test]
fn a_return_gates_the_turns_up_to_it() {
    let w = world(true);
    let local = moved(&w);
    std::fs::write(w.project.join("a.txt"), "my own edit\n").unwrap();
    run(&w, local).unwrap();
    let store = w.svc.store().unwrap();
    let rows = store.load(local).unwrap().rows.len();
    assert_eq!(store.returned_gate(local), Some(rows));
    drop(store);
    // In place too: the turns the cloud wrote ran on the other machine.
    let w = world(true);
    let local = moved(&w);
    run(&w, local).unwrap();
    let store = w.svc.store().unwrap();
    assert_eq!(
        store.returned_gate(local),
        Some(store.load(local).unwrap().rows.len())
    );
    // The gate only ever goes up.
    store.set_returned_gate(local, Some(1)).unwrap();
    store.set_returned_gate(local, None).unwrap();
    assert_eq!(
        store.returned_gate(local),
        Some(store.load(local).unwrap().rows.len())
    );
}

#[test]
fn deleting_a_session_forgets_its_return_state() {
    let w = world(true);
    let local = moved(&w);
    {
        let mut store = w.svc.store().unwrap();
        store.set_cloud_continue_progress(local, "{}").unwrap();
        store.set_returned_gate(local, Some(3)).unwrap();
        store
            .set_cloud_continue_broken(local, r#"{"backup":"/tmp/keep"}"#)
            .unwrap();
        // The only backup of someone's work is never deleted silently.
        assert!(store.delete(local).unwrap_err().contains("/tmp/keep"));
        store.set_cloud_continue_broken(local, "").unwrap();
        store.delete(local).unwrap();
        assert!(store.cloud_continue_progress(local).is_none());
        assert!(store.cloud_move_point(local).is_none());
        assert_eq!(store.returned_gate(local), None);
        assert!(store.cloud_continue_broken(local).is_none());
    }
}

fn hash_of(dir: &Path, rev: &str) -> String {
    git(dir, &["rev-parse", rev])
}

/// The state a marker records for the project as it is at the move.
fn write_marker(w: &World, local: i64, applied_head: &str, expected: &str) -> PathBuf {
    let backup = w.root.join("marker-backup");
    std::fs::create_dir_all(&backup).unwrap();
    std::fs::write(backup.join("changes.patch"), "x").unwrap();
    // As in a real attempt, the tree of the folder was written into the repository.
    let idx = w.root.join("marker.idx");
    let text = idx.to_string_lossy().into_owned();
    let run_in = |args: &[&str]| {
        let out = Command::new("git")
            .current_dir(&w.project)
            .env("GIT_INDEX_FILE", &text)
            .args(args)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    run_in(&["add", "-A"]);
    let tree = run_in(&["write-tree"]);
    let marker = serde_json::json!({
        "root": w.project, "backup": backup, "head": hash_of(&w.project, "HEAD"),
        "tree": tree, "branch": git(&w.project, &["symbolic-ref", "HEAD"]),
        "cloudHead": applied_head, "expected": expected, "at": 1
    });
    w.svc
        .store()
        .unwrap()
        .set_cloud_continue_broken(local, &marker.to_string())
        .unwrap();
    backup
}

#[test]
fn after_a_broken_return_new_edits_and_new_files_are_never_touched() {
    let w = world(true);
    let local = moved(&w);
    let backup = write_marker(&w, local, &hash_of(&w.ws, "HEAD"), &tree_of(&w.ws));
    // The user keeps working: edits a tracked file and creates a new one.
    std::fs::write(w.project.join("a.txt"), "edited after the failure\n").unwrap();
    std::fs::create_dir_all(w.project.join("src")).unwrap();
    std::fs::write(w.project.join("src/new.rs"), "fn main() {}\n").unwrap();
    let error = run(&w, local).unwrap_err().to_string();
    assert!(
        error.contains("marker-backup") && error.contains("Nothing was changed now"),
        "{error}"
    );
    assert_eq!(
        std::fs::read_to_string(w.project.join("a.txt")).unwrap(),
        "edited after the failure\n"
    );
    assert_eq!(
        std::fs::read_to_string(w.project.join("src/new.rs")).unwrap(),
        "fn main() {}\n"
    );
    assert!(backup.join("changes.patch").is_file());
    assert!(w
        .svc
        .store()
        .unwrap()
        .cloud_location(local)
        .unwrap()
        .is_cloud());
    // Only an explicit action restores, after a fresh backup of the current state.
    let fresh = w.svc.restore_continue_backup(local);
    assert!(fresh.is_ok(), "{fresh:?}");
    assert!(backup.join("changes.patch").is_file());
}

#[test]
fn a_crash_after_the_working_tree_changed_reports_the_backup_instead_of_claiming_nothing_happened()
{
    let w = world(true);
    let local = moved(&w);
    let backup = write_marker(&w, local, &hash_of(&w.ws, "HEAD"), &tree_of(&w.ws));
    // Crash between the file update and the branch move: new files, old HEAD.
    std::fs::write(w.project.join("a.txt"), "one\ntwo\n").unwrap();
    std::fs::write(w.project.join("made.txt"), "created by claude\n").unwrap();
    let error = run(&w, local).unwrap_err().to_string();
    assert!(error.contains("marker-backup"), "{error}");
    assert!(!error.contains("left alone"), "{error}");
    assert!(backup.is_dir());
    assert_eq!(
        std::fs::read_to_string(w.project.join("a.txt")).unwrap(),
        "one\ntwo\n"
    );
}

#[test]
fn a_crash_after_the_branch_moved_is_recognised_as_applied() {
    let w = world(true);
    let local = moved(&w);
    write_marker(&w, local, &hash_of(&w.ws, "HEAD"), &tree_of(&w.ws));
    git(
        &w.project,
        &["fetch", "-q", w.ws.to_str().unwrap(), "feat/x"],
    );
    git(&w.project, &["merge", "-q", "--ff-only", "FETCH_HEAD"]);
    std::fs::write(w.project.join("a.txt"), "one\ntwo\n").unwrap();
    std::fs::write(w.project.join("bin.dat"), [9u8, 8, 7, 0]).unwrap();
    std::fs::write(w.project.join("untracked.txt"), "new file\n").unwrap();
    let done = run(&w, local).unwrap();
    assert!(
        done.warnings.iter().any(|n| n.contains("already applied")),
        "{:?}",
        done.warnings
    );
    assert!(w
        .svc
        .store()
        .unwrap()
        .cloud_continue_broken(local)
        .is_none());
}

#[test]
fn read_tree_refuses_a_file_changed_or_created_after_the_snapshot() {
    use super::continue_local::g;
    let w = world(true);
    let _ = moved(&w);
    let tmp = tempfile::tempdir().unwrap();
    let idx = tmp.path().join("i.idx");
    let text = idx.to_string_lossy().into_owned();
    let envs = [("GIT_INDEX_FILE", text.as_str())];
    g(&w.project, &["add", "-A"], &envs).unwrap();
    let old = g(&w.project, &["write-tree"], &envs)
        .unwrap()
        .trim()
        .to_string();
    // The target tree changes a.txt and adds made.txt.
    let new_idx = tmp.path().join("n.idx");
    let ntext = new_idx.to_string_lossy().into_owned();
    let nenvs = [("GIT_INDEX_FILE", ntext.as_str())];
    std::fs::write(w.project.join("a.txt"), "cloud\n").unwrap();
    std::fs::write(w.project.join("made.txt"), "cloud\n").unwrap();
    g(&w.project, &["add", "-A"], &nenvs).unwrap();
    let target = g(&w.project, &["write-tree"], &nenvs)
        .unwrap()
        .trim()
        .to_string();
    git(&w.project, &["checkout", "-q", "--", "a.txt"]);
    std::fs::remove_file(w.project.join("made.txt")).unwrap();
    // (a) a tracked file edited after the snapshot
    std::fs::write(w.project.join("a.txt"), "typed after the snapshot\n").unwrap();
    assert!(g(&w.project, &["read-tree", "-m", "-u", &old, &target], &envs).is_err());
    assert_eq!(
        std::fs::read_to_string(w.project.join("a.txt")).unwrap(),
        "typed after the snapshot\n"
    );
    git(&w.project, &["checkout", "-q", "--", "a.txt"]);
    // (b) an untracked file created in the way
    std::fs::write(w.project.join("made.txt"), "mine\n").unwrap();
    assert!(g(&w.project, &["read-tree", "-m", "-u", &old, &target], &envs).is_err());
    assert_eq!(
        std::fs::read_to_string(w.project.join("made.txt")).unwrap(),
        "mine\n"
    );
}

#[test]
fn an_ignored_file_the_cloud_adds_as_tracked_is_never_overwritten() {
    let w = world(true);
    git(&w.ws, &["add", "-f", "-N", "."]);
    std::fs::write(w.ws.join("secret.cfg"), "from the cloud\n").unwrap();
    git(&w.ws, &["add", "secret.cfg"]);
    git(
        &w.ws,
        &["commit", "-q", "-m", "add secret.cfg", "secret.cfg"],
    );
    let local = moved(&w);
    std::fs::write(w.project.join(".git/info/exclude"), "secret.cfg\n").unwrap();
    std::fs::write(w.project.join("secret.cfg"), "my secret\n").unwrap();
    run(&w, local).unwrap();
    assert_eq!(
        std::fs::read_to_string(w.project.join("secret.cfg")).unwrap(),
        "my secret\n"
    );
    assert_eq!(git(&w.project, &["log", "-1", "--format=%s"]), "base");
    assert!(w
        .svc
        .store()
        .unwrap()
        .agent_settings(local)
        .unwrap()
        .unwrap()
        .worktree_cwd
        .is_some());
}

#[test]
fn hidden_edits_of_assume_unchanged_files_force_the_worktree() {
    let w = world(true);
    git(&w.project, &["update-index", "--assume-unchanged", "a.txt"]);
    std::fs::write(w.project.join("a.txt"), "hidden edit\n").unwrap();
    let local = moved(&w);
    run(&w, local).unwrap();
    assert_eq!(
        std::fs::read_to_string(w.project.join("a.txt")).unwrap(),
        "hidden edit\n"
    );
    assert_eq!(git(&w.project, &["log", "-1", "--format=%s"]), "base");
    assert!(w
        .svc
        .store()
        .unwrap()
        .agent_settings(local)
        .unwrap()
        .unwrap()
        .worktree_cwd
        .is_some());
}

#[test]
fn a_detached_head_another_branch_or_a_merge_in_progress_refuse_in_place() {
    // Detached HEAD at the move.
    let w = world(true);
    git(&w.project, &["checkout", "-q", "--detach"]);
    let local = moved(&w);
    run(&w, local).unwrap();
    assert!(w
        .svc
        .store()
        .unwrap()
        .agent_settings(local)
        .unwrap()
        .unwrap()
        .worktree_cwd
        .is_some());
    // Another branch checked out since the move.
    let w = world(true);
    let local = moved(&w);
    git(&w.project, &["checkout", "-q", "-b", "other"]);
    run(&w, local).unwrap();
    assert!(w
        .svc
        .store()
        .unwrap()
        .agent_settings(local)
        .unwrap()
        .unwrap()
        .worktree_cwd
        .is_some());
    assert_eq!(
        git(&w.project, &["symbolic-ref", "--short", "HEAD"]),
        "other"
    );
    // A merge in progress.
    let w = world(true);
    let local = moved(&w);
    let head = hash_of(&w.project, "HEAD");
    std::fs::write(w.project.join(".git/MERGE_HEAD"), format!("{head}\n")).unwrap();
    run(&w, local).unwrap();
    assert!(w
        .svc
        .store()
        .unwrap()
        .agent_settings(local)
        .unwrap()
        .unwrap()
        .worktree_cwd
        .is_some());
    assert_eq!(git(&w.project, &["log", "-1", "--format=%s"]), "base");
}

#[test]
fn a_refused_branch_move_rolls_back_files_and_keeps_the_staged_state() {
    let w = world(true);
    // A staged edit and an unstaged one at the move.
    std::fs::write(w.project.join("a.txt"), "staged edit\n").unwrap();
    git(&w.project, &["add", "a.txt"]);
    std::fs::write(w.project.join("bin.dat"), [1u8, 1, 1]).unwrap();
    let local = moved(&w);
    // A hook that refuses the first update of the checked out branch.
    let hook = w.project.join(".git/hooks/reference-transaction");
    std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
    let flag = w.project.join(".git/fail-once");
    std::fs::write(&flag, "1").unwrap();
    std::fs::write(
        &hook,
        format!(
            "#!/bin/sh\nif [ \"$1\" = prepared ] && [ -f '{f}' ] && grep -q refs/heads/main; then rm -f '{f}'; exit 1; fi\nexit 0\n",
            f = flag.display()
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    let done = run(&w, local).unwrap();
    assert!(
        done.warnings.iter().any(|n| n.contains("worktree")),
        "{:?}",
        done.warnings
    );
    assert_eq!(
        std::fs::read_to_string(w.project.join("a.txt")).unwrap(),
        "staged edit\n"
    );
    assert_eq!(
        std::fs::read(w.project.join("bin.dat")).unwrap(),
        [1u8, 1, 1]
    );
    assert_eq!(git(&w.project, &["log", "-1", "--format=%s"]), "base");
    // The staged part is still staged, the unstaged part still unstaged.
    assert_eq!(
        git(&w.project, &["diff", "--cached", "--name-only"]),
        "a.txt"
    );
    assert_eq!(git(&w.project, &["diff", "--name-only"]), "bin.dat");
    assert!(w
        .svc
        .store()
        .unwrap()
        .cloud_continue_broken(local)
        .is_none());
}

#[test]
fn a_submodule_edit_with_recurse_on_is_never_carried_away() {
    let w = world(true);
    let sub = w.root.join("subrepo");
    std::fs::create_dir_all(&sub).unwrap();
    git(&sub, &["init", "-q", "-b", "main"]);
    std::fs::write(sub.join("f.txt"), "f\n").unwrap();
    git(&sub, &["add", "."]);
    git(&sub, &["commit", "-q", "-m", "sub base"]);
    git(
        &w.project,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            sub.to_str().unwrap(),
            "lib",
        ],
    );
    git(&w.project, &["commit", "-q", "-m", "add lib"]);
    git(&w.project, &["push", "-q", "origin", "main"]);
    git(&w.ws, &["fetch", "-q", "origin"]);
    git(&w.ws, &["merge", "-q", "--no-edit", "origin/main"]);
    git(&w.project, &["config", "submodule.recurse", "true"]);
    std::fs::write(w.project.join("lib/f.txt"), "edited inside the submodule\n").unwrap();
    let local = moved(&w);
    run(&w, local).unwrap();
    assert_eq!(
        std::fs::read_to_string(w.project.join("lib/f.txt")).unwrap(),
        "edited inside the submodule\n"
    );
    assert!(w
        .svc
        .store()
        .unwrap()
        .agent_settings(local)
        .unwrap()
        .unwrap()
        .worktree_cwd
        .is_some());
}
