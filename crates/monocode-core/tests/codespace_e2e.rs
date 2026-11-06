//! Real GitHub Codespaces end-to-end check. Opt-in: needs `gh` logged in with the `codespace`
//! scope and an existing codespace of hhoangg/monoptah named in `MONOCODE_TEST_CODESPACE`.
//!
//! ```text
//! MONOCODE_TEST_CODESPACE=<name> cargo test --manifest-path crates/monocode-core/Cargo.toml \
//!     codespace_end_to_end -- --ignored --nocapture
//! ```
//!
//! It never creates or deletes a codespace and never prints secret values. It uses a scratch
//! connection store and session store, so the installed app's data is untouched.
use monocode_core::{
    cloud_sessions::{CloudSessions, CodespaceChoice, Target},
    codespaces::{self, SystemGh},
    remote::Connections,
    store::Store,
};
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    process::Command,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const REPO: &str = "hhoangg/monoptah";

fn step(text: &str) {
    println!(
        "[e2e {:>5.0}s] {text}",
        START
            .get()
            .map_or(0., |s: &Instant| s.elapsed().as_secs_f64())
    );
}
static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

fn wait_for(what: &str, timeout: Duration, mut check: impl FnMut() -> Result<bool, String>) {
    let end = Instant::now() + timeout;
    loop {
        if check().unwrap_or_else(|e| panic!("{what}: {e}")) {
            return;
        }
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_secs(3));
    }
}

fn assistant_text(snapshot: &Value) -> String {
    snapshot["session"]["blocks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|b| b["role"] == "assistant")
        .filter_map(|b| b["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
#[ignore = "needs real gh and MONOCODE_TEST_CODESPACE"]
fn codespace_end_to_end() {
    START.get_or_init(Instant::now);
    let name = std::env::var("MONOCODE_TEST_CODESPACE").expect("set MONOCODE_TEST_CODESPACE");
    let scratch = std::env::temp_dir().join(format!("monoptah-cs-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    // A local project whose origin is the fork, so the repo resolves like in the app.
    let project: PathBuf = scratch.join("project");
    std::fs::create_dir_all(&project).unwrap();
    for args in [
        vec!["init", "-q"],
        vec![
            "remote",
            "add",
            "origin",
            "https://github.com/hhoangg/monoptah",
        ],
    ] {
        assert!(Command::new("git")
            .arg("-C")
            .arg(&project)
            .args(args)
            .status()
            .unwrap()
            .success());
    }

    // gh: auth, list, state, secrets (names only, never values).
    let gh = SystemGh;
    let auth = codespaces::auth_status(&gh).unwrap();
    step(&format!(
        "auth_status: logged_in={} user={:?} has_codespace_scope={} scopes={:?}",
        auth.logged_in, auth.user, auth.has_codespace_scope, auth.scopes
    ));
    auth.require_codespace_scope().unwrap();
    let listed = codespaces::list(&gh, REPO).unwrap();
    step(&format!(
        "list: {:?}",
        listed
            .iter()
            .map(|c| (&c.name, &c.state, &c.machine_name))
            .collect::<Vec<_>>()
    ));
    step(&format!(
        "state({name}) = {:?}",
        codespaces::state(&gh, &name)
    ));
    let secrets = codespaces::secrets_status(&gh, REPO, &[]).unwrap();
    step(&format!(
        "secrets_status: {}",
        serde_json::to_string(&secrets).unwrap()
    ));
    assert!(secrets.iter().any(|s| s.key == "CLAUDE_CODE_OAUTH_TOKEN"));
    assert!(
        listed.iter().any(|c| c.name == name),
        "{name} is not in the codespaces of {REPO} (deleted or expired?)"
    );

    // CloudSessions + Connections: bootstrap, pair, tunnel, projects.open.
    let conns = Arc::new(Connections::new(scratch.join("machines")));
    let store = Arc::new(Mutex::new(
        Store::open(&scratch.join("sessions.db")).unwrap(),
    ));
    let cloud = CloudSessions::new(Arc::new(SystemGh), Arc::new(conns.clone()), store.clone());
    let binding = cloud
        .prepare_target(
            project.to_str().unwrap(),
            Target {
                codespace: CodespaceChoice::Named(name.clone()),
                branch: None,
                machine: None,
            },
            &mut |m| step(&format!("progress: {m}")),
        )
        .unwrap_or_else(|e| panic!("prepare_target failed: {e}"));
    step(&format!(
        "binding: machine={} env={} project={} cwd={}",
        binding.machine_id, binding.environment_id, binding.project_id, binding.remote_cwd
    ));
    assert_eq!(binding.remote_cwd, "/workspaces/monoptah");
    assert_eq!(
        binding.project_key,
        format!("remote://{}/workspaces/monoptah", binding.environment_id)
    );
    let machine = binding.machine_id.clone();

    let describe = conns
        .request(
            &machine,
            "environment.describe",
            json!({"supportedProviders":["claude","codex"]}),
        )
        .unwrap();
    step(&format!(
        "environment.describe: providers={} platform={} capabilities={}",
        describe["providers"],
        describe["platform"],
        describe["capabilities"].as_array().map_or(0, Vec::len)
    ));
    assert_eq!(describe["protocolVersion"], 1);
    let claude_installed = describe["providers"]
        .as_array()
        .is_some_and(|p| p.iter().any(|v| v == "claude"));
    step(&format!("claude provider installed: {claude_installed}"));
    assert!(
        claude_installed,
        "claude is not installed on the codespace host"
    );

    // Create a claude session and send a tiny prompt.
    let models = conns
        .request(
            &machine,
            "models.list",
            json!({"projectId": binding.project_id}),
        )
        .unwrap();
    let claude_models = &models["models"]["claude"];
    step(&format!(
        "claude models: {} errors={}",
        claude_models.as_array().map_or(0, Vec::len),
        models["errors"]
    ));
    step(&format!(
        "claude model ids: {:?}",
        claude_models.as_array().map(|m| m
            .iter()
            .map(|m| (m["id"].clone(), m["isDefault"].clone()))
            .collect::<Vec<_>>())
    ));
    let model = claude_models
        .as_array()
        .and_then(|m| {
            m.iter()
                .find(|m| m["isDefault"] == true)
                .or_else(|| m.first())
        })
        .and_then(|m| m["id"].as_str())
        .unwrap_or_else(|| panic!("no claude models: {models}"))
        .to_owned();
    let wire_model = if model.starts_with("claude:") {
        model
    } else {
        format!("claude:{model}")
    };
    let create = conns
        .request(
            &machine,
            "commands.dispatch",
            json!({"type":"create","commandId":monocode_core::mono::new_id(),"projectId":binding.project_id,
                   "harness":"claude","model":wire_model,"modelSettings":{},"runtimeMode":"supervised"}),
        )
        .unwrap();
    let session_id = create["sessionId"].as_str().unwrap().to_owned();
    step(&format!("created session {session_id}"));
    conns
        .request(
            &machine,
            "commands.dispatch",
            json!({"type":"send","commandId":monocode_core::mono::new_id(),"sessionId":session_id,
                   "text":"Reply with exactly: PONG","attachments":[]}),
        )
        .unwrap();
    let mut known: Option<Value> = None;
    let mut reply = String::new();
    wait_for("a reply containing PONG", Duration::from_secs(300), || {
        let snapshot = conns.load_session(&machine, &session_id, known.as_ref())?;
        reply = assistant_text(&snapshot);
        if !reply.is_empty() {
            step(&format!("status={} reply={reply:?}", snapshot["status"]));
        }
        let finished = reply.contains("PONG")
            || snapshot_failed(&snapshot)
            || (!reply.is_empty() && snapshot["status"] == "idle");
        known = Some(snapshot);
        Ok(finished)
    });
    assert!(
        reply.contains("PONG"),
        "no PONG; last snapshot: {}",
        known.as_ref().unwrap()["session"]["blocks"]
    );
    step("PONG received");

    // Stop, verify Shutdown, wake via ensure, verify describe and the session list (resume).
    codespaces::stop(&gh, &name).unwrap();
    step("stop requested");
    wait_for("state Shutdown", Duration::from_secs(300), || {
        Ok(codespaces::state(&gh, &name)? == "Shutdown")
    });
    step("state is Shutdown");
    match conns.request(&machine, "environment.describe", json!({})) {
        Err(e) => step(&format!("request while stopped (must not wake): {e}")),
        Ok(_) => step("request while stopped unexpectedly succeeded (tunnel still alive?)"),
    }
    assert_eq!(
        codespaces::state(&gh, &name).unwrap(),
        "Shutdown",
        "a plain request must not wake the codespace"
    );
    conns
        .ensure_codespace(&machine)
        .unwrap_or_else(|e| panic!("ensure_codespace failed: {e}"));
    step("ensure_codespace ok");
    let describe = conns
        .request(&machine, "environment.describe", json!({}))
        .unwrap();
    assert_eq!(describe["protocolVersion"], 1);
    let sessions = conns
        .request(
            &machine,
            "sessions.list",
            json!({"projectId": binding.project_id}),
        )
        .unwrap();
    step(&format!(
        "sessions after wake: {}",
        sessions.as_array().map_or(0, Vec::len)
    ));
    assert!(
        sessions
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["id"] == session_id),
        "session lost after wake"
    );
    let again = conns.load_session(&machine, &session_id, None).unwrap();
    assert!(assistant_text(&again).contains("PONG"));
    step("session resumed with its history");

    let _ = conns.request(&machine, "devices.revokeSelf", json!({}));
    conns.shutdown();
    let _ = std::fs::remove_dir_all(&scratch);
}

fn snapshot_failed(snapshot: &Value) -> bool {
    snapshot["status"] == "failed" || snapshot["status"] == "error"
}

/// Evidence pull against a real codespace: writes a file over `gh codespace ssh`, then copies
/// it back with `codespaces::copy_from` and with `CloudSessions::pull_evidence`. Needs the same
/// `MONOCODE_TEST_CODESPACE` (a codespace of hhoangg/monoptah) and never creates or deletes one.
#[test]
#[ignore = "needs real gh and MONOCODE_TEST_CODESPACE"]
fn codespace_evidence_pull() {
    START.get_or_init(Instant::now);
    let name = std::env::var("MONOCODE_TEST_CODESPACE").expect("set MONOCODE_TEST_CODESPACE");
    let scratch = std::env::temp_dir().join(format!("monoptah-cs-evidence-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    let gh = SystemGh;
    let ssh = |script: &str| {
        let args: Vec<String> = ["codespace", "ssh", "-c", &name, "--", "sh", "-s"]
            .map(String::from)
            .to_vec();
        let out = monocode_core::codespaces::GhRunner::run(&gh, &args, Some(script)).unwrap();
        assert!(out.success, "ssh failed: {}", out.stderr);
        out.stdout
    };

    // The setup the app runs after preparing a target: folder, git exclude, agent guidance.
    codespaces::prepare_evidence(&gh, &name, "/workspaces/monoptah").unwrap();
    step("prepare_evidence ok");
    let exclude = ssh("cat /workspaces/monoptah/.git/info/exclude");
    assert!(exclude.lines().any(|l| l == ".evidence/"), "{exclude}");
    let guide = ssh("cat /workspaces/monoptah/CLAUDE.local.md");
    assert!(guide.contains("monoptah-evidence:begin"), "{guide}");
    let status = ssh("git -C /workspaces/monoptah status --porcelain");
    assert!(
        !status.contains("CLAUDE.local.md") && !status.contains(".evidence"),
        "{status}"
    );
    ssh("echo t6-ok > /workspaces/monoptah/.evidence/t6.txt && mkdir -p /workspaces/monoptah/.evidence/sub && echo nested > /workspaces/monoptah/.evidence/sub/n.txt");
    step("remote files written");

    // 1. copy_from directly.
    let direct = scratch.join("direct");
    std::fs::create_dir_all(&direct).unwrap();
    codespaces::copy_from(&gh, &name, "/workspaces/monoptah/.evidence", &direct).unwrap();
    let copied = direct.join(".evidence/t6.txt");
    assert_eq!(std::fs::read_to_string(&copied).unwrap().trim(), "t6-ok");
    step(&format!("copy_from ok: {}", copied.display()));
    let missing = codespaces::copy_from(&gh, &name, "/workspaces/monoptah/.no-such-dir", &direct)
        .unwrap_err();
    assert!(
        codespaces::is_missing_path(&missing, "/workspaces/monoptah/.no-such-dir"),
        "{missing}"
    );

    // 2. The session-level pull into <db>.threads/<session>/evidence.
    let store = Arc::new(Mutex::new(
        Store::open(&scratch.join("sessions.db")).unwrap(),
    ));
    let session = {
        let db = store.lock().unwrap();
        let project = db
            .remember_cloud_project(
                "env-e2e",
                &json!({"id":"p1","cwd":"/workspaces/monoptah"}),
                "/tmp/local-project",
                &name,
            )
            .unwrap();
        db.create("claude", &project.key).unwrap().id
    };
    let cloud = CloudSessions::new(
        Arc::new(SystemGh),
        Arc::new(Arc::new(Connections::new(scratch.join("machines")))),
        store,
    );
    let pull = cloud.pull_evidence(session, None).unwrap();
    step(&format!("pull_evidence files: {:?}", pull.files));
    assert!(pull.files.iter().any(|f| f.ends_with("t6.txt")), "{pull:?}");
    assert!(
        pull.files.iter().any(|f| f.ends_with("sub/n.txt")),
        "{pull:?}"
    );
    assert_eq!(
        std::fs::read_to_string(pull.local_dir.join("t6.txt"))
            .unwrap()
            .trim(),
        "t6-ok"
    );
    let _ = std::fs::remove_dir_all(&scratch);
}

/// T7 live check: converts a real cloud session into an archived local copy, then restores the
/// code. Needs `MONOCODE_TEST_CODESPACE` (a codespace of hhoangg/monoptah) and
/// `MONOCODE_T7_PROJECT` (a scratch clone of the fork; NOT a real checkout). Deletes the
/// codespace as part of the Delete choice. Writes one Claude JSONL into the real
/// `~/.claude/projects/<encoded project>/` which the caller removes afterwards.
#[test]
#[ignore = "needs real gh, claude and MONOCODE_TEST_CODESPACE + MONOCODE_T7_PROJECT"]
fn codespace_archive_to_local() {
    use monocode_core::cloud_sessions::{
        claude_project_dir_name, pending_restore, restore_code, Resume, CODESPACE_DIR, MANIFEST,
    };
    START.get_or_init(Instant::now);
    let name = std::env::var("MONOCODE_TEST_CODESPACE").expect("set MONOCODE_TEST_CODESPACE");
    let project = PathBuf::from(std::env::var("MONOCODE_T7_PROJECT").expect("MONOCODE_T7_PROJECT"));
    assert!(
        project.join(".git").exists(),
        "{} is not a clone",
        project.display()
    );
    let scratch = std::env::temp_dir().join(format!("monoptah-cs-t7-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    let conns = Arc::new(Connections::new(scratch.join("machines")));
    let store = Arc::new(Mutex::new(
        Store::open(&scratch.join("sessions.db")).unwrap(),
    ));
    let cloud = CloudSessions::new(Arc::new(SystemGh), Arc::new(conns.clone()), store.clone());
    let binding = cloud
        .prepare_target(
            project.to_str().unwrap(),
            Target {
                codespace: CodespaceChoice::Named(name.clone()),
                branch: None,
                machine: None,
            },
            &mut |m| step(&format!("progress: {m}")),
        )
        .unwrap_or_else(|e| panic!("prepare_target failed: {e}"));
    let machine = binding.machine_id.clone();
    let models = conns
        .request(
            &machine,
            "models.list",
            json!({"projectId": binding.project_id}),
        )
        .unwrap();
    let model = models["models"]["claude"]
        .as_array()
        .and_then(|m| {
            m.iter()
                .find(|m| m["isDefault"] == true)
                .or_else(|| m.first())
        })
        .and_then(|m| m["id"].as_str())
        .unwrap_or_else(|| panic!("no claude models: {models}"))
        .to_owned();
    let wire_model = if model.starts_with("claude:") {
        model
    } else {
        format!("claude:{model}")
    };
    let create = conns
        .request(
            &machine,
            "commands.dispatch",
            json!({"type":"create","commandId":monocode_core::mono::new_id(),"projectId":binding.project_id,
                   "harness":"claude","model":wire_model,"modelSettings":{},"runtimeMode":"full-access"}),
        )
        .unwrap();
    let session_id = create["sessionId"].as_str().unwrap().to_owned();
    step(&format!("created session {session_id}"));
    let prompt = "Do exactly these steps with your tools, then reply with the single word DONE. \
        1) Run `git checkout -b t7b-live`. \
        2) Create the file t7-created.txt in the repository root containing the single line `hello from the codespace`. \
        3) Run `git add t7-created.txt && git -c user.name=t7 -c user.email=t7@example.com commit -m 'T7 live commit'`. \
        4) Append the line `uncommitted t7 change` to README.md and do not commit it. \
        5) Run `mkdir -p .evidence` and create .evidence/t7-proof.txt containing `proof`. \
        Do not push anything.";
    conns
        .request(
            &machine,
            "commands.dispatch",
            json!({"type":"send","commandId":monocode_core::mono::new_id(),"sessionId":session_id,
                   "text":prompt,"attachments":[]}),
        )
        .unwrap();
    let mut known: Option<Value> = None;
    wait_for(
        "claude to finish the work",
        Duration::from_secs(600),
        || {
            let snapshot = conns.load_session(&machine, &session_id, known.as_ref())?;
            let reply = assistant_text(&snapshot);
            let finished = snapshot_failed(&snapshot)
                || (reply.contains("DONE") && snapshot["status"] == "idle");
            if finished {
                step(&format!(
                    "status={} reply tail={:?}",
                    snapshot["status"],
                    reply
                        .chars()
                        .rev()
                        .take(80)
                        .collect::<String>()
                        .chars()
                        .rev()
                        .collect::<String>()
                ));
            }
            known = Some(snapshot);
            Ok(finished)
        },
    );
    let snapshot = known.unwrap();
    assert!(
        !snapshot_failed(&snapshot),
        "{}",
        snapshot["session"]["blocks"]
    );
    let provider_session = snapshot["session"]["providerSessionId"]
        .as_str()
        .expect("host reports the Claude session id")
        .to_owned();
    step(&format!("claude session id {provider_session}"));

    // The app's record of that host session.
    let cloud_id = {
        let db = store.lock().unwrap();
        let remote_project = db
            .remember_remote_project(
                &binding.environment_id,
                &json!({"id": binding.project_id, "cwd": binding.remote_cwd}),
            )
            .unwrap();
        db.register_remote_summary(&remote_project, &snapshot)
            .unwrap()
            .id
    };

    // The step under test: convert, then delete (the Delete choice).
    let converted = cloud
        .convert_session(cloud_id, &mut |m| step(&format!("convert: {m}")))
        .unwrap_or_else(|e| panic!("convert_session failed: {e}"));
    step(&format!(
        "converted: local={} rows={} resume={:?} evidence={:?} warnings={:?}",
        converted.local_id,
        converted.rows,
        converted.resume,
        converted.evidence,
        converted.warnings
    ));
    let jsonl = match &converted.resume {
        Resume::Installed(path) => path.clone(),
        other => panic!("expected an installed JSONL, got {other:?}"),
    };
    let outcome = cloud.delete_codespace_outcome(&name).unwrap();
    step(&format!("delete outcome: {outcome:?}"));

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let db = store.lock().unwrap();
        let local = db
            .sessions()
            .unwrap()
            .into_iter()
            .find(|s| s.id == converted.local_id)
            .unwrap();
        assert!(local.archived);
        assert_eq!(local.cwd, project.to_str().unwrap());
        assert_eq!(
            local.provider_id.as_deref(),
            Some(provider_session.as_str())
        );
        let rows = db.load(converted.local_id).unwrap().rows;
        assert!(rows.len() >= 2, "{} rows", rows.len());
        assert!(db.remote_session(cloud_id).unwrap().is_none());
        let thread = db.thread_dir(converted.local_id).unwrap().unwrap();
        drop(db);
        assert_eq!(
            std::fs::read_to_string(thread.join("evidence/t7-proof.txt"))
                .unwrap()
                .trim(),
            "proof"
        );
        let canonical = std::fs::canonicalize(&project).unwrap();
        let text = std::fs::read_to_string(&jsonl).unwrap();
        assert!(!text.contains("\"cwd\":\"/workspaces/monoptah\""));
        assert!(text.contains(canonical.to_str().unwrap()));
        assert!(jsonl.starts_with(
            dirs_claude()
                .join("projects")
                .join(claude_project_dir_name(canonical.to_str().unwrap()))
        ));
        step(&format!("jsonl ok: {}", jsonl.display()));
        let manifest: Value = serde_json::from_str(
            &std::fs::read_to_string(thread.join(CODESPACE_DIR).join(MANIFEST)).unwrap(),
        )
        .unwrap();
        step(&format!("manifest: {manifest}"));
        assert_eq!(manifest["branch"], "t7b-live");
        assert_eq!(manifest["unpushedCommits"], 1);
        assert!(thread.join(CODESPACE_DIR).join("commits.bundle").is_file());
        let patch =
            std::fs::read_to_string(thread.join(CODESPACE_DIR).join("changes.patch")).unwrap();
        assert!(patch.contains("uncommitted t7 change") && !patch.contains(".evidence"));

        // claude --resume from the scratch clone.
        let out = Command::new("claude")
            .current_dir(&project)
            .args([
                "--resume",
                &provider_session,
                "-p",
                "What file did you create? Answer briefly.",
            ])
            .output()
            .unwrap();
        step(&format!(
            "claude --resume exit={:?} stdout={:?} stderr={:?}",
            out.status.code(),
            String::from_utf8_lossy(&out.stdout).trim(),
            String::from_utf8_lossy(&out.stderr)
                .trim()
                .chars()
                .take(300)
                .collect::<String>()
        ));

        // Restore into a new worktree.
        assert!(pending_restore(&thread).is_some());
        let restored = restore_code(&project, &thread).unwrap();
        step(&format!("restored: {restored:?}"));
        assert!(
            restored.branch.starts_with("t7b-live"),
            "{}",
            restored.branch
        );
        assert!(restored.patch_applied, "{restored:?}");
        assert_eq!(
            std::fs::read_to_string(restored.worktree.join("t7-created.txt"))
                .unwrap()
                .trim(),
            "hello from the codespace"
        );
        assert!(std::fs::read_to_string(restored.worktree.join("README.md"))
            .unwrap()
            .contains("uncommitted t7 change"));
        let log = Command::new("git")
            .arg("-C")
            .arg(&restored.worktree)
            .args(["log", "--oneline", "-1"])
            .output()
            .unwrap();
        assert!(String::from_utf8_lossy(&log.stdout).contains("T7 live commit"));
        let main_status = Command::new("git")
            .arg("-C")
            .arg(&project)
            .args(["status", "--porcelain"])
            .output()
            .unwrap();
        assert!(main_status.stdout.is_empty(), "main checkout touched");
        step("restore ok");
    }));
    // Remove the JSONL this run wrote into the real Claude folder, pass or fail.
    let _ = std::fs::remove_file(&jsonl);
    conns.shutdown();
    let _ = std::fs::remove_dir_all(&scratch);
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

fn dirs_claude() -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap()).join(".claude"))
}
