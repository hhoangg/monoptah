use super::move_ui::*;
use std::{path::Path, process::Command};

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ALLOW_PROTOCOL", "file")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn commit_file(dir: &Path, name: &str, text: &str) {
    std::fs::write(dir.join(name), text).unwrap();
    git(dir, &["add", name]);
    git(dir, &["commit", "-q", "-m", name]);
}

/// A bare origin with `main` and a clone tracking it.
fn fixture() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = super::plain_path(tmp.path().canonicalize().unwrap());
    let origin = root.join("origin.git");
    let work = root.join("work");
    std::fs::create_dir_all(&origin).unwrap();
    git(&origin, &["init", "-q", "--bare", "-b", "main"]);
    std::fs::create_dir_all(&work).unwrap();
    git(&work, &["init", "-q", "-b", "main"]);
    for (k, v) in [
        ("user.email", "t@example.com"),
        ("user.name", "Test"),
        ("commit.gpgsign", "false"),
        ("core.autocrlf", "false"),
    ] {
        git(&work, &["config", k, v]);
    }
    git(
        &work,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    commit_file(&work, "README.md", "hello\n");
    git(&work, &["push", "-q", "-u", "origin", "main"]);
    git(&work, &["remote", "set-head", "origin", "main"]);
    (tmp, work)
}

#[test]
fn the_preview_lists_the_push_target_the_counts_and_the_ignored_files() {
    let (_tmp, work) = fixture();
    git(&work, &["checkout", "-q", "-b", "feature"]);
    commit_file(&work, "a.txt", "a\n");
    commit_file(&work, ".gitignore", ".env.local\n");
    commit_file(&work, "b.txt", "b\n");
    std::fs::write(work.join("README.md"), "changed\n").unwrap();
    std::fs::write(work.join("new.txt"), "new\n").unwrap();
    std::fs::write(work.join(".env.local"), "SECRET=1\n").unwrap();

    let p = preview(&work, &std::sync::atomic::AtomicBool::new(false)).unwrap();
    assert_eq!(p.branch, "feature");
    assert_eq!(p.local_branch, "feature");
    assert!(p.needs_push && p.creates_branch);
    assert_eq!(p.commits, 3);
    assert_eq!((p.changed, p.added), (1, 1));
    assert!(p.patch_bytes > 0);
    assert_eq!(p.ignored.len(), 1);
    assert_eq!(p.ignored[0].0, ".env.local");
    assert_eq!(p.ignored_bytes, 9);
    assert!(!p.lfs && p.dirty_submodules.is_empty());
    // The preview wrote nothing into the repository.
    let status = Command::new("git")
        .arg("-C")
        .arg(&work)
        .args(["status", "--porcelain"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&status.stdout).into_owned();
    assert!(text.contains("README.md") && text.contains("new.txt"));
}

#[test]
fn a_branch_that_origin_already_has_means_nothing_to_push() {
    let (_tmp, work) = fixture();
    let p = preview(&work, &std::sync::atomic::AtomicBool::new(false)).unwrap();
    assert!(!p.needs_push);
    assert_eq!(p.commits, 0);
    assert_eq!(p.branch, "main");
}

#[test]
fn the_default_branch_is_never_the_target() {
    let (_tmp, work) = fixture();
    commit_file(&work, "a.txt", "a\n");
    let p = preview(&work, &std::sync::atomic::AtomicBool::new(false)).unwrap();
    assert_eq!(p.branch, "main-cloud");
    assert!(p.creates_branch && p.needs_push);
    assert_eq!(p.commits, 1);
}

#[test]
fn the_mark_names_the_target_and_reads_back() {
    let mark = MoveMark {
        at_ms: 5,
        backend: super::backend::CloudBackendKind::Codespaces,
        target: "fluffy-guacamole".into(),
    };
    assert_eq!(mark.target_label(), "codespace fluffy-guacamole");
    let json = serde_json::to_string(&mark).unwrap();
    assert_eq!(serde_json::from_str::<MoveMark>(&json).unwrap(), mark);
}

#[test]
fn a_cancelled_preview_stops_and_the_project_is_inspected_one_at_a_time() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let (_tmp, work) = fixture();
    // Already cancelled: it returns at once without running git.
    let cancel = AtomicBool::new(true);
    assert_eq!(preview(&work, &cancel).unwrap_err(), "Cancelled");
    // Two inspections of one folder never overlap.
    let running = std::sync::atomic::AtomicUsize::new(0);
    let max = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|s| {
        for _ in 0..4 {
            s.spawn(|| {
                let _g = super::move_ui::project_guard(&work);
                let now = running.fetch_add(1, Ordering::SeqCst) + 1;
                max.fetch_max(now, Ordering::SeqCst);
                std::thread::sleep(std::time::Duration::from_millis(20));
                running.fetch_sub(1, Ordering::SeqCst);
            });
        }
    });
    assert_eq!(max.load(Ordering::SeqCst), 1);
}

#[test]
fn deleting_a_session_removes_its_move_mark() {
    let mut store = crate::store::Store::open(Path::new(":memory:")).unwrap();
    let session = store.create("claude", "/work/x").unwrap();
    let mark = MoveMark {
        at_ms: 1,
        backend: super::backend::CloudBackendKind::Codespaces,
        target: "cs".into(),
    };
    super::move_ui::save_mark_for_test(&store, session.id, &mark);
    assert!(move_mark(&store, session.id).is_some());
    store.delete(session.id).unwrap();
    assert!(move_mark(&store, session.id).is_none());
}
