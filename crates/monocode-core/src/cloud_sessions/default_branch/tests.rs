//! Temp git repositories only: an origin, a local clone and a second clone that pushes.
use super::*;
use std::path::PathBuf;
use std::process::Command;

fn git_in(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

struct Repos {
    _tmp: tempfile::TempDir,
    /// The user's checkout.
    local: PathBuf,
    /// Another clone, used to move origin forward.
    other: PathBuf,
}

fn commit(dir: &Path, file: &str, text: &str) {
    std::fs::write(dir.join(file), text).unwrap();
    git_in(dir, &["add", file]);
    git_in(dir, &["commit", "-q", "-m", text]);
}

fn repos() -> Repos {
    let tmp = tempfile::tempdir().unwrap();
    let origin = tmp.path().join("origin.git");
    std::fs::create_dir(&origin).unwrap();
    git_in(&origin, &["init", "-q", "--bare", "-b", "main"]);
    let local = tmp.path().join("local");
    let other = tmp.path().join("other");
    git_in(
        tmp.path(),
        &["clone", "-q", origin.to_str().unwrap(), "local"],
    );
    commit(&local, "a.txt", "one");
    git_in(&local, &["push", "-q", "origin", "main"]);
    git_in(
        tmp.path(),
        &["clone", "-q", origin.to_str().unwrap(), "other"],
    );
    Repos {
        _tmp: tmp,
        local,
        other,
    }
}

/// Origin moves forward by one commit that `local` has not seen.
fn advance_origin(r: &Repos) {
    commit(&r.other, "b.txt", "two");
    git_in(&r.other, &["push", "-q", "origin", "main"]);
}

fn head(dir: &Path, name: &str) -> String {
    git_in(dir, &["rev-parse", name])
}

#[test]
fn a_clean_checkout_that_is_behind_is_fast_forwarded() {
    let r = repos();
    advance_origin(&r);
    let note = refresh_default_branch(&r.local).unwrap();
    assert!(note.contains("fast-forwarded"), "{note}");
    assert_eq!(head(&r.local, "main"), head(&r.other, "main"));
    assert_eq!(
        std::fs::read_to_string(r.local.join("b.txt")).unwrap(),
        "two"
    );
}

#[test]
fn a_current_branch_says_nothing() {
    let r = repos();
    assert_eq!(refresh_default_branch(&r.local), None);
}

#[test]
fn a_dirty_checkout_is_never_touched_and_the_branch_stays_put() {
    let r = repos();
    advance_origin(&r);
    let before = head(&r.local, "main");
    std::fs::write(r.local.join("a.txt"), "my uncommitted work").unwrap();
    let note = refresh_default_branch(&r.local).unwrap();
    assert!(note.contains("was not updated"), "{note}");
    assert_eq!(head(&r.local, "main"), before);
    assert_eq!(
        std::fs::read_to_string(r.local.join("a.txt")).unwrap(),
        "my uncommitted work"
    );
}

#[test]
fn a_diverged_branch_is_left_untouched_with_a_note_and_no_question() {
    let r = repos();
    advance_origin(&r);
    commit(&r.local, "c.txt", "local only");
    let before = head(&r.local, "main");
    let note = refresh_default_branch(&r.local).unwrap();
    assert!(note.contains("diverged"), "{note}");
    assert_eq!(head(&r.local, "main"), before);
    assert!(r.local.join("c.txt").is_file());
    assert!(!r.local.join("b.txt").exists());
}

#[test]
fn local_commits_ahead_of_origin_are_kept() {
    let r = repos();
    commit(&r.local, "c.txt", "local only");
    let before = head(&r.local, "main");
    let note = refresh_default_branch(&r.local).unwrap();
    assert!(note.contains("has commits that origin"), "{note}");
    assert_eq!(head(&r.local, "main"), before);
}

#[test]
fn a_branch_that_is_not_checked_out_is_fast_forwarded_without_moving_the_checkout() {
    let r = repos();
    git_in(&r.local, &["checkout", "-q", "-b", "feature"]);
    advance_origin(&r);
    let note = refresh_default_branch(&r.local).unwrap();
    assert!(note.contains("fast-forwarded"), "{note}");
    assert_eq!(head(&r.local, "main"), head(&r.other, "main"));
    assert_eq!(
        git_in(&r.local, &["symbolic-ref", "--short", "HEAD"]),
        "feature"
    );
}

#[test]
fn no_origin_or_no_git_is_silent() {
    let tmp = tempfile::tempdir().unwrap();
    assert_eq!(refresh_default_branch(tmp.path()), None);
    git_in(tmp.path(), &["init", "-q"]);
    assert_eq!(refresh_default_branch(tmp.path()), None);
}
