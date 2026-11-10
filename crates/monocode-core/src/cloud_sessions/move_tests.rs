//! Tests of the move orchestrator with a real git remote (a temp bare repo) and fakes for the
//! Host, the backend and `gh`. Nothing here talks to a real workspace.
use super::move_session::*;
use super::move_session::{is_older, scrub};
use super::*;
use crate::codespaces::{GhOutput, GhRunner};
use crate::remote::Machine;
use crate::store::SessionLocation;
use crate::transcript::Row;
use base64::{engine::general_purpose::STANDARD, Engine};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::process::Command;

const CLAUDE_ID: &str = "11111111-2222-4333-8444-555555555555";
const ENV: &str = "env-cs1";

fn git(dir: &Path, args: &[&str]) -> String {
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
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn configure(dir: &Path) {
    git(dir, &["config", "user.email", "t@example.com"]);
    git(dir, &["config", "user.name", "Test"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
    git(dir, &["config", "core.autocrlf", "false"]);
}

fn commit_file(dir: &Path, name: &str, text: &str) {
    std::fs::write(dir.join(name), text).unwrap();
    git(dir, &["add", name]);
    git(dir, &["commit", "-q", "-m", name]);
}

// ------------------------------------------------------------------ fakes

fn machine() -> Machine {
    Machine {
        id: "m-cs1".into(),
        name: "cs1".into(),
        endpoint: "codespace://cs1".into(),
        environment_id: ENV.into(),
        ssh: None,
        codespace: Some("cs1".into()),
        sandbox: None,
    }
}

type Hook = Box<dyn Fn(&str) + Send + Sync>;

struct FakeHost {
    machines: Mutex<Vec<Machine>>,
    log: Mutex<Vec<(String, Value)>>,
    /// `(method, error)`: the next call of `method` fails once with `error`.
    failures: Mutex<Vec<(String, String)>>,
    /// upload id -> (name, bytes, committed)
    uploads: Mutex<BTreeMap<String, (String, Vec<u8>, bool)>>,
    sessions: Mutex<Vec<Value>>,
    capabilities: Mutex<Vec<String>>,
    /// Makes `applySnapshot` answer with another tree.
    wrong_tree: Mutex<Option<String>>,
    decision: Mutex<String>,
    on_request: Mutex<Option<Hook>>,
    /// Fails the next pairing with a codespace.
    connect_error: Mutex<Option<String>>,
}
impl FakeHost {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            machines: Mutex::new(vec![machine()]),
            log: Mutex::new(vec![]),
            failures: Mutex::new(vec![]),
            uploads: Mutex::new(BTreeMap::new()),
            sessions: Mutex::new(vec![]),
            capabilities: Mutex::new(
                ["workspace.applySnapshot", "sessions.import", "uploads"]
                    .map(String::from)
                    .to_vec(),
            ),
            wrong_tree: Mutex::new(None),
            decision: Mutex::new("installed".into()),
            on_request: Mutex::new(None),
            connect_error: Mutex::new(None),
        })
    }
    fn methods(&self) -> Vec<String> {
        self.log
            .lock()
            .unwrap()
            .iter()
            .map(|c| c.0.clone())
            .collect()
    }
    fn count(&self, method: &str) -> usize {
        self.methods().iter().filter(|m| *m == method).count()
    }
    fn calls(&self, method: &str) -> Vec<Value> {
        self.log
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.0 == method)
            .map(|c| c.1.clone())
            .collect()
    }
    fn fail_once(&self, method: &str, error: &str) {
        self.failures
            .lock()
            .unwrap()
            .push((method.into(), error.into()));
    }
    fn uploaded_names(&self) -> Vec<String> {
        self.uploads
            .lock()
            .unwrap()
            .values()
            .map(|u| u.0.clone())
            .collect()
    }
}
impl super::Host for FakeHost {
    fn machines(&self) -> Result<Vec<Machine>, String> {
        Ok(self.machines.lock().unwrap().clone())
    }
    fn connect(
        &self,
        codespace: &str,
        _: &str,
        _: &mut dyn FnMut(&str),
    ) -> Result<Machine, String> {
        if let Some(error) = self.connect_error.lock().unwrap().take() {
            return Err(error);
        }
        let mut m = machine();
        m.id = format!("m-{codespace}");
        m.codespace = Some(codespace.into());
        m.environment_id = format!("env-{codespace}");
        self.machines.lock().unwrap().push(m.clone());
        Ok(m)
    }
    fn ensure(&self, _: &str, _: &AtomicBool, _: &mut dyn FnMut(&str)) -> Result<(), String> {
        self.log.lock().unwrap().push(("ensure".into(), json!({})));
        Ok(())
    }
    fn disconnect(&self, id: &str) -> Result<(), String> {
        self.machines.lock().unwrap().retain(|m| m.id != id);
        Ok(())
    }
    fn request(&self, _machine: &str, method: &str, params: Value) -> Result<Value, String> {
        self.log
            .lock()
            .unwrap()
            .push((method.to_string(), params.clone()));
        if let Some(hook) = self.on_request.lock().unwrap().as_ref() {
            hook(method);
        }
        {
            let mut failures = self.failures.lock().unwrap();
            if let Some(at) = failures.iter().position(|f| f.0 == method) {
                return Err(failures.remove(at).1);
            }
        }
        match method {
            "environment.describe" => Ok(json!({
                "host": {"flavor": "monoptah", "version": "1.1.0"},
                "capabilities": *self.capabilities.lock().unwrap(),
            })),
            "projects.open" => Ok(json!({"id": "project-1", "cwd": params["cwd"]})),
            "projects.list" => Ok(json!([{"id": "project-1"}])),
            "sessions.list" => Ok(Value::Array(self.sessions.lock().unwrap().clone())),
            "sessions.delete" => {
                let id = params["sessionId"].as_str().unwrap_or_default().to_string();
                self.sessions
                    .lock()
                    .unwrap()
                    .retain(|s| s["id"].as_str() != Some(id.as_str()));
                Ok(json!({"deleted": true}))
            }
            "uploads.begin" => {
                let mut uploads = self.uploads.lock().unwrap();
                let id = match params["uploadId"].as_str() {
                    Some(id) if uploads.contains_key(id) => id.to_string(),
                    Some(_) => return Err("Upload not found".into()),
                    None => {
                        let id = format!("{:032x}", uploads.len() + 1);
                        uploads.insert(
                            id.clone(),
                            (params["name"].as_str().unwrap().to_string(), vec![], false),
                        );
                        id
                    }
                };
                let offset = uploads[&id].1.len();
                Ok(json!({"uploadId": id, "offset": offset, "chunkSize": 524288}))
            }
            "uploads.chunk" => {
                let mut uploads = self.uploads.lock().unwrap();
                let upload = uploads
                    .get_mut(params["uploadId"].as_str().unwrap_or_default())
                    .ok_or("Upload not found")?;
                upload
                    .1
                    .extend(STANDARD.decode(params["data"].as_str().unwrap()).unwrap());
                Ok(json!({"offset": upload.1.len()}))
            }
            "uploads.commit" => {
                let id = params["uploadId"].as_str().unwrap_or_default().to_string();
                let mut uploads = self.uploads.lock().unwrap();
                let upload = uploads.get_mut(&id).ok_or("Upload not found")?;
                upload.2 = true;
                let sha: String = Sha256::digest(&upload.1)
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect();
                Ok(json!({"path": format!("/uploads/{id}/payload"), "sha256": sha}))
            }
            "uploads.discard" => {
                let id = params["uploadId"].as_str().unwrap_or_default();
                Ok(json!({"discarded": self.uploads.lock().unwrap().remove(id).is_some()}))
            }
            "workspace.applySnapshot" => {
                if let Some(id) = params["patchUploadId"].as_str() {
                    match self.uploads.lock().unwrap().get(id) {
                        Some(u) if u.2 => {}
                        Some(_) => return Err("Upload is not committed".into()),
                        None => return Err("Upload not found".into()),
                    }
                }
                let tree = self
                    .wrong_tree
                    .lock()
                    .unwrap()
                    .clone()
                    .unwrap_or_else(|| params["tree"].as_str().unwrap().to_string());
                Ok(json!({
                    "cwd": "/workspaces/widgets-wt",
                    "branch": params["branch"],
                    "head": params["head"],
                    "tree": tree,
                    "createdWorktree": params["instance"] == "shared",
                    "reused": false,
                    "patchApplied": params["patchUploadId"].is_string(),
                }))
            }
            "sessions.import" => {
                match self
                    .uploads
                    .lock()
                    .unwrap()
                    .get(params["uploadId"].as_str().unwrap_or_default())
                {
                    Some(u) if u.2 => {}
                    Some(_) => return Err("Upload is not committed".into()),
                    None => return Err("Upload not found".into()),
                }
                let decision = self.decision.lock().unwrap().clone();
                let provider = if decision == "forked" {
                    "99999999-8888-4777-8666-555555555555".to_string()
                } else {
                    params["providerSessionId"].as_str().unwrap().to_string()
                };
                let mut out = json!({
                    "sessionId": "host-1",
                    "providerSessionId": provider,
                    "decision": decision,
                    "reused": false,
                });
                if decision == "forked" {
                    out["forkedFrom"] = params["providerSessionId"].clone();
                    out["notice"] = json!("The cloud and local conversations diverged.");
                }
                Ok(out)
            }
            "commands.dispatch" => Ok(json!({
                "sessionId": "host-new",
                "commandId": params["commandId"],
                "revision": 1,
            })),
            other => Err(format!("unexpected host method {other}")),
        }
    }
}

struct FakeBackend {
    scripts: Mutex<Vec<String>>,
    /// What `claude --version` prints.
    version: Mutex<String>,
    update_fails: Mutex<bool>,
    deleted: Mutex<Vec<String>>,
}
impl FakeBackend {
    fn new(version: &str) -> Arc<Self> {
        Arc::new(Self {
            scripts: Mutex::new(vec![]),
            version: Mutex::new(version.into()),
            update_fails: Mutex::new(false),
            deleted: Mutex::new(vec![]),
        })
    }
    fn scripts(&self) -> Vec<String> {
        self.scripts.lock().unwrap().clone()
    }
}
impl CloudBackend for FakeBackend {
    fn kind(&self) -> CloudBackendKind {
        CloudBackendKind::Codespaces
    }
    fn display_state(&self, _: &str) -> CloudState {
        CloudState::Available
    }
    fn is_stopped(&self, _: &str) -> bool {
        false
    }
    fn is_deleted_state(&self, _: &str) -> bool {
        false
    }
    fn is_missing_error(&self, _: &str) -> bool {
        false
    }
    fn state(&self, _: &str) -> Result<String, String> {
        Ok("Available".into())
    }
    fn exists(&self, _: &str) -> Result<bool, String> {
        Ok(true)
    }
    fn stop(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn delete(&self, name: &str) -> Result<(), String> {
        self.deleted.lock().unwrap().push(name.into());
        Ok(())
    }
    fn rebuild(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn web_url(&self, _: &str) -> Option<Result<String, String>> {
        None
    }
    fn run_script(&self, _: &str, script: &str, _: bool) -> Result<String, String> {
        self.scripts.lock().unwrap().push(script.to_string());
        if script.contains("claude update") {
            if *self.update_fails.lock().unwrap() {
                return Err("update failed".into());
            }
            *self.version.lock().unwrap() = "2.1.330 (Claude Code)".into();
            return Ok("updated".into());
        }
        if script.contains("claude --version") {
            return Ok(self.version.lock().unwrap().clone());
        }
        Ok(String::new())
    }
    fn copy_from(&self, _: &str, _: &str, _: &Path) -> Result<(), String> {
        Ok(())
    }
    fn prepare_evidence(&self, _: &str, _: &str) -> Result<(), String> {
        Ok(())
    }
}

/// A `gh` that must not be needed: the moves below target an existing instance.
struct NoGh;
impl GhRunner for NoGh {
    fn run(&self, args: &[String], _: Option<&str>) -> Result<GhOutput, String> {
        panic!("unexpected gh call: {}", args.join(" "))
    }
}

// ---------------------------------------------------------------- fixture

struct Fx {
    tmp: tempfile::TempDir,
    work: PathBuf,
    origin: PathBuf,
    home: PathBuf,
    store: Arc<Mutex<Store>>,
    session: i64,
    host: Arc<FakeHost>,
    backend: Arc<FakeBackend>,
    svc: CloudSessions,
}

fn encode(path: &Path) -> String {
    path.to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

fn rows() -> Vec<Row> {
    ["user", "assistant", "user", "assistant"]
        .iter()
        .enumerate()
        .map(|(i, role)| Row {
            role: (*role).into(),
            text: format!("row {i}"),
            first: *role == "user",
            sent_at: None,
        })
        .collect()
}

impl Fx {
    fn new() -> Self {
        Self::with(|_| {})
    }

    /// `prepare` may change the work clone before the session is created.
    fn with(prepare: impl FnOnce(&Path)) -> Self {
        Self::build("claude", "2.1.321", prepare)
    }

    fn build(provider: &str, version: &str, prepare: impl FnOnce(&Path)) -> Self {
        Self::build_with(provider, version, prepare, Arc::new(NoGh))
    }

    fn build_with(
        provider: &str,
        version: &str,
        prepare: impl FnOnce(&Path),
        gh: Arc<dyn GhRunner>,
    ) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let origin = root.join("origin.git");
        let work = root.join("work");
        std::fs::create_dir_all(&origin).unwrap();
        git(&origin, &["init", "-q", "--bare", "-b", "main"]);
        std::fs::create_dir_all(&work).unwrap();
        git(&work, &["init", "-q", "-b", "main"]);
        configure(&work);
        git(&work, &["remote", "add", "origin", origin.to_str().unwrap()]);
        std::fs::write(work.join(".gitignore"), ".env.local\n").unwrap();
        git(&work, &["add", ".gitignore"]);
        commit_file(&work, "README.md", "hello\n");
        git(&work, &["push", "-q", "-u", "origin", "main"]);
        git(&work, &["remote", "set-head", "origin", "main"]);
        // A feature branch with one local commit, a changed tracked file, a new file and an
        // ignored secret.
        git(&work, &["checkout", "-q", "-b", "feat"]);
        commit_file(&work, "feature.txt", "feature\n");
        std::fs::write(work.join("README.md"), "hello\nedited\n").unwrap();
        std::fs::write(work.join("notes.txt"), "new file\n").unwrap();
        std::fs::write(work.join(".env.local"), "SECRET=hunter2\n").unwrap();
        prepare(&work);

        let store = Arc::new(Mutex::new(Store::open(&root.join("native.db")).unwrap()));
        let work_text = work.to_string_lossy().into_owned();
        let session = {
            let mut guard = store.lock().unwrap();
            let session = guard.create(provider, &work_text).unwrap();
            guard.title(session.id, "Fix the login").unwrap();
            if provider == "claude" {
                guard.provider_id(session.id, CLAUDE_ID).unwrap();
            }
            guard.write_rows(session.id, 0, &rows()).unwrap();
            session.id
        };
        // The Claude transcript the session resumes.
        let home = root.join("claude");
        let project = home.join("projects").join(encode(&work));
        std::fs::create_dir_all(&project).unwrap();
        let line = |uuid: &str, kind: &str, v: &str| {
            format!(
                "{{\"type\":\"{kind}\",\"uuid\":\"{uuid}\",\"sessionId\":\"{CLAUDE_ID}\",\"cwd\":\"{work_text}\",\"version\":\"{v}\"}}\n"
            )
        };
        std::fs::write(
            project.join(format!("{CLAUDE_ID}.jsonl")),
            format!(
                "{}{}{}",
                line("e1", "user", "2.1.300"),
                line("e2", "assistant", version),
                line("e3", "user", version)
            ),
        )
        .unwrap();

        // The existing codespace the move targets already runs a session of this project.
        {
            let guard = store.lock().unwrap();
            guard
                .remember_cloud_ref_project(
                    ENV,
                    &json!({"id": "project-1", "cwd": "/workspaces/widgets"}),
                    &work_text,
                    &CloudRef::codespace("cs1"),
                )
                .unwrap();
        }
        let host = FakeHost::new();
        let backend = FakeBackend::new("2.1.321 (Claude Code)");
        let svc = CloudSessions::new(gh, host.clone(), store.clone())
            .with_origin(|_| Ok("git@github.com:octo/widgets.git".into()))
            .with_timing(Duration::from_secs(5), |_| {}, || 1_794_024_100_000)
            .with_backend(backend.clone());
        Self {
            tmp,
            work,
            origin,
            home,
            store,
            session,
            host,
            backend,
            svc,
        }
    }

    fn request(&self) -> MoveRequest {
        MoveRequest {
            session_id: self.session,
            target: MoveTarget::Existing {
                backend: CloudBackendKind::Codespaces,
                name: "cs1".into(),
            },
            account_home: Some(self.home.clone()),
        }
    }

    fn begin(&self) -> MoveRecord {
        self.svc.begin_move(self.request(), &|| false).unwrap()
    }

    /// Begins the move when none exists, then runs it.
    fn run(&self) -> (Result<MoveOutcome, String>, Vec<MoveEvent>) {
        self.run_with(&|| false, &AtomicBool::new(false))
    }

    fn run_with(
        &self,
        busy: &dyn Fn() -> bool,
        cancel: &AtomicBool,
    ) -> (Result<MoveOutcome, String>, Vec<MoveEvent>) {
        if self.record().is_none() {
            self.begin();
        }
        let mut events = vec![];
        let result = self
            .svc
            .run_move(self.session, busy, &mut |e| events.push(e), cancel);
        (result, events)
    }

    fn record(&self) -> Option<MoveRecord> {
        move_record(&self.store.lock().unwrap(), self.session)
    }

    fn save_record(&self, record: &MoveRecord) {
        self.store
            .lock()
            .unwrap()
            .save_setting(
                &format!("cloud-move:{}", self.session),
                &serde_json::to_string(record).unwrap(),
            )
            .unwrap();
    }

    fn location(&self) -> SessionLocation {
        self.store
            .lock()
            .unwrap()
            .cloud_location(self.session)
            .unwrap()
    }

    fn head(&self) -> String {
        git(&self.work, &["rev-parse", "HEAD"])
    }

    fn origin_ref(&self, branch: &str) -> Option<String> {
        let out = Command::new("git")
            .arg("-C")
            .arg(&self.origin)
            .args([
                "rev-parse",
                "--verify",
                "-q",
                &format!("refs/heads/{branch}"),
            ])
            .output()
            .unwrap();
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
    }

    fn staging(&self) -> PathBuf {
        self.store
            .lock()
            .unwrap()
            .thread_dir(self.session)
            .unwrap()
            .unwrap()
            .join("cloud-move")
    }

    /// Counts the pushes origin received (a hook appends a line per push).
    fn count_pushes(&self) -> usize {
        std::fs::read_to_string(self.tmp.path().join("pushes.log"))
            .map(|t| t.lines().count())
            .unwrap_or(0)
    }

    fn install_push_counter(&self) {
        use std::os::unix::fs::PermissionsExt;
        let log = self.tmp.path().canonicalize().unwrap().join("pushes.log");
        let hook = self.origin.join("hooks/pre-receive");
        std::fs::write(
            &hook,
            format!(
                "#!/bin/sh\necho push >> '{}'\ncat >/dev/null\n",
                log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn set_flip_hook(&self, hook: impl Fn(&Store) -> Result<(), String> + Send + Sync + 'static) {
        *self.svc.flip_hook.lock().unwrap() = Some(Box::new(hook));
    }

    /// A second service over the same store and Host: the app after a restart.
    fn restarted(&self) -> CloudSessions {
        CloudSessions::new(Arc::new(NoGh), self.host.clone(), self.store.clone())
            .with_backend(self.backend.clone())
    }

    fn row_count(&self) -> usize {
        self.store
            .lock()
            .unwrap()
            .row_anchor(self.session)
            .unwrap()
            .count
    }

    fn assert_still_local_and_usable(&self) {
        assert_eq!(self.location(), SessionLocation::Local);
        let mut store = self.store.lock().unwrap();
        store.require_local_run(self.session).unwrap();
        let n = store.row_anchor(self.session).unwrap().count;
        store
            .write_rows(
                self.session,
                n,
                &[Row {
                    role: "user".into(),
                    text: "still works".into(),
                    first: true,
                    sent_at: None,
                }],
            )
            .unwrap();
        assert_eq!(store.row_anchor(self.session).unwrap().count, n + 1);
        store.truncate_rows(self.session, n).unwrap();
    }
}

fn notices(events: &[MoveEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            MoveEvent::Notice(text) => Some(text.clone()),
            _ => None,
        })
        .collect()
}

fn index_of(methods: &[String], method: &str) -> usize {
    methods
        .iter()
        .position(|m| m == method)
        .unwrap_or_else(|| panic!("{method} was not called: {methods:?}"))
}

// ------------------------------------------------------------------ happy path

#[test]
fn a_move_runs_every_step_in_order_and_flips_the_location_last() {
    let fx = Fx::new();
    let head = fx.head();
    let (result, events) = fx.run();
    let outcome = result.unwrap();
    // The branch reached origin, and the default branch did not move.
    assert_eq!(fx.origin_ref("feat").as_deref(), Some(head.as_str()));
    assert_ne!(fx.origin_ref("main").as_deref(), Some(head.as_str()));
    // Host calls in the order of the design.
    let methods = fx.host.methods();
    let order = [
        "environment.describe",
        "uploads.begin",
        "workspace.applySnapshot",
        "projects.list",
        "sessions.list",
        "sessions.import",
    ];
    let at: Vec<usize> = order.iter().map(|m| index_of(&methods, m)).collect();
    assert!(at.windows(2).all(|w| w[0] < w[1]), "{methods:?}");
    // The existing instance restores into a new Host worktree.
    assert_eq!(
        fx.host.calls("workspace.applySnapshot")[0]["instance"],
        "shared"
    );
    // The location is Cloud, the Host record is linked, and the move point is the row count.
    let SessionLocation::Cloud(location) = fx.location() else {
        panic!("not in the cloud")
    };
    assert_eq!(location.host_session_id, "host-1");
    assert_eq!(location.environment_id, ENV);
    assert_eq!(location.moved_at_row, 4);
    assert_eq!(location.pushed_branch.as_deref(), Some("feat"));
    assert_eq!(location.remote_cwd, "/workspaces/widgets-wt");
    {
        let store = fx.store.lock().unwrap();
        let (_, record) = store.cloud_host_record(fx.session).unwrap();
        assert_eq!(record.session_id.as_deref(), Some("host-1"));
        assert!(record.is_linked());
        // The first cloud turn carries the moved-path note.
        let note = moved_path_note(&store, fx.session).unwrap();
        assert!(
            note.contains(&fx.work.to_string_lossy().into_owned()),
            "{note}"
        );
        assert!(note.contains("/workspaces/widgets-wt"));
        // The move is over: no record.
        assert!(move_record(&store, fx.session).is_none());
    }
    assert!(!fx.staging().exists());
    assert_eq!(fx.host.count("uploads.discard"), 2);
    assert_eq!(outcome.moved_at_row, 4);
    assert_eq!(outcome.provider_session_id.as_deref(), Some(CLAUDE_ID));
    assert!(!outcome.forked);
    assert!(matches!(events.last(), Some(MoveEvent::Done(_))));
    // Local runs are refused from now on (single writer).
    assert!(fx
        .store
        .lock()
        .unwrap()
        .require_local_run(fx.session)
        .is_err());
}

#[test]
fn the_same_claude_id_reaches_sessions_import_with_from_cwds() {
    let fx = Fx::new();
    fx.run().0.unwrap();
    let import = &fx.host.calls("sessions.import")[0];
    assert_eq!(import["providerSessionId"], CLAUDE_ID);
    assert_eq!(import["cwd"], "/workspaces/widgets-wt");
    assert_eq!(import["projectId"], "project-1");
    let from: Vec<String> = import["fromCwds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert!(!from.is_empty());
    assert!(
        from.contains(&fx.work.to_string_lossy().into_owned()),
        "{from:?}"
    );
    assert!(from.iter().all(|p| p.starts_with('/')));
    assert!(import["uploadId"].as_str().is_some());
    assert_eq!(import["model"], "claude:default");
}

#[test]
fn ignored_files_are_never_uploaded_and_only_two_files_travel() {
    let fx = Fx::new();
    fx.begin();
    // Stop at the code step so the uploads are still on the fake Host.
    fx.host.fail_once("workspace.applySnapshot", "stop here");
    let _ = fx.run();
    let names = {
        let mut names = fx.host.uploaded_names();
        names.sort();
        names
    };
    let mut expected = vec!["snapshot.patch".to_string(), format!("{CLAUDE_ID}.jsonl")];
    expected.sort();
    assert_eq!(names, expected);
    let uploads = fx.host.uploads.lock().unwrap().clone();
    for (name, bytes, _) in uploads.values() {
        let text = String::from_utf8_lossy(bytes);
        assert!(!text.contains("hunter2"), "{name} carries the secret");
        assert!(!text.contains(".env.local"), "{name} names the ignored file");
    }
    let patch = uploads
        .values()
        .find(|u| u.0 == "snapshot.patch")
        .map(|u| String::from_utf8_lossy(&u.1).into_owned())
        .unwrap();
    assert!(patch.contains("README.md") && patch.contains("notes.txt"));
    assert!(
        !patch.contains("feature.txt"),
        "committed work travels by push"
    );
    // The warning names the ignored file, but it stays local.
    let record = fx.record().unwrap();
    assert!(record
        .warnings
        .iter()
        .any(|w| w.contains(".env.local") && w.contains("stay on this computer")));
}

// ------------------------------------------------------------------- push rules

#[test]
fn a_push_is_never_forced_and_a_diverged_branch_goes_to_the_cloud_branch() {
    let fx = Fx::new();
    // Someone else moved `feat` on origin: the local branch diverged from it.
    let other = fx.tmp.path().join("other");
    git(
        fx.tmp.path(),
        &["clone", "-q", fx.origin.to_str().unwrap(), "other"],
    );
    configure(&other);
    git(&other, &["checkout", "-q", "-b", "feat", "origin/main"]);
    commit_file(&other, "theirs.txt", "theirs\n");
    git(&other, &["push", "-q", "origin", "feat"]);
    let theirs = fx.origin_ref("feat").unwrap();
    fx.run().0.unwrap();
    // Their branch is untouched and the move used `feat-cloud`.
    assert_eq!(fx.origin_ref("feat").as_deref(), Some(theirs.as_str()));
    assert_eq!(
        fx.origin_ref("feat-cloud").as_deref(),
        Some(fx.head().as_str())
    );
    assert_eq!(
        fx.host.calls("workspace.applySnapshot")[0]["branch"],
        "feat-cloud"
    );
    // The local branch did not get `feat-cloud` as its upstream.
    let upstream = Command::new("git")
        .arg("-C")
        .arg(&fx.work)
        .args(["rev-parse", "--abbrev-ref", "feat@{upstream}"])
        .output()
        .unwrap();
    assert!(
        !String::from_utf8_lossy(&upstream.stdout).contains("feat-cloud"),
        "the upstream of the local branch changed"
    );
}

#[test]
fn the_default_branch_is_never_the_push_target() {
    let fx = Fx::with(|work| {
        // Work directly on `main`, ahead of origin.
        git(work, &["stash", "-q", "-u"]);
        git(work, &["checkout", "-q", "main"]);
        git(work, &["merge", "-q", "--ff-only", "feat"]);
        git(work, &["branch", "-q", "-D", "feat"]);
        git(work, &["stash", "pop", "-q"]);
    });
    let main_before = fx.origin_ref("main").unwrap();
    fx.run().0.unwrap();
    assert_eq!(fx.origin_ref("main").as_deref(), Some(main_before.as_str()));
    assert_eq!(
        fx.origin_ref("main-cloud").as_deref(),
        Some(fx.head().as_str())
    );
    assert_eq!(
        fx.host.calls("workspace.applySnapshot")[0]["branch"],
        "main-cloud"
    );
}

#[test]
fn the_push_step_refuses_a_default_branch_even_if_the_record_says_so() {
    let fx = Fx::new();
    let mut forged = fx.begin();
    // Forge the target the way a bug or a stale record could.
    forged.branch = "main".into();
    forged.needs_push = true;
    forged.head = fx.head();
    forged.done = vec![MoveStep::Snapshot];
    fx.save_record(&forged);
    let main_before = fx.origin_ref("main").unwrap();
    let (result, _) = fx.run();
    let error = result.unwrap_err();
    assert!(error.contains("Refusing to push"), "{error}");
    assert_eq!(fx.origin_ref("main").as_deref(), Some(main_before.as_str()));
    assert_eq!(fx.record().unwrap().failed_at, Some(MoveStep::Push));
}

#[test]
fn push_commit_to_branch_cannot_overwrite_a_branch_that_moved() {
    let fx = Fx::new();
    git(&fx.work, &["push", "-q", "origin", "feat"]);
    let pushed = fx.origin_ref("feat").unwrap();
    // A rewritten local history: not a fast-forward of what origin has.
    git(&fx.work, &["stash", "-q", "-u"]);
    git(&fx.work, &["reset", "-q", "--hard", "main"]);
    commit_file(&fx.work, "rewritten.txt", "x\n");
    let error = crate::scm::push_commit_to_branch(&fx.work, "origin", &fx.head(), "feat", false)
        .unwrap_err();
    assert!(!error.is_empty());
    assert_eq!(fx.origin_ref("feat").as_deref(), Some(pushed.as_str()));
    // Bad names never reach git.
    for bad in ["", "-x", "+feat", "a:b", "a b"] {
        assert!(
            crate::scm::push_commit_to_branch(&fx.work, "origin", &fx.head(), bad, false).is_err(),
            "{bad:?}"
        );
    }
    assert!(
        crate::scm::push_commit_to_branch(&fx.work, "origin", "nothex", "feat", false).is_err()
    );
}

#[test]
fn nothing_is_pushed_when_origin_already_has_the_commit() {
    let fx = Fx::new();
    git(&fx.work, &["push", "-q", "-u", "origin", "feat"]);
    // Only uncommitted files differ now.
    fx.install_push_counter();
    fx.run().0.unwrap();
    assert_eq!(fx.count_pushes(), 0);
    assert_eq!(fx.host.calls("workspace.applySnapshot")[0]["branch"], "feat");
}

// ------------------------------------------------------- idempotence and resume

#[test]
fn a_finished_push_step_is_not_pushed_again_even_if_it_runs_again() {
    let fx = Fx::new();
    fx.install_push_counter();
    fx.host.fail_once("workspace.applySnapshot", "Host busy");
    let (result, _) = fx.run();
    assert!(result.is_err());
    assert_eq!(fx.count_pushes(), 1);
    // The step ran before: running it again (as if the record were lost) asks origin first.
    let mut record = fx.record().unwrap();
    record.done.retain(|s| *s != MoveStep::Push);
    fx.save_record(&record);
    fx.run().0.unwrap();
    assert_eq!(fx.count_pushes(), 1, "the branch was pushed twice");
}

#[test]
fn every_host_step_resumes_where_it_failed_and_earlier_steps_do_not_repeat() {
    // (method that fails once, step that must be marked failed)
    let cases = [
        ("environment.describe", MoveStep::Upload),
        ("uploads.begin", MoveStep::Upload),
        ("workspace.applySnapshot", MoveStep::RestoreCode),
        ("sessions.import", MoveStep::RestoreConversation),
    ];
    for (method, step) in cases {
        let fx = Fx::new();
        fx.install_push_counter();
        fx.host.fail_once(method, "tunnel closed");
        let (result, events) = fx.run();
        assert_eq!(result.unwrap_err(), "tunnel closed", "{method}");
        let record = fx.record().unwrap();
        assert_eq!(record.failed_at, Some(step), "{method}");
        assert!(matches!(events.last(), Some(MoveEvent::Failed(_))));
        // The local session is untouched and fully usable.
        fx.assert_still_local_and_usable();
        let describe = fx.host.count("environment.describe");
        let begins = fx.host.count("uploads.begin");
        // Retry after a restart: a new service over the same store continues.
        let svc = fx.restarted();
        let mut again = vec![];
        let outcome = svc
            .run_move(
                fx.session,
                &|| false,
                &mut |e| again.push(e),
                &AtomicBool::new(false),
            )
            .unwrap();
        assert_eq!(outcome.host_session_id, "host-1", "{method}");
        assert_eq!(fx.count_pushes(), 1, "{method}: pushed twice");
        if step != MoveStep::Upload {
            assert_eq!(fx.host.count("environment.describe"), describe, "{method}");
            assert_eq!(fx.host.count("uploads.begin"), begins, "{method}");
        }
        assert!(fx.location().is_cloud(), "{method}");
    }
}

#[test]
fn a_failure_at_the_flip_resumes_without_importing_twice() {
    let fx = Fx::new();
    // The flip fails once (a store error), after the Host session exists.
    fx.set_flip_hook(|_| Err("disk full".into()));
    let (result, _) = fx.run();
    assert_eq!(result.unwrap_err(), "disk full");
    assert_eq!(fx.record().unwrap().failed_at, Some(MoveStep::Flip));
    assert_eq!(fx.host.count("sessions.import"), 1);
    *fx.svc.flip_hook.lock().unwrap() = None;
    let (result, _) = fx.run();
    result.unwrap();
    assert_eq!(fx.host.count("sessions.import"), 1);
    assert_eq!(fx.host.count("workspace.applySnapshot"), 1);
    assert!(fx.location().is_cloud());
}

#[test]
fn a_flip_that_already_happened_is_not_done_twice_after_a_crash() {
    let fx = Fx::new();
    fx.run().0.unwrap();
    // Crash after the location was written but before the record was cleared.
    let mut record = fx.begin_unchecked_for_crash_test();
    record.done = MoveStep::ALL[..6].to_vec();
    fx.save_record(&record);
    let (result, _) = fx.run();
    let outcome = result.unwrap();
    assert_eq!(outcome.host_session_id, "host-1");
    assert_eq!(fx.host.count("sessions.import"), 1);
}

impl Fx {
    /// The record a crashed run would have left after the location flip: rebuilt from the
    /// finished move's own outcome.
    fn begin_unchecked_for_crash_test(&self) -> MoveRecord {
        let SessionLocation::Cloud(location) = self.location() else {
            panic!("not in the cloud")
        };
        let mut record = serde_json::from_value::<MoveRecord>(json!({
            "sessionId": self.session,
            "target": {"kind": "existing", "backend": "codespaces", "name": "cs1"},
            "kind": {"kind": "resume"},
            "projectCwd": self.work.to_string_lossy(),
            "workCwd": self.work.to_string_lossy(),
            "title": "Fix the login",
            "provider": "claude",
            "claudeId": CLAUDE_ID,
            "startedAt": 1,
            "done": [],
            "current": null,
            "failedAt": null,
            "error": null,
            "head": self.head(),
            "tree": "0".repeat(40),
            "branch": "feat",
            "hostCwd": location.remote_cwd,
            "hostSessionId": location.host_session_id,
            "providerSessionId": CLAUDE_ID,
        }))
        .unwrap();
        record.workspace = Some(WorkspaceInfo {
            backend: CloudBackendKind::Codespaces,
            name: "cs1".into(),
            created: false,
            start_session: None,
            project: self
                .store
                .lock()
                .unwrap()
                .remote_projects()
                .unwrap()
                .into_values()
                .next(),
        });
        record
    }
}

#[test]
fn an_expired_upload_sends_the_move_back_to_the_upload_step_once() {
    let fx = Fx::new();
    fx.host
        .fail_once("workspace.applySnapshot", "Upload not found");
    let (result, _) = fx.run();
    result.unwrap();
    assert_eq!(
        fx.host.count("uploads.begin"),
        4,
        "both files were uploaded again"
    );
    assert_eq!(fx.host.count("workspace.applySnapshot"), 2);
}

#[test]
fn pending_moves_lists_without_touching_any_host() {
    let fx = Fx::new();
    fx.begin();
    let store = fx.store.lock().unwrap();
    let pending = pending_moves(&store);
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].session_id, fx.session);
    assert!(
        fx.host.methods().is_empty(),
        "listing must not wake or call anything"
    );
}

#[test]
fn a_cancelled_run_keeps_the_record_so_it_can_resume() {
    let fx = Fx::new();
    fx.begin();
    let cancel = AtomicBool::new(true);
    let (result, events) = fx.run_with(&|| false, &cancel);
    assert_eq!(result.unwrap_err(), MOVE_ABORTED);
    assert!(!events.iter().any(|e| matches!(e, MoveEvent::Failed(_))));
    assert!(fx.record().is_some());
    assert_eq!(fx.origin_ref("feat"), None, "nothing was pushed");
    fx.assert_still_local_and_usable();
}

// ----------------------------------------------------------------- the flip

#[test]
fn the_host_link_is_written_before_the_location() {
    let fx = Fx::new();
    let session = fx.session;
    let seen = Arc::new(Mutex::new(vec![]));
    let probe = seen.clone();
    fx.set_flip_hook(move |store| {
        probe.lock().unwrap().push((
            store.linked_remote_session(session).unwrap().is_some(),
            store.cloud_location(session).unwrap().is_cloud(),
        ));
        Err("crash between the link and the location".into())
    });
    let (result, _) = fx.run();
    assert!(result.is_err());
    // At that instant the link existed and the location was still Local.
    assert_eq!(*seen.lock().unwrap(), vec![(true, false)]);
    // The session is Local and usable even though a link is already there.
    fx.assert_still_local_and_usable();
    // The retry completes the flip: one link, one location.
    *fx.svc.flip_hook.lock().unwrap() = None;
    fx.run().0.unwrap();
    let store = fx.store.lock().unwrap();
    assert!(store.cloud_location(session).unwrap().is_cloud());
    assert!(store.cloud_host_record(session).is_ok());
}

#[test]
fn the_flip_refuses_when_head_changed_after_the_snapshot() {
    let fx = Fx::new();
    let work = fx.work.clone();
    *fx.host.on_request.lock().unwrap() = Some(Box::new(move |method| {
        if method == "workspace.applySnapshot" {
            commit_file(&work, "late.txt", "late\n");
        }
    }));
    let (result, events) = fx.run();
    assert_eq!(result.unwrap_err(), CHANGED_AFTER_START);
    let record = fx.record().unwrap();
    assert!(record.stale);
    assert_eq!(record.failed_at, Some(MoveStep::Flip));
    assert!(matches!(events.last(), Some(MoveEvent::Failed(_))));
    // Nothing was linked and the session is still Local.
    assert!(fx
        .store
        .lock()
        .unwrap()
        .linked_remote_session(fx.session)
        .unwrap()
        .is_none());
    fx.assert_still_local_and_usable();
    // Retry cannot succeed: the user has to cancel and start again.
    let (again, _) = fx.run();
    assert_eq!(again.unwrap_err(), CHANGED_AFTER_START);
}

#[test]
fn the_flip_refuses_when_the_last_row_changed_after_the_snapshot() {
    let fx = Fx::new();
    let store = fx.store.clone();
    let session = fx.session;
    *fx.host.on_request.lock().unwrap() = Some(Box::new(move |method| {
        if method == "workspace.applySnapshot" {
            // A late turn lands: same row count, different last row.
            store
                .lock()
                .unwrap()
                .write_rows(
                    session,
                    3,
                    &[Row {
                        role: "assistant".into(),
                        text: "changed".into(),
                        first: false,
                        sent_at: None,
                    }],
                )
                .unwrap();
        }
    }));
    let (result, _) = fx.run();
    assert_eq!(result.unwrap_err(), CHANGED_AFTER_START);
    assert!(fx.record().unwrap().stale);
    assert_eq!(fx.location(), SessionLocation::Local);
}

#[test]
fn moved_at_row_is_the_row_count_at_the_flip() {
    let fx = Fx::new();
    fx.run().0.unwrap();
    let SessionLocation::Cloud(location) = fx.location() else {
        panic!()
    };
    assert_eq!(location.moved_at_row, fx.row_count());
    // The first row append is accepted exactly at that point.
    let snapshot = json!({"revision": 1, "status": "idle", "session": {"id": "host-1", "blocks": [
        {"id": "b0", "role": "user", "text": "cloud turn"}]}});
    let mut store = fx.store.lock().unwrap();
    let outcome = store.append_remote_rows(fx.session, &snapshot).unwrap();
    assert_eq!(outcome.start, 4);
}

#[test]
fn a_busy_session_is_refused_before_and_during_the_move() {
    let fx = Fx::new();
    // Refused at the start.
    let error = fx.svc.begin_move(fx.request(), &|| true).unwrap_err();
    assert_eq!(error, BUSY);
    assert!(fx.record().is_none());
    // Refused when a run starts.
    fx.begin();
    let (result, _) = fx.run_with(&|| true, &AtomicBool::new(false));
    assert_eq!(result.unwrap_err(), BUSY);
    assert!(fx.host.methods().is_empty());
    assert_eq!(fx.origin_ref("feat"), None);
    // A turn that starts while the move runs is caught at the flip.
    let busy = Arc::new(AtomicBool::new(false));
    let flag = busy.clone();
    *fx.host.on_request.lock().unwrap() = Some(Box::new(move |method| {
        if method == "sessions.import" {
            flag.store(true, Ordering::SeqCst);
        }
    }));
    let probe = busy.clone();
    let (result, _) = fx.run_with(&move || probe.load(Ordering::SeqCst), &AtomicBool::new(false));
    assert_eq!(result.unwrap_err(), BUSY);
    assert_eq!(fx.record().unwrap().failed_at, Some(MoveStep::Flip));
    assert!(
        !fx.record().unwrap().stale,
        "busy is not a reason to start over"
    );
    assert_eq!(fx.location(), SessionLocation::Local);
}

#[test]
fn begin_refuses_a_second_move_and_a_session_that_is_already_in_the_cloud() {
    let fx = Fx::new();
    fx.begin();
    assert!(fx
        .svc
        .begin_move(fx.request(), &|| false)
        .unwrap_err()
        .contains("already being moved"));
    fx.svc
        .cancel_move(fx.session, CancelChoice::default())
        .unwrap();
    fx.run().0.unwrap();
    assert!(fx
        .svc
        .begin_move(fx.request(), &|| false)
        .unwrap_err()
        .contains("already in the cloud"));
}

// ------------------------------------------------------------ import decisions

#[test]
fn a_forked_decision_tells_the_user_and_uses_the_new_id() {
    let fx = Fx::new();
    *fx.host.decision.lock().unwrap() = "forked".into();
    let (result, events) = fx.run();
    let outcome = result.unwrap();
    assert!(outcome.forked);
    assert_eq!(
        outcome.provider_session_id.as_deref(),
        Some("99999999-8888-4777-8666-555555555555")
    );
    assert!(notices(&events).iter().any(|n| n.contains("diverged")));
    assert!(outcome.notices.iter().any(|n| n.contains("diverged")));
    // The Host record carries the new id; the local conversation keeps its own.
    let store = fx.store.lock().unwrap();
    let (_, record) = store.cloud_host_record(fx.session).unwrap();
    assert_eq!(
        record.summary.provider_id.as_deref(),
        Some("99999999-8888-4777-8666-555555555555")
    );
    assert_eq!(
        store
            .sessions()
            .unwrap()
            .iter()
            .find(|s| s.id == fx.session)
            .unwrap()
            .provider_id
            .as_deref(),
        Some(CLAUDE_ID)
    );
}

#[test]
fn installed_replaced_and_already_current_keep_the_id() {
    for decision in ["installed", "replaced", "already_current"] {
        let fx = Fx::new();
        *fx.host.decision.lock().unwrap() = decision.into();
        let outcome = fx.run().0.unwrap();
        assert_eq!(
            outcome.provider_session_id.as_deref(),
            Some(CLAUDE_ID),
            "{decision}"
        );
        assert!(!outcome.forked, "{decision}");
    }
}

#[test]
fn an_unknown_decision_fails_the_conversation_step() {
    let fx = Fx::new();
    *fx.host.decision.lock().unwrap() = "mystery".into();
    let (result, _) = fx.run();
    assert!(result.unwrap_err().contains("unexpected decision"));
    assert_eq!(
        fx.record().unwrap().failed_at,
        Some(MoveStep::RestoreConversation)
    );
}

#[test]
fn an_older_host_session_with_the_same_claude_id_is_deleted_before_the_import() {
    let fx = Fx::new();
    fx.host.sessions.lock().unwrap().extend([
        json!({"id": "old-1", "harness": "claude", "providerSessionId": CLAUDE_ID, "status": "idle"}),
        json!({"id": "other", "harness": "claude", "providerSessionId": "someone-else", "status": "idle"}),
    ]);
    fx.run().0.unwrap();
    let deleted: Vec<String> = fx
        .host
        .calls("sessions.delete")
        .iter()
        .map(|c| c["sessionId"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(deleted, vec!["old-1".to_string()]);
    let methods = fx.host.methods();
    assert!(index_of(&methods, "sessions.delete") < index_of(&methods, "sessions.import"));
}

#[test]
fn a_running_older_host_session_blocks_the_move_and_is_not_deleted() {
    let fx = Fx::new();
    fx.host.sessions.lock().unwrap().push(
        json!({"id": "old-1", "harness": "claude", "providerSessionId": CLAUDE_ID, "status": "running"}),
    );
    let (result, _) = fx.run();
    assert!(result.unwrap_err().contains("turn is running in the cloud"));
    assert_eq!(fx.host.count("sessions.delete"), 0);
    assert_eq!(fx.host.count("sessions.import"), 0);
}

// ---------------------------------------------------------------- verification

#[test]
fn a_tree_mismatch_after_applysnapshot_fails_the_code_step() {
    let fx = Fx::new();
    *fx.host.wrong_tree.lock().unwrap() = Some("0".repeat(40));
    let (result, _) = fx.run();
    assert!(result.unwrap_err().contains("does not match the snapshot"));
    assert_eq!(fx.record().unwrap().failed_at, Some(MoveStep::RestoreCode));
    assert_eq!(fx.host.count("sessions.import"), 0);
    fx.assert_still_local_and_usable();
}

#[test]
fn a_host_without_the_move_capabilities_is_refused_before_anything_is_uploaded() {
    let fx = Fx::new();
    fx.host
        .capabilities
        .lock()
        .unwrap()
        .retain(|c| c != "sessions.import");
    let (result, _) = fx.run();
    assert!(result.unwrap_err().contains("sessions.import is missing"));
    assert_eq!(fx.host.count("uploads.begin"), 0);
    assert_eq!(fx.record().unwrap().failed_at, Some(MoveStep::Upload));
}

// ------------------------------------------------------------ Claude version

#[test]
fn an_older_remote_claude_is_updated_and_a_newer_one_is_left_alone() {
    let fx = Fx::new();
    *fx.backend.version.lock().unwrap() = "2.1.300 (Claude Code)".into();
    let (result, events) = fx.run();
    result.unwrap();
    assert!(fx
        .backend
        .scripts()
        .iter()
        .any(|s| s.contains("claude update")));
    assert!(
        notices(&events).iter().all(|n| !n.contains("older")),
        "{:?}",
        notices(&events)
    );

    let fx = Fx::new();
    *fx.backend.version.lock().unwrap() = "2.1.999 (Claude Code)".into();
    fx.run().0.unwrap();
    assert!(!fx
        .backend
        .scripts()
        .iter()
        .any(|s| s.contains("claude update")));
}

#[test]
fn a_failed_claude_update_is_only_a_warning() {
    let fx = Fx::new();
    *fx.backend.version.lock().unwrap() = "2.1.300 (Claude Code)".into();
    *fx.backend.update_fails.lock().unwrap() = true;
    let (result, events) = fx.run();
    let outcome = result.unwrap();
    assert!(fx.location().is_cloud());
    assert!(notices(&events).iter().any(|n| n.contains("older than")));
    assert!(outcome.notices.iter().any(|n| n.contains("claude update")));
}

#[test]
fn versions_compare_as_numbers() {
    assert!(is_older("2.1.9 (Claude Code)", "2.1.10"));
    assert!(!is_older("2.1.10", "2.1.9"));
    assert!(!is_older("garbage", "2.1.9"));
    assert!(!is_older("2.1.9", "2.1.9"));
}

// ----------------------------------------------------------------- summary path

#[test]
fn a_non_claude_session_takes_the_summary_path_with_a_new_id() {
    let fx = Fx::build("codex", "2.1.321", |_| {});
    let record = fx.begin();
    assert!(matches!(record.kind, MoveKind::Summary { .. }));
    // Stop after the uploads to see what travelled.
    fx.host.fail_once("workspace.applySnapshot", "stop here");
    let (_, first) = fx.run();
    // The user is told about the summary when the snapshot is taken.
    assert!(notices(&first).iter().any(|n| n.contains("summary")));
    assert_eq!(fx.host.uploaded_names(), vec!["snapshot.patch".to_string()]);
    let (result, _) = fx.run();
    let outcome = result.unwrap();
    // No transcript travels and no import happens: a blank Claude session is created.
    assert_eq!(fx.host.count("sessions.import"), 0);
    let create = &fx.host.calls("commands.dispatch")[0];
    assert_eq!(create["type"], "create");
    assert_eq!(create["harness"], "claude");
    assert_eq!(create["worktreeCwd"], "/workspaces/widgets-wt");
    assert_eq!(outcome.host_session_id, "host-new");
    assert_eq!(
        outcome.provider_session_id, None,
        "the Host starts a new Claude session"
    );
    // The summary is the provider-switch handoff of the local rows.
    let store = fx.store.lock().unwrap();
    let handoff = store
        .agent_settings(fx.session)
        .unwrap()
        .unwrap()
        .handoff
        .expect("a handoff for the first cloud turn");
    assert_eq!(handoff.from, "codex");
    assert!(handoff.brief.contains("row 0"), "{}", handoff.brief);
    assert!(store.cloud_location(fx.session).unwrap().is_cloud());
    // The code still travelled: the snapshot was restored.
    drop(store);
    assert_eq!(fx.host.count("workspace.applySnapshot"), 2);
}

#[test]
fn a_summary_create_is_retried_with_the_same_command_id() {
    let fx = Fx::build("codex", "2.1.321", |_| {});
    fx.host.fail_once("commands.dispatch", "lost response");
    fx.begin();
    assert!(fx.run().0.is_err());
    fx.run().0.unwrap();
    let ids: Vec<String> = fx
        .host
        .calls("commands.dispatch")
        .iter()
        .map(|c| c["commandId"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(ids.len(), 2);
    assert_eq!(ids[0], ids[1]);
}

#[test]
fn a_conversation_over_the_size_limit_moves_as_a_summary() {
    let fx = Fx::new();
    let jsonl = fx
        .home
        .join("projects")
        .join(encode(&fx.work))
        .join(format!("{CLAUDE_ID}.jsonl"));
    std::fs::OpenOptions::new()
        .write(true)
        .open(&jsonl)
        .unwrap()
        .set_len(51 * 1024 * 1024)
        .unwrap();
    let (result, events) = fx.run();
    result.unwrap();
    assert_eq!(fx.host.count("sessions.import"), 0);
    assert!(notices(&events)
        .iter()
        .any(|n| n.contains("over the 50 MB limit")));
}

// ----------------------------------------------------------------- warnings

#[test]
fn lfs_and_dirty_submodules_are_surfaced_as_warnings() {
    let fx = Fx::with(|work| {
        let sub = work.parent().unwrap().join("sub-src");
        std::fs::create_dir_all(&sub).unwrap();
        git(&sub, &["init", "-q", "-b", "main"]);
        configure(&sub);
        commit_file(&sub, "lib.txt", "lib\n");
        git(
            work,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "-q",
                sub.to_str().unwrap(),
                "vendor",
            ],
        );
        std::fs::write(
            work.join(".gitattributes"),
            "*.psd filter=lfs diff=lfs merge=lfs -text\n",
        )
        .unwrap();
        git(work, &["add", ".gitattributes", "vendor", ".gitmodules"]);
        git(work, &["commit", "-q", "-m", "lfs and submodule"]);
        std::fs::write(work.join("vendor/lib.txt"), "edited\n").unwrap();
    });
    let (result, events) = fx.run();
    result.unwrap();
    let all = notices(&events).join("\n");
    assert!(all.contains("Git LFS"), "{all}");
    assert!(all.contains("submodule vendor"), "{all}");
    assert!(all.contains("Ignored files stay on this computer"), "{all}");
}

// ------------------------------------------------------------------- cancel

#[test]
fn a_cancel_rolls_back_what_the_move_created_and_leaves_the_session_local() {
    let fx = Fx::new();
    fx.set_flip_hook(|_| Err("crash after the import".into()));
    let (result, _) = fx.run();
    assert!(result.is_err());
    // At this point the move created: a pushed branch, uploads, a Host worktree, a Host
    // session, a linked Host record and a staging folder.
    assert_eq!(fx.host.uploaded_names().len(), 2);
    assert!(fx.staging().exists());
    let report = fx
        .svc
        .cancel_move(fx.session, CancelChoice::default())
        .unwrap();
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    // The Host session and the uploads are gone; the worktree was removed through the backend.
    assert_eq!(fx.host.calls("sessions.delete")[0]["sessionId"], "host-1");
    assert!(fx.host.uploaded_names().is_empty());
    let scripts = fx.backend.scripts().join("\n");
    assert!(scripts.contains("worktree remove --force"), "{scripts}");
    assert!(scripts.contains("/workspaces/widgets-wt"));
    assert!(
        scripts.contains("'/workspaces/widgets'"),
        "the project root is the repository"
    );
    // Local state: no record, no staging, no note, no stray link or Host record.
    assert!(!fx.staging().exists());
    {
        let store = fx.store.lock().unwrap();
        assert!(move_record(&store, fx.session).is_none());
        assert!(moved_path_note(&store, fx.session).is_none());
        assert!(store.linked_remote_session(fx.session).unwrap().is_none());
        assert!(store
            .cloud_sessions(&fx.work.to_string_lossy())
            .unwrap()
            .is_empty());
    }
    fx.assert_still_local_and_usable();
    // The pushed branch is NEVER deleted: it is reported, and still on origin.
    assert_eq!(report.created_branch.as_deref(), Some("feat"));
    assert!(fx.origin_ref("feat").is_some());
    // An existing workspace is never offered for deletion.
    assert_eq!(report.kept_workspace, None);
    assert!(fx.backend.deleted.lock().unwrap().is_empty());
}

#[test]
fn a_cancel_after_a_failed_upload_removes_the_record_and_staging_but_not_the_push() {
    let fx = Fx::new();
    fx.host.fail_once("environment.describe", "stop at upload");
    let _ = fx.run();
    let report = fx
        .svc
        .cancel_move(fx.session, CancelChoice::default())
        .unwrap();
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert!(fx.record().is_none());
    assert!(!fx.staging().exists());
    assert!(fx.origin_ref("feat").is_some(), "the push is not undone");
    fx.assert_still_local_and_usable();
}

#[test]
fn cancel_without_a_move_is_an_error() {
    let fx = Fx::new();
    assert!(fx
        .svc
        .cancel_move(fx.session, CancelChoice::default())
        .is_err());
}

// ------------------------------------------------------------------ helpers

#[test]
fn credentials_in_urls_are_scrubbed_from_errors() {
    assert_eq!(
        scrub("fatal: could not read https://user:ghp_secret@github.com/o/r.git/ now"),
        "fatal: could not read https://github.com/o/r.git/ now"
    );
    assert_eq!(scrub("no url here"), "no url here");
}

// ------------------------------------------------------------ a new workspace

const AUTH_OK: &str = "github.com\n  \u{2713} Logged in to github.com account me (keyring)\n  - Token scopes: 'codespace', 'repo'\n";

/// A `gh` that creates one codespace, `octo-new1`, and says what origin had at that moment.
struct StartGh {
    calls: Mutex<Vec<String>>,
    /// Runs when the codespace is requested (the branch must be on origin by then).
    on_create: Box<dyn Fn() + Send + Sync>,
}
impl GhRunner for StartGh {
    fn run(&self, args: &[String], _: Option<&str>) -> Result<GhOutput, String> {
        let line = args.join(" ");
        self.calls.lock().unwrap().push(line.clone());
        let out = |stdout: &str| {
            Ok(GhOutput {
                success: true,
                stdout: stdout.into(),
                stderr: String::new(),
            })
        };
        if line.starts_with("auth status") {
            out(AUTH_OK)
        } else if line.starts_with("api --method POST") {
            (self.on_create)();
            out(r#"{"name":"octo-new1","state":"Provisioning","web_url":"https://octo-new1.github.dev"}"#)
        } else if line.starts_with("api user/codespaces") {
            out("https://asked.github.dev\n")
        } else if line.starts_with("codespace view") {
            out(r#"{"state":"Available"}"#)
        } else if line.starts_with("codespace ssh") {
            out("")
        } else {
            panic!("unexpected gh call: {line}")
        }
    }
}

fn new_codespace_fx() -> (Fx, Arc<StartGh>, Arc<Mutex<Vec<bool>>>) {
    let seen = Arc::new(Mutex::new(vec![]));
    let probe = seen.clone();
    let origin: Arc<Mutex<Option<PathBuf>>> = Arc::new(Mutex::new(None));
    let looked = origin.clone();
    let gh = Arc::new(StartGh {
        calls: Mutex::new(vec![]),
        on_create: Box::new(move || {
            let origin = looked.lock().unwrap().clone().unwrap();
            let out = Command::new("git")
                .arg("-C")
                .arg(origin)
                .args(["rev-parse", "--verify", "-q", "refs/heads/feat"])
                .output()
                .unwrap();
            probe.lock().unwrap().push(out.status.success());
        }),
    });
    let fx = Fx::build_with("claude", "2.1.321", |_| {}, gh.clone());
    *origin.lock().unwrap() = Some(fx.origin.clone());
    (fx, gh, seen)
}

fn new_request(fx: &Fx) -> MoveRequest {
    MoveRequest {
        target: MoveTarget::NewCodespace { machine: None },
        ..fx.request()
    }
}

#[test]
fn a_new_codespace_is_created_on_the_pushed_branch_after_the_push() {
    let (fx, gh, seen) = new_codespace_fx();
    fx.svc.begin_move(new_request(&fx), &|| false).unwrap();
    let outcome = fx.run().0.unwrap();
    // The branch was on origin when the codespace was requested, and the request names it.
    assert_eq!(*seen.lock().unwrap(), vec![true]);
    let create = gh
        .calls
        .lock()
        .unwrap()
        .iter()
        .find(|c| c.starts_with("api --method POST"))
        .cloned()
        .unwrap();
    assert!(create.contains("ref=feat"), "{create}");
    // A new instance restores into its own checkout, and nothing deletes it.
    assert_eq!(
        fx.host.calls("workspace.applySnapshot")[0]["instance"],
        "new"
    );
    assert_eq!(outcome.cloud, CloudRef::codespace("octo-new1"));
    assert!(fx.location().is_cloud());
    // The start's own blank session does not stay behind as a second sidebar row.
    let store = fx.store.lock().unwrap();
    let blanks: Vec<_> = store
        .cloud_sessions(&fx.work.to_string_lossy())
        .unwrap()
        .into_iter()
        .filter(|s| s.session_id.is_none())
        .collect();
    assert!(blanks.is_empty(), "{blanks:?}");
    assert!(crate::cloud_sessions::pending_starts(&store).is_empty());
}

#[test]
fn cancelling_deletes_a_workspace_the_move_created_only_when_asked() {
    for delete in [false, true] {
        let (fx, _gh, _) = new_codespace_fx();
        fx.svc.begin_move(new_request(&fx), &|| false).unwrap();
        fx.set_flip_hook(|_| Err("crash".into()));
        assert!(fx.run().0.is_err());
        let report = fx
            .svc
            .cancel_move(
                fx.session,
                CancelChoice {
                    delete_workspace: delete,
                },
            )
            .unwrap();
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        assert_eq!(report.deleted_workspace, delete);
        assert_eq!(
            report.kept_workspace,
            (!delete).then(|| CloudRef::codespace("octo-new1"))
        );
        assert_eq!(
            *fx.backend.deleted.lock().unwrap(),
            if delete {
                vec!["octo-new1".to_string()]
            } else {
                vec![]
            }
        );
        // A kept workspace's working copy is not touched when it is deleted with it, and the
        // new instance's own checkout is never removed by the move.
        assert!(!fx.backend.scripts().join("\n").contains("worktree remove"));
        assert!(fx.record().is_none());
        fx.assert_still_local_and_usable();
    }
}

#[test]
fn a_start_that_failed_resumes_after_a_restart_without_creating_a_second_codespace() {
    let (fx, gh, _) = new_codespace_fx();
    fx.svc.begin_move(new_request(&fx), &|| false).unwrap();
    *fx.host.connect_error.lock().unwrap() = Some("pairing failed".into());
    let (result, _) = fx.run();
    assert!(result.unwrap_err().contains("pairing failed"));
    let record = fx.record().unwrap();
    assert_eq!(record.failed_at, Some(MoveStep::Workspace));
    // The codespace is known to the move before the start runs, so a cancel could find it.
    assert_eq!(record.cloud(), Some(CloudRef::codespace("octo-new1")));
    // The app restarts and the user chooses Retry.
    let svc = CloudSessions::new(gh.clone(), fx.host.clone(), fx.store.clone())
        .with_origin(|_| Ok("git@github.com:octo/widgets.git".into()))
        .with_timing(Duration::from_secs(5), |_| {}, || 1_794_024_100_000)
        .with_backend(fx.backend.clone());
    let outcome = svc
        .run_move(
            fx.session,
            &|| false,
            &mut |_| {},
            &AtomicBool::new(false),
        )
        .unwrap();
    assert_eq!(outcome.cloud, CloudRef::codespace("octo-new1"));
    let creates = gh
        .calls
        .lock()
        .unwrap()
        .iter()
        .filter(|c| c.starts_with("api --method POST"))
        .count();
    assert_eq!(creates, 1, "a second codespace was created");
}
