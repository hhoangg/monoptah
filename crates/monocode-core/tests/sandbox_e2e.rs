//! Real Docker Sandboxes cloud end-to-end check. Opt-in and COSTS money: it creates ONE cloud
//! sandbox (2 CPU / 4g, ttl 30m) from a small public repository, walks the whole session
//! life and deletes the sandbox at the end (and in a Drop guard when anything fails).
//!
//! ```text
//! MONOCODE_TEST_SBX=1 cargo test --test sandbox_e2e -- --ignored --nocapture
//! ```
//!
//! Needs `sbx` signed in to the cloud and `sbx --cloud setup ssh` done. It uses no Git
//! credential (public repo) and no Claude account, a scratch session store and a scratch
//! connection store, so the installed app's data is untouched. It never prints a secret.
//! It also needs the Monoptah Host package of this crate's version to be downloadable from
//! the release URL the bootstrap uses.
use monocode_core::{
    cloud_sessions::{
        backend::docker::{SandboxShell, SystemShell},
        docker_config::{self, DockerProjectConfig},
        startup::{StartEvent, StepKind, StepStatus},
        CloudBackend, CloudBackendKind, CloudRef, CloudSessions, DeleteOutcome, Host,
    },
    codespaces::SystemGh,
    remote::Connections,
    sandboxes::{self, SystemSbx},
    store::Store,
};
use serde_json::json;
use std::{
    process::Command,
    sync::{atomic::AtomicBool, Arc, Mutex},
    time::{Duration, Instant},
};

const REPO: &str = "https://github.com/microsoft/vscode-remote-try-node.git";
const DUMMY_NOTE: &str = "no Claude account is configured, so no Claude variable may exist";
static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

fn step(text: &str) {
    println!(
        "[e2e {:>5.0}s] {text}",
        START.get().map_or(0., |s| s.elapsed().as_secs_f64())
    );
}

fn enabled() -> bool {
    if std::env::var("MONOCODE_TEST_SBX").as_deref() == Ok("1") {
        return true;
    }
    println!("skipped: set MONOCODE_TEST_SBX=1 to create a real Docker cloud sandbox");
    false
}

/// Deletes the sandbox called `name` however the test ends. Only ever touches that name.
struct Cleanup {
    name: String,
}
impl Drop for Cleanup {
    fn drop(&mut self) {
        let runner = SystemSbx;
        match sandboxes::find_by_name(&runner, &self.name) {
            Ok(Some(info)) => {
                let result = sandboxes::delete(&runner, &info.id);
                println!("[e2e] cleanup: rm {} -> {result:?}", info.id);
            }
            Ok(None) => println!("[e2e] cleanup: {} is already gone", self.name),
            Err(e) => println!("[e2e] cleanup: lookup of {} failed: {e}", self.name),
        }
    }
}

fn git(dir: &std::path::Path, args: &[&str]) {
    assert!(Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .status()
        .unwrap()
        .success());
}

/// Runs `script` as `sh -s` over ssh in the OUTER sandbox (user-initiated).
fn outer(shell: &SystemShell, id: &str, script: &str) -> String {
    let out = shell
        .run(
            id,
            "sh -s",
            script,
            &mut |_| {},
            &|| false,
            Duration::from_secs(120),
        )
        .unwrap_or_else(|e| panic!("ssh failed: {e}"));
    assert!(out.success, "script failed: {}{}", out.stdout, out.stderr);
    out.stdout
}

fn wait_for(what: &str, timeout: Duration, mut check: impl FnMut() -> Result<bool, String>) {
    let end = Instant::now() + timeout;
    loop {
        if check().unwrap_or_else(|e| panic!("{what}: {e}")) {
            return;
        }
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_secs(5));
    }
}

#[test]
#[ignore = "creates a real Docker cloud sandbox; needs MONOCODE_TEST_SBX=1"]
fn sandbox_end_to_end() {
    if !enabled() {
        return;
    }
    START.get_or_init(Instant::now);
    let scratch = tempfile::tempdir().unwrap();
    let project = scratch.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    git(&project, &["init", "-q"]);
    git(&project, &["remote", "add", "origin", REPO]);
    let local = project.to_str().unwrap().to_string();

    let conns = Arc::new(Connections::new(scratch.path().join("machines")));
    let store = Arc::new(Mutex::new(
        Store::open(&scratch.path().join("sessions.db")).unwrap(),
    ));
    let cloud = CloudSessions::new(Arc::new(SystemGh), Arc::new(conns.clone()), store.clone())
        .with_system_sandboxes(store.clone());
    let config = DockerProjectConfig {
        repo_url: REPO.into(),
        cpus: 2,
        memory: "4g".into(),
        ttl: "30m".into(),
        ..Default::default()
    };
    docker_config::save(&store.lock().unwrap(), &local, &config).unwrap();
    let shell = SystemShell::default();
    let sbx = SystemSbx;
    let never = AtomicBool::new(false);

    // 1. Start.
    let (session, record) = cloud
        .begin_sandbox_start(&local, &config, "e2e-run", "claude")
        .unwrap_or_else(|e| panic!("begin_sandbox_start: {e}"));
    let name = record.sandbox.clone().expect("a sandbox start has a name");
    assert!(name.starts_with("monoptah-"), "{name}");
    let _cleanup = Cleanup { name: name.clone() };
    step(&format!("begun: {name}"));

    let mut seen: Vec<(StepKind, StepStatus)> = Vec::new();
    let mut last = record.clone();
    let binding = cloud
        .run_start(
            session.id,
            &mut |event| match event {
                StartEvent::Updated(r) | StartEvent::Failed(r) => {
                    for s in &r.steps {
                        if seen.last() != Some(&(s.kind, s.status)) {
                            step(&format!("step {:?} -> {:?}", s.kind, s.status));
                        }
                        if !seen.contains(&(s.kind, s.status)) {
                            seen.push((s.kind, s.status));
                        }
                    }
                    last = r;
                }
                StartEvent::Log(line) => step(&format!("log: {line}")),
                StartEvent::Ready { .. } => step("Ready"),
            },
            &never,
        )
        .unwrap_or_else(|e| panic!("run_start: {e}"));
    let start = last.sandbox_start.clone().expect("sandbox start data");
    let id = start.sandbox_id.clone().expect("sandbox id");
    let container = start.container.clone().expect("container");
    let workspace = start.workspace.clone().expect("workspace");
    step(&format!("ready: id={id} container={}", container.id));

    let done = |kind: StepKind| seen.contains(&(kind, StepStatus::Done));
    for kind in [
        StepKind::Create,
        StepKind::Provision,
        StepKind::BuildContainer,
        StepKind::ConfigureCodespace,
        StepKind::Connect,
        StepKind::OpenProject,
    ] {
        assert!(done(kind), "step {kind:?} never finished: {seen:?}");
    }
    assert!(
        done(StepKind::BlockingCommands)
            || seen.contains(&(StepKind::BlockingCommands, StepStatus::Skipped)),
        "waitFor step neither done nor skipped: {seen:?}"
    );
    let first_running = |kind: StepKind| seen.iter().position(|e| e.0 == kind);
    let order = [
        StepKind::Create,
        StepKind::Provision,
        StepKind::BuildContainer,
        StepKind::ConfigureCodespace,
        StepKind::Connect,
        StepKind::OpenProject,
    ];
    let positions: Vec<_> = order.iter().map(|k| first_running(*k).unwrap()).collect();
    assert!(
        positions.windows(2).all(|w| w[0] < w[1]),
        "step order: {seen:?}"
    );

    // 2. Host reachable through the relay tunnel.
    let describe = conns
        .request(
            &binding.machine_id,
            "environment.describe",
            json!({"supportedProviders":["claude","codex"]}),
        )
        .unwrap_or_else(|e| panic!("environment.describe: {e}"));
    assert_eq!(describe["protocolVersion"], 1);
    let sessions = conns
        .request(
            &binding.machine_id,
            "sessions.list",
            json!({"projectId": binding.project_id}),
        )
        .unwrap();
    assert!(sessions.is_array(), "{sessions}");
    step("host reachable");

    // 3. Security checks, user-initiated, in the outer sandbox and in the container.
    let cid = &container.id;
    let probe = format!(
        "env | grep -c -E '^(GITHUB_TOKEN|GH_TOKEN|GIT_TOKEN|ANTHROPIC_API_KEY|ANTHROPIC_AUTH_TOKEN|CLAUDE_CODE_OAUTH_TOKEN)=' || true\n\
         docker exec {cid} sh -c \"env | grep -c -E '^(GITHUB_TOKEN|GH_TOKEN|GIT_TOKEN|ANTHROPIC_API_KEY|ANTHROPIC_AUTH_TOKEN|CLAUDE_CODE_OAUTH_TOKEN)=' || true\"\n\
         docker exec {cid} sh -c 'git config --global --get-regexp credential || true'\n\
         echo @@ps-outer\nps -eo args\necho @@ps-container\ndocker exec {cid} ps -eo args\n"
    );
    let report = outer(&shell, &id, &probe);
    let (head, ps) = report.split_once("@@ps-outer").unwrap();
    let counts: Vec<_> = head.lines().take(2).map(str::trim).collect();
    assert_eq!(
        counts,
        ["0", "0"],
        "credential variables present ({DUMMY_NOTE})"
    );
    assert!(
        !head.contains("password") && !head.contains("token"),
        "git config names a credential: {head}"
    );
    let secretish = regex_free_secret_like(ps);
    assert!(
        secretish.is_empty(),
        "secret-looking process args: {secretish:?}"
    );
    step("security checks passed");

    // 4. Marker file, stop, wait, reconnect.
    outer(
        &shell,
        &id,
        &format!("echo e2e-marker > {workspace}/E2E_MARKER.txt\n"),
    );
    cloud
        .stop_cloud(&CloudRef::sandbox(&name))
        .unwrap_or_else(|e| panic!("stop_cloud: {e}"));
    step("stop accepted");
    wait_for("state stopped", Duration::from_secs(300), || {
        let state = sandboxes::state(&sbx, &id)?;
        Ok(sandboxes::is_stopped(&state))
    });
    step("stopped");
    cloud
        .reconnect_sandbox(
            session.id,
            &mut |m| step(&format!("reconnect: {m}")),
            &never,
        )
        .unwrap_or_else(|e| panic!("reconnect_sandbox: {e}"));
    let machines = conns.machines().unwrap();
    let machine = machines
        .iter()
        .find(|m| m.sandbox.as_ref().is_some_and(|s| s.name == name))
        .expect("a machine for the sandbox after reconnect");
    conns
        .request(&machine.id, "environment.describe", json!({}))
        .unwrap_or_else(|e| panic!("describe after reconnect: {e}"));
    let marker = outer(&shell, &id, &format!("cat {workspace}/E2E_MARKER.txt\n"));
    assert_eq!(marker.trim(), "e2e-marker");
    step("reconnected, marker survived");

    // 5. Archive, with an uncommitted file that must land in the patch.
    outer(
        &shell,
        &id,
        &format!("echo uncommitted-e2e > {workspace}/E2E_UNCOMMITTED.txt\n"),
    );
    let converted = cloud
        .convert_session(session.id, &mut |m| step(&format!("convert: {m}")))
        .unwrap_or_else(|e| panic!("convert_session: {e}"));
    let code_dir = converted.code_dir.clone().expect("code dir");
    assert!(code_dir.join("manifest.json").is_file(), "{code_dir:?}");
    let patch = std::fs::read_to_string(code_dir.join("changes.patch")).unwrap_or_default();
    assert!(patch.contains("E2E_UNCOMMITTED.txt"), "patch: {patch}");
    step(&format!("archived as local session {}", converted.local_id));

    // 6. Delete.
    let outcome = cloud
        .delete_cloud_outcome(&CloudRef::sandbox(&name))
        .unwrap_or_else(|e| panic!("delete_cloud_outcome: {e}"));
    assert!(matches!(
        outcome,
        DeleteOutcome::Deleted | DeleteOutcome::DeletedWithWarning(_)
    ));
    let gone = sandboxes::inspect(&sbx, &id);
    let backend = monocode_core::cloud_sessions::backend::DockerBackend::new(Arc::new(SystemSbx));
    assert!(
        gone.as_ref()
            .err()
            .is_some_and(|e| backend.is_missing_error(e)),
        "sandbox still inspectable: {gone:?}"
    );
    assert_eq!(backend.kind(), CloudBackendKind::DockerSandbox);
    step("deleted");
    conns.shutdown();
}

/// Lines of `ps` output that carry something shaped like a token or key.
fn regex_free_secret_like(ps: &str) -> Vec<String> {
    ps.lines()
        .filter(|line| {
            let l = line.to_ascii_lowercase();
            l.contains("sk-ant-")
                || l.contains("ghp_")
                || l.contains("github_pat_")
                || l.contains("glpat-")
                || l.contains("oauth_token=")
                || l.contains("password=")
                || l.contains("api_key=")
        })
        .map(|l| l.chars().take(80).collect())
        .collect()
}
