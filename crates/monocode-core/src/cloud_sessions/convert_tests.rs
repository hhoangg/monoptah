//! The "codespace" in these tests is a local git repository: the fake `gh` runs the real
//! snapshot script with `sh` and copies files with `cp`, so the git behaviour is real.
use super::*;
use crate::codespaces::GhOutput;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;

const SESSION: &str = "5b0a3c1e-1111-4222-8333-444455556666";

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Runs scripts and copies against the local disk instead of a codespace.
struct LocalGh {
    home: PathBuf,
    calls: Mutex<Vec<String>>,
    fail_snapshot: Mutex<Option<String>>,
}
impl LocalGh {
    fn new(home: &Path) -> Arc<Self> {
        Arc::new(Self {
            home: home.into(),
            calls: Mutex::new(vec![]),
            fail_snapshot: Mutex::new(None),
        })
    }
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}
fn out(success: bool, stdout: &str, stderr: &str) -> Result<GhOutput, String> {
    Ok(GhOutput {
        success,
        stdout: stdout.into(),
        stderr: stderr.into(),
    })
}
impl GhRunner for LocalGh {
    fn run(&self, args: &[String], stdin: Option<&str>) -> Result<GhOutput, String> {
        self.calls.lock().unwrap().push(args.join(" "));
        match (args[1].as_str(), args.contains(&"-e".to_string())) {
            ("ssh", _) => {
                let long = args.last().map(String::as_str) == Some(codespaces::LONG_SCRIPT_MARKER);
                if long {
                    if let Some(error) = self.fail_snapshot.lock().unwrap().clone() {
                        return out(false, "", &error);
                    }
                }
                let mut child = Command::new("sh")
                    .arg("-s")
                    .env("HOME", &self.home)
                    .env_remove("CLAUDE_CONFIG_DIR")
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .unwrap();
                use std::io::Write;
                child
                    .stdin
                    .take()
                    .unwrap()
                    .write_all(stdin.unwrap().as_bytes())
                    .unwrap();
                let result = child.wait_with_output().unwrap();
                out(
                    result.status.success(),
                    &String::from_utf8_lossy(&result.stdout),
                    &String::from_utf8_lossy(&result.stderr),
                )
            }
            ("cp", true) => {
                let remote = args.iter().find_map(|a| a.strip_prefix("remote:")).unwrap();
                let local = args.last().unwrap();
                if !Path::new(remote).exists() {
                    return out(
                        false,
                        "",
                        &format!("scp: '{remote}': No such file or directory"),
                    );
                }
                let status = Command::new("cp")
                    .args(["-R", remote, local])
                    .status()
                    .unwrap();
                out(status.success(), "", "")
            }
            other => panic!("unexpected gh call {other:?}: {args:?}"),
        }
    }
}

struct SyncHost {
    machines: Vec<Machine>,
    snapshot: Value,
    log: Mutex<Vec<String>>,
}
impl Host for SyncHost {
    fn machines(&self) -> Result<Vec<Machine>, String> {
        Ok(self.machines.clone())
    }
    fn connect(&self, _: &str, _: &str, _: &mut dyn FnMut(&str)) -> Result<Machine, String> {
        Err("unused".into())
    }
    fn ensure(&self, id: &str, _: &AtomicBool, _: &mut dyn FnMut(&str)) -> Result<(), String> {
        self.log.lock().unwrap().push(format!("ensure {id}"));
        Ok(())
    }
    fn request(&self, id: &str, method: &str, _: Value) -> Result<Value, String> {
        self.log.lock().unwrap().push(format!("{id} {method}"));
        match method {
            "sessions.sync" => Ok(json!({"kind":"snapshot","value":self.snapshot})),
            other => Err(format!("unexpected {other}")),
        }
    }
    fn disconnect(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
}

struct World {
    _dir: tempfile::TempDir,
    root: PathBuf,
    /// The "codespace" workspace.
    ws: PathBuf,
    /// The shared bare remote.
    origin: PathBuf,
    /// Claude config dir on this machine.
    claude: PathBuf,
    /// The local project folder (a clone of the origin).
    project: PathBuf,
    gh: Arc<LocalGh>,
    host: Arc<SyncHost>,
    svc: CloudSessions,
    cloud_id: i64,
}

fn world(ran_claude: bool) -> World {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let origin = root.join("origin.git");
    let ws = root.join("cs-ws");
    let project = root.join("local-project");
    let remote_home = root.join("cs-home");
    let claude = root.join("claude");
    std::fs::create_dir_all(&origin).unwrap();
    git(&origin, &["init", "--bare", "-q", "-b", "main"]);
    std::fs::create_dir_all(&ws).unwrap();
    git(&ws, &["init", "-q", "-b", "main"]);
    std::fs::write(ws.join("a.txt"), "one\n").unwrap();
    std::fs::write(ws.join("bin.dat"), [0u8, 1, 2, 255]).unwrap();
    git(&ws, &["add", "."]);
    git(&ws, &["commit", "-q", "-m", "base"]);
    git(&ws, &["remote", "add", "origin", origin.to_str().unwrap()]);
    git(&ws, &["push", "-q", "origin", "main"]);
    // Local project: a clone made before the codespace did any work.
    git(
        &root,
        &[
            "clone",
            "-q",
            origin.to_str().unwrap(),
            project.to_str().unwrap(),
        ],
    );
    // Work in the codespace: unpushed commit on a new branch, uncommitted edits, untracked.
    git(&ws, &["checkout", "-q", "-b", "feat/x"]);
    std::fs::write(ws.join("made.txt"), "created by claude\n").unwrap();
    git(&ws, &["add", "made.txt"]);
    git(&ws, &["commit", "-q", "-m", "claude commit"]);
    std::fs::write(ws.join("a.txt"), "one\ntwo\n").unwrap();
    std::fs::write(ws.join("bin.dat"), [9u8, 8, 7, 0]).unwrap();
    std::fs::write(ws.join("untracked.txt"), "new file\n").unwrap();
    std::fs::create_dir_all(ws.join(".evidence")).unwrap();
    std::fs::write(ws.join(".evidence/shot.txt"), "evidence\n").unwrap();
    std::fs::write(
        ws.join(".git/info/exclude"),
        ".evidence/\nCLAUDE.local.md\n",
    )
    .unwrap();
    let ws_text = ws.to_str().unwrap().to_string();
    if ran_claude {
        let enc = claude_project_dir_name(&ws_text);
        let dir = remote_home.join(".claude/projects").join(enc);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(format!("{SESSION}.jsonl")),
            format!(
                "{{\"type\":\"user\",\"cwd\":\"{ws_text}\",\"sessionId\":\"{SESSION}\"}}\n\
                 {{\"type\":\"assistant\",\"cwd\":\"{ws_text}/sub\",\"note\":\"{ws_text}-other\"}}\n"
            ),
        )
        .unwrap();
    }
    let snapshot = json!({"projectId":"p","revision":3,"status":"idle","updatedAt":50,
        "session":{"id":"host-1","harness":"claude","title":"Add made.txt",
            "providerSessionId": ran_claude.then_some(SESSION),
            "model":"claude:sonnet","runtimeMode":"auto","cwd":ws_text,
            "blocks":[{"id":"u","role":"user","text":"create made.txt"},
                      {"id":"a","role":"assistant","text":"Done."}]}});
    let machine = Machine {
        id: "m1".into(),
        name: "cs".into(),
        endpoint: "codespace://cs-1".into(),
        environment_id: "env-1".into(),
        ssh: None,
        codespace: Some("cs-1".into()),
    };
    let host = Arc::new(SyncHost {
        machines: vec![machine],
        snapshot: snapshot.clone(),
        log: Mutex::new(vec![]),
    });
    let gh = LocalGh::new(&remote_home);
    let store = Store::open(&root.join("native.db")).unwrap();
    let remote_project = store
        .remember_cloud_project(
            "env-1",
            &json!({"id":"p","cwd":ws_text}),
            project.to_str().unwrap(),
            "cs-1",
        )
        .unwrap();
    let summary = store
        .register_remote_summary(&remote_project, &snapshot)
        .unwrap();
    let svc = CloudSessions::new(gh.clone(), host.clone(), Arc::new(Mutex::new(store)))
        .with_claude_dir(claude.clone())
        .with_workspace_root(&root);
    World {
        _dir: dir,
        root,
        ws,
        origin,
        claude,
        project,
        gh,
        host,
        svc,
        cloud_id: summary.id,
    }
}

#[test]
fn project_folder_names_follow_claudes_rule_and_cwd_rewrites_stop_at_a_boundary() {
    assert_eq!(
        claude_project_dir_name("/Users/me/my.repo_x/feat y"),
        "-Users-me-my-repo-x-feat-y"
    );
    let text =
        r#"{"cwd":"/workspaces/app"} /workspaces/app-two /workspaces/app/src /workspaces/app."#;
    assert_eq!(
        rewrite_cwd(text, "/workspaces/app", "/Users/me/\"q\""),
        r#"{"cwd":"/Users/me/\"q\""} /workspaces/app-two /Users/me/\"q\"/src /Users/me/\"q\"."#
    );
}

#[test]
fn snapshot_output_is_parsed_and_the_stage_path_is_validated() {
    let snap = parse_snapshot("stage=/tmp/monoptah-archive.AbC123\njsonl=1\nhead=abc\nbranch=feat/x\nunpushed=2\nbundle=1\nnoise\n")
        .unwrap();
    assert_eq!(snap.stage, "/tmp/monoptah-archive.AbC123");
    assert!(snap.jsonl && snap.bundle && !snap.patch);
    assert_eq!(snap.branch.as_deref(), Some("feat/x"));
    assert_eq!(snap.unpushed, 2);
    assert!(parse_snapshot("jsonl=1").is_err());
    assert!(parse_snapshot("stage=/tmp/x y; rm -rf").is_err());
}

#[test]
fn converting_keeps_transcript_jsonl_evidence_and_the_code_state_locally() {
    let w = world(true);
    // The real working copy must not change: remember its index and status.
    let status_before = git(&w.ws, &["status", "--porcelain"]);
    let index_before = std::fs::read(w.ws.join(".git/index")).unwrap();
    let mut lines = vec![];
    let done = w
        .svc
        .convert_session(w.cloud_id, &mut |l| lines.push(l.to_string()))
        .unwrap();
    assert!(lines.iter().any(|l| l.contains("Waking")));
    assert_eq!(done.codespace, "cs-1");
    assert_eq!(done.title, "Add made.txt");
    assert!(done.rows >= 2);
    assert!(done.warnings.is_empty(), "{:?}", done.warnings);

    // Local archived session in the binding's local project.
    let store = w.svc.store().unwrap();
    let sessions = store.sessions().unwrap();
    let local = sessions.iter().find(|s| s.id == done.local_id).unwrap();
    assert!(local.archived);
    assert_eq!(local.cwd, w.project.to_str().unwrap());
    assert_eq!(local.provider, "claude");
    assert_eq!(local.provider_id.as_deref(), Some(SESSION));
    assert_eq!(local.title, "Add made.txt");
    let rows = store.load(done.local_id).unwrap().rows;
    assert!(rows.iter().any(|r| r.text == "create made.txt"));
    assert!(rows.iter().any(|r| r.text.contains("Done.")));
    let origin: Value =
        serde_json::from_str(&store.setting(&origin_key(done.local_id)).unwrap().unwrap()).unwrap();
    assert_eq!(origin["codespace"], "cs-1");
    assert_eq!(origin["remoteCwd"], w.ws.to_str().unwrap());
    // The cloud record is gone, the local copy replaces it.
    assert!(store.remote_session(w.cloud_id).unwrap().is_none());
    let thread = store.thread_dir(done.local_id).unwrap().unwrap();
    drop(store);
    assert!(w
        .svc
        .cloud_sessions_for(w.project.to_str().unwrap())
        .unwrap()
        .is_empty());

    // Claude's own transcript, with the path rewritten, where `--resume` looks.
    let local_dir = claude_project_dir_name(w.project.to_str().unwrap());
    let jsonl = w
        .claude
        .join("projects")
        .join(local_dir)
        .join(format!("{SESSION}.jsonl"));
    assert_eq!(done.resume, Resume::Installed(jsonl.clone()));
    let text = std::fs::read_to_string(&jsonl).unwrap();
    assert!(text.contains(&format!("\"cwd\":\"{}\"", w.project.display())));
    assert!(text.contains(&format!("{}/sub", w.project.display())));
    assert!(!text.contains(&format!("\"cwd\":\"{}\"", w.ws.display())));
    // A path that merely starts with the codespace path is not rewritten.
    assert!(text.contains(&format!("{}-other", w.ws.display())));

    // Evidence.
    assert_eq!(
        std::fs::read_to_string(thread.join("evidence/shot.txt")).unwrap(),
        "evidence\n"
    );
    assert_eq!(done.evidence.len(), 1);

    // Code state.
    let code = thread.join(CODESPACE_DIR);
    assert_eq!(done.code_dir.as_deref(), Some(code.as_path()));
    let manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(code.join(MANIFEST)).unwrap()).unwrap();
    assert_eq!(manifest["branch"], "feat/x");
    assert_eq!(manifest["head"], git(&w.ws, &["rev-parse", "HEAD"]));
    assert_eq!(manifest["base"], git(&w.ws, &["rev-parse", "main~0"]));
    assert_eq!(manifest["remoteUrl"], w.origin.to_str().unwrap());
    assert_eq!(manifest["unpushedCommits"], 1);
    assert_eq!(manifest["bundle"], BUNDLE);
    assert_eq!(manifest["patch"], PATCH);
    let patch = std::fs::read_to_string(code.join(PATCH)).unwrap();
    assert!(patch.contains("untracked.txt") && patch.contains("+two"));
    assert!(patch.contains("GIT binary patch"));
    assert!(!patch.contains(".evidence"));
    assert!(code.join(BUNDLE).is_file());

    // Nothing changed in the codespace's repository, and the stage is removed.
    assert_eq!(git(&w.ws, &["status", "--porcelain"]), status_before);
    assert_eq!(
        std::fs::read(w.ws.join(".git/index")).unwrap(),
        index_before
    );
    assert!(w
        .gh
        .calls()
        .iter()
        .any(|c| c.contains("sh -s -- monoptah-long")));
    assert!(w.gh.calls().last().unwrap().contains("ssh"));
    assert!(w.host.log.lock().unwrap()[0].starts_with("ensure m1"));
}

#[test]
fn restore_builds_a_new_worktree_with_the_commit_and_the_uncommitted_changes() {
    let w = world(true);
    let done = w.svc.convert_session(w.cloud_id, &mut |_| {}).unwrap();
    let thread = w
        .svc
        .store()
        .unwrap()
        .thread_dir(done.local_id)
        .unwrap()
        .unwrap();
    let main_head = git(&w.project, &["rev-parse", "HEAD"]);
    let manifest = pending_restore(&thread).unwrap();
    assert_eq!(manifest.branch.as_deref(), Some("feat/x"));

    let restored = restore_code(&w.project, &thread).unwrap();
    assert_eq!(restored.branch, "feat/x");
    assert!(restored.patch_applied && restored.conflicts.is_none());
    assert!(restored
        .worktree
        .starts_with(w.root.join("local-project-worktrees")));
    assert_eq!(
        git(&restored.worktree, &["rev-parse", "HEAD"]),
        git(&w.ws, &["rev-parse", "HEAD"])
    );
    assert_eq!(
        std::fs::read_to_string(restored.worktree.join("made.txt")).unwrap(),
        "created by claude\n"
    );
    assert_eq!(
        std::fs::read_to_string(restored.worktree.join("a.txt")).unwrap(),
        "one\ntwo\n"
    );
    assert_eq!(
        std::fs::read(restored.worktree.join("bin.dat")).unwrap(),
        vec![9u8, 8, 7, 0]
    );
    assert_eq!(
        std::fs::read_to_string(restored.worktree.join("untracked.txt")).unwrap(),
        "new file\n"
    );
    // The main checkout is untouched.
    assert_eq!(git(&w.project, &["rev-parse", "HEAD"]), main_head);
    assert_eq!(git(&w.project, &["status", "--porcelain"]), "");
    assert!(!w.project.join("made.txt").exists());
    // One-time action.
    assert!(pending_restore(&thread).is_none());
    assert!(restore_code(&w.project, &thread).is_err());
}

#[test]
fn restore_uses_another_branch_name_when_the_branch_exists() {
    let w = world(true);
    let done = w.svc.convert_session(w.cloud_id, &mut |_| {}).unwrap();
    let thread = w
        .svc
        .store()
        .unwrap()
        .thread_dir(done.local_id)
        .unwrap()
        .unwrap();
    git(&w.project, &["branch", "feat/x"]);
    let restored = restore_code(&w.project, &thread).unwrap();
    assert_eq!(restored.branch, "feat/x-codespace");
    // The user's own branch keeps pointing where it did.
    assert_eq!(
        git(&w.project, &["rev-parse", "feat/x"]),
        git(&w.project, &["rev-parse", "main"])
    );
    assert!(restored.worktree.join("made.txt").is_file());
}

#[test]
fn a_patch_that_does_not_apply_is_reported_and_the_worktree_stays_at_head() {
    let w = world(true);
    let done = w.svc.convert_session(w.cloud_id, &mut |_| {}).unwrap();
    let thread = w
        .svc
        .store()
        .unwrap()
        .thread_dir(done.local_id)
        .unwrap()
        .unwrap();
    std::fs::write(thread.join(CODESPACE_DIR).join(PATCH), "not a patch\n").unwrap();
    let restored = restore_code(&w.project, &thread).unwrap();
    assert!(!restored.patch_applied);
    assert!(restored.conflicts.is_some());
    assert!(restored.worktree.join("made.txt").is_file());
    assert_eq!(git(&restored.worktree, &["status", "--porcelain"]), "");
}

#[test]
fn a_failed_snapshot_creates_nothing_and_leaves_the_cloud_session_for_a_retry() {
    let w = world(true);
    *w.gh.fail_snapshot.lock().unwrap() = Some("ssh: connection reset".into());
    let before = w.svc.store().unwrap().sessions().unwrap().len();
    let error = w.svc.convert_session(w.cloud_id, &mut |_| {}).unwrap_err();
    assert!(error.contains("connection reset"), "{error}");
    let store = w.svc.store().unwrap();
    assert_eq!(store.sessions().unwrap().len(), before);
    assert!(store.remote_session(w.cloud_id).unwrap().is_some());
    drop(store);
    assert!(!w.claude.join("projects").exists());
    // The retry succeeds and converts exactly once.
    *w.gh.fail_snapshot.lock().unwrap() = None;
    w.svc.convert_session(w.cloud_id, &mut |_| {}).unwrap();
    assert!(w.svc.convert_session(w.cloud_id, &mut |_| {}).is_err());
    assert_eq!(w.svc.store().unwrap().sessions().unwrap().len(), before);
}

#[test]
fn an_existing_local_claude_file_is_never_overwritten() {
    let w = world(true);
    let dir = w
        .claude
        .join("projects")
        .join(claude_project_dir_name(w.project.to_str().unwrap()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join(format!("{SESSION}.jsonl"));
    std::fs::write(&file, "mine\n").unwrap();
    let done = w.svc.convert_session(w.cloud_id, &mut |_| {}).unwrap();
    assert_eq!(done.resume, Resume::AlreadyThere(file.clone()));
    assert_eq!(std::fs::read_to_string(file).unwrap(), "mine\n");
}

#[test]
fn a_session_that_never_ran_claude_is_kept_without_a_resume_id() {
    let w = world(false);
    let done = w.svc.convert_session(w.cloud_id, &mut |_| {}).unwrap();
    assert!(matches!(done.resume, Resume::Missing(_)));
    assert!(done.warnings.iter().any(|w| w.contains("fresh")));
    let store = w.svc.store().unwrap();
    let local = store
        .sessions()
        .unwrap()
        .into_iter()
        .find(|s| s.id == done.local_id)
        .unwrap();
    assert!(local.provider_id.is_none() && local.archived);
}

#[test]
fn a_missing_claude_file_resets_the_resume_id_and_warns() {
    let w = world(true);
    // The id is known to the host, but the file is not on the codespace.
    let enc = claude_project_dir_name(w.ws.to_str().unwrap());
    std::fs::remove_dir_all(w.root.join("cs-home/.claude/projects").join(enc)).unwrap();
    let done = w.svc.convert_session(w.cloud_id, &mut |_| {}).unwrap();
    assert!(matches!(done.resume, Resume::Missing(_)));
    let store = w.svc.store().unwrap();
    let local = store
        .sessions()
        .unwrap()
        .into_iter()
        .find(|s| s.id == done.local_id)
        .unwrap();
    assert!(local.provider_id.is_none());
}

#[test]
fn a_workspace_without_unpushed_commits_or_changes_has_no_bundle_and_no_patch() {
    let w = world(true);
    git(&w.ws, &["checkout", "-q", "main"]);
    git(&w.ws, &["checkout", "-q", "--", "."]);
    git(&w.ws, &["clean", "-fdq"]);
    let done = w.svc.convert_session(w.cloud_id, &mut |_| {}).unwrap();
    let code = done.code_dir.unwrap();
    assert!(!code.join(BUNDLE).exists() && !code.join(PATCH).exists());
    let thread = w
        .svc
        .store()
        .unwrap()
        .thread_dir(done.local_id)
        .unwrap()
        .unwrap();
    // Nothing unsaved, but a manifest still lets the user recreate the checkout.
    let restored = restore_code(&w.project, &thread).unwrap();
    assert_eq!(restored.branch, "main-codespace");
    assert!(!restored.patch_applied && restored.conflicts.is_none());
}

#[test]
fn a_workspace_that_is_not_a_git_repository_keeps_only_the_conversation() {
    let w = world(true);
    std::fs::remove_dir_all(w.ws.join(".git")).unwrap();
    let done = w.svc.convert_session(w.cloud_id, &mut |_| {}).unwrap();
    assert!(done.code_dir.is_none());
    let thread = w
        .svc
        .store()
        .unwrap()
        .thread_dir(done.local_id)
        .unwrap()
        .unwrap();
    assert!(pending_restore(&thread).is_none());
}
