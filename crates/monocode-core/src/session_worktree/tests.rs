use super::*;
use crate::store::ArchiveTimes;

struct Fixture {
    _dir: tempfile::TempDir,
    repo: PathBuf,
    store: Store,
}

fn run(dir: &Path, args: &[&str]) -> String {
    git(dir, args).unwrap_or_else(|e| panic!("git {args:?}: {e}"))
}

impl Fixture {
    /// A repository with one pushed commit, so only later work counts as unpushed.
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let repo = root.join("repo");
        let remote = root.join("remote.git");
        std::fs::create_dir_all(&repo).unwrap();
        run(&root, &["init", "--bare", "-b", "main", "remote.git"]);
        run(&repo, &["init", "-b", "main"]);
        run(&repo, &["config", "user.email", "fixture@example.invalid"]);
        run(&repo, &["config", "user.name", "Fixture"]);
        std::fs::write(repo.join("file"), "before").unwrap();
        std::fs::write(repo.join(".gitignore"), "target/\nnode_modules/\n").unwrap();
        run(&repo, &["add", "."]);
        run(&repo, &["commit", "-m", "Initial"]);
        run(
            &repo,
            &["remote", "add", "origin", &remote.to_string_lossy()],
        );
        run(&repo, &["push", "origin", "main"]);
        Self {
            _dir: dir,
            repo,
            store: Store::open(Path::new(":memory:")).unwrap(),
        }
    }
    /// A session with a worktree made the way a new session makes it.
    fn session(&self) -> (i64, PathBuf) {
        let session = self
            .store
            .create("codex", &self.repo.to_string_lossy())
            .unwrap();
        let path = crate::git::create_worktree(&self.repo, "HEAD", session.id).unwrap();
        self.attach(session.id, &path);
        (session.id, path.canonicalize().unwrap())
    }
    fn attach(&self, id: i64, path: &Path) {
        let settings = crate::agent::Settings {
            worktree_cwd: Some(path.to_string_lossy().into_owned()),
            ..Default::default()
        };
        self.store.save_agent_settings(id, &settings).unwrap();
    }
}

fn commit(path: &Path, name: &str) {
    std::fs::write(path.join(name), name).unwrap();
    run(path, &["add", name]);
    run(path, &["commit", "-m", name]);
}

#[test]
fn only_a_linked_checkout_is_reported_as_a_worktree() {
    let f = Fixture::new();
    let (_, path) = f.session();
    let repo = f.repo.to_string_lossy().into_owned();
    let tree = path.to_string_lossy().into_owned();
    assert_eq!(recorded_worktree(&repo, &repo), None);
    assert_eq!(recorded_worktree(&repo, ""), None);
    assert_eq!(recorded_worktree(&repo, &tree), Some(tree));
    // The main checkout has a `.git` folder, not a file.
    assert_eq!(recorded_worktree("/elsewhere", &repo), None);
}

#[test]
fn dirty_counts_untracked_files_but_not_ignored_ones() {
    let f = Fixture::new();
    let (_, path) = f.session();
    assert_eq!(inspect(&path), (Some(false), Some(false)));
    std::fs::create_dir_all(path.join("target")).unwrap();
    std::fs::write(path.join("target/build"), "ignored").unwrap();
    assert_eq!(inspect(&path).0, Some(false), "ignored output is not work");
    std::fs::write(path.join("new.txt"), "untracked").unwrap();
    assert_eq!(inspect(&path).0, Some(true));
    std::fs::remove_file(path.join("new.txt")).unwrap();
    std::fs::write(path.join("file"), "edited").unwrap();
    assert_eq!(inspect(&path).0, Some(true));
}

#[test]
fn unpushed_counts_commits_no_remote_branch_has() {
    let f = Fixture::new();
    let (_, path) = f.session();
    commit(&path, "work.txt");
    assert_eq!(inspect(&path), (Some(false), Some(true)));
    run(&path, &["push", "origin", "HEAD:refs/heads/pushed"]);
    assert_eq!(inspect(&path), (Some(false), Some(false)));
}

#[test]
fn a_missing_path_is_not_computed() {
    let f = Fixture::new();
    assert_eq!(inspect(&f.repo.join("gone")), (None, None));
}

#[test]
fn removal_clears_ignored_build_output_and_drops_an_empty_branch() {
    let f = Fixture::new();
    let (id, path) = f.session();
    std::fs::create_dir_all(path.join("target/debug")).unwrap();
    std::fs::write(path.join("target/debug/big"), "x").unwrap();
    std::fs::create_dir_all(path.join("node_modules/a")).unwrap();
    std::fs::write(path.join("node_modules/a/i.js"), "x").unwrap();
    let branch = run(&path, &["symbolic-ref", "--short", "HEAD"])
        .trim()
        .to_owned();
    let plan = plan(&f.store, id).unwrap();
    let outcome = execute(&plan);
    assert!(outcome.worktree_removed, "{outcome:?}");
    assert!(!path.exists(), "the folder is gone with its ignored output");
    assert_eq!(outcome.branch.as_deref(), Some(branch.as_str()));
    assert!(outcome.branch_deleted, "{outcome:?}");
    assert!(run(&f.repo, &["branch", "--list", &branch])
        .trim()
        .is_empty());
    assert!(!run(&f.repo, &["worktree", "list"]).contains("mc-native"));
}

#[test]
fn a_session_attached_while_deleting_keeps_the_worktree() {
    let mut f = Fixture::new();
    let (id, path) = f.session();
    let planned = plan(&f.store, id).unwrap();
    f.store.delete(id).unwrap();
    let other = f.store.create("codex", &f.repo.to_string_lossy()).unwrap();
    f.attach(other.id, &path);
    let outcome = execute_if_unused(&f.store, &planned);
    assert!(!outcome.worktree_removed, "{outcome:?}");
    assert!(path.join("file").exists());
    f.store.delete(other.id).unwrap();
    assert!(execute_if_unused(&f.store, &planned).worktree_removed);
}

#[test]
fn a_worktree_locked_after_planning_is_kept() {
    let f = Fixture::new();
    let (id, path) = f.session();
    let planned = plan(&f.store, id).unwrap();
    run(&f.repo, &["worktree", "lock", &path.to_string_lossy()]);
    let outcome = execute(&planned);
    assert!(!outcome.worktree_removed, "{outcome:?}");
    assert!(path.join("file").exists());
}

#[test]
fn a_worktree_switched_after_planning_is_kept() {
    let f = Fixture::new();
    let (id, path) = f.session();
    let planned = plan(&f.store, id).unwrap();
    run(&path, &["checkout", "-b", "user-branch"]);
    let outcome = execute(&planned);
    assert!(!outcome.worktree_removed, "{outcome:?}");
    assert!(path.join("file").exists());
}

#[test]
fn a_branch_with_commits_of_its_own_is_kept() {
    let f = Fixture::new();
    let (id, path) = f.session();
    let branch = run(&path, &["symbolic-ref", "--short", "HEAD"])
        .trim()
        .to_owned();
    let planned = plan(&f.store, id).unwrap();
    // Work lands after the plan, as it could between the check and the delete.
    commit(&path, "late.txt");
    let kept = execute(&planned);
    assert!(
        !kept.worktree_removed,
        "unpushed work keeps the worktree: {kept:?}"
    );
    assert!(path.exists());
    // Pushed to a differently named branch it is no longer unpushed, nor unique.
    run(&path, &["push", "origin", "HEAD:refs/heads/elsewhere"]);
    let removed = execute(&planned);
    assert!(removed.worktree_removed, "{removed:?}");
    assert!(removed.branch_deleted, "{removed:?}");
    assert!(run(&f.repo, &["branch", "--list", &branch])
        .trim()
        .is_empty());
}

#[test]
fn unique_commits_keep_the_branch() {
    let f = Fixture::new();
    let (_, path) = f.session();
    let branch = run(&path, &["symbolic-ref", "--short", "HEAD"])
        .trim()
        .to_owned();
    commit(&path, "mine.txt");
    let planned = Plan {
        path: path.clone(),
        main: f.repo.clone(),
        branch: Some(branch.clone()),
        base: None,
    };
    let (deleted, reason) = drop_branch(&planned);
    assert!(!deleted);
    assert!(reason.contains("1 commit"), "{reason}");
    assert!(!run(&f.repo, &["branch", "--list", &branch])
        .trim()
        .is_empty());
    // Another branch holding the commit makes it not unique.
    run(&f.repo, &["branch", "keeper", &branch]);
    run(&path, &["checkout", "--detach"]);
    let (deleted, _) = drop_branch(&planned);
    assert!(deleted);
}

#[test]
fn a_branch_merged_into_the_base_is_dropped_but_other_names_stay() {
    let f = Fixture::new();
    let (_, path) = f.session();
    let branch = run(&path, &["symbolic-ref", "--short", "HEAD"])
        .trim()
        .to_owned();
    commit(&path, "x.txt");
    run(&f.repo, &["branch", "side", &branch]);
    run(&path, &["checkout", "--detach"]);
    run(&f.repo, &["branch", "-D", "side"]);
    let mut planned = Plan {
        path: path.clone(),
        main: f.repo.clone(),
        branch: Some(branch.clone()),
        base: Some(branch.clone()),
    };
    assert!(drop_branch(&planned).0, "its own tip is the base");
    run(&f.repo, &["branch", &branch, "HEAD"]);
    planned.branch = Some("feature/user".into());
    let (deleted, reason) = drop_branch(&planned);
    assert!(!deleted);
    assert!(reason.contains("not made for this session"), "{reason}");
}

#[test]
fn refusals_leave_everything_in_place() {
    let f = Fixture::new();
    // No worktree recorded.
    let plain = f.store.create("codex", &f.repo.to_string_lossy()).unwrap();
    assert!(plan(&f.store, plain.id)
        .unwrap_err()
        .contains("no worktree"));
    // The main checkout recorded as the worktree.
    f.attach(plain.id, &f.repo);
    assert!(plan(&f.store, plain.id)
        .unwrap_err()
        .contains("main checkout"));
    // A worktree the user made elsewhere.
    let outside = f.repo.parent().unwrap().join("mine");
    run(
        &f.repo,
        &["worktree", "add", "-b", "mine", &outside.to_string_lossy()],
    );
    f.attach(plain.id, &outside);
    assert!(plan(&f.store, plain.id)
        .unwrap_err()
        .contains("not made by the app"));
    // Dirty.
    let (id, path) = f.session();
    std::fs::write(path.join("wip"), "x").unwrap();
    assert!(plan(&f.store, id).unwrap_err().contains("uncommitted"));
    std::fs::remove_file(path.join("wip")).unwrap();
    // Unpushed.
    commit(&path, "local.txt");
    assert!(plan(&f.store, id).unwrap_err().contains("remote branch"));
    run(&path, &["push", "origin", "HEAD:refs/heads/safe"]);
    // Shared with another session, hidden ones included.
    let other = f.store.create("codex", &f.repo.to_string_lossy()).unwrap();
    f.attach(other.id, &path);
    assert!(plan(&f.store, id).unwrap_err().contains("Another session"));
    f.store
        .save_agent_settings(other.id, &Default::default())
        .unwrap();
    assert!(plan(&f.store, id).is_ok());
    assert!(path.exists());
}

#[test]
fn a_locked_worktree_is_refused() {
    let f = Fixture::new();
    let (id, path) = f.session();
    run(
        &f.repo,
        &["worktree", "lock", "--", &path.to_string_lossy()],
    );
    assert!(plan(&f.store, id).unwrap_err().contains("locked"));
}

#[test]
fn archive_times_follow_the_archive_flag() {
    let f = Fixture::new();
    let session = f.store.create("codex", "/p").unwrap();
    let find = || {
        f.store
            .sessions()
            .unwrap()
            .into_iter()
            .find(|s| s.id == session.id)
            .unwrap()
    };
    let live = find();
    let times = f.store.archive_times(session.id);
    assert_eq!(times.archived_at(&live), None);
    assert_eq!(times.last_activity_at(&live), Some(live.updated_at));
    f.store.set_archived(session.id, true).unwrap();
    let archived = find();
    let times = f.store.archive_times(session.id);
    assert!(times.archived_at(&archived).unwrap() >= live.updated_at);
    assert_eq!(
        times.last_activity_at(&archived),
        Some(live.updated_at),
        "archiving is not activity"
    );
    f.store.set_archived(session.id, false).unwrap();
    let back = find();
    let times = f.store.archive_times(session.id);
    assert_eq!(times.archived_at(&back), None);
    assert!(times.unarchived_at.is_some());
    assert!(times.last_activity_at(&back).unwrap() >= live.updated_at);
}

#[test]
fn legacy_archived_sessions_fall_back_to_their_last_update() {
    let f = Fixture::new();
    let mut session = f.store.create("codex", "/p").unwrap();
    session.archived = true;
    session.updated_at = 1_700_000_000_000;
    let none: ArchiveTimes = serde_json::from_str("{}").unwrap();
    assert_eq!(none, ArchiveTimes::default());
    assert_eq!(none.archived_at(&session), Some(1_700_000_000_000));
    session.updated_at = 0;
    assert_eq!(none.archived_at(&session), None);
    assert_eq!(none.last_activity_at(&session), None);
    let old: ArchiveTimes = serde_json::from_str(r#"{"archivedAt":5,"unknown":1}"#).unwrap();
    assert_eq!(old.archived_at(&session), Some(5));
    session.updated_at = 9;
    let unarchived = ArchiveTimes {
        unarchived_at: Some(20),
        ..Default::default()
    };
    assert_eq!(unarchived.last_activity_at(&session), Some(20));
}

#[test]
fn session_facts_map_the_api_fields_and_only_read_asks_git() {
    let f = Fixture::new();
    let (id, path) = f.session();
    std::fs::write(path.join("wip"), "x").unwrap();
    let main = f.store.create("codex", &f.repo.to_string_lossy()).unwrap();
    let sessions = f.store.sessions().unwrap();
    let refs: Vec<_> = sessions.iter().collect();
    let list = f.store.app_session_facts(&refs, false).unwrap();
    assert_eq!(list[&id]["worktreePath"], path.to_string_lossy().as_ref());
    assert!(list[&id]["worktreeDirty"].is_null());
    assert!(list[&id]["worktreeUnpushed"].is_null());
    assert!(list[&id]["archivedAt"].is_null());
    assert!(list[&id]["lastActivityAt"].as_u64().unwrap() > 0);
    assert!(list[&main.id]["worktreePath"].is_null());
    let read = f.store.app_session_facts(&refs, true).unwrap();
    assert_eq!(read[&id]["worktreeDirty"], true);
    assert_eq!(read[&id]["worktreeUnpushed"], false);
    assert!(read[&main.id]["worktreeDirty"].is_null());
    let page = f
        .store
        .app_conversation(id, &serde_json::json!({}), false, false)
        .unwrap();
    assert_eq!(page["worktreePath"], path.to_string_lossy().as_ref());
    assert_eq!(page["worktreeDirty"], true);
    std::fs::remove_dir_all(&path).unwrap();
    let gone = f.store.app_session_facts(&refs, true).unwrap();
    assert_eq!(gone[&id]["worktreePath"], path.to_string_lossy().as_ref());
    assert!(gone[&id]["worktreeDirty"].is_null());
}
