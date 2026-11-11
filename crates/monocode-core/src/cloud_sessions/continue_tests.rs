//! "Continue locally" against the same real-git fixtures the archive tests use.
use super::convert_tests::{git, world, World, SESSION};
use super::*;
use crate::store::{CloudLocation, MovePoint, SessionLocation};
use std::path::{Path, PathBuf};

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
        tree: tree_of(&w.project),
        work_cwd: w.project.to_str().unwrap().into(),
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
