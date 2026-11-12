use super::*;
use crate::codespaces::GhOutput;
use std::sync::Mutex;

const AUTH_OK: &str = "github.com\n  ✓ Logged in to github.com account me (keyring)\n  - Token scopes: 'codespace', 'repo'\n";
const AUTH_NO_SCOPE: &str =
    "github.com\n  ✓ Logged in to github.com account me (keyring)\n  - Token scopes: 'repo'\n";

/// Replies by the first rule whose words are all in the joined argv.
struct FakeGh {
    rules: Mutex<Vec<(String, Result<GhOutput, String>)>>,
    calls: Mutex<Vec<String>>,
}
impl FakeGh {
    fn new(rules: Vec<(&str, Result<GhOutput, String>)>) -> Arc<Self> {
        Arc::new(Self {
            rules: Mutex::new(rules.into_iter().map(|(k, v)| (k.to_string(), v)).collect()),
            calls: Mutex::new(vec![]),
        })
    }
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}
impl GhRunner for FakeGh {
    fn run(&self, args: &[String], _stdin: Option<&str>) -> Result<GhOutput, String> {
        let line = args.join(" ");
        self.calls.lock().unwrap().push(line.clone());
        // Evidence folder setup runs after every prepare; it is best effort.
        if line.starts_with("codespace ssh") {
            return ok("");
        }
        self.rules
            .lock()
            .unwrap()
            .iter()
            .find(|(key, _)| line.contains(key.as_str()))
            .map(|(_, reply)| reply.clone())
            .unwrap_or_else(|| panic!("unexpected gh call: {line}"))
    }
}
fn ok(stdout: &str) -> Result<GhOutput, String> {
    Ok(GhOutput {
        success: true,
        stdout: stdout.into(),
        stderr: String::new(),
    })
}
fn fail(stderr: &str) -> Result<GhOutput, String> {
    Ok(GhOutput {
        success: false,
        stdout: String::new(),
        stderr: stderr.into(),
    })
}

#[derive(Default)]
struct FakeHost {
    machines: Mutex<Vec<Machine>>,
    log: Mutex<Vec<String>>,
    request_error: Mutex<Option<String>>,
}
impl FakeHost {
    fn with_machine(name: &str) -> Arc<Self> {
        let host = Arc::new(Self::default());
        host.machines.lock().unwrap().push(machine(name));
        host
    }
    fn log(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
}
fn machine(codespace: &str) -> Machine {
    Machine {
        id: format!("m-{codespace}"),
        name: codespace.into(),
        endpoint: format!("codespace://{codespace}"),
        environment_id: format!("env-{codespace}"),
        ssh: None,
        codespace: Some(codespace.into()),
        sandbox: None,
    }
}
impl Host for FakeHost {
    fn machines(&self) -> Result<Vec<Machine>, String> {
        Ok(self.machines.lock().unwrap().clone())
    }
    fn connect(
        &self,
        codespace: &str,
        display: &str,
        _: &AtomicBool,
        progress: &mut dyn FnMut(&str),
    ) -> Result<Machine, String> {
        progress("pairing");
        self.log
            .lock()
            .unwrap()
            .push(format!("connect {codespace} {display}"));
        let m = machine(codespace);
        self.machines.lock().unwrap().push(m.clone());
        Ok(m)
    }
    fn ensure(
        &self,
        machine_id: &str,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(&str),
    ) -> Result<(), String> {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(remote::WAKE_CANCELLED.into());
        }
        progress("Waking codespace…");
        self.log
            .lock()
            .unwrap()
            .push(format!("ensure {machine_id}"));
        Ok(())
    }
    fn request(&self, machine_id: &str, method: &str, params: Value) -> Result<Value, String> {
        self.log
            .lock()
            .unwrap()
            .push(format!("request {machine_id} {method} {}", params["cwd"]));
        if let Some(error) = self.request_error.lock().unwrap().clone() {
            return Err(error);
        }
        Ok(json!({"id":"project-1","cwd":params["cwd"]}))
    }
    fn disconnect(&self, machine_id: &str) -> Result<(), String> {
        self.log
            .lock()
            .unwrap()
            .push(format!("disconnect {machine_id}"));
        self.machines.lock().unwrap().retain(|m| m.id != machine_id);
        Ok(())
    }
}

fn service(gh: &Arc<FakeGh>, host: &Arc<FakeHost>) -> CloudSessions {
    let store = Arc::new(Mutex::new(Store::open(Path::new(":memory:")).unwrap()));
    CloudSessions::new(gh.clone(), host.clone(), store)
        .with_origin(|_| Ok("git@github.com:octo/widgets.git".into()))
}
const LIST: &str =
    r#"[{"name":"octo-abc","displayName":"my cs","state":"Shutdown","repository":"octo/widgets"}]"#;

fn named(name: &str) -> Target {
    Target {
        codespace: CodespaceChoice::Named(name.into()),
        branch: None,
        machine: None,
    }
}
use std::path::Path;

#[test]
fn named_target_reuses_machine_wakes_it_opens_the_project_and_records_the_binding() {
    let gh = FakeGh::new(vec![
        ("auth status", ok(AUTH_OK)),
        ("codespace list", ok(LIST)),
    ]);
    let host = FakeHost::with_machine("octo-abc");
    let svc = service(&gh, &host);
    let binding = svc
        .prepare_target("/Users/me/widgets", named("octo-abc"), &mut |_| {})
        .unwrap();
    assert_eq!(binding.repo, "octo/widgets");
    assert_eq!(binding.codespace, "octo-abc");
    assert_eq!(
        binding.project_key,
        "remote://env-octo-abc/workspaces/widgets"
    );
    assert_eq!(binding.remote_cwd, "/workspaces/widgets");
    // Reused machine: wake it, never pair again.
    assert_eq!(
        host.log(),
        vec![
            "ensure m-octo-abc".to_string(),
            "request m-octo-abc projects.open \"/workspaces/widgets\"".to_string()
        ]
    );
    // The binding links the remote project back to the local one.
    let session = svc.create_session(&binding, "claude").unwrap();
    assert_eq!(session.cwd, binding.project_key);
    let found = svc.cloud_sessions_for("/Users/me/widgets").unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].summary.id, session.id);
    assert_eq!(found[0].codespace, "octo-abc");
    assert!(svc
        .cloud_sessions_for("/Users/me/other")
        .unwrap()
        .is_empty());
}

#[test]
fn new_target_creates_waits_pairs_and_skips_the_extra_wake() {
    let gh = FakeGh::new(vec![
        ("auth status", ok(AUTH_OK)),
        (
            "--method POST",
            ok(r#"{"name":"octo-new1","state":"Provisioning"}"#),
        ),
        ("codespace view", ok(r#"{"state":"Available"}"#)),
    ]);
    let host = Arc::new(FakeHost::default());
    let svc = service(&gh, &host);
    let mut progress = vec![];
    let binding = svc
        .prepare_target(
            "/Users/me/widgets",
            Target {
                codespace: CodespaceChoice::New,
                branch: Some("main".into()),
                machine: None,
            },
            &mut |line| progress.push(line.to_string()),
        )
        .unwrap();
    assert_eq!(binding.codespace, "octo-new1");
    assert!(gh.calls().iter().any(|c| c.contains("ref=main")));
    assert_eq!(host.log()[0], "connect octo-new1 monoptah-main".to_string());
    assert!(!host.log().iter().any(|l| l.starts_with("ensure")));
    assert!(progress.iter().any(|p| p == "pairing"), "{progress:?}");
}

#[test]
fn bad_targets_fail_before_touching_codespaces() {
    let host = Arc::new(FakeHost::default());
    // Missing scope: actionable, nothing else runs.
    let gh = FakeGh::new(vec![("auth status", ok(AUTH_NO_SCOPE))]);
    let error = service(&gh, &host)
        .prepare_target("/p", named("octo-abc"), &mut |_| {})
        .unwrap_err();
    assert!(error.contains("gh auth refresh"), "{error}");
    assert_eq!(gh.calls().len(), 1);
    // New needs a branch.
    let gh = FakeGh::new(vec![("auth status", ok(AUTH_OK))]);
    let error = service(&gh, &host)
        .prepare_target(
            "/p",
            Target {
                codespace: CodespaceChoice::New,
                branch: Some("  ".into()),
                machine: None,
            },
            &mut |_| {},
        )
        .unwrap_err();
    assert!(error.contains("branch"), "{error}");
    // A codespace of another repository is refused.
    let gh = FakeGh::new(vec![
        ("auth status", ok(AUTH_OK)),
        ("codespace list", ok("[]")),
    ]);
    let error = service(&gh, &host)
        .prepare_target("/p", named("octo-abc"), &mut |_| {})
        .unwrap_err();
    assert!(error.contains("does not belong"), "{error}");
    assert!(host.log().is_empty());
    // No GitHub origin.
    let gh = FakeGh::new(vec![("auth status", ok(AUTH_OK))]);
    let svc = service(&gh, &host).with_origin(|_| Err("no origin".into()));
    assert_eq!(
        svc.prepare_target("/p", named("x"), &mut |_| {})
            .unwrap_err(),
        "no origin"
    );
}

#[test]
fn only_claude_sessions_can_be_created() {
    let gh = FakeGh::new(vec![
        ("auth status", ok(AUTH_OK)),
        ("codespace list", ok(LIST)),
    ]);
    let host = FakeHost::with_machine("octo-abc");
    let svc = service(&gh, &host);
    let binding = svc
        .prepare_target("/p", named("octo-abc"), &mut |_| {})
        .unwrap();
    let error = svc.create_session(&binding, "codex").unwrap_err();
    assert!(error.contains("Claude only"), "{error}");
    assert!(svc.cloud_sessions_for("/p").unwrap().is_empty());
}

#[test]
fn state_is_cached_never_errors_and_maps_gh_states() {
    let gh = FakeGh::new(vec![("codespace view", ok(r#"{"state":"Shutdown"}"#))]);
    let svc = service(&gh, &Arc::new(FakeHost::default()));
    assert_eq!(svc.cloud_state("octo-abc"), CloudState::Stopped);
    assert_eq!(svc.codespace_state("octo-abc").unwrap(), "Shutdown");
    assert_eq!(gh.calls().len(), 1, "second read must hit the cache");
    svc.invalidate("octo-abc");
    svc.cloud_state("octo-abc");
    assert_eq!(gh.calls().len(), 2);
    // A failing lookup is a state, not an error.
    let gh = FakeGh::new(vec![("codespace view", fail("boom"))]);
    let svc = service(&gh, &Arc::new(FakeHost::default()));
    assert_eq!(svc.cloud_state("octo-abc"), CloudState::Unknown);
    for (raw, state) in [
        ("Available", CloudState::Available),
        ("Starting", CloudState::Starting),
        ("Provisioning", CloudState::Starting),
        ("ShuttingDown", CloudState::Stopping),
        ("Failed", CloudState::Unavailable),
        ("???", CloudState::Unknown),
    ] {
        assert_eq!(CloudState::from_gh(raw), state);
    }
}

#[test]
fn state_cache_expires_after_the_ttl() {
    let gh = FakeGh::new(vec![("codespace view", ok(r#"{"state":"Available"}"#))]);
    let mut svc = service(&gh, &Arc::new(FakeHost::default()));
    svc.ttl = Duration::ZERO;
    svc.cloud_state("octo-abc");
    svc.cloud_state("octo-abc");
    assert_eq!(gh.calls().len(), 2);
}

fn cloud_session(provider: &str, environment: &str, codespace: Option<&str>) -> RemoteSession {
    let store = Store::open(Path::new(":memory:")).unwrap();
    let mut project = store
        .remember_remote_project(environment, &json!({"id":"p","cwd":"/workspaces/widgets"}))
        .unwrap();
    project.codespace = codespace.map(str::to_string);
    let summary = store.create("claude", &project.key).unwrap();
    let mut session = store.remote_session(summary.id).unwrap().unwrap();
    session.project = project;
    session.summary.provider = provider.into();
    session
}

fn ensure(host: &FakeHost, session: &RemoteSession) -> Result<Vec<String>, String> {
    let mut lines = vec![];
    ensure_before_send(host, session, &AtomicBool::new(false), &mut |l| {
        lines.push(l.to_string())
    })
    .map(|_| lines)
}

#[test]
fn ensure_before_send_wakes_only_cloud_sessions_and_only_for_claude() {
    let host = FakeHost::with_machine("octo-abc");
    // Ordinary remote session: untouched, no progress.
    let lines = ensure(&host, &cloud_session("codex", "env-octo-abc", None)).unwrap();
    assert!(lines.is_empty() && host.log().is_empty());
    // Cloud + claude: wakes the matching machine and reports progress.
    let lines = ensure(
        &host,
        &cloud_session("claude", "env-octo-abc", Some("octo-abc")),
    )
    .unwrap();
    assert_eq!(host.log(), vec!["ensure m-octo-abc".to_string()]);
    assert_eq!(lines, vec!["Waking codespace…".to_string()]);
    // Cloud + other harness: rejected before any wake.
    let error = ensure(
        &host,
        &cloud_session("codex", "env-octo-abc", Some("octo-abc")),
    )
    .unwrap_err();
    assert!(error.contains("Claude only"), "{error}");
    // Machine forgotten: clear error, no wake.
    let error = ensure(&host, &cloud_session("claude", "env-gone", Some("gone"))).unwrap_err();
    assert!(error.contains("no longer connected"), "{error}");
    assert_eq!(host.log().len(), 1);
}

#[test]
fn ensure_before_send_honours_cancel() {
    let host = FakeHost::with_machine("octo-abc");
    let cancel = AtomicBool::new(true);
    let error = ensure_before_send(
        &*host,
        &cloud_session("claude", "env-octo-abc", Some("octo-abc")),
        &cancel,
        &mut |_| {},
    )
    .unwrap_err();
    assert_eq!(error, remote::WAKE_CANCELLED);
    assert!(host.log().is_empty());
}

#[test]
fn session_target_uses_the_current_binding_and_state() {
    let gh = FakeGh::new(vec![
        ("auth status", ok(AUTH_OK)),
        ("codespace list", ok(LIST)),
        ("codespace view", ok(r#"{"state":"Shutdown"}"#)),
    ]);
    let host = FakeHost::with_machine("octo-abc");
    let svc = service(&gh, &host);
    // A plain remote session on the same project key, created before binding.
    let plain = svc
        .store()
        .unwrap()
        .remember_remote_project(
            "env-octo-abc",
            &json!({"id":"project-1","cwd":"/workspaces/widgets"}),
        )
        .unwrap();
    let early = svc.store().unwrap().create("claude", &plain.key).unwrap();
    assert_eq!(svc.target_for_session(early.id), None);
    let binding = svc
        .prepare_target("/p", named("octo-abc"), &mut |_| {})
        .unwrap();
    let target = svc.target_for_session(early.id).unwrap();
    assert_eq!(target.codespace, "octo-abc");
    assert_eq!(target.state, CloudState::Stopped);
    // The run hook resolves the same binding for that early session.
    let stored = svc
        .store()
        .unwrap()
        .remote_session(early.id)
        .unwrap()
        .unwrap();
    ensure(&host, &stored).unwrap();
    assert!(
        host.log()
            .iter()
            .filter(|l| l.starts_with("ensure"))
            .count()
            >= 2
    );
    // Delete forgets it too.
    gh.rules
        .lock()
        .unwrap()
        .push(("codespace delete".to_string(), ok("")));
    svc.delete_codespace("octo-abc").unwrap();
    assert_eq!(svc.target_for_session(early.id), None);
    let _ = binding;
}

#[test]
fn unavailable_named_codespace_is_a_clear_error() {
    let gh = FakeGh::new(vec![
        ("auth status", ok(AUTH_OK)),
        (
            "codespace list",
            ok(r#"[{"name":"octo-abc","displayName":"x","state":"Failed"}]"#),
        ),
    ]);
    let error = service(&gh, &Arc::new(FakeHost::default()))
        .prepare_target("/p", named("octo-abc"), &mut |_| {})
        .unwrap_err();
    assert!(error.contains("unavailable"), "{error}");
}

#[test]
fn delete_forgets_bindings_even_when_disconnect_fails() {
    struct Failing(FakeHost);
    impl Host for Failing {
        fn machines(&self) -> Result<Vec<Machine>, String> {
            self.0.machines()
        }
        fn connect(
            &self,
            c: &str,
            d: &str,
            h: &AtomicBool,
            p: &mut dyn FnMut(&str),
        ) -> Result<Machine, String> {
            self.0.connect(c, d, h, p)
        }
        fn ensure(&self, m: &str, c: &AtomicBool, p: &mut dyn FnMut(&str)) -> Result<(), String> {
            self.0.ensure(m, c, p)
        }
        fn request(&self, m: &str, method: &str, params: Value) -> Result<Value, String> {
            self.0.request(m, method, params)
        }
        fn disconnect(&self, _: &str) -> Result<(), String> {
            Err("boom".into())
        }
    }
    let gh = FakeGh::new(vec![
        ("auth status", ok(AUTH_OK)),
        ("codespace list", ok(LIST)),
        ("codespace delete", ok("")),
    ]);
    let inner = FakeHost::default();
    inner.machines.lock().unwrap().push(machine("octo-abc"));
    let store = Arc::new(Mutex::new(Store::open(Path::new(":memory:")).unwrap()));
    let svc = CloudSessions::new(gh, Arc::new(Failing(inner)), store)
        .with_origin(|_| Ok("git@github.com:octo/widgets.git".into()));
    let binding = svc
        .prepare_target("/p", named("octo-abc"), &mut |_| {})
        .unwrap();
    svc.create_session(&binding, "claude").unwrap();
    let error = svc.delete_codespace("octo-abc").unwrap_err();
    assert!(error.contains("disconnecting failed"), "{error}");
    assert!(svc.cloud_sessions_for("/p").unwrap().is_empty());
}

#[test]
fn new_codespaces_get_the_monoptah_display_name() {
    let gh = FakeGh::new(vec![
        ("auth status", ok(AUTH_OK)),
        (
            "--method POST",
            ok(r#"{"name":"octo-new1","state":"Provisioning"}"#),
        ),
        ("codespace view", ok(r#"{"state":"Available"}"#)),
    ]);
    let svc = service(&gh, &Arc::new(FakeHost::default()));
    svc.prepare_target(
        "/p",
        Target {
            codespace: CodespaceChoice::New,
            branch: Some("main".into()),
            machine: None,
        },
        &mut |_| {},
    )
    .unwrap();
    assert!(gh
        .calls()
        .iter()
        .any(|c| c.contains("display_name=monoptah-main")));
}

#[test]
fn delete_disconnects_and_forgets_but_a_failed_delete_keeps_everything() {
    let gh = FakeGh::new(vec![
        ("auth status", ok(AUTH_OK)),
        ("codespace list", ok(LIST)),
        ("codespace delete", fail("HTTP 500")),
    ]);
    let host = FakeHost::with_machine("octo-abc");
    let svc = service(&gh, &host);
    let binding = svc
        .prepare_target("/p", named("octo-abc"), &mut |_| {})
        .unwrap();
    svc.create_session(&binding, "claude").unwrap();
    assert!(svc.delete_codespace("octo-abc").is_err());
    assert_eq!(svc.cloud_sessions_for("/p").unwrap().len(), 1);
    assert!(!host.log().iter().any(|l| l.starts_with("disconnect")));

    gh.rules.lock().unwrap()[2].1 = ok("");
    svc.delete_codespace("octo-abc").unwrap();
    assert!(host.log().contains(&"disconnect m-octo-abc".to_string()));
    assert!(svc.cloud_sessions_for("/p").unwrap().is_empty());
    assert!(gh.calls().iter().any(|c| c.contains("--force")));
}

#[test]
fn stop_runs_gh_and_drops_the_cached_state() {
    let gh = FakeGh::new(vec![
        ("codespace view", ok(r#"{"state":"Available"}"#)),
        ("codespace stop", ok("")),
    ]);
    let svc = service(&gh, &Arc::new(FakeHost::default()));
    assert_eq!(svc.cloud_state("octo-abc"), CloudState::Available);
    svc.stop_codespace("octo-abc").unwrap();
    svc.cloud_state("octo-abc");
    let calls = gh.calls();
    assert_eq!(calls.len(), 3, "{calls:?}");
    assert!(calls[1].contains("codespace stop"));
}

#[test]
fn not_connected_errors_are_recognised_as_a_state() {
    assert!(is_not_connected(remote::CODESPACE_NOT_CONNECTED));
    assert!(!is_not_connected("Machine is unreachable"));
}

/// Fakes `gh codespace cp`: writes `files` (relative paths) under `<dest>/<basename>`.
struct CpGh {
    files: Vec<&'static str>,
    missing: bool,
    /// Copies the files, then fails like scp does for a dangling link inside the folder.
    nested_failure: bool,
    calls: Mutex<Vec<String>>,
}
impl GhRunner for CpGh {
    fn run(&self, args: &[String], _stdin: Option<&str>) -> Result<GhOutput, String> {
        let line = args.join(" ");
        self.calls.lock().unwrap().push(line.clone());
        if args.get(..2) != Some(&["codespace".to_string(), "cp".to_string()]) {
            panic!("unexpected gh call: {line}");
        }
        if self.missing {
            return fail("scp: /workspaces/widgets/.evidence: No such file or directory");
        }
        let remote = args[args.len() - 2].strip_prefix("remote:").unwrap();
        let dest = Path::new(args.last().unwrap());
        let root = dest.join(remote.rsplit('/').next().unwrap());
        for file in &self.files {
            let path = root.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, format!("{remote}/{file}")).unwrap();
        }
        if self.nested_failure {
            return fail(&format!(
                "scp: '{remote}/latest.png': No such file or directory"
            ));
        }
        ok("")
    }
}

fn evidence_service(gh: Arc<CpGh>, dir: &Path) -> (CloudSessions, i64) {
    let store = Arc::new(Mutex::new(Store::open(&dir.join("sessions.db")).unwrap()));
    let host = FakeHost::with_machine("octo-abc");
    let svc = CloudSessions::new(gh, host, store.clone())
        .with_origin(|_| Ok("git@github.com:octo/widgets.git".into()));
    // Bind a project by hand: no gh needed.
    let project = store
        .lock()
        .unwrap()
        .remember_cloud_project(
            "env-octo-abc",
            &json!({"id":"project-1","cwd":"/workspaces/widgets"}),
            "/Users/me/widgets",
            "octo-abc",
        )
        .unwrap();
    let session = store
        .lock()
        .unwrap()
        .create("claude", &project.key)
        .unwrap();
    (svc, session.id)
}

#[test]
fn pull_evidence_copies_the_default_folder_into_the_session_folder() {
    let dir = tempfile::tempdir().unwrap();
    let gh = Arc::new(CpGh {
        files: vec!["shot.png", "sub/report.md"],
        missing: false,
        nested_failure: false,
        calls: Mutex::new(vec![]),
    });
    let (svc, id) = evidence_service(gh.clone(), dir.path());
    let pull = svc.pull_evidence(id, None).unwrap();
    assert_eq!(pull.instance, "octo-abc");
    assert_eq!(pull.remote_path, "/workspaces/widgets/.evidence");
    assert_eq!(pull.files.len(), 2, "{pull:?}");
    assert!(pull.files.iter().all(|f| f.starts_with(&pull.local_dir)));
    assert_eq!(
        std::fs::read_to_string(pull.local_dir.join("sub/report.md")).unwrap(),
        "/workspaces/widgets/.evidence/sub/report.md"
    );
    assert!(pull.message.is_none());
    let call = gh.calls.lock().unwrap()[0].clone();
    assert!(
        call.contains("-c octo-abc remote:/workspaces/widgets/.evidence "),
        "{call}"
    );
    // No staging folder is left behind.
    let leftovers: Vec<_> = std::fs::read_dir(pull.local_dir.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(leftovers, vec!["evidence".to_string()]);
}

#[test]
fn pulling_again_refreshes_files_and_keeps_older_ones() {
    let dir = tempfile::tempdir().unwrap();
    let first = Arc::new(CpGh {
        files: vec!["old.png", "same.txt"],
        missing: false,
        nested_failure: false,
        calls: Mutex::new(vec![]),
    });
    let (svc, id) = evidence_service(first, dir.path());
    let pull = svc.pull_evidence(id, None).unwrap();
    std::fs::write(pull.local_dir.join("same.txt"), "stale").unwrap();
    let svc = CloudSessions::new(
        Arc::new(CpGh {
            files: vec!["same.txt"],
            missing: false,
            nested_failure: false,
            calls: Mutex::new(vec![]),
        }),
        FakeHost::with_machine("octo-abc"),
        svc.store.clone(),
    );
    svc.pull_evidence(id, None).unwrap();
    assert!(pull.local_dir.join("old.png").exists());
    assert_eq!(
        std::fs::read_to_string(pull.local_dir.join("same.txt")).unwrap(),
        "/workspaces/widgets/.evidence/same.txt"
    );
}

#[test]
fn a_missing_evidence_folder_is_an_empty_pull_with_a_message() {
    let dir = tempfile::tempdir().unwrap();
    let gh = Arc::new(CpGh {
        files: vec![],
        missing: true,
        nested_failure: false,
        calls: Mutex::new(vec![]),
    });
    let (svc, id) = evidence_service(gh, dir.path());
    let pull = svc.pull_evidence(id, None).unwrap();
    assert!(pull.files.is_empty());
    assert!(pull.message.unwrap().contains("No evidence files found"));
}

#[test]
fn a_custom_path_is_resolved_against_the_workspace_and_confined_to_it() {
    let dir = tempfile::tempdir().unwrap();
    let gh = Arc::new(CpGh {
        files: vec!["a.png"],
        missing: false,
        nested_failure: false,
        calls: Mutex::new(vec![]),
    });
    let (svc, id) = evidence_service(gh.clone(), dir.path());
    let pull = svc.pull_evidence(id, Some("out/shots/")).unwrap();
    assert_eq!(pull.remote_path, "/workspaces/widgets/out/shots");
    assert!(pull.local_dir.join("shots/a.png").exists());
    let pull = svc
        .pull_evidence(id, Some("/workspaces/widgets/report.pdf"))
        .unwrap();
    assert!(pull.local_dir.join("report.pdf/a.png").exists());
    for bad in [
        "../etc",
        "/etc/passwd",
        "/workspaces/other/x",
        "/workspaces/widgets",
        "/workspaces/widgets-evil/x",
        "-r",
        "",
        ".",
        "./",
        "/workspaces/widgets/.",
        "/workspaces/widgets/./",
    ] {
        assert!(svc.pull_evidence(id, Some(bad)).is_err(), "{bad:?}");
    }
    assert_eq!(gh.calls.lock().unwrap().len(), 2);
}

#[test]
fn local_and_unknown_sessions_cannot_pull_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let gh = Arc::new(CpGh {
        files: vec![],
        missing: false,
        nested_failure: false,
        calls: Mutex::new(vec![]),
    });
    let (svc, _) = evidence_service(gh.clone(), dir.path());
    assert!(svc.pull_evidence(-999_999, None).is_err());
    assert!(gh.calls.lock().unwrap().is_empty());
}

#[test]
fn a_failure_inside_the_folder_keeps_the_copied_files_and_warns() {
    let dir = tempfile::tempdir().unwrap();
    let gh = Arc::new(CpGh {
        files: vec!["a.png", "b.md"],
        missing: false,
        nested_failure: true,
        calls: Mutex::new(vec![]),
    });
    let (svc, id) = evidence_service(gh, dir.path());
    let pull = svc.pull_evidence(id, None).unwrap();
    assert_eq!(pull.files.len(), 2, "{pull:?}");
    assert!(pull.local_dir.join("a.png").exists());
    assert!(pull.message.is_none());
    assert!(pull.warning.unwrap().contains("latest.png"));
}

#[test]
fn a_hard_failure_with_nothing_copied_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let gh = Arc::new(CpGh {
        files: vec![],
        missing: false,
        nested_failure: true,
        calls: Mutex::new(vec![]),
    });
    let (svc, id) = evidence_service(gh, dir.path());
    assert!(svc
        .pull_evidence(id, None)
        .unwrap_err()
        .contains("latest.png"));
}

#[test]
fn an_unreadable_name_collision_is_reported_and_other_files_still_arrive() {
    let dir = tempfile::tempdir().unwrap();
    let gh = Arc::new(CpGh {
        files: vec!["a.png", "clash"],
        missing: false,
        nested_failure: false,
        calls: Mutex::new(vec![]),
    });
    let (svc, id) = evidence_service(gh, dir.path());
    let first = svc.pull_evidence(id, None).unwrap();
    // A directory now sits where the next pull wants a file.
    std::fs::remove_file(first.local_dir.join("clash")).unwrap();
    std::fs::create_dir(first.local_dir.join("clash")).unwrap();
    std::fs::write(first.local_dir.join("clash/keep"), "x").unwrap();
    let again = svc.pull_evidence(id, None).unwrap();
    assert!(again.files.iter().any(|f| f.ends_with("a.png")));
    assert!(again.warning.unwrap().contains("clash"));
}

#[test]
fn a_failing_evidence_setup_never_blocks_the_session_target() {
    struct SshFails(FakeGh2);
    type FakeGh2 = Arc<FakeGh>;
    impl GhRunner for SshFails {
        fn run(&self, args: &[String], stdin: Option<&str>) -> Result<GhOutput, String> {
            if args.get(1).map(String::as_str) == Some("ssh") {
                return Err("ssh stalled".into());
            }
            self.0.run(args, stdin)
        }
    }
    let inner = FakeGh::new(vec![
        ("auth status", ok(AUTH_OK)),
        ("codespace list", ok(LIST)),
    ]);
    let host = FakeHost::with_machine("octo-abc");
    let store = Arc::new(Mutex::new(Store::open(Path::new(":memory:")).unwrap()));
    let svc = CloudSessions::new(Arc::new(SshFails(inner)), host, store)
        .with_origin(|_| Ok("git@github.com:octo/widgets.git".into()));
    let mut progress = vec![];
    let binding = svc
        .prepare_target("/Users/me/widgets", named("octo-abc"), &mut |p| {
            progress.push(p.to_string())
        })
        .unwrap();
    assert_eq!(binding.codespace, "octo-abc");
    assert!(progress
        .iter()
        .any(|p| p.contains("Could not prepare the evidence folder")));
}

#[test]
fn deleting_a_session_of_a_missing_codespace_only_removes_its_local_record() {
    let gh = FakeGh::new(vec![
        ("auth status", ok(AUTH_OK)),
        (
            "user/codespaces",
            ok(r#"[{"total_count":0,"codespaces":[]}]"#),
        ),
    ]);
    let host = FakeHost::with_machine("octo-abc");
    let store = Store::open(Path::new(":memory:")).unwrap();
    let project = store
        .remember_cloud_project(
            "env-octo-abc",
            &json!({"id":"project","cwd":"/workspaces/widgets"}),
            "/work/widgets",
            "octo-abc",
        )
        .unwrap();
    let summary = store.create("claude", &project.key).unwrap();
    let other = store.create("claude", &project.key).unwrap();
    let mut remote = store.remote_session(summary.id).unwrap().unwrap();
    remote.session_id = Some("host-session".into());
    store.save_remote_session(&remote).unwrap();

    store
        .delete_remote_session_with(summary.id, &*gh, &*host)
        .unwrap();

    assert!(store.remote_session(summary.id).unwrap().is_none());
    assert!(store.remote_session(other.id).unwrap().is_some());
    assert!(host.log().is_empty(), "no host operation: {:?}", host.log());
    assert_eq!(gh.calls().len(), 2, "authenticate and enumerate only");
}

fn deletion_fixture() -> (Store, i64) {
    let store = Store::open(Path::new(":memory:")).unwrap();
    let project = store
        .remember_cloud_project(
            "env-octo-abc",
            &json!({"id":"project","cwd":"/workspaces/widgets"}),
            "/work/widgets",
            "octo-abc",
        )
        .unwrap();
    let summary = store.create("claude", &project.key).unwrap();
    let mut remote = store.remote_session(summary.id).unwrap().unwrap();
    remote.session_id = Some("host-session".into());
    store.save_remote_session(&remote).unwrap();
    (store, summary.id)
}

#[test]
fn failed_codespace_lookups_preserve_the_session_without_contacting_the_host() {
    for response in [
        fail("HTTP 401: Bad credentials"),
        fail("HTTP 403: Resource not accessible"),
        fail("HTTP 404: Not Found"),
        fail("HTTP 429: Rate limit exceeded"),
        fail("HTTP 500: Internal Server Error"),
        Err("network timeout".into()),
        Err("gh: command not found".into()),
        ok("not json"),
        ok("[]"),
        ok(r#"[{"total_count":1,"codespaces":[]}]"#),
        ok(r#"[{"codespaces":[]}]"#),
        ok(r#"[{"total_count":2,"codespaces":[{"name":"other"},{"name":"other"}]}]"#),
    ] {
        let gh = FakeGh::new(vec![
            ("auth status", ok(AUTH_OK)),
            ("user/codespaces", response),
        ]);
        let host = FakeHost::with_machine("octo-abc");
        let (store, id) = deletion_fixture();
        assert!(store.delete_remote_session_with(id, &*gh, &*host).is_err());
        assert!(store.remote_session(id).unwrap().is_some());
        assert!(host.log().is_empty());
    }
    for response in [
        ok(AUTH_NO_SCOPE),
        fail("not logged in"),
        Err("auth timeout".into()),
    ] {
        let gh = FakeGh::new(vec![("auth status", response)]);
        let host = FakeHost::with_machine("octo-abc");
        let (store, id) = deletion_fixture();
        assert!(store.delete_remote_session_with(id, &*gh, &*host).is_err());
        assert!(store.remote_session(id).unwrap().is_some());
        assert!(host.log().is_empty());
    }
}

#[test]
fn live_codespace_deletion_still_requires_the_host_to_accept_the_delete() {
    let gh = FakeGh::new(vec![
        ("auth status", ok(AUTH_OK)),
        (
            "user/codespaces",
            ok(
                r#"[{"total_count":2,"codespaces":[{"name":"other"}]},{"total_count":2,"codespaces":[{"name":"octo-abc"}]}]"#,
            ),
        ),
    ]);
    let host = FakeHost::default();
    let (store, id) = deletion_fixture();
    assert!(store.delete_remote_session_with(id, &*gh, &host).is_err());
    assert!(
        store.remote_session(id).unwrap().is_some(),
        "disconnected live host must retain the row"
    );
    host.machines.lock().unwrap().push(machine("octo-abc"));
    *host.request_error.lock().unwrap() = Some("host API failed".into());
    assert!(store.delete_remote_session_with(id, &*gh, &host).is_err());
    assert!(store.remote_session(id).unwrap().is_some());
    *host.request_error.lock().unwrap() = None;
    store.delete_remote_session_with(id, &*gh, &host).unwrap();
    assert!(store.remote_session(id).unwrap().is_none());
    assert_eq!(host.log().len(), 2);
    assert!(host.log()[0].contains("sessions.delete"));
}

/// Records the calls of a backend registered as the Docker sandbox one.
#[derive(Default)]
struct FakeSandboxBackend {
    calls: Mutex<Vec<String>>,
}
impl FakeSandboxBackend {
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
    fn note(&self, call: String) {
        self.calls.lock().unwrap().push(call);
    }
}
impl CloudBackend for FakeSandboxBackend {
    fn kind(&self) -> CloudBackendKind {
        CloudBackendKind::DockerSandbox
    }
    fn display_state(&self, raw: &str) -> CloudState {
        match raw {
            "Available" => CloudState::Available,
            "Stopped" => CloudState::Stopped,
            _ => CloudState::Unknown,
        }
    }
    fn is_stopped(&self, raw: &str) -> bool {
        raw == "Stopped"
    }
    fn is_deleted_state(&self, raw: &str) -> bool {
        raw == "Removed"
    }
    fn is_missing_error(&self, error: &str) -> bool {
        error.contains("no such sandbox")
    }
    fn state(&self, name: &str) -> Result<String, String> {
        self.note(format!("state {name}"));
        Ok("Available".into())
    }
    fn exists(&self, name: &str) -> Result<bool, String> {
        self.note(format!("exists {name}"));
        Ok(true)
    }
    fn stop(&self, name: &str) -> Result<(), String> {
        self.note(format!("stop {name}"));
        Ok(())
    }
    fn delete(&self, name: &str) -> Result<(), String> {
        self.note(format!("delete {name}"));
        Ok(())
    }
    fn rebuild(&self, name: &str) -> Result<(), String> {
        self.note(format!("rebuild {name}"));
        Ok(())
    }
    fn web_url(&self, _name: &str) -> Option<Result<String, String>> {
        None
    }
    fn run_script(&self, name: &str, _script: &str, _long: bool) -> Result<String, String> {
        self.note(format!("run_script {name}"));
        Ok(String::new())
    }
    fn copy_from(&self, name: &str, remote: &str, _local: &Path) -> Result<(), String> {
        self.note(format!("copy_from {name} {remote}"));
        Ok(())
    }
    fn prepare_evidence(&self, name: &str, _workspace: &str) -> Result<(), String> {
        self.note(format!("prepare_evidence {name}"));
        Ok(())
    }
}

#[test]
fn a_sandbox_bound_project_is_stopped_and_deleted_through_its_own_backend() {
    // Any gh call would panic: FakeGh has no rule for it.
    let gh = FakeGh::new(vec![]);
    let host = FakeHost::with_machine("octo-abc");
    let sandbox = Arc::new(FakeSandboxBackend::default());
    let svc = service(&gh, &host).with_backend(sandbox.clone());
    let cloud = CloudRef::sandbox("sbx-1");
    let session = {
        let store = svc.store().unwrap();
        let project =
            crate::store::pending_cloud_project("/Users/me/widgets", &cloud, "/workspaces/widgets");
        assert_eq!(project.cloud(), Some(cloud.clone()));
        assert_eq!(project.codespace, None);
        store
            .create_pending_cloud_session("claude", project)
            .unwrap()
    };
    assert_eq!(svc.cloud_state_of(&cloud), CloudState::Available);
    assert_eq!(svc.sessions_in_cloud(&cloud), vec![session.id]);
    // The listing includes it, tagged with its backend so callers can tell it from a codespace.
    let listed = svc.cloud_sessions_for("/Users/me/widgets").unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].cloud, cloud);
    assert_eq!(listed[0].summary.id, session.id);

    svc.stop_cloud(&cloud).unwrap();
    assert_eq!(
        svc.delete_cloud_outcome(&cloud).unwrap(),
        DeleteOutcome::Deleted
    );
    assert!(svc.sessions_in_cloud(&cloud).is_empty());
    assert_eq!(
        sandbox.calls(),
        vec!["state sbx-1", "stop sbx-1", "delete sbx-1"]
    );
    assert_eq!(gh.calls(), Vec::<String>::new());
    // The sandbox namespace never touched the machine of the codespace with another name.
    assert_eq!(host.log(), Vec::<String>::new());
}

#[test]
fn state_and_gone_caches_keep_codespaces_and_sandboxes_apart() {
    let gh = FakeGh::new(vec![("codespace view", ok(r#"{"state":"Shutdown"}"#))]);
    let host = FakeHost::with_machine("octo-abc");
    let sandbox = Arc::new(FakeSandboxBackend::default());
    let svc = service(&gh, &host).with_backend(sandbox.clone());
    // Same name, two backends: each answers for itself.
    assert_eq!(svc.cloud_state("same"), CloudState::Stopped);
    assert_eq!(
        svc.cloud_state_of(&CloudRef::sandbox("same")),
        CloudState::Available
    );
    svc.mark_gone(&CloudRef::sandbox("same"));
    assert!(svc.cloud_gone(&CloudRef::sandbox("same")));
    assert!(!svc.codespace_gone("same"));
}

#[test]
fn an_unregistered_backend_is_an_error_not_a_panic() {
    let gh = FakeGh::new(vec![]);
    let svc = service(&gh, &FakeHost::with_machine("octo-abc"));
    let error = svc.stop_cloud(&CloudRef::sandbox("sbx-1")).unwrap_err();
    assert!(error.contains("sandbox"), "{error}");
}

pub(super) fn resolved(name: &str) -> ResolvedCodespace {
    ResolvedCodespace {
        name: name.into(),
        display: "monoptah-main".into(),
        repo: "octo/widgets".into(),
        remote_cwd: "/workspaces/widgets".into(),
        created: true,
        web_url: None,
        state: "Provisioning".into(),
        branch: Some("main".into()),
        machine: None,
    }
}

/// Stores a pending session and a start record for `cloud` and returns the session id.
fn begin_pending_start(svc: &CloudSessions, cloud: &CloudRef) -> i64 {
    let store = svc.store().unwrap();
    let project =
        crate::store::pending_cloud_project("/Users/me/widgets", cloud, "/workspaces/widgets");
    let session = store
        .create_pending_cloud_session("claude", project)
        .unwrap();
    let mut record = StartRecord::fresh(
        &resolved(&cloud.name),
        session.id,
        "/Users/me/widgets",
        // Codespace starts are older, so the pending order is deterministic.
        if cloud.kind == CloudBackendKind::Codespaces {
            1
        } else {
            2
        },
    );
    if cloud.kind == CloudBackendKind::DockerSandbox {
        record.codespace = String::new();
        record.sandbox = Some(cloud.name.clone());
    }
    startup::save_start_record(&store, &record).unwrap();
    session.id
}

#[test]
fn a_codespace_has_a_web_url_and_a_sandbox_has_none() {
    let gh = FakeGh::new(vec![(
        "api user/codespaces/octo-abc",
        ok("https://octo-abc.github.dev\n"),
    )]);
    let host = FakeHost::with_machine("octo-abc");
    let sandbox = Arc::new(FakeSandboxBackend::default());
    let svc = service(&gh, &host).with_backend(sandbox.clone());

    let url = svc.workspace_web_url(&CloudRef::codespace("octo-abc"));
    let none = svc
        .workspace_web_url(&CloudRef::sandbox("sbx-1"))
        .unwrap_err();

    assert_eq!(url.as_deref(), Ok("https://octo-abc.github.dev"));
    assert!(none.contains("sandbox"), "{none}");
    // Only the read of the URL: nothing woke the workspace.
    assert_eq!(
        gh.calls(),
        vec!["api user/codespaces/octo-abc --jq .web_url"]
    );
    assert_eq!(host.log(), Vec::<String>::new());
}

#[test]
fn a_sandbox_start_without_sandbox_support_never_reaches_gh() {
    let gh = FakeGh::new(vec![]);
    let host = FakeHost::with_machine("octo-abc");
    let sandbox = Arc::new(FakeSandboxBackend::default());
    let svc = service(&gh, &host).with_backend(sandbox.clone());
    let id = begin_pending_start(&svc, &CloudRef::sandbox("sbx-1"));
    let cancel = AtomicBool::new(false);
    let mut events = |_: StartEvent| {};

    let resumed = svc.resume_start(id, &mut events, &cancel).unwrap_err();
    let run = svc.run_start(id, &mut events, &cancel).unwrap_err();
    let rebuilt = svc.rebuild_start(id).unwrap_err();
    let url = svc.start_web_url(id).unwrap_err();

    assert!(resumed.contains("not available"), "{resumed}");
    assert!(run.contains("not available"), "{run}");
    assert!(rebuilt.contains("not provisioned"), "{rebuilt}");
    assert!(url.contains("sandbox"), "{url}");
    assert_eq!(gh.calls(), Vec::<String>::new());
    assert_eq!(host.log(), Vec::<String>::new());
    // Only the read-only state lookup of resume reached the backend; nothing was rebuilt.
    assert_eq!(sandbox.calls(), vec!["state sbx-1"]);
    // The refused record is untouched.
    let record = startup::start_record(&svc.store().unwrap(), id).unwrap();
    assert!(!record.is_failed());
    assert_eq!(record.sandbox.as_deref(), Some("sbx-1"));
}

#[test]
fn sandbox_starts_live_under_their_own_setting() {
    let gh = FakeGh::new(vec![]);
    let svc = service(&gh, &FakeHost::with_machine("octo-abc"));
    let codespace_id = begin_pending_start(&svc, &CloudRef::codespace("cs-1"));
    let old = svc.store().unwrap().setting("cloud-starts.v1").unwrap();
    assert!(old.is_some());

    let sandbox_id = begin_pending_start(&svc, &CloudRef::sandbox("sbx-1"));
    let store = svc.store().unwrap();
    assert_eq!(store.setting("cloud-starts.v1").unwrap(), old);
    let new = store.setting("cloud-starts-sbx.v1").unwrap().unwrap();
    assert!(new.contains("\"sandbox\":\"sbx-1\""), "{new}");
    assert!(!old.as_ref().unwrap().contains("sandbox"));
    let ids: Vec<i64> = startup::pending_starts(&store)
        .iter()
        .map(|r| r.session_id)
        .collect();
    assert_eq!(ids, vec![codespace_id, sandbox_id]);

    store.forget_cloud(&CloudRef::sandbox("sbx-1")).unwrap();
    assert_eq!(store.setting("cloud-starts.v1").unwrap(), old);
    assert!(startup::start_record(&store, sandbox_id).is_none());
    assert!(startup::start_record(&store, codespace_id).is_some());
}

#[test]
fn stopping_a_sandbox_pauses_its_pending_start() {
    let gh = FakeGh::new(vec![]);
    let sandbox = Arc::new(FakeSandboxBackend::default());
    let svc = service(&gh, &FakeHost::with_machine("octo-abc")).with_backend(sandbox.clone());
    let cloud = CloudRef::sandbox("sbx-1");
    let id = begin_pending_start(&svc, &cloud);
    let other = begin_pending_start(&svc, &CloudRef::sandbox("sbx-2"));

    svc.stop_cloud(&cloud).unwrap();

    let store = svc.store().unwrap();
    let record = startup::start_record(&store, id).unwrap();
    assert_eq!(
        record.error.as_deref(),
        Some("Sandbox stopped. Choose Retry to start it again.")
    );
    assert!(!startup::start_record(&store, other).unwrap().is_failed());
    assert_eq!(sandbox.calls(), vec!["stop sbx-1"]);
    assert_eq!(gh.calls(), Vec::<String>::new());
}

#[test]
fn split_created_note_reads_what_created_note_for_writes() {
    for (cloud, kind) in [
        (CloudRef::codespace("octo-1"), CloudBackendKind::Codespaces),
        (CloudRef::sandbox("box-1"), CloudBackendKind::DockerSandbox),
    ] {
        let note = created_note_for(&cloud, "it broke");
        assert_eq!(
            split_created_note(&note),
            Some(("it broke", kind, cloud.name.as_str()))
        );
    }
    assert_eq!(split_created_note("plain error"), None);
    assert_eq!(
        split_created_note("x (sandbox  was created and kept; y)"),
        None
    );
}

#[test]
fn a_cloud_located_local_session_is_one_cloud_row_and_its_host_record_is_not_listed() {
    let gh = FakeGh::new(vec![
        ("auth status", ok(AUTH_OK)),
        ("codespace list", ok(LIST)),
    ]);
    let host = FakeHost::with_machine("octo-abc");
    let svc = service(&gh, &host);
    let binding = svc
        .prepare_target("/Users/me/widgets", named("octo-abc"), &mut |_| {})
        .unwrap();
    // Two Host sessions of the project: one born in the cloud, one that backs a local session.
    let born = svc.create_session(&binding, "claude").unwrap();
    let backing = svc.create_session(&binding, "claude").unwrap();
    let local = {
        let mut store = svc.store().unwrap();
        let local = store.create("claude", "/Users/me/widgets").unwrap();
        let rows: Vec<crate::transcript::Row> = (0..10)
            .map(|i| crate::transcript::Row {
                role: if i % 2 == 0 { "user" } else { "assistant" }.into(),
                text: format!("row {i}"),
                first: i % 2 == 0,
                sent_at: None,
            })
            .collect();
        store.write_rows(local.id, 0, &rows).unwrap();
        local
    };
    let ids = |svc: &CloudSessions| -> Vec<i64> {
        let mut ids: Vec<i64> = svc
            .cloud_sessions_for("/Users/me/widgets")
            .unwrap()
            .iter()
            .map(|s| s.summary.id)
            .collect();
        ids.sort();
        ids
    };
    let mut both = vec![born.id, backing.id];
    both.sort();
    // Location Local: the local session is not a cloud session; both Host records are.
    assert_eq!(ids(&svc), both);
    // The location flips to Cloud and the Host record is linked: the sidebar mark follows.
    {
        let store = svc.store().unwrap();
        store
            .set_cloud_location(
                local.id,
                &crate::store::SessionLocation::Cloud(crate::store::CloudLocation {
                    cloud: crate::cloud_sessions::CloudRef::codespace("octo-abc"),
                    environment_id: binding.project_key.clone(),
                    project_id: String::new(),
                    host_session_id: "host-1".into(),
                    remote_cwd: "/workspaces/widgets".into(),
                    local_cwd: "/Users/me/widgets".into(),
                    moved_at_row: 10,
                    pushed_branch: None,
                }),
            )
            .unwrap();
        store
            .link_remote_session(backing.id, Some(local.id))
            .unwrap();
    }
    let mut expected = vec![born.id, local.id];
    expected.sort();
    assert_eq!(
        ids(&svc),
        expected,
        "the linked record is hidden, the unlinked one is not"
    );
    let row = svc
        .cloud_sessions_for("/Users/me/widgets")
        .unwrap()
        .into_iter()
        .find(|s| s.summary.id == local.id)
        .unwrap();
    assert_eq!(row.codespace, "octo-abc");
    assert_eq!(row.summary.cwd, "/Users/me/widgets");
    let located = svc.located_sessions_for("/Users/me/widgets").unwrap();
    assert_eq!(
        located,
        vec![LocatedSession {
            id: local.id,
            moved_at_row: 10,
            // The fake Host record has no Host session id: the link cannot be resolved.
            sync_error: Some(crate::store::MISSING_HOST.into()),
            unreadable: false,
            host_active: false,
            unsynced: true,
        }]
    );
    // Back to Local: both Host records are cloud sessions of their own again.
    svc.store()
        .unwrap()
        .reset_cloud_location_to_local(local.id)
        .unwrap();
    assert_eq!(ids(&svc), both);
    assert!(svc
        .located_sessions_for("/Users/me/widgets")
        .unwrap()
        .is_empty());
}

#[test]
fn deleting_the_workspace_keeps_a_located_session_and_does_not_convert_its_host_record() {
    let gh = FakeGh::new(vec![
        ("auth status", ok(AUTH_OK)),
        ("codespace list", ok(LIST)),
        ("codespace delete", ok("")),
    ]);
    let host = FakeHost::with_machine("octo-abc");
    let svc = service(&gh, &host);
    let binding = svc
        .prepare_target("/Users/me/widgets", named("octo-abc"), &mut |_| {})
        .unwrap();
    let born = svc.create_session(&binding, "claude").unwrap();
    let backing = svc.create_session(&binding, "claude").unwrap();
    let local = {
        let mut store = svc.store().unwrap();
        let local = store.create("claude", "/Users/me/widgets").unwrap();
        let rows: Vec<crate::transcript::Row> = (0..10)
            .map(|i| crate::transcript::Row {
                role: if i % 2 == 0 { "user" } else { "assistant" }.into(),
                text: format!("row {i}"),
                first: true,
                sent_at: None,
            })
            .collect();
        store.write_rows(local.id, 0, &rows).unwrap();
        store
            .set_cloud_location(
                local.id,
                &crate::store::SessionLocation::Cloud(crate::store::CloudLocation {
                    cloud: crate::cloud_sessions::CloudRef::codespace("octo-abc"),
                    environment_id: binding.project_key.clone(),
                    project_id: String::new(),
                    host_session_id: "host-1".into(),
                    remote_cwd: "/workspaces/widgets".into(),
                    local_cwd: "/Users/me/widgets".into(),
                    moved_at_row: 10,
                    pushed_branch: None,
                }),
            )
            .unwrap();
        store
            .link_remote_session(backing.id, Some(local.id))
            .unwrap();
        local
    };
    let cloud = crate::cloud_sessions::CloudRef::codespace("octo-abc");
    // The linked Host record is not offered for conversion or archiving.
    let offered = svc.sessions_in_cloud(&cloud);
    assert!(offered.contains(&born.id));
    assert!(!offered.contains(&backing.id), "{offered:?}");
    // The workspace is deleted: the Host records go, the local session stays with a marker.
    svc.delete_cloud_outcome(&cloud).unwrap();
    let store = svc.store().unwrap();
    assert!(store.remote_session(backing.id).unwrap().is_none());
    assert!(store.sessions().unwrap().iter().any(|s| s.id == local.id));
    assert_eq!(store.load(local.id).unwrap().rows.len(), 10);
    assert_eq!(
        store.cloud_sync_error(local.id).unwrap().as_deref(),
        Some(crate::store::WORKSPACE_LOST)
    );
    drop(store);
    let located = svc.located_sessions_for("/Users/me/widgets").unwrap();
    assert_eq!(located.len(), 1);
    assert_eq!(
        located[0].sync_error.as_deref(),
        Some(crate::store::WORKSPACE_LOST)
    );
    // Reset to local is the way out, and it does not need a Host record.
    svc.store().unwrap().leave_cloud(local.id).unwrap();
    assert!(svc
        .located_sessions_for("/Users/me/widgets")
        .unwrap()
        .is_empty());
}
