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
    world_with(ran_claude, false, true)
}

/// `worktree`: the host session runs in its own git worktree. `extras`: a second branch with
/// work, a stash and ignored files.
fn world_with(ran_claude: bool, worktree: bool, extras: bool) -> World {
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
    if extras {
        // A second branch with work of its own, a stash, and ignored files.
        git(
            &ws,
            &["stash", "push", "-q", "--include-untracked", "-m", "keep"],
        );
        git(&ws, &["checkout", "-q", "-b", "side", "main"]);
        std::fs::write(ws.join("side.txt"), "side\n").unwrap();
        git(&ws, &["add", "side.txt"]);
        git(&ws, &["commit", "-q", "-m", "side work"]);
        git(&ws, &["checkout", "-q", "feat/x"]);
        git(&ws, &["stash", "apply", "-q"]);
        std::fs::write(ws.join("junk.log"), "log\n").unwrap();
        let mut exclude = std::fs::read_to_string(ws.join(".git/info/exclude")).unwrap();
        exclude.push_str("*.log\n");
        std::fs::write(ws.join(".git/info/exclude"), exclude).unwrap();
    }
    let mut session_cwd = ws.clone();
    if worktree {
        let tree = root.join("cs-wt");
        git(
            &ws,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "wt-branch",
                tree.to_str().unwrap(),
                "main",
            ],
        );
        std::fs::write(tree.join("in-worktree.txt"), "wt\n").unwrap();
        session_cwd = tree;
    }
    let ws_text = ws.to_str().unwrap().to_string();
    let session_text = session_cwd.to_str().unwrap().to_string();
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
            "model":"claude:sonnet","runtimeMode":"auto","cwd":session_text,
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
        rewrite_cwd(text, &["/workspaces/app"], "/Users/me/\"q\""),
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
    assert!(
        done.warnings.iter().all(|w| w.contains("ignored")),
        "{:?}",
        done.warnings
    );

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
    assert!(manifest["unpushedCommits"].as_u64().unwrap() >= 2);
    let refs = manifest["refs"].to_string();
    assert!(refs.contains("refs/heads/side") && refs.contains("refs/heads/feat/x"));
    assert!(refs.contains("refs/stash"));
    assert_eq!(manifest["patches"], PATCHES);
    assert_eq!(manifest["baseOnOrigin"], true);
    // Ignored files are not saved, but listed.
    let ignored = manifest["ignoredNotSaved"].to_string();
    assert!(
        ignored.contains("junk.log") && !ignored.contains(".evidence"),
        "{ignored}"
    );
    assert!(
        done.warnings.iter().any(|w| w.contains("ignored")),
        "{:?}",
        done.warnings
    );
    assert!(code.join("patches").read_dir().unwrap().count() >= 1);
    // Claude's raw transcript is always kept, with the codespace path untouched.
    let raw = std::fs::read_to_string(code.join(RAW_JSONL)).unwrap();
    assert!(raw.contains(&format!("\"cwd\":\"{}\"", w.ws.display())));
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
fn regenerable_directories_are_recognised_by_path_component() {
    for path in [
        "node_modules/",
        "web/node_modules/",
        "target/",
        "crates/x/target/",
        ".venv/",
        "venv/",
        "pkg/__pycache__/",
        "dist/",
        "build/",
        ".next/",
        ".turbo/",
        ".cache/",
        ".gradle/",
        "ios/Pods/",
        "vendor/bundle/",
        "app/vendor/bundle/",
        "node_modules/a/b.js",
    ] {
        assert!(is_regenerable(path), "{path}");
    }
    for path in [
        ".env",
        "junk.log",
        "target",
        "build",
        "notes/build.txt",
        "my_node_modules/",
        "vendor/",
        "vendor/gems/",
        "bundle/",
        "secrets/",
    ] {
        assert!(!is_regenerable(path), "{path}");
    }
}

#[test]
fn only_irreplaceable_ignored_files_produce_a_warning_naming_the_first_few() {
    let regen = vec![(638_976, "node_modules/".to_string())];
    assert_eq!(ignored_warning(&regen, false), None);
    assert_eq!(ignored_warning(&[], false), None);

    let mut mixed = regen.clone();
    mixed.push((2048, ".env".into()));
    let text = ignored_warning(&mixed, false).unwrap();
    assert!(
        text.starts_with("1 ignored file or folder (about 2 MB) was not saved"),
        "{text}"
    );
    assert!(
        text.contains(".env") && !text.contains("node_modules"),
        "{text}"
    );

    let many: Vec<(u64, String)> = (0..5).map(|i| (1, format!("f{i}.key"))).collect();
    let text = ignored_warning(&many, false).unwrap();
    assert!(text.starts_with("5 ignored"), "{text}");
    assert!(text.contains("f0.key, f1.key, f2.key and 2 more"), "{text}");
    assert!(!text.contains("f3.key"), "{text}");

    // A small file says "under 1 MB", not "about 0 MB".
    let small = ignored_warning(&[(3, ".env".into())], false).unwrap();
    assert!(
        small.starts_with("1 ignored file or folder (under 1 MB) was not saved: .env."),
        "{small}"
    );
    // A cut-off listing never claims to be complete.
    let cut = ignored_warning(&[(3, ".env".into())], true).unwrap();
    assert!(
        cut.starts_with("At least 1") && cut.contains("more than the manifest lists"),
        "{cut}"
    );
}

#[test]
fn git_quoted_paths_are_classified_without_their_quotes() {
    assert!(is_regenerable("\"odd \\\"name\\\"/node_modules/\""));
    assert!(!is_regenerable("\"odd \\\"name\\\".env\""));
}

#[test]
fn many_regenerable_dirs_do_not_hide_a_real_ignored_file() {
    let w = world_with(true, false, true);
    for i in 0..50 {
        let dir = w.ws.join(format!("pkg{i:02}/__pycache__"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("m.pyc"), "x").unwrap();
        // A source file keeps git from collapsing the folder into `pkgNN/`.
        std::fs::write(w.ws.join(format!("pkg{i:02}/a.py")), "x").unwrap();
    }
    std::fs::write(w.ws.join("zz-secrets.json"), "{}").unwrap();
    // A symlinked dependency folder is a folder too.
    let target = w.root.join("elsewhere");
    std::fs::create_dir_all(&target).unwrap();
    std::os::unix::fs::symlink(&target, w.ws.join("node_modules")).unwrap();
    let mut exclude = std::fs::read_to_string(w.ws.join(".git/info/exclude")).unwrap();
    exclude.push_str("__pycache__/\nzz-secrets.json\nnode_modules\n");
    std::fs::write(w.ws.join(".git/info/exclude"), exclude).unwrap();

    let done = w.svc.convert_session(w.cloud_id, &mut |_| {}).unwrap();
    let names = done.unsaved_names();
    assert!(names.contains(&"zz-secrets.json"), "{names:?}");
    assert!(names.contains(&"junk.log"), "{names:?}");
    assert!(
        !names
            .iter()
            .any(|n| n.contains("__pycache__") || n.contains("node_modules")),
        "{names:?}"
    );
    assert!(!done.ignored_truncated);
    let warning = done
        .warnings
        .iter()
        .find(|w| w.contains("not saved"))
        .unwrap();
    assert!(
        warning.starts_with("2 ignored files or folders (under 1 MB)"),
        "{warning}"
    );
    assert!(
        warning.contains("zz-secrets.json") && !warning.contains("__pycache__"),
        "{warning}"
    );
    let thread = w
        .svc
        .store()
        .unwrap()
        .thread_dir(done.local_id)
        .unwrap()
        .unwrap();
    let manifest: Value = serde_json::from_str(
        &std::fs::read_to_string(thread.join(CODESPACE_DIR).join(MANIFEST)).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["ignoredTruncated"], false);
    let listed = manifest["ignoredNotSaved"].to_string();
    assert!(
        listed.contains("zz-secrets.json") && listed.contains("pkg00/__pycache__/"),
        "{listed}"
    );
    assert!(
        listed.contains("\"node_modules/\""),
        "symlink listed as a folder: {listed}"
    );
}

#[test]
fn a_long_list_of_real_ignored_files_is_marked_as_cut_off() {
    let w = world_with(true, false, true);
    for i in 0..45 {
        std::fs::write(w.ws.join(format!("k{i:02}.secret")), "s").unwrap();
    }
    let mut exclude = std::fs::read_to_string(w.ws.join(".git/info/exclude")).unwrap();
    exclude.push_str("*.secret\n");
    std::fs::write(w.ws.join(".git/info/exclude"), exclude).unwrap();
    let done = w.svc.convert_session(w.cloud_id, &mut |_| {}).unwrap();
    assert!(done.ignored_truncated);
    let warning = done
        .warnings
        .iter()
        .find(|w| w.contains("not saved"))
        .unwrap();
    assert!(warning.starts_with("At least 40 ignored"), "{warning}");
    let thread = w
        .svc
        .store()
        .unwrap()
        .thread_dir(done.local_id)
        .unwrap()
        .unwrap();
    let manifest: Value = serde_json::from_str(
        &std::fs::read_to_string(thread.join(CODESPACE_DIR).join(MANIFEST)).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["ignoredTruncated"], true);
}

#[test]
fn ignored_files_are_never_copied_into_the_session_folder() {
    let w = world_with(true, false, true);
    let secret = "SECRET-TOKEN-4f9c1d";
    let dep = "DEP-CONTENT-77aa";
    std::fs::write(w.ws.join(".env"), format!("KEY={secret}\n")).unwrap();
    std::fs::create_dir_all(w.ws.join("node_modules")).unwrap();
    std::fs::write(w.ws.join("node_modules/x"), format!("{dep}\n")).unwrap();
    let mut exclude = std::fs::read_to_string(w.ws.join(".git/info/exclude")).unwrap();
    exclude.push_str(".env\nnode_modules/\n");
    std::fs::write(w.ws.join(".git/info/exclude"), exclude).unwrap();

    let done = w.svc.convert_session(w.cloud_id, &mut |_| {}).unwrap();
    let thread = w
        .svc
        .store()
        .unwrap()
        .thread_dir(done.local_id)
        .unwrap()
        .unwrap();

    // Listed in the manifest (node_modules marked regenerable), never stored.
    let code = thread.join(CODESPACE_DIR);
    let manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(code.join(MANIFEST)).unwrap()).unwrap();
    let listed = manifest["ignoredNotSaved"].as_array().unwrap();
    let entry = |name: &str| {
        listed
            .iter()
            .find(|e| e["path"].as_str().is_some_and(|p| p.starts_with(name)))
            .unwrap_or_else(|| panic!("{name} missing from {listed:?}"))
    };
    assert_eq!(entry("node_modules")["regenerable"], true);
    assert_eq!(entry(".env")["regenerable"], false);
    // Only the .env-like entries count towards the warning.
    let warning = done
        .warnings
        .iter()
        .find(|w| w.contains("ignored"))
        .unwrap();
    assert!(
        warning.contains(".env") && !warning.contains("node_modules"),
        "{warning}"
    );

    // Every file under the session folder is an expected artifact.
    let mut files = vec![];
    collect_files(&thread, &thread, &mut files);
    assert!(!files.is_empty());
    for (rel, bytes) in &files {
        let expected = rel.starts_with("evidence/")
            || rel.starts_with("codespace/patches/")
            || [
                "claude.jsonl",
                "manifest.json",
                "commits.bundle",
                "changes.patch",
            ]
            .iter()
            .any(|n| rel == &format!("codespace/{n}"));
        assert!(expected, "unexpected file {rel}");
        for needle in [secret, dep] {
            assert!(
                !bytes.windows(needle.len()).any(|w| w == needle.as_bytes()),
                "{needle} leaked into {rel}"
            );
        }
        assert!(
            !rel.contains("node_modules") && !rel.ends_with(".env"),
            "{rel}"
        );
    }
    let names: Vec<&str> = files.iter().map(|(r, _)| r.as_str()).collect();
    assert!(names.contains(&"codespace/changes.patch"), "{names:?}");
}

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect_files(root, &path, out);
        } else {
            let rel = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            out.push((rel, std::fs::read(&path).unwrap()));
        }
    }
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
    let w = world_with(true, false, false);
    git(&w.ws, &["checkout", "-q", "main"]);
    git(&w.ws, &["checkout", "-q", "--", "."]);
    git(&w.ws, &["clean", "-fdq"]);
    git(&w.ws, &["branch", "-D", "feat/x"]);
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
fn a_workspace_that_is_not_a_git_repository_fails_so_the_user_must_decide() {
    let w = world(true);
    std::fs::remove_dir_all(w.ws.join(".git")).unwrap();
    let before = w.svc.store().unwrap().sessions().unwrap().len();
    let error = w.svc.convert_session(w.cloud_id, &mut |_| {}).unwrap_err();
    assert!(error.contains("not a git repository"), "{error}");
    let store = w.svc.store().unwrap();
    assert_eq!(store.sessions().unwrap().len(), before);
    assert!(store.remote_session(w.cloud_id).unwrap().is_some());
}

#[test]
fn a_repository_without_commits_also_fails() {
    let w = world(true);
    std::fs::remove_dir_all(w.ws.join(".git")).unwrap();
    git(&w.ws, &["init", "-q"]);
    let error = w.svc.convert_session(w.cloud_id, &mut |_| {}).unwrap_err();
    assert!(error.contains("at least one commit"), "{error}");
    // The script cleaned its stage up after itself.
}

#[test]
fn the_session_worktree_is_what_gets_saved() {
    let w = world_with(true, true, false);
    let done = w.svc.convert_session(w.cloud_id, &mut |_| {}).unwrap();
    let thread = w
        .svc
        .store()
        .unwrap()
        .thread_dir(done.local_id)
        .unwrap()
        .unwrap();
    let manifest: Value = serde_json::from_str(
        &std::fs::read_to_string(thread.join(CODESPACE_DIR).join(MANIFEST)).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["branch"], "wt-branch");
    assert_eq!(
        manifest["remoteCwd"],
        w.root.join("cs-wt").to_str().unwrap()
    );
    let patch = std::fs::read_to_string(thread.join(CODESPACE_DIR).join(PATCH)).unwrap();
    assert!(patch.contains("in-worktree.txt") && !patch.contains("untracked.txt"));
}

#[test]
fn a_converted_host_session_is_never_imported_again_and_a_retry_skips_it() {
    let w = world(true);
    let done = w.svc.convert_session(w.cloud_id, &mut |_| {}).unwrap();
    let store = w.svc.store().unwrap();
    assert!(store.is_cloud_converted("env-1", "host-1").unwrap());
    // The poll of an open pane, or a later session listing, tries to register it again.
    let project = store
        .remember_remote_project("env-1", &json!({"id":"p","cwd": w.ws.to_str().unwrap()}))
        .unwrap();
    let snapshot = w.host.snapshot.clone();
    let error = store
        .register_remote_summary(&project, &snapshot)
        .unwrap_err();
    assert_eq!(error, crate::store::CONVERTED);
    assert!(store.cloud_session_ids("cs-1").unwrap().is_empty());
    drop(store);
    assert!(!w.svc.is_cloud_session(w.cloud_id));
    assert!(w.svc.sessions_in_codespace("cs-1").is_empty());
    assert!(w.svc.convert_session(w.cloud_id, &mut |_| {}).is_err());
    assert!(done.local_id != w.cloud_id);
}

#[test]
fn every_session_of_a_codespace_is_listed_archived_or_not() {
    let w = world(true);
    let other = {
        let store = w.svc.store().unwrap();
        let project = store
            .remember_remote_project("env-1", &json!({"id":"p","cwd": w.ws.to_str().unwrap()}))
            .unwrap();
        let mut value = w.host.snapshot.clone();
        value["session"]["id"] = json!("host-2");
        value["archived"] = json!(true);
        store.register_remote_summary(&project, &value).unwrap().id
    };
    let mut ids = w.svc.sessions_in_codespace("cs-1");
    ids.sort();
    let mut want = vec![w.cloud_id, other];
    want.sort();
    assert_eq!(ids, want);
    assert!(w.svc.sessions_in_codespace("elsewhere").is_empty());
}

#[test]
fn a_failed_jsonl_install_still_keeps_the_raw_transcript_and_warns() {
    let w = world(true);
    // `projects` is a file, so the folder for the local project cannot be created.
    std::fs::create_dir_all(&w.claude).unwrap();
    std::fs::write(w.claude.join("projects"), "x").unwrap();
    let done = w.svc.convert_session(w.cloud_id, &mut |_| {}).unwrap();
    assert!(matches!(done.resume, Resume::Missing(_)));
    assert!(done.warnings.iter().any(|m| m.contains("raw transcript")));
    let thread = w
        .svc
        .store()
        .unwrap()
        .thread_dir(done.local_id)
        .unwrap()
        .unwrap();
    assert!(thread.join(CODESPACE_DIR).join(RAW_JSONL).is_file());
}

/// A commit pushed to origin after the local clone was made: the bundle needs it.
fn push_a_commit_the_clone_lacks(w: &World) {
    git(&w.ws, &["checkout", "-q", "main"]);
    std::fs::write(w.ws.join("later.txt"), "later\n").unwrap();
    git(&w.ws, &["add", "later.txt"]);
    git(&w.ws, &["commit", "-q", "-m", "pushed later"]);
    git(&w.ws, &["push", "-q", "origin", "main"]);
    git(&w.ws, &["checkout", "-q", "-b", "feat/y"]);
    std::fs::write(w.ws.join("y.txt"), "y\n").unwrap();
    git(&w.ws, &["add", "y.txt"]);
    git(&w.ws, &["commit", "-q", "-m", "on top of later"]);
}

#[test]
fn commits_built_on_something_the_clone_lacks_are_restored_from_the_patch_series_or_origin() {
    let w = world_with(true, false, false);
    push_a_commit_the_clone_lacks(&w);
    let done = w.svc.convert_session(w.cloud_id, &mut |_| {}).unwrap();
    let thread = w
        .svc
        .store()
        .unwrap()
        .thread_dir(done.local_id)
        .unwrap()
        .unwrap();
    let main_before = git(&w.project, &["rev-parse", "main"]);
    let remote_ref = git(&w.project, &["rev-parse", "origin/main"]);
    // The bundle is unusable at first; origin has the missing commit, so it works.
    let restored = restore_code(&w.project, &thread).unwrap();
    assert!(restored.worktree.join("y.txt").is_file());
    assert!(restored.worktree.join("later.txt").is_file());
    // Nothing the user owns moved: not main, not origin/main.
    assert_eq!(git(&w.project, &["rev-parse", "main"]), main_before);
    assert_eq!(git(&w.project, &["rev-parse", "origin/main"]), remote_ref);
}

#[test]
fn without_the_bundle_the_series_is_replayed_onto_the_base() {
    let w = world_with(true, false, false);
    push_a_commit_the_clone_lacks(&w);
    let done = w.svc.convert_session(w.cloud_id, &mut |_| {}).unwrap();
    let thread = w
        .svc
        .store()
        .unwrap()
        .thread_dir(done.local_id)
        .unwrap()
        .unwrap();
    std::fs::remove_file(thread.join(CODESPACE_DIR).join(BUNDLE)).unwrap();
    let restored = restore_code(&w.project, &thread).unwrap();
    assert!(restored.conflicts.is_none(), "{:?}", restored.conflicts);
    assert!(restored.worktree.join("y.txt").is_file());
    assert_eq!(
        git(&restored.worktree, &["log", "--format=%s", "-1"]),
        "on top of later"
    );
    assert_eq!(git(&w.project, &["status", "--porcelain"]), "");
}

#[test]
fn a_bundle_that_cannot_verify_and_has_no_fallback_stops_the_conversion() {
    let dir = tempfile::tempdir().unwrap();
    let other = dir.path().join("other");
    let local = dir.path().join("local");
    for repo in [&other, &local] {
        std::fs::create_dir_all(repo).unwrap();
        git(repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("f"), repo.to_str().unwrap()).unwrap();
        git(repo, &["add", "f"]);
        git(repo, &["commit", "-q", "-m", "one"]);
    }
    std::fs::write(other.join("f"), "2").unwrap();
    git(&other, &["commit", "-q", "-am", "two"]);
    let code = dir.path().join("code");
    std::fs::create_dir_all(&code).unwrap();
    git(
        &other,
        &[
            "bundle",
            "create",
            "-q",
            code.join(BUNDLE).to_str().unwrap(),
            "main",
            "^main~1",
        ],
    );
    let head = git(&local, &["rev-parse", "HEAD"]);
    let snap = Snapshot {
        head: Some(head.clone()),
        ..Snapshot::default()
    };
    let error = check_restorable(&local, &code, &snap, "s1").unwrap_err();
    assert!(error.contains("cannot be restored"), "{error}");
    // With a patch series and a base we have, the same bundle is accepted with a warning...
    let snap = Snapshot {
        head: Some(head.clone()),
        patches: 1,
        base: Some(head.clone()),
        ..Snapshot::default()
    };
    assert!(check_restorable(&local, &code, &snap, "s1")
        .unwrap()
        .is_some());
    // ...but not when the base is unknown here, or a stash would be lost.
    let unknown = Snapshot {
        base: Some("a".repeat(40)),
        ..snap.clone()
    };
    assert!(check_restorable(&local, &code, &unknown, "s1").is_err());
    let stash = Snapshot {
        stash: true,
        ..snap
    };
    assert!(check_restorable(&local, &code, &stash, "s1").is_err());
}

#[test]
fn with_no_bundle_every_saved_commit_must_already_be_here() {
    let dir = tempfile::tempdir().unwrap();
    let local = dir.path().join("local");
    std::fs::create_dir_all(&local).unwrap();
    git(&local, &["init", "-q", "-b", "main"]);
    std::fs::write(local.join("f"), "1").unwrap();
    git(&local, &["add", "f"]);
    git(&local, &["commit", "-q", "-m", "one"]);
    let code = dir.path().join("code");
    std::fs::create_dir_all(&code).unwrap();
    let here = git(&local, &["rev-parse", "HEAD"]);
    let ok = Snapshot {
        head: Some(here.clone()),
        refs: vec![("refs/heads/main".into(), here.clone())],
        ..Snapshot::default()
    };
    assert!(check_restorable(&local, &code, &ok, "s1")
        .unwrap()
        .is_none());
    let lost = Snapshot {
        head: Some("b".repeat(40)),
        ..Snapshot::default()
    };
    let error = check_restorable(&local, &code, &lost, "s1").unwrap_err();
    assert!(error.contains("not on origin"), "{error}");
    let lost_ref = Snapshot {
        head: Some(here),
        refs: vec![("refs/heads/x".into(), "c".repeat(40))],
        ..Snapshot::default()
    };
    assert!(check_restorable(&local, &code, &lost_ref, "s1").is_err());
}

#[test]
fn restore_keeps_other_branches_and_the_stash_out_of_the_users_namespace() {
    let w = world(true);
    let done = w.svc.convert_session(w.cloud_id, &mut |_| {}).unwrap();
    let thread = w
        .svc
        .store()
        .unwrap()
        .thread_dir(done.local_id)
        .unwrap()
        .unwrap();
    restore_code(&w.project, &thread).unwrap();
    let head = pin_key(w.cloud_id);
    assert!(git(&w.project, &["branch", "--list", "side"]).is_empty());
    assert!(!git(
        &w.project,
        &["rev-parse", &format!("refs/codespace/{head}/heads/side")]
    )
    .is_empty());
    assert!(!git(
        &w.project,
        &["rev-parse", &format!("refs/codespace/{head}/stash")]
    )
    .is_empty());
}

#[test]
fn stage_paths_outside_the_expected_prefix_are_refused() {
    assert!(parse_snapshot(
        "stage=/tmp/other
"
    )
    .is_err());
    assert!(parse_snapshot(
        "stage=/home/x/monoptah-archive.A
"
    )
    .is_err());
    assert!(parse_snapshot(
        "stage=/tmp/monoptah-archive.A
"
    )
    .is_ok());
}

#[test]
fn rewriting_respects_both_boundaries() {
    let text = "/tmp/workspaces/app /workspaces/app.git /workspaces/app.bak /workspaces/app. \"/workspaces/app\" /workspaces/app/x";
    let out = rewrite_cwd(text, &["/workspaces/app"], "/L");
    assert_eq!(
        out,
        "/tmp/workspaces/app /workspaces/app.git /workspaces/app.bak /L. \"/L\" /L/x"
    );
    // The longer path wins over its parent.
    let out = rewrite_cwd(
        "/workspaces/app-wt/x /workspaces/app",
        &["/workspaces/app", "/workspaces/app-wt"],
        "/L",
    );
    assert_eq!(out, "/L/x /L");
}

fn no_codespace_namespace_left(project: &Path) {
    let refs = git(project, &["for-each-ref", "refs/codespace"]);
    assert!(!refs.contains("/origin/"), "{refs}");
}

#[test]
fn work_reachable_only_from_a_stale_remote_ref_is_still_bundled_and_restorable() {
    let w = world_with(true, false, false);
    // Pushed, then the branch was deleted on origin (a squash merge); the codespace still
    // has the stale origin/feat/z until it prunes.
    git(&w.ws, &["checkout", "-q", "-b", "feat/z", "main"]);
    std::fs::write(w.ws.join("z.txt"), "z\n").unwrap();
    git(&w.ws, &["add", "z.txt"]);
    git(&w.ws, &["commit", "-q", "-m", "z work"]);
    git(&w.ws, &["push", "-q", "origin", "feat/z"]);
    git(&w.origin, &["branch", "-D", "feat/z"]);
    assert!(git(&w.ws, &["branch", "-r"]).contains("origin/feat/z"));
    let done = w.svc.convert_session(w.cloud_id, &mut |_| {}).unwrap();
    let thread = w
        .svc
        .store()
        .unwrap()
        .thread_dir(done.local_id)
        .unwrap()
        .unwrap();
    let code = thread.join(CODESPACE_DIR);
    assert!(
        code.join(BUNDLE).is_file(),
        "pruned, so the commit is unpushed"
    );
    no_codespace_namespace_left(&w.project);
    let restored = restore_code(&w.project, &thread).unwrap();
    assert_eq!(restored.branch, "feat/z");
    assert!(restored.worktree.join("z.txt").is_file());
    no_codespace_namespace_left(&w.project);
}

#[test]
fn work_that_exists_only_on_another_remote_is_bundled() {
    let w = world_with(true, false, false);
    let upstream = w.root.join("upstream.git");
    std::fs::create_dir_all(&upstream).unwrap();
    git(&upstream, &["init", "--bare", "-q", "-b", "main"]);
    git(
        &w.ws,
        &["remote", "add", "upstream", upstream.to_str().unwrap()],
    );
    git(&w.ws, &["checkout", "-q", "-b", "feat/u", "main"]);
    std::fs::write(w.ws.join("u.txt"), "u\n").unwrap();
    git(&w.ws, &["add", "u.txt"]);
    git(&w.ws, &["commit", "-q", "-m", "u work"]);
    git(&w.ws, &["push", "-q", "upstream", "feat/u"]);
    let done = w.svc.convert_session(w.cloud_id, &mut |_| {}).unwrap();
    let thread = w
        .svc
        .store()
        .unwrap()
        .thread_dir(done.local_id)
        .unwrap()
        .unwrap();
    assert!(thread.join(CODESPACE_DIR).join(BUNDLE).is_file());
    let restored = restore_code(&w.project, &thread).unwrap();
    assert!(restored.worktree.join("u.txt").is_file());
}

#[test]
fn a_session_saved_as_a_local_copy_cannot_be_journaled_back() {
    let w = world(true);
    let record = w
        .svc
        .store()
        .unwrap()
        .remote_session(w.cloud_id)
        .unwrap()
        .unwrap();
    let done = w.svc.convert_session(w.cloud_id, &mut |_| {}).unwrap();
    let store = w.svc.store().unwrap();
    // The outbox journal of a send racing the conversion.
    let mut late = record.clone();
    late.outbox.push(json!({"type":"send","commandId":"late"}));
    assert_eq!(
        store.save_remote_session(&late).unwrap_err(),
        crate::store::CONVERTED
    );
    assert!(store.remote_session(w.cloud_id).unwrap().is_none());
    assert!(store.cloud_session_ids("cs-1").unwrap().is_empty());
    drop(store);
    assert!(done.local_id != w.cloud_id);
}

#[test]
fn deleting_the_local_copy_releases_the_tombstone() {
    let w = world(true);
    let done = w.svc.convert_session(w.cloud_id, &mut |_| {}).unwrap();
    let mut store = w.svc.store().unwrap();
    assert!(store.is_cloud_converted("env-1", "host-1").unwrap());
    store.delete(done.local_id).unwrap();
    assert!(!store.is_cloud_converted("env-1", "host-1").unwrap());
}

#[test]
fn commits_only_on_origin_stay_reachable_after_origin_loses_them() {
    let w = world_with(true, false, false);
    // Pushed work: nothing to bundle, so restore depends on the commit staying here.
    git(&w.ws, &["checkout", "-q", "-b", "feat/p", "main"]);
    std::fs::write(w.ws.join("p.txt"), "p\n").unwrap();
    git(&w.ws, &["add", "p.txt"]);
    git(&w.ws, &["commit", "-q", "-m", "p work"]);
    git(&w.ws, &["push", "-q", "origin", "feat/p"]);
    let done = w.svc.convert_session(w.cloud_id, &mut |_| {}).unwrap();
    let thread = w
        .svc
        .store()
        .unwrap()
        .thread_dir(done.local_id)
        .unwrap()
        .unwrap();
    no_codespace_namespace_left(&w.project);
    let keep = git(&w.project, &["for-each-ref", "refs/codespace"]);
    assert!(keep.contains("/keep/head"), "{keep}");
    // The branch disappears from origin and the repository forgets unreachable objects.
    git(&w.origin, &["branch", "-D", "feat/p"]);
    git(&w.project, &["reflog", "expire", "--expire=now", "--all"]);
    git(&w.project, &["gc", "-q", "--prune=now"]);
    let restored = restore_code(&w.project, &thread).unwrap();
    assert!(restored.worktree.join("p.txt").is_file());
    // The pins are released once the branch holds the commits.
    assert!(!git(&w.project, &["for-each-ref", "refs/codespace"]).contains("/keep/"));
}

#[test]
fn a_failed_patch_leaves_a_partial_restore_that_a_retry_finishes_in_the_same_worktree() {
    let w = world(true);
    let done = w.svc.convert_session(w.cloud_id, &mut |_| {}).unwrap();
    let thread = w
        .svc
        .store()
        .unwrap()
        .thread_dir(done.local_id)
        .unwrap()
        .unwrap();
    let patch = thread.join(CODESPACE_DIR).join(PATCH);
    let good = std::fs::read_to_string(&patch).unwrap();
    std::fs::write(&patch, "not a patch\n").unwrap();
    let first = restore_code(&w.project, &thread).unwrap();
    let message = first.conflicts.clone().unwrap();
    assert!(
        message.contains(first.worktree.to_str().unwrap()),
        "{message}"
    );
    assert!(message.contains("git apply --binary"), "{message}");
    let pending = pending_restore(&thread).expect("still pending");
    assert_eq!(pending.partial.unwrap().worktree, first.worktree);
    // Fixed: the retry continues there and does not make a second worktree.
    std::fs::write(&patch, good).unwrap();
    let second = restore_code(&w.project, &thread).unwrap();
    assert_eq!(second.worktree, first.worktree);
    assert!(second.patch_applied && second.conflicts.is_none());
    assert!(pending_restore(&thread).is_none());
    let trees = git(&w.project, &["worktree", "list"]);
    assert_eq!(trees.lines().count(), 2, "{trees}");
}

fn pushed_only_work(w: &World) {
    git(&w.ws, &["checkout", "-q", "-b", "feat/p", "main"]);
    std::fs::write(w.ws.join("p.txt"), "p\n").unwrap();
    git(&w.ws, &["add", "p.txt"]);
    git(&w.ws, &["commit", "-q", "-m", "p work"]);
    git(&w.ws, &["push", "-q", "origin", "feat/p"]);
}

fn second_cloud_session(w: &World) -> i64 {
    let store = w.svc.store().unwrap();
    let project = store
        .remember_remote_project("env-1", &json!({"id":"p","cwd": w.ws.to_str().unwrap()}))
        .unwrap();
    let mut value = w.host.snapshot.clone();
    value["session"]["id"] = json!("host-2");
    store.register_remote_summary(&project, &value).unwrap().id
}

#[test]
fn sessions_sharing_a_head_keep_their_own_pins() {
    let w = world_with(true, false, false);
    pushed_only_work(&w);
    let second = second_cloud_session(&w);
    let first = w.svc.convert_session(w.cloud_id, &mut |_| {}).unwrap();
    let other = w.svc.convert_session(second, &mut |_| {}).unwrap();
    let refs = git(&w.project, &["for-each-ref", "refs/codespace"]);
    assert!(
        refs.contains(&format!("{}/keep/head", pin_key(w.cloud_id))),
        "{refs}"
    );
    assert!(
        refs.contains(&format!("{}/keep/head", pin_key(second))),
        "{refs}"
    );
    let thread_of = |id| w.svc.store().unwrap().thread_dir(id).unwrap().unwrap();
    let restored = restore_code(&w.project, &thread_of(first.local_id)).unwrap();
    // Forget everything else about that commit.
    git(
        &w.project,
        &[
            "worktree",
            "remove",
            "--force",
            restored.worktree.to_str().unwrap(),
        ],
    );
    git(&w.project, &["branch", "-D", &restored.branch]);
    git(&w.origin, &["branch", "-D", "feat/p"]);
    git(&w.project, &["reflog", "expire", "--expire=now", "--all"]);
    git(&w.project, &["gc", "-q", "--prune=now"]);
    let again = restore_code(&w.project, &thread_of(other.local_id)).unwrap();
    assert!(again.worktree.join("p.txt").is_file());
}

#[test]
fn restore_gives_every_saved_ref_its_own_pin_before_the_keep_pins_go() {
    let w = world_with(true, false, false);
    pushed_only_work(&w);
    let done = w.svc.convert_session(w.cloud_id, &mut |_| {}).unwrap();
    let thread = w
        .svc
        .store()
        .unwrap()
        .thread_dir(done.local_id)
        .unwrap()
        .unwrap();
    restore_code(&w.project, &thread).unwrap();
    let key = pin_key(w.cloud_id);
    let refs = git(&w.project, &["for-each-ref", "refs/codespace"]);
    assert!(!refs.contains("/keep/"), "{refs}");
    assert!(refs.contains(&format!("{key}/heads/feat/p")), "{refs}");
    assert!(refs.contains(&format!("{key}/heads/main")), "{refs}");
}

#[test]
fn pins_are_released_when_the_check_fails_or_the_copy_is_deleted_unrestored() {
    // Failed capture: nothing stays pinned.
    let dir = tempfile::tempdir().unwrap();
    let local = dir.path().join("local");
    std::fs::create_dir_all(&local).unwrap();
    git(&local, &["init", "-q", "-b", "main"]);
    std::fs::write(local.join("f"), "1").unwrap();
    git(&local, &["add", "f"]);
    git(&local, &["commit", "-q", "-m", "one"]);
    let here = git(&local, &["rev-parse", "HEAD"]);
    git(
        &local,
        &["update-ref", "refs/codespace/s9/keep/head", &here],
    );
    let snap = Snapshot {
        head: Some(here),
        refs: vec![("refs/heads/x".into(), "c".repeat(40))],
        ..Snapshot::default()
    };
    assert!(check_restorable(&local, dir.path(), &snap, "s9").is_err());
    assert!(git(&local, &["for-each-ref", "refs/codespace"]).is_empty());

    // Deleted without ever restoring.
    let w = world_with(true, false, false);
    pushed_only_work(&w);
    let done = w.svc.convert_session(w.cloud_id, &mut |_| {}).unwrap();
    assert!(git(&w.project, &["for-each-ref", "refs/codespace"]).contains("/keep/head"));
    w.svc.store().unwrap().delete(done.local_id).unwrap();
    assert!(git(&w.project, &["for-each-ref", "refs/codespace"]).is_empty());
}
