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
