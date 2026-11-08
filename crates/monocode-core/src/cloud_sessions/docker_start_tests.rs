use super::backend::docker::{SandboxShell, ShellOutput};
use super::backend::CloudBackendKind;
use super::docker_config::DockerProjectConfig;
use super::docker_start::{
    parse_up_line, repo_dir_of, starts_blocking_commands, ClaudeAuth, SandboxSecrets, UpLine,
};
// Only the unix-only `scripts` tests run the embedded shell scripts.
#[cfg(unix)]
use super::docker_start::{
    with_vars, GIT_CONFIG_SCRIPT, PROVISION_SCRIPT, RELAY_SCRIPT, UP_SCRIPT,
};
use super::startup::{
    pending_starts, save_start_record, start_record, StartEvent, StartRecord, Step, StepKind,
    StepStatus,
};
use super::{CloudBinding, CloudSessions, Host, START_ABORTED};
use crate::codespaces::{GhOutput, GhRunner};
use crate::remote::Machine;
use crate::sandboxes::{SbxOutput, SbxRunner};
use crate::store::{Session, Store};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use zeroize::Zeroizing;

const SECRET: &str = "sk-ant-oat01-SECRETSECRETSECRET";
const GIT_TOKEN: &str = "ghp_GITTOKENGITTOKEN1234";
const REPO: &str = "https://github.com/octo/widgets.git";
const NEW_ID: &str = "sbx_001newbox";
const WORKSPACE: &str = "/home/agent/workspaces/widgets";
const CONTAINER: &str = "0123456789abcdef0123";

// ------------------------------------------------------------------ fake sbx

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CreateMode {
    Ok,
    /// The call times out but the sandbox exists.
    TimeoutKept,
    /// The call times out and nothing was created.
    TimeoutLost,
    /// The name is taken: sbx answers `already_exists` (the sandbox exists).
    AlreadyExists,
    /// sbx answers with a clear pre-flight refusal (quota).
    Refused,
    /// sbx fails with an error that proves nothing about whether a sandbox was made.
    Flaky,
}

struct Sbx {
    sandboxes: Mutex<Vec<(String, String, String)>>,
    mode: Mutex<CreateMode>,
    calls: Mutex<Vec<String>>,
}
impl Sbx {
    fn new(mode: CreateMode) -> Arc<Self> {
        Arc::new(Self {
            sandboxes: Mutex::new(vec![]),
            mode: Mutex::new(mode),
            calls: Mutex::new(vec![]),
        })
    }
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
    fn count(&self, verb: &str) -> usize {
        self.calls()
            .iter()
            .filter(|c| c.split_whitespace().nth(2) == Some(verb))
            .count()
    }
    fn add(&self, id: &str, name: &str, state: &str) {
        if self.sandboxes.lock().unwrap().iter().any(|s| s.1 == name) {
            return;
        }
        self.sandboxes
            .lock()
            .unwrap()
            .push((id.into(), name.into(), state.into()));
    }
    fn set_state(&self, id: &str, state: &str) {
        for s in self.sandboxes.lock().unwrap().iter_mut() {
            if s.0 == id {
                s.2 = state.into();
            }
        }
    }
    fn remove(&self, id: &str) {
        self.sandboxes.lock().unwrap().retain(|s| s.0 != id);
    }
}
fn out(success: bool, stdout: &str, stderr: &str) -> Result<SbxOutput, String> {
    Ok(SbxOutput {
        success,
        stdout: stdout.into(),
        stderr: stderr.into(),
    })
}
impl SbxRunner for Sbx {
    fn run(&self, args: &[String], stdin: Option<&str>) -> Result<SbxOutput, String> {
        assert!(stdin.is_none(), "no stdin for sbx");
        let line = args.join(" ");
        self.calls.lock().unwrap().push(line.clone());
        let verb = args.get(2).map(String::as_str).unwrap_or("");
        let list = || {
            let items: Vec<Value> = self
                .sandboxes
                .lock()
                .unwrap()
                .iter()
                .map(|(id, name, state)| json!({"id":id,"name":name,"agent":"shell","state":state}))
                .collect();
            serde_json::to_string(&items).unwrap()
        };
        match verb {
            "ls" => out(true, &list(), ""),
            "inspect" => {
                let id = &args[3];
                let found = self
                    .sandboxes
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|s| &s.0 == id)
                    .cloned();
                match found {
                    Some((id, name, state)) => out(
                        true,
                        &json!({"id":id,"name":name,"agent":"shell","state":state}).to_string(),
                        "",
                    ),
                    None => out(false, "", &format!("error: sandbox '{id}' not found")),
                }
            }
            "create" => {
                let name = args[args.iter().position(|a| a == "--name").unwrap() + 1].clone();
                let mode = *self.mode.lock().unwrap();
                match mode {
                    CreateMode::Ok => {
                        self.add(NEW_ID, &name, "running");
                        out(true, &format!("Creating\n{NEW_ID}\n"), "")
                    }
                    CreateMode::TimeoutKept => {
                        self.add(NEW_ID, &name, "running");
                        Err("Docker Sandboxes CLI did not answer within 120 seconds (`sbx create`). Check your network connection.".into())
                    }
                    CreateMode::TimeoutLost => Err("Docker Sandboxes CLI did not answer within 120 seconds (`sbx create`). Check your network connection.".into()),
                    CreateMode::AlreadyExists => {
                        self.add(NEW_ID, &name, "running");
                        out(false, "", "error: already_exists: name taken")
                    }
                    CreateMode::Refused => out(false, "", "error: quota exceeded"),
                    CreateMode::Flaky => out(false, "", "error: internal server error"),
                }
            }
            "stop" | "rm" => out(true, "", ""),
            other => panic!("unexpected sbx call: {other} ({line})"),
        }
    }
}

// ----------------------------------------------------------------- fake shell

#[derive(Clone, Debug)]
struct ShellCall {
    id: String,
    remote: String,
    stdin: String,
}
impl ShellCall {
    /// The `PHASE`, `MODE` or other tag of the embedded script this call carries.
    fn tag(&self) -> String {
        let text = format!("{}\n{}", self.remote, self.stdin).replace("'\\''", "'");
        for key in ["PHASE", "MODE"] {
            if let Some(i) = text.find(&format!("{key}='")) {
                let rest = &text[i + key.len() + 2..];
                return format!(
                    "{}:{}",
                    key.to_lowercase(),
                    &rest[..rest.find('\'').unwrap()]
                );
            }
        }
        if text.contains("CID='") {
            "relay".into()
        } else if text.contains("WANT_CRED='") {
            "gitconfig".into()
        } else if text.contains("MONOCODE_HOST_TRACK_TOKEN=1") {
            "bootstrap".into()
        } else if text.contains("monoptah-evidence") {
            "evidence".into()
        } else {
            "other".into()
        }
    }
}

struct Shell {
    calls: Mutex<Vec<ShellCall>>,
    up_log: Mutex<Vec<String>>,
    up_exit: Mutex<String>,
    /// The first N follow attempts end without the exit marker.
    follow_drops: Mutex<usize>,
    fail_tag: Mutex<Option<(String, String)>>,
    /// Makes the "outer sandbox" the local disk for archive scripts: HOME and the real folder
    /// that stands for [`WORKSPACE`].
    local: Mutex<Option<(std::path::PathBuf, std::path::PathBuf)>>,
}
impl Shell {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(vec![]),
            up_log: Mutex::new(vec![
                r#"{"type":"start","level":2,"timestamp":1,"text":"Resolving Remote"}"#.into(),
                r#"{"type":"start","level":2,"timestamp":2,"text":"Starting container"}"#.into(),
                r#"{"type":"progress","name":"Running onCreateCommand...","status":"running"}"#
                    .into(),
                r#"{"type":"progress","name":"Running onCreateCommand...","status":"succeeded"}"#
                    .into(),
                format!(
                    r#"{{"outcome":"success","containerId":"{CONTAINER}","remoteUser":"vscode","remoteWorkspaceFolder":"/workspaces/widgets"}}"#
                ),
            ]),
            up_exit: Mutex::new("0".into()),
            follow_drops: Mutex::new(0),
            fail_tag: Mutex::new(None),
            local: Mutex::new(None),
        })
    }
    fn calls(&self) -> Vec<ShellCall> {
        self.calls.lock().unwrap().clone()
    }
    fn tags(&self) -> Vec<String> {
        self.calls().iter().map(ShellCall::tag).collect()
    }
    fn call(&self, tag: &str) -> ShellCall {
        self.calls()
            .into_iter()
            .find(|c| c.tag() == tag)
            .unwrap_or_else(|| panic!("no {tag} call in {:?}", self.tags()))
    }
    fn fail(&self, tag: &str, stderr: &str) {
        *self.fail_tag.lock().unwrap() = Some((tag.into(), stderr.into()));
    }
}
impl SandboxShell for Shell {
    fn run(
        &self,
        id: &str,
        remote: &str,
        stdin: &str,
        on_line: &mut dyn FnMut(&str),
        _abort: &dyn Fn() -> bool,
        _timeout: Duration,
    ) -> Result<ShellOutput, String> {
        let call = ShellCall {
            id: id.into(),
            remote: remote.into(),
            stdin: stdin.into(),
        };
        let tag = call.tag();
        self.calls.lock().unwrap().push(call);
        let failing = self.fail_tag.lock().unwrap().clone();
        if let Some((fail_tag, stderr)) = failing {
            if fail_tag == tag {
                return Ok(ShellOutput {
                    success: false,
                    stdout: String::new(),
                    stderr,
                });
            }
        }
        let mut lines: Vec<String> = vec![];
        match tag.as_str() {
            "phase:setup" => lines.extend([
                "[monoptah] installing xz-utils".into(),
                "MONOPTAH_HOME=/home/agent".into(),
            ]),
            "phase:approve" => lines.push("MONOPTAH_CREDENTIAL=cached".into()),
            "phase:clone" => lines.extend([
                "[monoptah] cloning the repository".into(),
                format!("MONOPTAH_WORKSPACE={WORKSPACE}"),
            ]),
            "mode:start" | "mode:rebuild" => lines.push("MONOPTAH_UP=started".into()),
            "mode:follow" => {
                let offset: usize = stdin
                    .lines()
                    .find_map(|l| l.strip_prefix("OFFSET='"))
                    .and_then(|l| l.trim_end_matches('\'').parse().ok())
                    .unwrap_or(0);
                let mut drops = self.follow_drops.lock().unwrap();
                let log = self.up_log.lock().unwrap().clone();
                let mut shown: Vec<String> = log.iter().skip(offset).cloned().collect();
                let dropped = *drops > 0;
                if dropped {
                    *drops -= 1;
                    // The connection drops after the first two lines.
                    shown.truncate(2);
                }
                for line in shown {
                    // ssh noise comes between the log lines and must not be counted.
                    lines.push("Connection to host closed by remote host.".into());
                    lines.push(format!("L {line}"));
                }
                if !dropped {
                    lines.push(format!("MONOPTAH_UP_EXIT={}", self.up_exit.lock().unwrap()));
                }
            }
            "mode:lifecycle" => lines.push("MONOPTAH_LIFECYCLE=started".into()),
            "gitconfig" => lines.push("MONOPTAH_GIT=configured".into()),
            "bootstrap" => lines.extend([
                "Installing Claude Code on this host...".into(),
                r#"{"port":3774,"pid":9}"#.into(),
            ]),
            "relay" => lines.push("MONOPTAH_RELAY=started".into()),
            "evidence" => {}
            "other" if self.local.lock().unwrap().is_some() => {
                let (home, ws) = self.local.lock().unwrap().clone().unwrap();
                assert!(
                    remote.starts_with("sh -c ")
                        && remote.contains("sh -s")
                        && !remote.contains("devcontainer"),
                    "an archive script runs in the outer sandbox: {remote}"
                );
                let mut child = std::process::Command::new("sh")
                    .arg("-s")
                    .env("HOME", home)
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .spawn()
                    .unwrap();
                use std::io::Write as _;
                child
                    .stdin
                    .take()
                    .unwrap()
                    .write_all(stdin.replace(WORKSPACE, ws.to_str().unwrap()).as_bytes())
                    .unwrap();
                let result = child.wait_with_output().unwrap();
                let stdout = String::from_utf8_lossy(&result.stdout).to_string();
                for line in stdout.lines() {
                    on_line(line);
                }
                return Ok(ShellOutput {
                    success: result.status.success(),
                    stdout,
                    stderr: String::from_utf8_lossy(&result.stderr).to_string(),
                });
            }
            other => panic!("unexpected sandbox command {other}: {remote}"),
        }
        for line in &lines {
            on_line(line);
        }
        Ok(ShellOutput {
            success: true,
            stdout: lines.join("\n") + "\n",
            stderr: String::new(),
        })
    }
}

// ----------------------------------------------------------------- the rest

struct NoGh;
impl GhRunner for NoGh {
    fn run(&self, args: &[String], _: Option<&str>) -> Result<GhOutput, String> {
        panic!("a sandbox flow must not call gh: {args:?}")
    }
}

struct Secrets {
    auth: Option<(&'static str, String)>,
}
impl SandboxSecrets for Secrets {
    fn claude_auth(&self, account: Option<&str>) -> Result<Option<ClaudeAuth>, String> {
        assert_eq!(account, Some("work"));
        Ok(self
            .auth
            .clone()
            .map(|(variable, secret)| ClaudeAuth::new(variable, secret)))
    }
    fn git_payload(&self, credential: &str, repo_url: &str) -> Result<Zeroizing<String>, String> {
        assert_eq!(credential, "cred-1");
        assert_eq!(repo_url, REPO);
        Ok(Zeroizing::new(format!(
            "protocol=https\nhost=github.com\npath=octo/widgets.git\nusername=me\npassword={GIT_TOKEN}\n\n"
        )))
    }
}

#[derive(Default)]
struct FakeHost {
    machines: Mutex<Vec<Machine>>,
    log: Mutex<Vec<String>>,
    /// Shell tags seen when the host was asked to connect (ordering evidence).
    shell: Mutex<Option<Arc<Shell>>>,
    seen_before_connect: Mutex<Vec<String>>,
    /// Environment id the next pairing reports (a recreated container has a new Host).
    environment: Mutex<Option<String>>,
    /// Whether the live tunnel answers `environment.describe`.
    connected: Mutex<bool>,
    /// `sessions.delete` answers `SANDBOX_NOT_CONNECTED`.
    delete_offline: Mutex<bool>,
}
impl Host for FakeHost {
    fn machines(&self) -> Result<Vec<Machine>, String> {
        Ok(self.machines.lock().unwrap().clone())
    }
    fn connect(&self, _: &str, _: &str, _: &mut dyn FnMut(&str)) -> Result<Machine, String> {
        panic!("a sandbox must not connect as a codespace")
    }
    fn connect_sandbox(
        &self,
        sandbox: crate::remote::SandboxTarget,
        display: &str,
        progress: &mut dyn FnMut(&str),
    ) -> Result<Machine, String> {
        if let Some(shell) = self.shell.lock().unwrap().as_ref() {
            *self.seen_before_connect.lock().unwrap() = shell.tags();
        }
        progress("pairing");
        self.log.lock().unwrap().push(format!(
            "connect {} {display} {}",
            sandbox.name, sandbox.relay_port
        ));
        let machine = Machine {
            id: format!("m-{}", sandbox.name),
            name: sandbox.name.clone(),
            endpoint: format!("sbx://{}", sandbox.name),
            environment_id: self
                .environment
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_else(|| format!("env-{}", sandbox.name)),
            ssh: None,
            codespace: None,
            sandbox: Some(sandbox),
        };
        self.machines.lock().unwrap().push(machine.clone());
        Ok(machine)
    }
    fn ensure(&self, id: &str, _: &AtomicBool, _: &mut dyn FnMut(&str)) -> Result<(), String> {
        self.log.lock().unwrap().push(format!("ensure {id}"));
        Ok(())
    }
    fn request(&self, machine_id: &str, method: &str, params: Value) -> Result<Value, String> {
        if method == "environment.describe" {
            return if *self.connected.lock().unwrap() {
                Ok(json!({}))
            } else {
                Err(crate::remote::SANDBOX_NOT_CONNECTED.into())
            };
        }
        if method == "sessions.delete" && *self.delete_offline.lock().unwrap() {
            return Err(crate::remote::SANDBOX_NOT_CONNECTED.into());
        }
        self.log
            .lock()
            .unwrap()
            .push(format!("request {machine_id} {method} {}", params["cwd"]));
        Ok(json!({"id":"project-1","cwd":params["cwd"]}))
    }
    fn disconnect(&self, id: &str) -> Result<(), String> {
        self.log.lock().unwrap().push(format!("disconnect {id}"));
        self.machines.lock().unwrap().retain(|m| m.id != id);
        Ok(())
    }
}

struct Fx {
    svc: CloudSessions,
    sbx: Arc<Sbx>,
    shell: Arc<Shell>,
    host: Arc<FakeHost>,
    store: Arc<Mutex<Store>>,
    _home: tempfile::TempDir,
}

fn fx_with(mode: CreateMode, auth: Option<(&'static str, String)>) -> Fx {
    fx_in(mode, auth, Store::open(Path::new(":memory:")).unwrap())
}
fn fx_in(mode: CreateMode, auth: Option<(&'static str, String)>, store: Store) -> Fx {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".ssh/sandboxes")).unwrap();
    std::fs::write(
        home.path().join(".ssh/sandboxes/config"),
        "Host sbx_cloud\n  ProxyCommand sbx --cloud ssh-connect '%r' %h %p\n",
    )
    .unwrap();
    let sbx = Sbx::new(mode);
    let shell = Shell::new();
    let host = Arc::new(FakeHost::default());
    *host.shell.lock().unwrap() = Some(shell.clone());
    let store = Arc::new(Mutex::new(store));
    let tick = Arc::new(AtomicU64::new(1_794_024_100_000));
    let svc = CloudSessions::new(Arc::new(NoGh), host.clone(), store.clone())
        .with_timing(
            Duration::from_secs(5),
            |_| {},
            move || tick.fetch_add(1000, Ordering::SeqCst),
        )
        .with_sandboxes(sbx.clone(), shell.clone(), Arc::new(Secrets { auth }))
        .with_sandbox_home(home.path().to_path_buf());
    Fx {
        svc,
        sbx,
        shell,
        host,
        store,
        _home: home,
    }
}
fn fx() -> Fx {
    fx_with(
        CreateMode::Ok,
        Some(("CLAUDE_CODE_OAUTH_TOKEN", SECRET.into())),
    )
}

fn config() -> DockerProjectConfig {
    DockerProjectConfig {
        repo_url: REPO.into(),
        credential_id: Some("cred-1".into()),
        claude_account: Some("work".into()),
        ..Default::default()
    }
}

impl Fx {
    fn begin(&self) -> Result<(Session, StartRecord), String> {
        self.svc
            .begin_sandbox_start("/Users/me/widgets", &config(), "fix/login", "claude")
    }
    fn record(&self, id: i64) -> Option<StartRecord> {
        start_record(&self.store.lock().unwrap(), id)
    }
    fn run(&self, id: i64) -> (Result<CloudBinding, String>, Vec<StartEvent>) {
        let mut events = vec![];
        let result = self
            .svc
            .run_start(id, &mut |e| events.push(e), &AtomicBool::new(false));
        (result, events)
    }
}
fn status(record: &StartRecord, kind: StepKind) -> StepStatus {
    record.step(kind).status
}

// ------------------------------------------------------------ pure helpers

#[test]
fn up_log_lines_parse_into_progress_text_and_the_outcome() {
    assert_eq!(
        parse_up_line(
            r#"{"type":"progress","name":"Running onCreateCommand...","status":"running"}"#
        ),
        UpLine::Progress {
            name: "Running onCreateCommand...".into(),
            status: "running".into()
        }
    );
    assert_eq!(
        parse_up_line(r#"{"type":"start","level":2,"timestamp":2,"text":"Starting container"}"#),
        UpLine::Text("Starting container".into())
    );
    assert_eq!(
        parse_up_line(&format!(
            r#"{{"outcome":"success","containerId":"{CONTAINER}","remoteUser":"vscode","remoteWorkspaceFolder":"/workspaces/ws"}}"#
        )),
        UpLine::Outcome {
            success: true,
            container_id: Some(CONTAINER.into()),
            remote_user: Some("vscode".into()),
            remote_workspace: Some("/workspaces/ws".into()),
            message: None
        }
    );
    assert!(matches!(
        parse_up_line(r#"{"outcome":"error","message":"boom","description":"Build failed"}"#),
        UpLine::Outcome { success: false, message: Some(m), .. } if m == "Build failed"
    ));
    assert_eq!(
        parse_up_line("plain text line"),
        UpLine::Plain("plain text line".into())
    );
    assert!(starts_blocking_commands(
        "Running onCreateCommand...",
        "running"
    ));
    assert!(!starts_blocking_commands(
        "Running onCreateCommand...",
        "succeeded"
    ));
    assert!(!starts_blocking_commands("Resolving Remote", "running"));
}

#[test]
fn the_prelude_unsets_every_claude_variable_and_exports_exactly_one() {
    let token = ClaudeAuth::new("CLAUDE_CODE_OAUTH_TOKEN", "sk-ant-oat01-abc".into()).prelude();
    assert_eq!(
        &**token,
        "unset ANTHROPIC_AUTH_TOKEN ANTHROPIC_API_KEY CLAUDE_CODE_OAUTH_TOKEN\nexport CLAUDE_CODE_OAUTH_TOKEN='sk-ant-oat01-abc'\n"
    );
    let key = ClaudeAuth::new("ANTHROPIC_API_KEY", "sk-ant-api03-x'y".into()).prelude();
    assert!(
        key.starts_with("unset ANTHROPIC_AUTH_TOKEN ANTHROPIC_API_KEY CLAUDE_CODE_OAUTH_TOKEN\n")
    );
    assert!(key.contains("MONOCODE_HOST_TOKEN_VAR=ANTHROPIC_API_KEY\n"));
    assert!(key.contains("export ANTHROPIC_API_KEY='sk-ant-api03-x'\\''y'\n"));
    assert_eq!(key.matches("export ").count(), 1);
    // The bootstrap text a codespace runs is exactly what it was.
    let plain = crate::remote::sandbox_bootstrap_script(None);
    assert_eq!(plain, crate::remote::sandbox_bootstrap_script(None));
    let with = crate::remote::sandbox_bootstrap_script(Some(&token));
    assert!(with.starts_with(&**token));
    assert!(with.ends_with(&plain));
}

#[test]
fn repo_dirs_are_plain_folder_names() {
    assert_eq!(repo_dir_of("octo/widgets"), "widgets");
    assert_eq!(repo_dir_of("group/sub/proj.git"), "proj");
    assert_eq!(repo_dir_of("a/..weird name"), "weirdname");
    assert_eq!(repo_dir_of("a/."), "repo");
}

// ------------------------------------------------------------------- begin

#[test]
fn begin_persists_the_record_before_create_and_the_id_right_after() {
    let fx = fx();
    // Observe the store at the moment `sbx create` runs.
    struct Spy {
        inner: Arc<Sbx>,
        store: Arc<Mutex<Store>>,
        seen: Mutex<Option<StartRecord>>,
    }
    impl SbxRunner for Spy {
        fn run(&self, args: &[String], stdin: Option<&str>) -> Result<SbxOutput, String> {
            if args.get(2).map(String::as_str) == Some("create") {
                let all = pending_starts(&self.store.lock().unwrap());
                *self.seen.lock().unwrap() = all.into_iter().next();
            }
            self.inner.run(args, stdin)
        }
    }
    let spy = Arc::new(Spy {
        inner: fx.sbx.clone(),
        store: fx.store.clone(),
        seen: Mutex::new(None),
    });
    let svc = CloudSessions::new(Arc::new(NoGh), fx.host.clone(), fx.store.clone())
        .with_sandboxes(
            spy.clone(),
            fx.shell.clone(),
            Arc::new(Secrets { auth: None }),
        )
        .with_sandbox_home(fx._home.path().to_path_buf());
    let (session, record) = svc
        .begin_sandbox_start("/Users/me/widgets", &config(), "fix/login", "claude")
        .unwrap();
    let before = spy
        .seen
        .lock()
        .unwrap()
        .clone()
        .expect("record before create");
    let name = before.sandbox.clone().unwrap();
    assert!(name.starts_with("monoptah-fix-login-"), "{name}");
    let saved = before.sandbox_start.clone().unwrap();
    assert_eq!(saved.sandbox_id, None);
    assert_eq!(saved.config, config().validated().unwrap());
    assert_eq!(saved.request_id.len(), 36);
    assert_eq!(before.branch.as_deref(), Some("fix/login"));
    assert_eq!(before.repo, "octo/widgets");
    assert_eq!(before.backend(), CloudBackendKind::DockerSandbox);
    // After create the id is stored and the record names the same sandbox.
    assert_eq!(record.sandbox.as_deref(), Some(name.as_str()));
    assert_eq!(
        record.sandbox_start.as_ref().unwrap().sandbox_id.as_deref(),
        Some(NEW_ID)
    );
    let stored = start_record(&fx.store.lock().unwrap(), session.id).unwrap();
    assert_eq!(
        stored.sandbox_start.as_ref().unwrap().sandbox_id.as_deref(),
        Some(NEW_ID)
    );
    assert_eq!(status(&stored, StepKind::Create), StepStatus::Done);
    // The session sits on a provisional sandbox project.
    let remote = fx
        .store
        .lock()
        .unwrap()
        .remote_session(session.id)
        .unwrap()
        .unwrap();
    assert!(remote.project.environment_id.starts_with("pending-sbx-"));
    assert_eq!(remote.project.sandbox.as_deref(), Some(name.as_str()));
    assert_eq!(fx.sbx.count("create"), 1);
}

#[test]
fn the_persisted_record_holds_no_secret() {
    let fx = fx();
    let (session, _) = fx.begin().unwrap();
    let _ = fx.run(session.id);
    let store = fx.store.lock().unwrap();
    for key in ["cloud-starts-sbx.v1", "cloud-starts.v1"] {
        let raw = store.setting(key).unwrap().unwrap_or_default();
        assert!(!raw.contains(SECRET) && !raw.contains(GIT_TOKEN), "{key}");
    }
}

#[test]
fn begin_requires_claude_and_ready_prerequisites() {
    let fx = fx();
    assert!(fx
        .svc
        .begin_sandbox_start("/p", &config(), "main", "codex")
        .is_err());
    let empty = tempfile::tempdir().unwrap();
    let svc = CloudSessions::new(Arc::new(NoGh), fx.host.clone(), fx.store.clone())
        .with_sandboxes(
            fx.sbx.clone(),
            fx.shell.clone(),
            Arc::new(Secrets { auth: None }),
        )
        .with_sandbox_home(empty.path().to_path_buf());
    let error = svc
        .begin_sandbox_start("/p", &config(), "main", "claude")
        .unwrap_err();
    assert!(error.contains("sbx --cloud setup ssh"), "{error}");
    assert_eq!(fx.sbx.count("create"), 0);
    assert!(pending_starts(&fx.store.lock().unwrap()).is_empty());
}

#[test]
fn begin_without_sandbox_support_is_refused_and_a_bad_branch_too() {
    let store = Arc::new(Mutex::new(Store::open(Path::new(":memory:")).unwrap()));
    let svc = CloudSessions::new(Arc::new(NoGh), Arc::new(FakeHost::default()), store);
    assert!(svc
        .begin_sandbox_start("/p", &config(), "main", "claude")
        .unwrap_err()
        .contains("not available"));
    let fx = fx();
    assert!(fx
        .svc
        .begin_sandbox_start("/p", &config(), "bad branch;rm", "claude")
        .is_err());
    assert_eq!(fx.sbx.count("create"), 0);
}

#[test]
fn already_exists_adopts_the_sandbox_without_creating_twice() {
    let fx = fx_with(CreateMode::AlreadyExists, None);
    let (_, record) = fx.begin().unwrap();
    assert_eq!(
        record.sandbox_start.unwrap().sandbox_id.as_deref(),
        Some(NEW_ID)
    );
    assert_eq!(fx.sbx.count("create"), 1);
    assert_eq!(
        fx.sbx.count("ls"),
        2,
        "prerequisites, then the lookup by name"
    );
}

#[test]
fn a_timeout_that_created_the_sandbox_reconciles_by_name() {
    let fx = fx_with(CreateMode::TimeoutKept, None);
    let (_, record) = fx.begin().unwrap();
    assert_eq!(
        record.sandbox_start.as_ref().unwrap().sandbox_id.as_deref(),
        Some(NEW_ID)
    );
    assert!(!record.is_failed());
    assert_eq!(fx.sbx.count("create"), 1);
}

#[test]
fn a_timeout_that_found_nothing_keeps_a_failed_record_and_retry_never_duplicates() {
    let fx = fx_with(
        CreateMode::TimeoutLost,
        Some(("CLAUDE_CODE_OAUTH_TOKEN", SECRET.into())),
    );
    let (session, record) = fx.begin().unwrap();
    assert!(record.is_failed());
    assert_eq!(record.sandbox_start.as_ref().unwrap().sandbox_id, None);
    assert_eq!(status(&record, StepKind::Create), StepStatus::Failed);
    // The create did go through after all: Retry finds it by name and creates nothing.
    let name = record.sandbox.clone().unwrap();
    fx.sbx.add(NEW_ID, &name, "running");
    *fx.sbx.mode.lock().unwrap() = CreateMode::Refused;
    let (result, _) = fx.run(session.id);
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(
        fx.sbx.count("create"),
        1,
        "the retry looked the name up first"
    );
    assert!(fx.shell.tags().contains(&"phase:clone".to_string()));
}

#[test]
fn a_refused_create_leaves_no_session_and_no_record() {
    let fx = fx_with(CreateMode::Refused, None);
    let error = fx.begin().unwrap_err();
    assert!(error.contains("quota exceeded"), "{error}");
    assert!(pending_starts(&fx.store.lock().unwrap()).is_empty());
}

#[test]
fn an_unclear_create_failure_keeps_a_failed_record_to_retry_or_delete() {
    let fx = fx_with(CreateMode::Flaky, None);
    let (session, record) = fx.begin().unwrap();
    assert!(record.is_failed());
    assert!(record
        .error
        .as_deref()
        .unwrap()
        .contains("internal server error"));
    let stored = fx.record(session.id).unwrap();
    assert!(stored.is_failed());
    assert_eq!(stored.sandbox_start.as_ref().unwrap().sandbox_id, None);
    assert!(fx
        .store
        .lock()
        .unwrap()
        .remote_session(session.id)
        .unwrap()
        .is_some());
    // Retry looks the name up first, then creates (nothing exists yet): still failing.
    let before = fx.sbx.count("ls");
    let (result, _) = fx.run(session.id);
    assert!(result.is_err());
    assert!(fx.sbx.count("ls") > before);
}

// --------------------------------------------------------------- the start

#[test]
fn a_start_runs_every_phase_in_order_and_ends_ready() {
    let fx = fx();
    let (session, _) = fx.begin().unwrap();
    let (result, events) = fx.run(session.id);
    let binding = result.unwrap();
    assert_eq!(
        fx.shell.tags(),
        [
            "phase:setup",
            "phase:approve",
            "phase:clone",
            "mode:start",
            "mode:follow",
            "gitconfig",
            "bootstrap",
            "relay",
            "evidence",
            "mode:lifecycle"
        ]
    );
    // The Host is pinned before the tunnel opens, and the relay too.
    let before = fx.host.seen_before_connect.lock().unwrap().clone();
    assert_eq!(
        before,
        [
            "phase:setup",
            "phase:approve",
            "phase:clone",
            "mode:start",
            "mode:follow",
            "gitconfig",
            "bootstrap",
            "relay"
        ]
    );
    assert!(matches!(events.last(), Some(StartEvent::Ready { .. })));
    assert_eq!(binding.remote_cwd, "/workspaces/widgets");
    assert_eq!(binding.codespace, fx.record_name(&binding));
    assert!(binding.machine_id.starts_with("m-monoptah-fix-login-"));
    // The project was opened at the container's folder and bound to the sandbox.
    let remote = fx
        .store
        .lock()
        .unwrap()
        .remote_session(session.id)
        .unwrap()
        .unwrap();
    assert_eq!(remote.project.cwd, "/workspaces/widgets");
    assert_eq!(
        remote.project.sandbox.as_deref(),
        Some(binding.codespace.as_str())
    );
    assert_eq!(remote.project.codespace, None);
    assert_eq!(remote.project.environment_id, binding.environment_id);
    assert!(
        fx.record(session.id).is_none(),
        "the record is removed once ready"
    );
    assert!(fx
        .host
        .log
        .lock()
        .unwrap()
        .iter()
        .any(|l| l.contains("projects.open \"/workspaces/widgets\"")));
    // The relay port is the deterministic one.
    assert!(fx
        .host
        .log
        .lock()
        .unwrap()
        .iter()
        .any(|l| l.ends_with(" 47200")));
}
impl Fx {
    fn record_name(&self, binding: &CloudBinding) -> String {
        binding.codespace.clone()
    }
}

#[test]
fn steps_follow_the_up_log() {
    let fx = fx();
    let (session, _) = fx.begin().unwrap();
    let (result, events) = fx.run(session.id);
    result.unwrap();
    let last = events
        .iter()
        .rev()
        .find_map(|e| match e {
            StartEvent::Updated(r) => Some(r.clone()),
            _ => None,
        })
        .unwrap();
    for kind in [
        StepKind::Create,
        StepKind::Provision,
        StepKind::BuildContainer,
        StepKind::BlockingCommands,
        StepKind::ConfigureCodespace,
        StepKind::Connect,
        StepKind::OpenProject,
        StepKind::LifecycleCommands,
    ] {
        assert_eq!(status(&last, kind), StepStatus::Done, "{kind:?}");
    }
    // The blocking-commands step starts at the progress line and the container build ends there.
    let order: Vec<(StepKind, StepStatus)> = events
        .iter()
        .filter_map(|e| match e {
            StartEvent::Updated(r) => Some((
                StepKind::BlockingCommands,
                status(r, StepKind::BlockingCommands),
            )),
            _ => None,
        })
        .collect();
    assert!(order.iter().any(|(_, s)| *s == StepStatus::Running));
    let logs: Vec<String> = events
        .iter()
        .filter_map(|e| match e {
            StartEvent::Log(l) => Some(l.clone()),
            _ => None,
        })
        .collect();
    assert!(logs.iter().any(|l| l == "Starting container"), "{logs:?}");
    assert!(
        logs.iter()
            .any(|l| l == "Running onCreateCommand... (running)"),
        "{logs:?}"
    );
    assert!(logs.iter().any(|l| l == "Container ready"));
    assert!(logs.iter().any(|l| l.contains("cloning the repository")));
    assert!(
        !logs.iter().any(|l| l.contains("MONOPTAH_")),
        "markers are not log lines"
    );
}

#[test]
fn a_build_without_blocking_commands_skips_that_step() {
    let fx = fx();
    *fx.shell.up_log.lock().unwrap() = vec![format!(
        r#"{{"outcome":"success","containerId":"{CONTAINER}","remoteUser":"root","remoteWorkspaceFolder":"/workspaces/widgets"}}"#
    )];
    let (session, _) = fx.begin().unwrap();
    let (_, events) = fx.run(session.id);
    let last = events
        .iter()
        .rev()
        .find_map(|e| match e {
            StartEvent::Updated(r) => Some(r.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        status(&last, StepKind::BlockingCommands),
        StepStatus::Skipped
    );
    assert_eq!(status(&last, StepKind::BuildContainer), StepStatus::Done);
}

#[test]
fn the_provision_script_has_no_secret_and_the_credential_is_stdin_only() {
    let fx = fx();
    let (session, _) = fx.begin().unwrap();
    fx.run(session.id).0.unwrap();
    for call in fx.shell.calls() {
        let tag = call.tag();
        assert!(
            !call.remote.contains(GIT_TOKEN) && !call.remote.contains(SECRET),
            "{tag}: argv"
        );
        if tag != "phase:approve" {
            assert!(
                !call.stdin.contains(GIT_TOKEN),
                "{tag}: stdin holds the git token"
            );
        }
        if tag != "bootstrap" {
            assert!(
                !call.stdin.contains(SECRET),
                "{tag}: stdin holds the Claude secret"
            );
        }
    }
    // The credential arrives as stdin of the approve call, after a script that is in argv.
    let approve = fx.shell.call("phase:approve");
    assert!(approve
        .stdin
        .starts_with("protocol=https\nhost=github.com\npath=octo/widgets.git"));
    assert!(approve.stdin.contains(GIT_TOKEN));
    assert!(approve.remote.starts_with("sh -c '"));
    assert!(approve.remote.contains("credential approve"));
    assert!(approve
        .remote
        .replace("'\\''", "'")
        .contains("PHASE='approve'"));
    // The Claude secret travels in the bootstrap's stdin only.
    let bootstrap = fx.shell.call("bootstrap");
    assert!(bootstrap.stdin.starts_with("unset ANTHROPIC_AUTH_TOKEN ANTHROPIC_API_KEY CLAUDE_CODE_OAUTH_TOKEN\nexport CLAUDE_CODE_OAUTH_TOKEN='"));
    assert!(bootstrap.stdin.contains(SECRET));
    assert!(!bootstrap.remote.contains(SECRET));
    assert!(bootstrap
        .remote
        .contains("devcontainer exec --workspace-folder"));
    assert!(bootstrap.remote.ends_with("sh -l -s"));
    // The bootstrap text after the prelude is the sandbox bootstrap, codespace flags included.
    assert!(bootstrap
        .stdin
        .contains("MONOCODE_HOST_TRACK_TOKEN=1\nMONOCODE_HOST_INSTALL_CLAUDE=1\n"));
    assert!(bootstrap.stdin.contains("MONOCODE_HOST_DETACHED=1\n"));
    assert!(bootstrap.stdin.contains("\nset -eu\n"));
}

#[test]
fn the_clone_and_up_scripts_carry_the_validated_settings() {
    let fx = fx();
    let (session, _) = fx.begin().unwrap();
    fx.run(session.id).0.unwrap();
    let clone = fx.shell.call("phase:clone");
    assert!(clone.stdin.contains(&format!("REPO_URL='{REPO}'\n")));
    assert!(clone.stdin.contains("REPO_DIR='widgets'\n"));
    assert!(clone.stdin.contains("BRANCH='fix/login'\n"));
    assert_eq!(clone.remote, "sh -s");
    assert_eq!(clone.id, NEW_ID);
    let up = fx.shell.call("mode:start");
    assert!(up.stdin.contains(&format!("WS='{WORKSPACE}'\n")));
    assert!(up.stdin.contains("MOUNT='1'\n"));
    assert!(up.stdin.contains("MOUNT_DST='/run/monoptah-git'\n"));
    assert!(up
        .stdin
        .contains("--skip-non-blocking-commands --log-format json"));
    assert!(
        up.stdin.contains("</dev/null"),
        "devcontainer up must not read stdin"
    );
    let relay = fx.shell.call("relay");
    assert!(relay.stdin.contains(&format!("CID='{CONTAINER}'\n")));
    assert!(relay.stdin.contains("RUN_USER='vscode'\n"));
    assert!(relay.stdin.contains("PORT='47200'\n"));
    assert!(relay.stdin.contains("HOST_PORT='3774'\n"));
    assert!(relay.stdin.contains("bind=127.0.0.1"));
    assert!(
        relay.stdin.contains("monocode-host/runtime-path"),
        "the inner end is the Host's own Node"
    );
    assert!(
        !relay.stdin.contains("socat - TCP"),
        "no socat inside the container"
    );
    let git = fx.shell.call("gitconfig");
    assert!(git.stdin.contains("WANT_CRED='1'\n"));
    assert!(git.stdin.contains(&format!("CLONE_URL='{REPO}'\n")));
    assert!(git.stdin.contains("SOCKET='/run/monoptah-git/sock'\n"));
}

#[test]
fn a_public_repo_skips_the_credential_and_the_mount() {
    let fx = fx();
    let mut cfg = config();
    cfg.credential_id = None;
    let (session, _) = fx
        .svc
        .begin_sandbox_start("/Users/me/widgets", &cfg, "main", "claude")
        .unwrap();
    // The fake secrets assert the credential id, so a public repo must not ask for a payload.
    fx.run(session.id).0.unwrap();
    assert!(!fx.shell.tags().contains(&"phase:approve".to_string()));
    assert!(fx.shell.call("mode:start").stdin.contains("MOUNT='0'\n"));
    assert!(fx.shell.call("gitconfig").stdin.contains("WANT_CRED='0'\n"));
}

#[test]
fn a_non_default_devcontainer_path_reaches_up_and_exec() {
    let fx = fx();
    let mut cfg = config();
    cfg.devcontainer_path = ".devcontainer/web/devcontainer.json".into();
    let (session, _) = fx
        .svc
        .begin_sandbox_start("/Users/me/widgets", &cfg, "main", "claude")
        .unwrap();
    fx.run(session.id).0.unwrap();
    assert!(fx
        .shell
        .call("mode:start")
        .stdin
        .contains("CONFIG='.devcontainer/web/devcontainer.json'\n"));
    let machine = fx.host.machines.lock().unwrap()[0].sandbox.clone().unwrap();
    assert_eq!(
        machine.config.as_deref(),
        Some(".devcontainer/web/devcontainer.json")
    );
    assert!(fx.shell.call("bootstrap").remote.contains(&format!(
        "--config '{WORKSPACE}/.devcontainer/web/devcontainer.json'"
    )));
}

#[test]
fn an_api_key_account_hashes_its_own_variable() {
    let fx = fx_with(
        CreateMode::Ok,
        Some(("ANTHROPIC_API_KEY", "sk-ant-api03-KEYKEYKEY".into())),
    );
    let (session, _) = fx.begin().unwrap();
    fx.run(session.id).0.unwrap();
    let stdin = fx.shell.call("bootstrap").stdin;
    assert!(stdin.contains("MONOCODE_HOST_TOKEN_VAR=ANTHROPIC_API_KEY\n"));
    assert!(stdin.contains("export ANTHROPIC_API_KEY='sk-ant-api03-KEYKEYKEY'"));
    assert!(!stdin.contains("export CLAUDE_CODE_OAUTH_TOKEN"));
}

#[test]
fn failures_are_redacted_and_name_the_phase() {
    let fx = fx();
    let (session, _) = fx.begin().unwrap();
    fx.shell.fail(
        "phase:clone",
        &format!("fatal: could not read from https://me:{GIT_TOKEN}@github.com/octo/widgets.git using {SECRET}"),
    );
    let (result, events) = fx.run(session.id);
    let error = result.unwrap_err();
    assert!(
        error.starts_with("Cloning the repository failed"),
        "{error}"
    );
    assert!(
        !error.contains(GIT_TOKEN) && !error.contains(SECRET),
        "{error}"
    );
    let Some(StartEvent::Failed(record)) = events.last() else {
        panic!("{events:?}")
    };
    assert_eq!(record.failed_at, Some(StepKind::Provision));
    let stored = fx.record(session.id).unwrap();
    assert!(stored.is_failed());
    let all = serde_json::to_string(&stored).unwrap();
    assert!(!all.contains(GIT_TOKEN) && !all.contains(SECRET));
    for event in &events {
        if let StartEvent::Log(line) = event {
            assert!(!line.contains(GIT_TOKEN) && !line.contains(SECRET));
        }
    }
}

#[test]
fn a_failed_build_reports_the_cli_message() {
    let fx = fx();
    *fx.shell.up_log.lock().unwrap() = vec![
        r#"{"type":"progress","name":"Running onCreateCommand...","status":"failed"}"#.into(),
        r#"{"outcome":"error","message":"x","description":"The command failed with exit 2"}"#
            .into(),
    ];
    *fx.shell.up_exit.lock().unwrap() = "1".into();
    let (session, _) = fx.begin().unwrap();
    let (result, _) = fx.run(session.id);
    let error = result.unwrap_err();
    assert!(error.contains("The command failed with exit 2"), "{error}");
    assert!(!fx.shell.tags().contains(&"relay".to_string()));
    let stored = fx.record(session.id).unwrap();
    assert!(matches!(
        stored.failed_at,
        Some(StepKind::BuildContainer | StepKind::BlockingCommands)
    ));
}

#[test]
fn a_dropped_log_connection_is_followed_again_without_repeating_lines() {
    let fx = fx();
    *fx.shell.follow_drops.lock().unwrap() = 1;
    let (session, _) = fx.begin().unwrap();
    let (result, events) = fx.run(session.id);
    result.unwrap();
    assert_eq!(
        fx.shell
            .tags()
            .iter()
            .filter(|t| *t == "mode:follow")
            .count(),
        2
    );
    let logs: Vec<&String> = events
        .iter()
        .filter_map(|e| match e {
            StartEvent::Log(l) => Some(l),
            _ => None,
        })
        .collect();
    assert_eq!(
        logs.iter()
            .filter(|l| l.as_str() == "Resolving Remote")
            .count(),
        1
    );
}

#[test]
fn a_deleted_sandbox_fails_the_start_and_is_never_recreated() {
    let fx = fx();
    let (session, _) = fx.begin().unwrap();
    fx.sbx.remove(NEW_ID);
    let (result, events) = fx.run(session.id);
    assert_eq!(
        result.unwrap_err(),
        "Sandbox was deleted. Start a new session."
    );
    assert!(matches!(events.last(), Some(StartEvent::Failed(_))));
    assert_eq!(fx.sbx.count("create"), 1, "no second create");
    assert!(
        fx.shell.calls().is_empty(),
        "nothing ran in a sandbox that is gone"
    );
    assert!(fx.record(session.id).unwrap().is_failed());
}

#[test]
fn retry_after_a_failure_resumes_without_redoing_finished_work() {
    let fx = fx();
    let (session, _) = fx.begin().unwrap();
    fx.shell.fail("relay", "socat: command not found");
    let (result, _) = fx.run(session.id);
    assert!(result.unwrap_err().contains("Starting the relay failed"));
    let stored = fx.record(session.id).unwrap();
    assert_eq!(status(&stored, StepKind::Provision), StepStatus::Done);
    assert_eq!(status(&stored, StepKind::BuildContainer), StepStatus::Done);
    assert_eq!(
        stored.sandbox_start.as_ref().unwrap().workspace.as_deref(),
        Some(WORKSPACE)
    );
    *fx.shell.fail_tag.lock().unwrap() = None;
    let before = fx.shell.tags().len();
    let (result, _) = fx.run(session.id);
    result.unwrap();
    let after: Vec<String> = fx.shell.tags()[before..].to_vec();
    assert!(!after.contains(&"phase:setup".to_string()), "{after:?}");
    assert!(!after.contains(&"phase:clone".to_string()), "{after:?}");
    // The container is checked again (a resumed sandbox may have stopped it) and the
    // credential cache refreshed.
    assert!(after.contains(&"mode:start".to_string()));
    assert!(after.contains(&"phase:approve".to_string()));
    assert_eq!(fx.sbx.count("create"), 1);
}

#[test]
fn a_crash_between_the_record_and_the_id_is_reconciled_on_run() {
    let fx = fx();
    let (session, record) = fx.begin().unwrap();
    // Forget the id as if the app died right after `create`.
    let mut crashed = record.clone();
    crashed.sandbox_start.as_mut().unwrap().sandbox_id = None;
    crashed.steps[0] = Step::pending(StepKind::Create);
    save_start_record(&fx.store.lock().unwrap(), &crashed).unwrap();
    *fx.sbx.mode.lock().unwrap() = CreateMode::AlreadyExists;
    let (result, _) = fx.run(session.id);
    result.unwrap();
    assert_eq!(
        fx.sbx.count("create"),
        1,
        "the name was looked up before creating"
    );
    assert_eq!(fx.sbx.sandboxes.lock().unwrap().len(), 1);
    assert!(fx.shell.calls().iter().all(|c| c.id == NEW_ID));
}

#[test]
fn resume_does_not_wake_a_stopped_sandbox() {
    let fx = fx();
    let (session, _) = fx.begin().unwrap();
    fx.sbx.set_state(NEW_ID, "stopped");
    let mut events = vec![];
    let result = fx
        .svc
        .resume_start(session.id, &mut |e| events.push(e), &AtomicBool::new(false));
    assert_eq!(
        result.unwrap_err(),
        "Sandbox stopped. Choose Retry to start it again."
    );
    assert!(fx.shell.calls().is_empty(), "no ssh");
    assert!(fx.record(session.id).unwrap().is_failed());
}

#[test]
fn rebuild_resets_the_steps_and_the_next_run_removes_the_container() {
    let fx = fx();
    let (session, _) = fx.begin().unwrap();
    fx.shell.fail("relay", "boom");
    let _ = fx.run(session.id);
    *fx.shell.fail_tag.lock().unwrap() = None;
    let rebuilt = fx.svc.rebuild_start(session.id).unwrap();
    assert!(rebuilt.sandbox_start.as_ref().unwrap().rebuild);
    assert_eq!(
        status(&rebuilt, StepKind::BuildContainer),
        StepStatus::Pending
    );
    assert_eq!(status(&rebuilt, StepKind::Provision), StepStatus::Done);
    assert_eq!(status(&rebuilt, StepKind::Create), StepStatus::Done);
    assert!(!rebuilt.is_failed());
    assert_eq!(fx.sbx.count("create"), 1, "same sandbox");
    let before = fx.shell.tags().len();
    fx.run(session.id).0.unwrap();
    let tags: Vec<String> = fx.shell.tags()[before..].to_vec();
    assert_eq!(tags[1], "mode:rebuild", "{tags:?}");
    assert!(!tags.contains(&"phase:clone".to_string()));
    let rebuild = fx.shell.calls()[before + 1].clone();
    assert!(rebuild.stdin.contains("--remove-existing-container"));
}

#[test]
fn rebuild_refuses_a_start_that_was_never_provisioned() {
    let fx = fx();
    let (session, _) = fx.begin().unwrap();
    assert!(fx
        .svc
        .rebuild_start(session.id)
        .unwrap_err()
        .contains("not provisioned"));
}

#[test]
fn a_cancelled_start_reports_neither_ready_nor_failed() {
    let fx = fx();
    let (session, _) = fx.begin().unwrap();
    let mut events = vec![];
    let cancel = AtomicBool::new(true);
    let result = fx
        .svc
        .run_start(session.id, &mut |e| events.push(e), &cancel);
    assert_eq!(result.unwrap_err(), START_ABORTED);
    assert!(events.is_empty());
}

#[test]
fn the_backend_resolves_names_to_ids_without_ssh() {
    let fx = fx();
    let backend = fx.svc.backend(CloudBackendKind::DockerSandbox).unwrap();
    fx.sbx.add("sbx_001other", "monoptah-x-111111", "stopped");
    assert_eq!(backend.state("monoptah-x-111111").unwrap(), "stopped");
    assert!(backend.is_stopped("stopped"));
    assert!(backend.exists("monoptah-x-111111").unwrap());
    let missing = backend.state("monoptah-nope-000000").unwrap_err();
    assert!(backend.is_missing_error(&missing), "{missing}");
    assert!(!backend.exists("monoptah-nope-000000").unwrap());
    assert!(!backend.is_missing_error("connection refused"));
    assert!(!backend.is_missing_error("error: runtime 'x' not found"));
    assert!(backend.is_missing_error("error: sandbox 'sbx_001x' not found"));
    backend.delete("monoptah-x-111111").unwrap();
    assert!(fx.shell.calls().is_empty());
}

#[test]
fn codespace_starts_still_use_gh_and_ignore_sandbox_settings() {
    // The sandbox registry is additive: the codespace backend is untouched.
    let fx = fx();
    assert!(fx.svc.backend(CloudBackendKind::Codespaces).is_ok());
    assert!(fx.svc.backend(CloudBackendKind::DockerSandbox).is_ok());
}

// ------------------------------------------------- the embedded scripts, run for real

#[cfg(unix)]
mod scripts {
    use super::*;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};

    fn executable(path: &Path, body: &str) {
        std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    /// A fake outer sandbox: HOME plus a PATH dir whose `setsid` just runs its command.
    struct Outer {
        home: tempfile::TempDir,
        bin: tempfile::TempDir,
    }
    impl Outer {
        fn new() -> Self {
            let outer = Self {
                // Short path: a unix socket lives under HOME.
                home: tempfile::tempdir_in("/tmp").unwrap(),
                bin: tempfile::tempdir().unwrap(),
            };
            executable(
                &outer.bin.path().join("setsid"),
                "[ \"$1\" = -w ] && shift\nexec \"$@\"",
            );
            outer
        }
        /// Like [`Self::run`] but the script is `sh -c` argv, so stdin is free for a payload.
        fn run_c(&self, script: &str, stdin: &str) -> (bool, String, String) {
            let path = format!(
                "{}:{}",
                self.bin.path().display(),
                std::env::var("PATH").unwrap()
            );
            let mut child = Command::new("sh")
                .arg("-c")
                .arg(script)
                .env("HOME", self.home.path())
                .env("PATH", path)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(stdin.as_bytes())
                .unwrap();
            let out = child.wait_with_output().unwrap();
            (
                out.status.success(),
                String::from_utf8_lossy(&out.stdout).into_owned(),
                String::from_utf8_lossy(&out.stderr).into_owned(),
            )
        }
        fn run(&self, script: &str, stdin_after: &str) -> (bool, String, String) {
            let path = format!(
                "{}:{}",
                self.bin.path().display(),
                std::env::var("PATH").unwrap()
            );
            let mut child = Command::new("sh")
                .arg("-s")
                .env("HOME", self.home.path())
                .env("PATH", path)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let mut stdin = child.stdin.take().unwrap();
            stdin.write_all(script.as_bytes()).unwrap();
            stdin.write_all(stdin_after.as_bytes()).unwrap();
            drop(stdin);
            let out = child.wait_with_output().unwrap();
            (
                out.status.success(),
                String::from_utf8_lossy(&out.stdout).into_owned(),
                String::from_utf8_lossy(&out.stderr).into_owned(),
            )
        }
    }

    fn git(dir: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    }
    fn upstream() -> tempfile::TempDir {
        let repo = tempfile::tempdir().unwrap();
        git(repo.path(), &["init", "-q", "-b", "main"]);
        std::fs::write(repo.path().join("a.txt"), "main\n").unwrap();
        git(repo.path(), &["add", "."]);
        git(repo.path(), &["commit", "-q", "-m", "one"]);
        git(repo.path(), &["checkout", "-q", "-b", "dev"]);
        std::fs::write(repo.path().join("a.txt"), "dev\n").unwrap();
        git(repo.path(), &["commit", "-qam", "dev"]);
        git(repo.path(), &["checkout", "-q", "main"]);
        repo
    }
    fn clone_script(url: &str, base: &str) -> String {
        with_vars(
            &[
                ("PHASE", "clone"),
                ("REPO_URL", url),
                ("REPO_DIR", "w"),
                ("BASE_REF", base),
                ("BRANCH", "feat/x"),
            ],
            PROVISION_SCRIPT,
        )
    }

    #[test]
    fn the_clone_phase_is_idempotent_and_never_touches_a_workspace_with_changes() {
        let outer = Outer::new();
        let repo = upstream();
        let url = repo.path().display().to_string();
        let script = clone_script(&url, "dev");
        let (ok, stdout, stderr) = outer.run(&script, "");
        assert!(ok, "{stderr}");
        let ws = outer.home.path().join("workspaces/w");
        assert!(stdout.contains(&format!("MONOPTAH_WORKSPACE={}", ws.display())));
        let head = |ws: &Path| {
            String::from_utf8(
                Command::new("git")
                    .args([
                        "-C",
                        &ws.display().to_string(),
                        "rev-parse",
                        "--abbrev-ref",
                        "HEAD",
                    ])
                    .output()
                    .unwrap()
                    .stdout,
            )
            .unwrap()
            .trim()
            .to_string()
        };
        assert_eq!(head(&ws), "feat/x");
        assert_eq!(
            std::fs::read_to_string(ws.join("a.txt")).unwrap(),
            "dev\n",
            "branched from the base ref"
        );
        // Local work survives a second run, and so does a commit on the branch.
        std::fs::write(ws.join("a.txt"), "my work\n").unwrap();
        let (ok, _, stderr) = outer.run(&script, "");
        assert!(ok, "{stderr}");
        assert_eq!(
            std::fs::read_to_string(ws.join("a.txt")).unwrap(),
            "my work\n"
        );
        assert_eq!(head(&ws), "feat/x");
        // A folder that is not a checkout is refused and left alone.
        let other = outer.home.path().join("workspaces/w2");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("keep"), "x").unwrap();
        let script = with_vars(
            &[
                ("PHASE", "clone"),
                ("REPO_URL", &url),
                ("REPO_DIR", "w2"),
                ("BASE_REF", ""),
                ("BRANCH", "b"),
            ],
            PROVISION_SCRIPT,
        );
        let (ok, _, stderr) = outer.run(&script, "");
        assert!(!ok);
        assert!(stderr.contains("not a Git checkout"), "{stderr}");
        assert!(other.join("keep").exists());
        // An unknown base ref is an error, not a silent fallback.
        let bad = clone_script(&url, "nope");
        let script = bad.replace("REPO_DIR='w'", "REPO_DIR='w3'");
        let (ok, _, stderr) = outer.run(&script, "");
        assert!(!ok);
        assert!(stderr.contains("does not exist"), "{stderr}");
    }

    #[test]
    fn up_runs_detached_with_the_documented_flags_and_follow_replays_the_log() {
        let outer = Outer::new();
        let dc = outer.home.path().join(".devcontainers/bin");
        std::fs::create_dir_all(&dc).unwrap();
        let argv = outer.home.path().join("dc-argv");
        let stdin_seen = outer.home.path().join("dc-stdin");
        executable(
            &dc.join("devcontainer"),
            &format!(
                "printf '%s\\n' \"$@\" > {}\ncat > {}\necho '{{\"type\":\"start\",\"text\":\"Starting container\"}}' >&2\nsleep 1\necho '{{\"outcome\":\"success\",\"containerId\":\"abcdef012345\",\"remoteUser\":\"u\",\"remoteWorkspaceFolder\":\"/workspaces/w\"}}'",
                argv.display(),
                stdin_seen.display()
            ),
        );
        let up = |mode: &str| {
            with_vars(
                &[
                    ("MODE", mode),
                    ("WS", "/home/agent/workspaces/w"),
                    ("CONFIG", ".devcontainer/web/devcontainer.json"),
                    ("MOUNT", "1"),
                    ("MOUNT_DST", "/run/monoptah-git"),
                ],
                UP_SCRIPT,
            )
        };
        let (ok, stdout, stderr) = outer.run(&up("start"), "");
        assert!(ok, "{stderr}");
        assert!(stdout.contains("MONOPTAH_UP=started"));
        let (ok, stdout, stderr) = outer.run(&up("follow"), "");
        assert!(ok, "{stderr}");
        assert!(stdout.contains("Starting container"), "{stdout}");
        assert!(stdout.contains("\"outcome\":\"success\""), "{stdout}");
        assert!(
            stdout.trim_end().ends_with("MONOPTAH_UP_EXIT=0"),
            "{stdout}"
        );
        let args = std::fs::read_to_string(&argv).unwrap();
        let args: Vec<&str> = args.lines().collect();
        assert_eq!(args[0], "up");
        for want in [
            "--workspace-folder",
            "/home/agent/workspaces/w",
            "--skip-non-blocking-commands",
            "--log-format",
            "json",
            "--config",
            "/home/agent/workspaces/w/.devcontainer/web/devcontainer.json",
            "--mount",
        ] {
            assert!(args.contains(&want), "{want} in {args:?}");
        }
        assert!(args.iter().any(|a| a.starts_with("type=bind,source=")
            && a.ends_with("/.monoptah/git,target=/run/monoptah-git")));
        assert!(!args.contains(&"--remove-existing-container"));
        assert_eq!(
            std::fs::read_to_string(&stdin_seen).unwrap(),
            "",
            "devcontainer up reads /dev/null"
        );
        // Rebuild adds the flag; a finished build is started again.
        let (ok, stdout, stderr) = outer.run(&up("rebuild"), "");
        assert!(ok, "{stderr}");
        assert!(stdout.contains("MONOPTAH_UP=started"));
        outer.run(&up("follow"), "");
        assert!(std::fs::read_to_string(&argv)
            .unwrap()
            .contains("--remove-existing-container"));
        // Lifecycle commands are started detached.
        let (ok, stdout, stderr) = outer.run(&up("lifecycle"), "");
        assert!(ok, "{stderr}");
        assert!(stdout.contains("MONOPTAH_LIFECYCLE=started"));
    }

    #[test]
    fn the_relay_listens_on_loopback_bridges_through_the_hosts_node_and_follows_the_container() {
        let outer = Outer::new();
        let socat_args = outer.home.path().join("socat-args");
        executable(
            &outer.bin.path().join("socat"),
            &format!("printf '%s\\n' \"$@\" > {}\nsleep 30", socat_args.display()),
        );
        let relay = |cid: &str| {
            with_vars(
                &[
                    ("CID", cid),
                    ("RUN_USER", "vscode"),
                    ("PORT", "47200"),
                    ("HOST_PORT", "3774"),
                ],
                RELAY_SCRIPT,
            )
        };
        let (ok, stdout, stderr) = outer.run(&relay("aaaaaaaaaaaa"), "");
        assert!(ok, "{stderr}");
        assert!(stdout.contains("MONOPTAH_RELAY=started"));
        let run = outer.home.path().join(".monoptah/run");
        let pid: String = std::fs::read_to_string(run.join("relay-47200.pid")).unwrap();
        for _ in 0..100 {
            if socat_args.exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let args = std::fs::read_to_string(&socat_args).unwrap();
        let args: Vec<&str> = args.lines().collect();
        assert_eq!(args[0], "TCP-LISTEN:47200,bind=127.0.0.1,reuseaddr,fork");
        assert!(args[1].starts_with("EXEC:sh ") && args[1].ends_with("relay-47200-inner.sh"));
        let inner = std::fs::read_to_string(run.join("relay-47200-inner.sh")).unwrap();
        assert!(inner
            .contains("docker exec -i -u vscode -e MONOPTAH_HOST_PORT=3774 aaaaaaaaaaaa sh -c"));
        assert!(inner.contains("monocode-host/runtime-path"));
        let bridge = std::fs::read_to_string(run.join("relay-bridge.js")).unwrap();
        assert!(bridge.contains("127.0.0.1") && bridge.contains("process.stdin.pipe"));
        // Same container: left alone. Another container (rebuild): the old relay is replaced.
        let (_, stdout, _) = outer.run(&relay("aaaaaaaaaaaa"), "");
        assert!(stdout.contains("MONOPTAH_RELAY=running"));
        let (ok, stdout, stderr) = outer.run(&relay("bbbbbbbbbbbb"), "");
        assert!(ok, "{stderr}");
        assert!(stdout.contains("MONOPTAH_RELAY=started"));
        assert!(std::fs::read_to_string(run.join("relay-47200-inner.sh"))
            .unwrap()
            .contains("bbbbbbbbbbbb"));
        let new_pid: String = std::fs::read_to_string(run.join("relay-47200.pid")).unwrap();
        assert_ne!(pid, new_pid);
        let _ = Command::new("kill")
            .arg(new_pid.trim())
            .stderr(Stdio::null())
            .status();
        let _ = Command::new("kill")
            .arg(pid.trim())
            .stderr(Stdio::null())
            .status();
    }

    #[test]
    fn the_container_git_script_scopes_the_helper_to_the_exact_repo() {
        let outer = Outer::new();
        let git_dir: PathBuf = outer.home.path().join("gitconfig");
        let script = with_vars(
            &[
                ("WANT_CRED", "1"),
                ("CLONE_URL", "https://github.com/octo/widgets.git"),
                ("SOCKET", "/run/monoptah-git/sock"),
            ],
            GIT_CONFIG_SCRIPT,
        );
        let path = format!(
            "{}:{}",
            outer.bin.path().display(),
            std::env::var("PATH").unwrap()
        );
        let mut child = Command::new("sh")
            .arg("-s")
            .env("HOME", outer.home.path())
            .env("PATH", path)
            .env("GIT_CONFIG_GLOBAL", &git_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(script.as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let config = std::fs::read_to_string(&git_dir).unwrap();
        assert!(
            config.contains("[credential \"https://github.com/octo/widgets.git\"]"),
            "{config}"
        );
        assert!(config.contains("helper = cache --socket=/run/monoptah-git/sock"));
        assert!(config.contains("useHttpPath = true"));
        // Nothing secret is involved at all.
        assert!(!config.contains("password") && !config.contains("token"));
    }

    #[test]
    fn real_credential_cache_serves_only_the_configured_url_and_hides_the_token() {
        const TOKEN: &str = "ghp_REALGITTOKEN987654321";
        let outer = Outer::new();
        let script = with_vars(
            &[("PHASE", "approve"), ("CACHE_TIMEOUT", "600")],
            PROVISION_SCRIPT,
        );
        // The script that goes into argv holds no secret.
        assert!(!script.contains(TOKEN));
        let payload = format!(
            "protocol=https\nhost=github.com\npath=octo/widgets.git\nusername=me\npassword={TOKEN}\n\n"
        );
        let (ok, stdout, stderr) = outer.run_c(&script, &payload);
        assert!(ok, "{stderr}");
        assert!(stdout.contains("MONOPTAH_CREDENTIAL=cached"));
        let sock = outer.home.path().join(".monoptah/git/sock");
        let sock = sock.display().to_string();
        let fill = |path: &str| {
            let mut child = Command::new("git")
                .args([
                    "-c",
                    "credential.helper=",
                    "-c",
                    &format!("credential.helper=cache --socket={sock}"),
                    "-c",
                    "credential.useHttpPath=true",
                    "credential",
                    "fill",
                ])
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_TERMINAL_PROMPT", "0")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(format!("protocol=https\nhost=github.com\npath={path}\n\n").as_bytes())
                .unwrap();
            child.wait_with_output().unwrap()
        };
        let exact = fill("octo/widgets.git");
        assert!(String::from_utf8_lossy(&exact.stdout).contains(&format!("password={TOKEN}")));
        let other = fill("octo/other.git");
        assert!(!other.status.success());
        assert!(!String::from_utf8_lossy(&other.stdout).contains(TOKEN));
        // The daemon's command line holds no secret.
        let ps = Command::new("ps").args(["-axo", "args="]).output().unwrap();
        let ps = String::from_utf8_lossy(&ps.stdout).into_owned();
        let daemon: Vec<&str> = ps
            .lines()
            .filter(|l| l.contains("credential-cache--daemon") && l.contains(&sock))
            .collect();
        assert!(!daemon.is_empty(), "the cache daemon should be running");
        assert!(daemon.iter().all(|l| !l.contains(TOKEN)));
        assert!(!ps.contains(TOKEN), "no process has the token in its argv");
        let _ = Command::new("git")
            .args(["credential-cache", "--socket", &sock, "exit"])
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .status();
    }

    #[test]
    fn a_rebuild_is_refused_while_a_start_build_runs_and_a_second_launcher_waits() {
        let outer = Outer::new();
        let dc = outer.home.path().join(".devcontainers/bin");
        std::fs::create_dir_all(&dc).unwrap();
        executable(&dc.join("devcontainer"), "sleep 5");
        let up = |mode: &str| {
            with_vars(
                &[
                    ("MODE", mode),
                    ("WS", "/home/agent/workspaces/w"),
                    ("CONFIG", ""),
                    ("MOUNT", "0"),
                    ("MOUNT_DST", "/x"),
                ],
                UP_SCRIPT,
            )
        };
        let (ok, stdout, _) = outer.run(&up("start"), "");
        assert!(ok && stdout.contains("started"));
        let (ok, stdout, _) = outer.run(&up("start"), "");
        assert!(ok && stdout.contains("MONOPTAH_UP=running"), "{stdout}");
        let (ok, _, stderr) = outer.run(&up("rebuild"), "");
        assert!(!ok);
        assert!(stderr.contains("already running"), "{stderr}");
        let pid = std::fs::read_to_string(outer.home.path().join(".monoptah/run/up.pid")).unwrap();
        let _ = Command::new("kill")
            .arg(pid.trim())
            .stderr(Stdio::null())
            .status();
        // A pid that is not our wrapper is not trusted: a stale file does not block a start.
        std::fs::write(outer.home.path().join(".monoptah/run/up.pid"), "1\n").unwrap();
        let (ok, stdout, stderr) = outer.run(&up("start"), "");
        assert!(
            ok && stdout.contains("MONOPTAH_UP=started"),
            "{stdout}{stderr}"
        );
    }
}

#[test]
fn resume_only_continues_a_running_sandbox() {
    for state in [
        "stopping",
        "starting",
        "failed",
        "deleting",
        "weird-new-state",
        "stopped",
    ] {
        let fx = fx();
        let (session, _) = fx.begin().unwrap();
        fx.sbx.set_state(NEW_ID, state);
        let mut events = vec![];
        let result =
            fx.svc
                .resume_start(session.id, &mut |e| events.push(e), &AtomicBool::new(false));
        let error = result.unwrap_err();
        assert!(error.contains("Choose Retry"), "{state}: {error}");
        assert!(fx.shell.calls().is_empty(), "{state}: no ssh");
        assert!(fx.record(session.id).unwrap().is_failed(), "{state}");
    }
    let fx = fx();
    let (session, _) = fx.begin().unwrap();
    let mut events = vec![];
    fx.svc
        .resume_start(session.id, &mut |e| events.push(e), &AtomicBool::new(false))
        .unwrap();
    assert!(!fx.shell.calls().is_empty());
}

#[test]
fn a_broken_sandbox_fails_the_create_step_without_ssh() {
    for state in ["failed", "error", "deleting", "deleted"] {
        let fx = fx();
        let (session, _) = fx.begin().unwrap();
        fx.sbx.set_state(NEW_ID, state);
        let (result, _) = fx.run(session.id);
        assert!(result.is_err(), "{state}");
        assert!(fx.shell.calls().is_empty(), "{state}: no ssh");
        assert!(fx.record(session.id).unwrap().is_failed());
    }
}

#[test]
fn rebuild_forgets_the_paired_machine_because_the_container_is_new() {
    let fx = fx();
    let (session, record) = fx.begin().unwrap();
    fx.shell.fail("relay", "boom");
    let _ = fx.run(session.id);
    let sandbox_name = record.sandbox.clone().unwrap();
    let target = crate::remote::SandboxTarget {
        id: NEW_ID.into(),
        name: sandbox_name.clone(),
        workspace: WORKSPACE.into(),
        relay_port: 47200,
        config: None,
    };
    fx.host.machines.lock().unwrap().push(Machine {
        id: "m-old".into(),
        name: sandbox_name,
        endpoint: "sbx://x".into(),
        environment_id: "env-old".into(),
        ssh: None,
        codespace: None,
        sandbox: Some(target),
    });
    *fx.shell.fail_tag.lock().unwrap() = None;
    fx.svc.rebuild_start(session.id).unwrap();
    assert!(fx
        .host
        .log
        .lock()
        .unwrap()
        .contains(&"disconnect m-old".to_string()));
    assert!(fx.host.machines.lock().unwrap().is_empty());
}

#[test]
fn a_new_container_id_forgets_the_paired_machine() {
    let fx = fx();
    let (session, record) = fx.begin().unwrap();
    fx.shell.fail("relay", "boom");
    let _ = fx.run(session.id);
    let name = record.sandbox.clone().unwrap();
    fx.host.machines.lock().unwrap().push(Machine {
        id: "m-old".into(),
        name: name.clone(),
        endpoint: "sbx://x".into(),
        environment_id: "env-old".into(),
        ssh: None,
        codespace: None,
        sandbox: Some(crate::remote::SandboxTarget {
            id: NEW_ID.into(),
            name,
            workspace: WORKSPACE.into(),
            relay_port: 47200,
            config: None,
        }),
    });
    *fx.shell.fail_tag.lock().unwrap() = None;
    fx.shell.up_log.lock().unwrap()[4] = r#"{"outcome":"success","containerId":"fedcba9876543210ffff","remoteUser":"vscode","remoteWorkspaceFolder":"/workspaces/widgets"}"#.to_string();
    fx.run(session.id).0.unwrap();
    let log = fx.host.log.lock().unwrap().clone();
    let gone = log
        .iter()
        .position(|l| l == "disconnect m-old")
        .expect("{log:?}");
    let pair = log.iter().position(|l| l.starts_with("connect ")).unwrap();
    assert!(gone < pair, "{log:?}");
}

#[test]
fn the_credential_can_be_delivered_again_for_a_reconnect() {
    let fx = fx();
    fx.svc
        .deliver_git_credential(NEW_ID, &config(), &|| false)
        .unwrap();
    let calls = fx.shell.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].tag(), "phase:approve");
    assert!(calls[0].stdin.contains(GIT_TOKEN));
    assert!(!calls[0].remote.contains(GIT_TOKEN));
    assert!(calls[0].remote.contains("CACHE_TIMEOUT"));
    // A public repo has nothing to deliver.
    let mut public = config();
    public.credential_id = None;
    fx.svc
        .deliver_git_credential(NEW_ID, &public, &|| false)
        .unwrap();
    assert_eq!(fx.shell.calls().len(), 1);
    fx.shell
        .fail("phase:approve", &format!("denied {GIT_TOKEN}"));
    let error = fx
        .svc
        .deliver_git_credential(NEW_ID, &config(), &|| false)
        .unwrap_err();
    assert!(!error.contains(GIT_TOKEN), "{error}");
}

#[test]
fn ssh_noise_between_log_lines_is_not_counted_as_log() {
    let fx = fx();
    *fx.shell.follow_drops.lock().unwrap() = 2;
    let (session, _) = fx.begin().unwrap();
    let (result, events) = fx.run(session.id);
    result.unwrap();
    let logs: Vec<String> = events
        .iter()
        .filter_map(|e| match e {
            StartEvent::Log(l) => Some(l.clone()),
            _ => None,
        })
        .collect();
    assert!(
        !logs.iter().any(|l| l.contains("Connection to host")),
        "{logs:?}"
    );
    assert_eq!(
        logs.iter().filter(|l| *l == "Starting container").count(),
        1
    );
    assert_eq!(logs.iter().filter(|l| *l == "Container ready").count(), 1);
    let offsets: Vec<String> = fx
        .shell
        .calls()
        .iter()
        .filter(|c| c.tag() == "mode:follow")
        .map(|c| c.stdin.lines().next().unwrap().to_string())
        .collect();
    assert_eq!(offsets, ["OFFSET='0'", "OFFSET='2'", "OFFSET='4'"]);
}

// ------------------------------------------------------------------ reconnect

/// A finished start with its config saved, ready to be stopped and reconnected.
fn started() -> (Fx, i64, String) {
    let fx = fx();
    let (session, _) = fx.begin().unwrap();
    let binding = fx.run(session.id).0.unwrap();
    super::docker_config::save(&fx.store.lock().unwrap(), "/Users/me/widgets", &config()).unwrap();
    (fx, session.id, binding.codespace)
}
fn sandbox_id(fx: &Fx) -> String {
    fx.sbx.sandboxes.lock().unwrap()[0].0.clone()
}
impl Fx {
    fn reconnect(&self, id: i64) -> (Result<(), String>, Vec<String>) {
        let mut lines = vec![];
        let result = self.svc.reconnect_sandbox(
            id,
            &mut |l| lines.push(l.to_string()),
            &AtomicBool::new(false),
        );
        (result, lines)
    }
    /// Forgets what the start did, so only the reconnect shows.
    fn reset(&self) {
        self.shell.calls.lock().unwrap().clear();
        self.sbx.calls.lock().unwrap().clear();
        self.host.log.lock().unwrap().clear();
    }
}

#[test]
fn a_user_reconnect_resumes_a_stopped_sandbox_and_restores_everything() {
    let (fx, id, _) = started();
    fx.sbx.set_state(&sandbox_id(&fx), "stopped");
    fx.reset();
    let (result, lines) = fx.reconnect(id);
    result.unwrap();
    assert_eq!(
        fx.shell.tags(),
        [
            "mode:start",
            "mode:follow",
            "phase:approve",
            "gitconfig",
            "bootstrap",
            "relay",
            "evidence"
        ]
    );
    // Every call goes to the one sandbox; no secret in any remote command word.
    for call in fx.shell.calls() {
        assert_eq!(call.id, sandbox_id(&fx));
        assert!(!call.remote.contains(SECRET) && !call.remote.contains(GIT_TOKEN));
    }
    // The Claude prelude is on the Host bootstrap's stdin, the Git token on the approve's.
    assert!(fx.shell.call("bootstrap").stdin.contains(SECRET));
    assert!(fx.shell.call("phase:approve").stdin.contains(GIT_TOKEN));
    assert!(!lines
        .iter()
        .any(|l| l.contains(SECRET) || l.contains(GIT_TOKEN)));
    // The stale machine is dropped and a new tunnel is paired, then the project reopens.
    let log = fx.host.log.lock().unwrap().clone();
    let disconnect = log
        .iter()
        .position(|l| l.starts_with("disconnect"))
        .unwrap();
    let connect = log.iter().position(|l| l.starts_with("connect")).unwrap();
    let open = log
        .iter()
        .position(|l| l.contains("projects.open"))
        .unwrap();
    assert!(disconnect < connect && connect < open, "{log:?}");
    // Only read-only sbx verbs: it never creates, stops or deletes.
    assert_eq!(fx.sbx.count("create"), 0);
    assert_eq!(fx.sbx.count("stop"), 0);
    assert_eq!(fx.sbx.count("rm"), 0);
    assert!(fx.sbx.count("inspect") + fx.sbx.count("ls") >= 1);
}

#[test]
fn a_reconnect_of_a_running_sandbox_works_too_and_a_public_repo_skips_the_credential() {
    let (fx, id, _) = started();
    let mut public = config();
    public.credential_id = None;
    super::docker_config::save(&fx.store.lock().unwrap(), "/Users/me/widgets", &public).unwrap();
    fx.reset();
    fx.reconnect(id).0.unwrap();
    assert!(!fx.shell.tags().contains(&"phase:approve".to_string()));
    assert!(fx.shell.tags().contains(&"bootstrap".to_string()));
}

#[test]
fn a_reconnect_of_a_stopping_sandbox_is_refused_without_ssh() {
    let (fx, id, _) = started();
    fx.sbx.set_state(&sandbox_id(&fx), "stopping");
    fx.reset();
    let (result, _) = fx.reconnect(id);
    assert_eq!(
        result.unwrap_err(),
        "Sandbox is stopping. Try again in a minute."
    );
    assert!(fx.shell.calls().is_empty());
    assert!(fx.host.log.lock().unwrap().is_empty());
}

#[test]
fn a_reconnect_of_a_deleted_or_failed_sandbox_never_ssh_es() {
    let (fx, id, _) = started();
    fx.sbx.set_state(&sandbox_id(&fx), "failed");
    fx.reset();
    let error = fx.reconnect(id).0.unwrap_err();
    assert!(error.contains("state failed"), "{error}");
    fx.sbx.remove(&sandbox_id(&fx));
    let error = fx.reconnect(id).0.unwrap_err();
    assert!(error.contains("deleted"), "{error}");
    assert!(fx.shell.calls().is_empty());
    assert_eq!(fx.sbx.count("create"), 0);
}

#[test]
fn a_recreated_container_is_reported_instead_of_silently_rebinding() {
    let (fx, id, _) = started();
    *fx.host.environment.lock().unwrap() = Some("env-new-host".into());
    fx.reset();
    let error = fx.reconnect(id).0.unwrap_err();
    assert!(error.contains("recreated"), "{error}");
    let remote = fx
        .store
        .lock()
        .unwrap()
        .remote_session(id)
        .unwrap()
        .unwrap();
    assert_ne!(remote.project.environment_id, "env-new-host");
}

#[test]
fn a_reconnect_needs_a_sandbox_session_with_a_paired_machine() {
    let (fx, id, _) = started();
    fx.host.machines.lock().unwrap().clear();
    fx.reset();
    let error = fx.reconnect(id).0.unwrap_err();
    assert!(error.contains("connection was removed"), "{error}");
    assert!(fx.shell.calls().is_empty());
    assert!(fx.reconnect(i64::MIN).0.is_err());
}

#[test]
fn a_reconnect_failure_is_scrubbed_and_a_second_reconnect_is_allowed_after() {
    let (fx, id, _) = started();
    fx.reset();
    fx.shell.fail("bootstrap", &format!("boom {SECRET}"));
    let error = fx.reconnect(id).0.unwrap_err();
    assert!(!error.contains(SECRET), "{error}");
    *fx.shell.fail_tag.lock().unwrap() = None;
    fx.reconnect(id).0.unwrap();
}

#[test]
fn a_cancelled_reconnect_stops_without_touching_the_host() {
    let (fx, id, _) = started();
    fx.reset();
    let cancel = AtomicBool::new(true);
    let error = fx
        .svc
        .reconnect_sandbox(id, &mut |_| {}, &cancel)
        .unwrap_err();
    assert_eq!(error, START_ABORTED);
    assert!(fx.host.log.lock().unwrap().is_empty());
}

// ------------------------------------------------------------- call sites

#[test]
fn sandbox_sessions_are_listed_and_targeted_with_their_backend() {
    let (fx, id, name) = started();
    let listed = fx.svc.cloud_sessions_for("/Users/me/widgets").unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].cloud, super::backend::CloudRef::sandbox(&name));
    fx.sbx.set_state(&sandbox_id(&fx), "stopped");
    fx.svc.invalidate_ref(&listed[0].cloud);
    fx.reset();
    let target = fx.svc.target_for_session(id).unwrap();
    assert_eq!(target.cloud.kind, CloudBackendKind::DockerSandbox);
    assert_eq!(target.state, super::CloudState::Stopped);
    // Reading a state is inspect only: no ssh.
    assert!(fx.shell.calls().is_empty());
}

#[test]
fn deleting_a_sandbox_disconnects_its_machine_too() {
    let (fx, _, name) = started();
    fx.reset();
    let outcome = fx
        .svc
        .delete_cloud_outcome(&super::backend::CloudRef::sandbox(&name))
        .unwrap();
    assert_eq!(outcome, super::DeleteOutcome::Deleted);
    assert_eq!(fx.sbx.count("rm"), 1);
    assert!(fx
        .host
        .log
        .lock()
        .unwrap()
        .iter()
        .any(|l| l.starts_with("disconnect m-")));
    assert!(fx.shell.calls().is_empty());
}

#[test]
fn stopping_a_sandbox_goes_through_the_backend_without_ssh() {
    let (fx, _, name) = started();
    fx.reset();
    fx.svc
        .stop_cloud(&super::backend::CloudRef::sandbox(&name))
        .unwrap();
    assert_eq!(fx.sbx.count("stop"), 1);
    assert!(fx.shell.calls().is_empty());
}

#[test]
fn a_session_whose_sandbox_is_gone_can_be_deleted_without_a_host() {
    let (fx, id, _) = started();
    let store = fx.store.lock().unwrap();
    let mut remote = store.remote_session(id).unwrap().unwrap();
    remote.session_id = Some("host-session".into());
    store.save_remote_session(&remote).unwrap();
    fx.reset();
    // While the sandbox exists the Host is asked to delete the conversation.
    store
        .delete_remote_session_with_sbx(id, &NoGh, &*fx.sbx, &*fx.host)
        .unwrap();
    assert!(fx
        .host
        .log
        .lock()
        .unwrap()
        .iter()
        .any(|l| l.contains("sessions.delete")));
    drop(store);

    let (fx, id, _) = started();
    let store = fx.store.lock().unwrap();
    let mut remote = store.remote_session(id).unwrap().unwrap();
    remote.session_id = Some("host-session".into());
    store.save_remote_session(&remote).unwrap();
    fx.sbx.remove(&sandbox_id_of(&fx.sbx));
    fx.reset();
    store
        .delete_remote_session_with_sbx(id, &NoGh, &*fx.sbx, &*fx.host)
        .unwrap();
    assert!(store.remote_session(id).unwrap().is_none());
    assert!(
        !fx.host
            .log
            .lock()
            .unwrap()
            .iter()
            .any(|l| l.contains("sessions.delete")),
        "a gone sandbox has no conversation to delete"
    );
}
fn sandbox_id_of(sbx: &Sbx) -> String {
    sbx.sandboxes.lock().unwrap()[0].0.clone()
}

#[test]
fn a_reconnect_of_a_connected_sandbox_only_refreshes_the_git_credential_when_it_is_old() {
    let (fx, id, _) = started();
    *fx.host.connected.lock().unwrap() = true;
    fx.reset();
    fx.reconnect(id).0.unwrap();
    // Nothing is restarted or paired; only the expiring credential cache is refreshed.
    assert_eq!(fx.shell.tags(), ["phase:approve"]);
    assert!(fx.shell.call("phase:approve").stdin.contains(GIT_TOKEN));
    assert!(fx.host.log.lock().unwrap().is_empty());
    // A fresh delivery is not repeated.
    fx.reset();
    fx.reconnect(id).0.unwrap();
    assert!(fx.shell.calls().is_empty());
}

#[test]
fn a_recreated_container_is_detected_before_anything_local_changes() {
    let (fx, id, name) = started();
    // The start remembered the container its Host lives in.
    assert_eq!(
        fx.store
            .lock()
            .unwrap()
            .setting(&super::docker_start::container_key(&name))
            .unwrap()
            .as_deref(),
        Some(CONTAINER)
    );
    fx.store
        .lock()
        .unwrap()
        .save_setting(
            &super::docker_start::container_key(&name),
            "ffffffffffffffffffff",
        )
        .unwrap();
    let before = fx
        .store
        .lock()
        .unwrap()
        .remote_session(id)
        .unwrap()
        .unwrap();
    fx.reset();
    let error = fx.reconnect(id).0.unwrap_err();
    assert!(error.contains("recreated"), "{error}");
    // No disconnect, no pairing, no project open, no Host bootstrap.
    assert!(fx.host.log.lock().unwrap().is_empty());
    assert_eq!(fx.host.machines.lock().unwrap().len(), 1);
    assert!(!fx.shell.tags().contains(&"bootstrap".to_string()));
    let after = fx
        .store
        .lock()
        .unwrap()
        .remote_session(id)
        .unwrap()
        .unwrap();
    assert_eq!(before.project.environment_id, after.project.environment_id);
    assert_eq!(before.project.key, after.project.key);
}

#[test]
fn deleting_a_stopped_sandbox_session_never_resumes_it_and_can_be_removed_locally() {
    let (fx, id, name) = started();
    let store = fx.store.lock().unwrap();
    let mut remote = store.remote_session(id).unwrap().unwrap();
    remote.session_id = Some("host-session".into());
    store.save_remote_session(&remote).unwrap();
    *fx.host.delete_offline.lock().unwrap() = true;
    fx.reset();
    let error = store
        .delete_remote_session_with_sbx(id, &NoGh, &*fx.sbx, &*fx.host)
        .unwrap_err();
    assert_eq!(error, crate::store::SANDBOX_STOPPED_DELETE);
    assert!(fx.shell.calls().is_empty());
    assert!(store.remote_session(id).unwrap().is_some());
    // The explicit local removal forgets the record and says the sandbox remains.
    let warning = store.remove_cloud_session_locally(id).unwrap();
    assert!(
        warning.contains(&name) && warning.contains("sbx --cloud rm"),
        "{warning}"
    );
    assert!(store.remote_session(id).unwrap().is_none());
    assert_eq!(fx.sbx.count("rm"), 0);
}

// -------------------------------------------------------------- files-only archive

fn run_git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
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
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// A started sandbox session whose outer workspace is a real git folder with unpushed work,
/// an uncommitted edit and an untracked file, and a local clone of the project to archive into.
struct Archivable {
    fx: Fx,
    id: i64,
    name: String,
    project: std::path::PathBuf,
    _dir: tempfile::TempDir,
}
fn archivable() -> Archivable {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let origin = root.join("origin.git");
    let ws = root.join("outer-ws");
    let project = root.join("local-project");
    std::fs::create_dir_all(&origin).unwrap();
    run_git(&origin, &["init", "--bare", "-q", "-b", "main"]);
    std::fs::create_dir_all(&ws).unwrap();
    run_git(&ws, &["init", "-q", "-b", "main"]);
    std::fs::write(ws.join("a.txt"), "one\n").unwrap();
    run_git(&ws, &["add", "."]);
    run_git(&ws, &["commit", "-q", "-m", "base"]);
    run_git(&ws, &["remote", "add", "origin", origin.to_str().unwrap()]);
    run_git(&ws, &["push", "-q", "origin", "main"]);
    run_git(
        &root,
        &[
            "clone",
            "-q",
            origin.to_str().unwrap(),
            project.to_str().unwrap(),
        ],
    );
    run_git(&ws, &["checkout", "-q", "-b", "feat/x"]);
    std::fs::write(ws.join("made.txt"), "created in the old container\n").unwrap();
    run_git(&ws, &["add", "made.txt"]);
    run_git(&ws, &["commit", "-q", "-m", "unpushed work"]);
    std::fs::write(ws.join("a.txt"), "one\ntwo\n").unwrap();
    std::fs::write(ws.join("untracked.txt"), "new file\n").unwrap();

    let fx = fx_in(
        CreateMode::Ok,
        Some(("CLAUDE_CODE_OAUTH_TOKEN", SECRET.into())),
        Store::open(&root.join("native.db")).unwrap(),
    );
    let local = project.to_str().unwrap().to_string();
    let (session, _) = fx
        .svc
        .begin_sandbox_start(&local, &config(), "fix/login", "claude")
        .unwrap();
    let binding = fx.run(session.id).0.unwrap();
    super::docker_config::save(&fx.store.lock().unwrap(), &local, &config()).unwrap();
    {
        // The session ran Claude in the (old) container.
        let store = fx.store.lock().unwrap();
        let mut remote = store.remote_session(session.id).unwrap().unwrap();
        remote.summary.provider_id = Some("claude-session-1".into());
        remote.session_id = Some("host-1".into());
        store.save_remote_session(&remote).unwrap();
    }
    *fx.shell.local.lock().unwrap() = Some((root.join("home"), ws));
    Archivable {
        fx,
        id: session.id,
        name: binding.codespace,
        project,
        _dir: dir,
    }
}
impl Archivable {
    /// The container the start remembered is no longer the one `devcontainer up` reports.
    fn recreate(&self) {
        self.fx
            .store
            .lock()
            .unwrap()
            .save_setting(
                &super::docker_start::container_key(&self.name),
                "ffffffffffffffffffff",
            )
            .unwrap();
        self.fx.reset();
    }
    fn convert(&self) -> (Result<super::Converted, String>, Vec<String>) {
        let mut lines = vec![];
        let result = self
            .fx
            .svc
            .convert_session(self.id, &mut |l| lines.push(l.to_string()));
        (result, lines)
    }
    /// The calls that ran an archive script (not part of the start or reconnect flow).
    fn archive_calls(&self) -> Vec<ShellCall> {
        self.fx
            .shell
            .calls()
            .into_iter()
            .filter(|c| c.tag() == "other")
            .collect()
    }
}

#[test]
fn a_recreated_container_still_archives_its_files_from_the_outer_workspace() {
    let a = archivable();
    a.recreate();
    let (result, lines) = a.convert();
    let done = result.unwrap();
    assert!(lines.iter().any(|l| l.contains("files only")), "{lines:?}");
    // The local copy has the bundle and the patch (with the untracked file) from the workspace.
    let code = done.code_dir.clone().unwrap();
    assert!(code.join(super::BUNDLE).is_file());
    let patch = std::fs::read_to_string(code.join(super::PATCH)).unwrap();
    assert!(
        patch.contains("untracked.txt") && patch.contains("two"),
        "{patch}"
    );
    // Claude's JSONL lived in the old container: absent, and the warnings say so.
    assert!(!code.join(super::RAW_JSONL).exists());
    assert!(!matches!(done.resume, super::Resume::Installed(_)));
    let warnings = done.warnings.join("\n");
    assert!(warnings.contains("History may be incomplete"), "{warnings}");
    assert!(warnings.contains("old container"), "{warnings}");
    assert!(warnings.contains("evidence"), "{warnings}");
    // The cloud record is gone, the copy is archived and tagged files-only.
    let store = a.fx.store.lock().unwrap();
    assert!(store.remote_session(a.id).unwrap().is_none());
    let origin: Value = serde_json::from_str(
        &store
            .setting(&super::origin_key(
                CloudBackendKind::DockerSandbox,
                done.local_id,
            ))
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(origin["filesOnly"], true);
    assert_eq!(origin["backend"], "dockerSandbox");
    drop(store);
    // Every archive script ran in the outer sandbox, none in the container.
    let calls = a.archive_calls();
    assert!(calls.len() >= 3, "{calls:?}");
    assert!(calls.iter().all(|c| c.remote.starts_with("sh -c ")
        && c.remote.contains(WORKSPACE)
        && c.id == sandbox_id(&a.fx)));
    // They run as the workspace's owner, and the conversation is reported as not saved.
    assert!(
        calls[0].remote.contains("sudo -n -u"),
        "{}",
        calls[0].remote
    );
    assert!(!done.history_saved);
    assert!(super::unsaved_lines(std::slice::from_ref(&done))
        .iter()
        .any(|l| l.contains("conversation") && l.contains("was not saved")));
    assert!(a
        .fx
        .shell
        .calls()
        .iter()
        .all(|c| !c.remote.contains("devcontainer exec")));
    // The stop or delete that follows is the ordinary one.
    a.fx.svc
        .stop_cloud(&super::backend::CloudRef::sandbox(&a.name))
        .unwrap();
    assert_eq!(a.fx.sbx.count("stop"), 1);
    let _ = &a.project;
}

#[test]
fn the_outer_archive_scripts_carry_no_secret_and_leave_no_stage_behind() {
    let a = archivable();
    a.recreate();
    a.convert().0.unwrap();
    for call in a.archive_calls() {
        for secret in [SECRET, GIT_TOKEN] {
            assert!(!call.stdin.contains(secret) && !call.remote.contains(secret));
        }
        assert!(!call.stdin.contains("CLAUDE_CODE_OAUTH_TOKEN"));
        // No Claude lookup: the JSONL is not in this sandbox's reach.
        if call.stdin.contains("ws='") {
            assert!(call.stdin.contains("id=''"), "{}", call.stdin);
            assert!(!call.stdin.contains("claude-session-1"));
        }
    }
    // The secret-carrying flows ran before, never after, the archive started.
    let tags = a.fx.shell.tags();
    assert!(
        tags.iter().filter(|t| *t == "other").count() >= 3,
        "{tags:?}"
    );
    // The stage is removed again through the same outer door.
    assert!(a
        .archive_calls()
        .iter()
        .any(|c| c.stdin.starts_with("rm -rf '/tmp/monoptah-archive.")));
}

#[test]
fn an_unreachable_host_needs_a_confirmation_before_the_files_only_archive() {
    let a = archivable();
    a.fx.reset();
    a.fx.shell.fail("relay", "boom");
    // The Host may still hold the conversation: nothing is converted or forgotten.
    let error =
        a.fx.svc
            .convert_session_confirmed(a.id, false, &mut |_| {})
            .unwrap_err();
    assert!(
        matches!(error, super::ConvertError::NeedsFilesOnlyConfirm { .. }),
        "{error:?}"
    );
    assert!(a.archive_calls().is_empty());
    assert!(a
        .fx
        .store
        .lock()
        .unwrap()
        .remote_session(a.id)
        .unwrap()
        .is_some());
    // The plain entry point tells the user to retry.
    let text = a.convert().0.unwrap_err();
    assert!(
        text.contains("could not be reached") && text.contains("Retry"),
        "{text}"
    );
    assert!(a.archive_calls().is_empty());
    // Only an explicit confirmation archives the files.
    let done =
        a.fx.svc
            .convert_session_confirmed(a.id, true, &mut |_| {})
            .unwrap();
    assert!(done.code_dir.unwrap().join(super::BUNDLE).is_file());
    assert!(!done.history_saved);
    assert!(done
        .warnings
        .iter()
        .any(|w| w.contains("History may be incomplete")));
}

#[test]
fn a_host_worktree_is_refused_and_a_removed_one_is_ignored() {
    let a = archivable();
    a.recreate();
    let set = |cwd: &str, removed: bool| {
        let store = a.fx.store.lock().unwrap();
        let mut settings = store.agent_settings(a.id).unwrap().unwrap_or_default();
        settings.worktree_cwd = Some(cwd.into());
        settings.worktree_removed = removed;
        store.save_agent_settings(a.id, &settings).unwrap();
    };
    set("/workspaces/widgets-worktrees/fix-login", false);
    let error = a.convert().0.unwrap_err();
    assert!(
        error.contains("worktree lived only in the container"),
        "{error}"
    );
    assert!(a.archive_calls().is_empty());
    assert!(a
        .fx
        .store
        .lock()
        .unwrap()
        .remote_session(a.id)
        .unwrap()
        .is_some());
    // A removed worktree is not looked at: the main workspace is archived.
    set("/workspaces/widgets-worktrees/fix-login", true);
    a.recreate();
    a.convert().0.unwrap();
}

#[test]
fn a_deleted_sandbox_is_not_archived_as_files_only() {
    let a = archivable();
    a.fx.sbx.remove(&sandbox_id(&a.fx));
    a.fx.reset();
    let error = a.convert().0.unwrap_err();
    assert!(error.contains("deleted"), "{error}");
    assert!(a.fx.shell.calls().is_empty());
    assert!(a
        .fx
        .store
        .lock()
        .unwrap()
        .remote_session(a.id)
        .unwrap()
        .is_some());
}

#[test]
fn the_plain_reconnect_reports_a_recreated_container_and_never_archives() {
    let a = archivable();
    a.recreate();
    let error = a.fx.reconnect(a.id).0.unwrap_err();
    assert!(error.contains("recreated"), "{error}");
    let typed =
        a.fx.svc
            .reconnect_sandbox_typed(a.id, &mut |_| {}, &AtomicBool::new(false))
            .unwrap_err();
    assert_eq!(typed, super::docker_start::ReconnectError::Recreated);
    assert!(a.archive_calls().is_empty());
    assert!(a
        .fx
        .store
        .lock()
        .unwrap()
        .remote_session(a.id)
        .unwrap()
        .is_some());
}

#[test]
fn background_paths_against_a_recreated_container_make_no_outer_calls() {
    let a = archivable();
    a.recreate();
    // What a Mono, a poll or the app API can do: the plain reconnect (typed or not), a list
    // and a stop. None of them archive or run anything in the outer sandbox.
    assert!(a.fx.reconnect(a.id).0.is_err());
    assert!(a
        .fx
        .svc
        .reconnect_sandbox_typed(a.id, &mut |_| {}, &AtomicBool::new(false))
        .is_err());
    let cloud = super::backend::CloudRef::sandbox(&a.name);
    let _ = a.fx.svc.sessions_in_cloud(&cloud);
    a.fx.svc.stop_cloud(&cloud).unwrap();
    assert!(a.archive_calls().is_empty());
    assert!(a
        .fx
        .shell
        .calls()
        .iter()
        .all(|c| !c.remote.contains("stat -c")));
    assert!(a
        .fx
        .store
        .lock()
        .unwrap()
        .remote_session(a.id)
        .unwrap()
        .is_some());
}

#[test]
fn only_a_deleted_or_recreated_sandbox_is_unrecoverable() {
    use super::docker_start::is_sandbox_unrecoverable;
    assert!(is_sandbox_unrecoverable(
        "Sandbox was deleted. Start a new session."
    ));
    assert!(is_sandbox_unrecoverable(
        "The sandbox container was recreated, so this session's history is gone. Start a new session."
    ));
    assert!(!is_sandbox_unrecoverable(
        "Sandbox is stopping. Try again in a minute."
    ));
    assert!(!is_sandbox_unrecoverable("Could not reach sbx"));
}
