//! Cloud sessions: ordinary remote sessions that live in a GitHub codespace.
//!
//! A cloud session is a ready-made remote session (`remote://<env>/workspaces/<repo>`)
//! on a machine whose transport is `gh codespace ssh` (see `remote::ssh`). What makes
//! it "cloud" is the binding stored on its `store::RemoteProject`:
//! `local_project` (the cwd key of the local project it twins) and `codespace`
//! (the codespace name). Everything else (chat, composer, stop, archive) reuses the
//! remote session path unchanged.
//!
//! # Public API (shared by the GUI and the app API)
//!
//! All of it hangs off [`CloudSessions`]; construct it with
//! [`CloudSessions::system`] (real `gh` + default connections) or
//! [`CloudSessions::new`] (injected seams, used by tests).
//!
//! * [`CloudSessions::prepare_target`] `(local_cwd, Target, progress) -> CloudBinding`:
//!   resolve the local project's GitHub repo from `origin`, create a codespace
//!   (`CodespaceChoice::New`, needs a branch) or validate an existing one
//!   (`Named`), reuse or create its machine, wake it, open `/workspaces/<repo>`
//!   and record the binding. BLOCKS for minutes (creation, boot, bootstrap): run
//!   it on a background thread and surface `progress` lines.
//! * [`CloudSessions::create_session`] `(&CloudBinding, harness) -> Session`:
//!   start a blank session on the binding. Phase 1 accepts only `claude`
//!   ([`HARNESS`]); anything else is rejected with a clear error. The first send
//!   goes through the normal remote run path.
//! * [`CloudSessions::cloud_sessions_for`] `(local_cwd) -> Vec<CloudSession>`:
//!   the sessions bound to codespaces of a local project (pure store read).
//! * [`CloudSessions::codespace_state`] / [`CloudSessions::cloud_state`]: the
//!   codespace lifecycle state, cached for [`STATE_TTL`] (30s). Never wakes
//!   anything. `cloud_state` is infallible: failures become [`CloudState::Unknown`],
//!   so an idle-stopped or unreachable codespace is a displayable state, never a
//!   toast.
//! * [`CloudSessions::stop_codespace`] / [`CloudSessions::delete_codespace`]:
//!   `delete` also disconnects the machine and forgets every binding.
//! * [`ensure_before_send`] `(&dyn Host, &RemoteSession)`: the wake hook used by
//!   the GUI remote run path before dispatching. No-op for non-cloud sessions;
//!   for cloud sessions it enforces the Claude-only rule and calls
//!   `Connections::ensure_codespace` (wakes after an idle stop). BLOCKS; only
//!   call from a worker thread on a user-initiated send/retry/stop, never from
//!   background polling.
//!
//! * [`CloudSessions::pull_evidence`] `(session_id, Option<remote_path>) -> EvidencePull`:
//!   copy the files an agent left in the codespace (default
//!   `/workspaces/<repo>/.evidence`) into the session's own folder
//!   `<db>.threads/<session>/evidence/` (a custom path lands in `evidence/<its name>`),
//!   merging over earlier pulls, and return the local
//!   file paths. A missing remote folder is `Ok` with no files and a `message`. This is a
//!   user action: `gh codespace cp` starts a stopped codespace. BLOCKS (gh).
//!
//! Errors are user-facing `String`s. `CODESPACE_NOT_CONNECTED` from
//! `Connections::request` means "stopped / not connected": render it as a state
//! (see [`is_not_connected`]).
use crate::codespaces::{self, GhRunner};
use crate::remote::{self, Connections, Machine};
use crate::store::{RemoteSession, Store};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The only harness cloud sessions support in phase 1.
pub const HARNESS: &str = "claude";
pub const STATE_TTL: Duration = Duration::from_secs(30);
const UNSUPPORTED_HARNESS: &str = "Cloud sessions support Claude only for now.";

/// True when `error` is `Connections::request`'s "codespace is not connected".
pub fn is_not_connected(error: &str) -> bool {
    error.starts_with("Codespace is not connected")
}

/// Error text for a start that failed after its codespace was created.
pub fn created_note(codespace: &str, error: &str) -> String {
    format!(
        "{error} (codespace {codespace} was created and kept; retry with target {{\"codespace\":\"{codespace}\"}} or remove it with codespaces.delete)"
    )
}

/// Which codespace a cloud session should use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CodespaceChoice {
    /// Create a codespace (needs `Target::branch`).
    New,
    /// Use an existing codespace of the project's repository.
    Named(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub codespace: CodespaceChoice,
    /// Branch for a new codespace.
    pub branch: Option<String>,
    /// Machine type for a new codespace (default `basicLinux32gb`).
    pub machine: Option<String>,
}

/// A ready remote project bound to a codespace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CloudBinding {
    pub local_project: String,
    pub codespace: String,
    /// `owner/repo`.
    pub repo: String,
    pub machine_id: String,
    pub environment_id: String,
    pub project_id: String,
    /// Project cwd key (`remote://<env>/workspaces/<repo>`): what sessions use as `cwd`.
    pub project_key: String,
    /// Path on the codespace host (`/workspaces/<repo>`).
    pub remote_cwd: String,
}

/// One cloud session of a local project.
#[derive(Clone, Debug)]
pub struct CloudSession {
    pub summary: crate::store::Session,
    pub codespace: String,
    pub environment_id: String,
    pub session_id: Option<String>,
    pub running: bool,
    pub needs_input: bool,
}

/// Displayable codespace lifecycle state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloudState {
    Available,
    Starting,
    Stopping,
    Stopped,
    Unavailable,
    Unknown,
}
impl CloudState {
    /// Maps a `gh` codespace state string.
    pub fn from_gh(state: &str) -> Self {
        match state {
            "Available" => Self::Available,
            "Shutdown" => Self::Stopped,
            "ShuttingDown" => Self::Stopping,
            "Starting" | "Queued" | "Provisioning" | "Awaiting" | "Created" | "Rebuilding"
            | "Exporting" | "Updating" | "Moved" => Self::Starting,
            "Failed" | "Deleted" | "Unavailable" => Self::Unavailable,
            _ => Self::Unknown,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Available => "Available",
            Self::Starting => "Starting",
            Self::Stopping => "Stopping",
            Self::Stopped => "Stopped",
            Self::Unavailable => "Unavailable",
            Self::Unknown => "Unknown",
        }
    }
}

/// Machine/transport seam over [`Connections`] so the service is testable.
pub trait Host: Send + Sync {
    fn machines(&self) -> Result<Vec<Machine>, String>;
    /// Pair with a codespace (starting it via gh if needed); blocks until done.
    fn connect(
        &self,
        codespace: &str,
        display: &str,
        progress: &mut dyn FnMut(&str),
    ) -> Result<Machine, String>;
    /// Wake the codespace and reconnect if needed (blocking, no-op when
    /// connected). `progress` fires only when a wake actually starts; `cancel`
    /// aborts the wait with `remote::WAKE_CANCELLED`. Concurrent calls for one
    /// machine share a single reconnect.
    fn ensure(
        &self,
        machine_id: &str,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(&str),
    ) -> Result<(), String>;
    fn request(&self, machine_id: &str, method: &str, params: Value) -> Result<Value, String>;
    fn disconnect(&self, machine_id: &str) -> Result<(), String>;
}

impl Host for Arc<Connections> {
    fn machines(&self) -> Result<Vec<Machine>, String> {
        self.list()
    }
    fn connect(
        &self,
        codespace: &str,
        display: &str,
        progress: &mut dyn FnMut(&str),
    ) -> Result<Machine, String> {
        let job = self.codespace_begin(codespace, Some(display))?;
        let mut last = String::new();
        loop {
            let view = self.ssh_poll(&job)?;
            if view.message != last {
                progress(&view.message);
                last = view.message.clone();
            }
            if view.prompt.is_some() {
                let _ = self.ssh_cancel(&job);
                return Err("The codespace connection asked an unexpected question".into());
            }
            if view.done {
                return match (view.error, view.machine) {
                    (Some(error), _) => Err(error),
                    (None, Some(machine)) => Ok(machine),
                    (None, None) => Err("Codespace connection finished without a machine".into()),
                };
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    fn ensure(
        &self,
        machine_id: &str,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(&str),
    ) -> Result<(), String> {
        self.ensure_codespace_with(machine_id, cancel, progress)
    }
    fn request(&self, machine_id: &str, method: &str, params: Value) -> Result<Value, String> {
        Connections::request(self, machine_id, method, params)
    }
    fn disconnect(&self, machine_id: &str) -> Result<(), String> {
        Connections::disconnect(self, machine_id)
    }
}

/// Wake hook for the remote run path; see the module docs.
pub fn ensure_before_send(
    host: &dyn Host,
    session: &RemoteSession,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&str),
) -> Result<(), String> {
    if session.project.codespace.is_none() {
        return Ok(());
    }
    require_claude(&session.summary.provider)?;
    let machine = host
        .machines()?
        .into_iter()
        .find(|m| m.environment_id == session.project.environment_id)
        .ok_or("Codespace machine is no longer connected. Start a new cloud session.")?;
    host.ensure(&machine.id, cancel, progress)
}

/// Phase 1 rule: cloud sessions run Claude only.
pub fn require_claude(harness: &str) -> Result<(), String> {
    if harness == HARNESS {
        Ok(())
    } else {
        Err(UNSUPPORTED_HARNESS.into())
    }
}

fn origin_url(cwd: &str) -> Result<String, String> {
    let out = std::process::Command::new("git")
        .args(["-C", cwd, "remote", "get-url", "origin"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|e| format!("Cannot run git: {e}"))?;
    let url = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !out.status.success() || url.is_empty() {
        return Err("This project has no `origin` remote, so it has no GitHub repository.".into());
    }
    Ok(url)
}

type Database = Arc<Mutex<Store>>;
type OriginFn = Box<dyn Fn(&str) -> Result<String, String> + Send + Sync>;

pub struct CloudSessions {
    gh: Arc<dyn GhRunner>,
    host: Arc<dyn Host>,
    store: Database,
    states: Mutex<HashMap<String, (Instant, String)>>,
    /// Codespaces gh reported as not found (or that were deleted here).
    gone: Mutex<std::collections::HashSet<String>>,
    ttl: Duration,
    origin: OriginFn,
    claude_dir: std::path::PathBuf,
    /// Parent of the codespace checkouts; only tests use another one.
    workspace_root: String,
}

impl CloudSessions {
    pub fn new(gh: Arc<dyn GhRunner>, host: Arc<dyn Host>, store: Database) -> Self {
        Self {
            gh,
            host,
            store,
            states: Mutex::new(HashMap::new()),
            gone: Mutex::new(Default::default()),
            ttl: STATE_TTL,
            origin: Box::new(origin_url),
            claude_dir: convert::default_claude_dir(),
            workspace_root: "/workspaces".into(),
        }
    }
    #[cfg(all(test, unix))]
    fn with_claude_dir(mut self, dir: std::path::PathBuf) -> Self {
        self.claude_dir = dir;
        self
    }
    #[cfg(all(test, unix))]
    fn with_workspace_root(mut self, root: &std::path::Path) -> Self {
        self.workspace_root = root.to_string_lossy().into_owned();
        self
    }
    /// Real `gh` and the app's default connections.
    pub fn system(store: Database) -> Result<Self, String> {
        Ok(Self::new(
            Arc::new(codespaces::SystemGh),
            Arc::new(remote::default_connections()?),
            store,
        ))
    }
    #[cfg(test)]
    fn with_origin(
        mut self,
        origin: impl Fn(&str) -> Result<String, String> + Send + Sync + 'static,
    ) -> Self {
        self.origin = Box::new(origin);
        self
    }
    fn store(&self) -> Result<std::sync::MutexGuard<'_, Store>, String> {
        self.store
            .lock()
            .map_err(|_| "Native session store unavailable".into())
    }

    /// `owner/repo` of the local project's `origin`.
    pub fn repo_for(&self, local_cwd: &str) -> Result<String, String> {
        codespaces::repo_from_remote(&(self.origin)(local_cwd)?)
            .ok_or_else(|| "The project's origin is not a GitHub repository.".to_string())
    }

    /// Get a connected, project-open codespace for `local_cwd`. BLOCKS.
    ///
    /// A codespace this call created is never deleted on failure: the error then names it
    /// (see [`created_note`]) so the caller retries with `CodespaceChoice::Named`.
    pub fn prepare_target(
        &self,
        local_cwd: &str,
        target: Target,
        progress: &mut dyn FnMut(&str),
    ) -> Result<CloudBinding, String> {
        let mut created = None;
        self.prepare_inner(local_cwd, target, progress, &mut created)
            .map_err(|error| match &created {
                Some(name) => created_note(name, &error),
                None => error,
            })
    }
    fn prepare_inner(
        &self,
        local_cwd: &str,
        target: Target,
        progress: &mut dyn FnMut(&str),
        created: &mut Option<String>,
    ) -> Result<CloudBinding, String> {
        codespaces::auth_status(&*self.gh)?.require_codespace_scope()?;
        let repo = self.repo_for(local_cwd)?;
        let remote_cwd = codespaces::workspace_path(&repo)
            .ok_or_else(|| format!("Invalid repository {repo}"))?;
        let (name, display) = match target.codespace {
            CodespaceChoice::New => {
                let branch = target
                    .branch
                    .as_deref()
                    .map(str::trim)
                    .filter(|b| !b.is_empty())
                    .ok_or("A branch is required to create a codespace")?;
                progress("Creating codespace");
                let name = codespaces::create(
                    &*self.gh,
                    &repo,
                    branch,
                    target.machine.as_deref(),
                    None,
                    progress,
                )?;
                *created = Some(name.clone());
                progress("Waiting for the codespace to become available");
                codespaces::wait_ready(&*self.gh, &name)?;
                (name, codespaces::display_name_for(branch))
            }
            CodespaceChoice::Named(name) => {
                let known = codespaces::list(&*self.gh, &repo)?;
                let found = known
                    .iter()
                    .find(|c| c.name == name)
                    .ok_or_else(|| format!("Codespace {name} does not belong to {repo}"))?;
                match CloudState::from_gh(&found.state) {
                    CloudState::Unavailable => {
                        return Err(format!(
                            "Codespace {name} is unavailable ({}). Pick another or create a new one.",
                            found.state
                        ));
                    }
                    CloudState::Stopping => {
                        return Err(format!(
                            "Codespace {name} is shutting down. Try again in a moment."
                        ));
                    }
                    _ => {}
                }
                let display = if found.display_name.is_empty() {
                    name.clone()
                } else {
                    found.display_name.clone()
                };
                (name, display)
            }
        };
        self.invalidate(&name);
        let machine = match self
            .host
            .machines()?
            .into_iter()
            .find(|m| m.codespace.as_deref() == Some(name.as_str()))
        {
            Some(machine) => {
                progress("Connecting to the codespace");
                self.host
                    .ensure(&machine.id, &AtomicBool::new(false), progress)?;
                machine
            }
            None => self.host.connect(&name, &display, progress)?,
        };
        progress("Opening the project");
        let project =
            self.host
                .request(&machine.id, "projects.open", json!({"cwd": remote_cwd}))?;
        // Best effort: a failure here must not block the session, the agent just has no hint.
        progress("Preparing the evidence folder");
        if let Err(error) = codespaces::prepare_evidence(&*self.gh, &name, &remote_cwd) {
            progress(&format!("Could not prepare the evidence folder: {error}"));
        }
        let remembered = self.store()?.remember_cloud_project(
            &machine.environment_id,
            &project,
            local_cwd,
            &name,
        )?;
        Ok(CloudBinding {
            local_project: local_cwd.into(),
            codespace: name,
            repo,
            machine_id: machine.id,
            environment_id: remembered.environment_id,
            project_id: remembered.project_id,
            project_key: remembered.key,
            remote_cwd: remembered.cwd,
        })
    }

    /// Start a blank cloud session on `binding`. Claude only (phase 1).
    pub fn create_session(
        &self,
        binding: &CloudBinding,
        harness: &str,
    ) -> Result<crate::store::Session, String> {
        require_claude(harness)?;
        self.store()?.create(harness, &binding.project_key)
    }

    /// Sessions of `local_cwd` that live in codespaces, newest first.
    pub fn cloud_sessions_for(&self, local_cwd: &str) -> Result<Vec<CloudSession>, String> {
        Ok(self
            .store()?
            .cloud_sessions(local_cwd)?
            .into_iter()
            .filter_map(|s| {
                Some(CloudSession {
                    codespace: s.project.codespace.clone()?,
                    environment_id: s.project.environment_id.clone(),
                    session_id: s.session_id.clone(),
                    running: s.running,
                    needs_input: s.needs_input,
                    summary: s.summary,
                })
            })
            .collect())
    }

    /// Raw gh state of a codespace, cached for the TTL. Never wakes it.
    pub fn codespace_state(&self, name: &str) -> Result<String, String> {
        let cached = self
            .states
            .lock()
            .map_err(|_| "State cache poisoned")?
            .get(name)
            .filter(|(at, _)| at.elapsed() < self.ttl)
            .map(|(_, state)| state.clone());
        if let Some(state) = cached {
            return Ok(state);
        }
        let state = codespaces::state(&*self.gh, name)?;
        self.states
            .lock()
            .map_err(|_| "State cache poisoned")?
            .insert(name.into(), (Instant::now(), state.clone()));
        Ok(state)
    }
    /// Infallible, displayable variant of [`Self::codespace_state`].
    pub fn cloud_state(&self, name: &str) -> CloudState {
        self.codespace_state(name)
            .map_or(CloudState::Unknown, |s| CloudState::from_gh(&s))
    }
    /// Whether gh says the codespace no longer exists. A codespace that cannot be read for
    /// another reason (offline, not signed in) is not gone.
    pub fn codespace_gone(&self, name: &str) -> bool {
        if self.gone.lock().is_ok_and(|g| g.contains(name)) {
            return true;
        }
        match self.codespace_state(name) {
            Ok(state) => state == "Deleted",
            Err(e) => {
                let e = e.to_lowercase();
                // gh prints "HTTP 404: Not Found" for a missing codespace; a missing `gh` binary
                // says "command not found" and must not count.
                let missing = e.contains("http 404");
                if missing {
                    self.mark_gone(name);
                }
                missing
            }
        }
    }
    fn mark_gone(&self, name: &str) {
        if let Ok(mut gone) = self.gone.lock() {
            gone.insert(name.to_string());
        }
    }

    /// Converted local sessions and the codespace each one came from.
    pub fn converted_origins(&self) -> Vec<(i64, String)> {
        self.store()
            .and_then(|s| s.codespace_origins())
            .unwrap_or_default()
    }

    /// What deleting `codespace` would lose, as recorded when its sessions were saved.
    pub fn unsaved_for(&self, codespace: &str) -> Vec<String> {
        self.store()
            .ok()
            .and_then(|s| s.setting(&convert::unsaved_key(codespace)).ok().flatten())
            .and_then(|v| serde_json::from_str(&v).ok())
            .unwrap_or_default()
    }
    /// Remembers `lines` for `codespace` (merged with what is already recorded), so every later
    /// delete of it asks again.
    pub fn record_unsaved(&self, codespace: &str, lines: &[String]) -> Result<(), String> {
        if lines.is_empty() {
            return Ok(());
        }
        let mut all = self.unsaved_for(codespace);
        for line in lines {
            if !all.contains(line) {
                all.push(line.clone());
            }
        }
        let value = serde_json::to_string(&all).map_err(|e| e.to_string())?;
        self.store()?
            .save_setting(&convert::unsaved_key(codespace), &value)
    }
    pub fn invalidate(&self, name: &str) {
        if let Ok(mut states) = self.states.lock() {
            states.remove(name);
        }
    }

    pub fn stop_codespace(&self, name: &str) -> Result<(), String> {
        codespaces::stop(&*self.gh, name)?;
        self.invalidate(name);
        Ok(())
    }

    /// Delete the codespace, disconnect its machine and forget all bindings.
    /// A failed disconnect after a successful delete is an `Err` here; use
    /// [`Self::delete_codespace_outcome`] to tell the two apart.
    pub fn delete_codespace(&self, name: &str) -> Result<(), String> {
        match self.delete_codespace_outcome(name)? {
            DeleteOutcome::Deleted => Ok(()),
            DeleteOutcome::DeletedWithWarning(e) => {
                Err(format!("Codespace deleted, but disconnecting failed: {e}"))
            }
        }
    }

    /// Like [`Self::delete_codespace`], with a typed result: `Err` means the
    /// codespace is still there; a warning means it is gone and its bindings
    /// were forgotten, but the machine could not be disconnected.
    pub fn delete_codespace_outcome(&self, name: &str) -> Result<DeleteOutcome, String> {
        codespaces::delete(&*self.gh, name)?;
        self.invalidate(name);
        let mut disconnect_error = None;
        match self.host.machines() {
            Ok(machines) => {
                for machine in machines {
                    if machine.codespace.as_deref() == Some(name) {
                        if let Err(e) = self.host.disconnect(&machine.id) {
                            disconnect_error.get_or_insert(e);
                        }
                    }
                }
            }
            Err(e) => disconnect_error = Some(e),
        }
        self.mark_gone(name);
        // The codespace is gone, so what it held is no longer a question.
        let _ = self
            .store()?
            .save_setting(&convert::unsaved_key(name), "[]");
        self.store()?.forget_codespace(name)?;
        Ok(match disconnect_error {
            Some(e) => DeleteOutcome::DeletedWithWarning(e),
            None => DeleteOutcome::Deleted,
        })
    }

    /// Copies the evidence of a cloud session into its local folder, see the module docs.
    /// Remote symlinks are followed by scp (it copies what they point at), so keep secrets
    /// out of the folder. A failure inside the folder keeps what was copied and is reported
    /// in `warning`; only a missing top-level folder is the empty `message` case.
    pub fn pull_evidence(
        &self,
        session_id: i64,
        remote_path: Option<&str>,
    ) -> Result<EvidencePull, String> {
        let (codespace, workspace, thread) = {
            let store = self.store()?;
            let session = store
                .remote_session(session_id)?
                .ok_or("Session not found")?;
            let codespace = session
                .project
                .codespace
                .clone()
                .ok_or("This session does not run in a codespace.")?;
            let thread = store
                .thread_dir(session_id)?
                .ok_or("This store keeps no session folders, so evidence cannot be saved.")?;
            (codespace, session.project.cwd.clone(), thread)
        };
        let remote = evidence_remote_path(&self.workspace_root, &workspace, remote_path)?;
        let local_dir = thread.join(EVIDENCE_DIR);
        let mut pull = EvidencePull {
            session_id,
            codespace: codespace.clone(),
            remote_path: remote.clone(),
            local_dir: local_dir.clone(),
            files: vec![],
            message: None,
            warning: None,
        };
        std::fs::create_dir_all(&thread).map_err(|e| e.to_string())?;
        let staging = thread.join(format!(
            "evidence.pull-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        std::fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
        let copied = codespaces::copy_from(&*self.gh, &codespace, &remote, &staging);
        // gh may have started the codespace.
        self.invalidate(&codespace);
        let mut problems = Vec::new();
        let mut hard_error = None;
        match copied {
            Ok(()) => {}
            Err(e) if codespaces::is_missing_path(&e, &remote) => {}
            // scp copies what it can: keep it, then say what failed.
            Err(e) => problems.push(e),
        }
        let name = remote.rsplit('/').next().unwrap_or_default();
        let source = staging.join(name);
        match std::fs::symlink_metadata(&source) {
            // The default folder is the evidence folder itself; anything else keeps its
            // name so two pulls cannot overwrite each other.
            Ok(meta) if meta.is_dir() && remote_path.is_none() => {
                merge_files(&source, &local_dir, &mut pull.files, &mut problems)
            }
            Ok(meta) if meta.is_dir() || meta.is_file() => merge_files(
                &source,
                &local_dir.join(name),
                &mut pull.files,
                &mut problems,
            ),
            Ok(_) => {}
            Err(_) => {}
        }
        let _ = std::fs::remove_dir_all(&staging);
        pull.files.sort();
        if !problems.is_empty() {
            let text = problems.join("; ");
            if pull.files.is_empty() {
                hard_error = Some(text);
            } else {
                pull.warning = Some(format!(
                    "Some files could not be copied; the rest were kept: {text}"
                ));
            }
        }
        if let Some(error) = hard_error {
            return Err(error);
        }
        if pull.files.is_empty() {
            pull.message = Some(format!(
                "No evidence files found at {remote} in codespace {codespace}."
            ));
        }
        Ok(pull)
    }

    /// Codespace binding of one session (desktop session id), with its cached
    /// state. `None` for non-cloud sessions. May run `gh codespace view` on a
    /// cache miss; never wakes anything.
    pub fn target_for_session(&self, session_id: i64) -> Option<SessionTarget> {
        let codespace = self
            .store()
            .ok()?
            .remote_session(session_id)
            .ok()??
            .project
            .codespace?;
        let state = self.cloud_state(&codespace);
        Some(SessionTarget { codespace, state })
    }
}

/// Folder inside a session's thread folder that holds pulled evidence.
pub const EVIDENCE_DIR: &str = "evidence";

/// What [`CloudSessions::pull_evidence`] fetched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvidencePull {
    pub session_id: i64,
    pub codespace: String,
    /// The folder or file that was copied, as an absolute codespace path.
    pub remote_path: String,
    /// The session's local evidence folder.
    pub local_dir: std::path::PathBuf,
    /// Local files from this pull, sorted. Empty when there was nothing to pull.
    pub files: Vec<std::path::PathBuf>,
    /// Why `files` is empty.
    pub message: Option<String>,
    /// Set when part of the copy failed; `files` holds what did arrive.
    pub warning: Option<String>,
}

/// The codespace path to pull: default `<workspace>/.evidence`; a given path is absolute or
/// relative to the workspace and must stay strictly inside it.
fn evidence_remote_path(
    root: &str,
    workspace: &str,
    given: Option<&str>,
) -> Result<String, String> {
    let workspace = workspace.trim_end_matches('/');
    if !workspace.starts_with(&format!("{}/", root.trim_end_matches('/'))) {
        return Err("This session has no codespace workspace to pull from.".into());
    }
    let Some(path) = given else {
        return Ok(format!("{workspace}/.evidence"));
    };
    codespaces::valid_remote_path(path)?;
    let path = path.trim_end_matches('/');
    let full = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("{workspace}/{}", path.trim_start_matches("./"))
    };
    if full.len() > workspace.len() + 1 && full.starts_with(&format!("{workspace}/")) {
        Ok(full)
    } else {
        Err(format!(
            "Evidence path must be a file or folder inside {workspace}"
        ))
    }
}

/// Moves the regular files under `source` into `dest` (the final path of `source` itself),
/// replacing same-named files and skipping symlinks. Pushes each final path.
fn merge_files(
    source: &std::path::Path,
    dest: &std::path::Path,
    out: &mut Vec<std::path::PathBuf>,
    problems: &mut Vec<String>,
) {
    let meta = match std::fs::symlink_metadata(source) {
        Ok(meta) => meta,
        Err(e) => return problems.push(format!("{}: {e}", source.display())),
    };
    if meta.is_dir() {
        match std::fs::read_dir(source) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    merge_files(&entry.path(), &dest.join(entry.file_name()), out, problems);
                }
            }
            Err(e) => problems.push(format!("{}: {e}", source.display())),
        }
    } else if meta.is_file() {
        let moved = dest
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| {
                std::fs::rename(source, dest).or_else(|_| std::fs::copy(source, dest).map(|_| ()))
            });
        match moved {
            Ok(()) => out.push(dest.to_path_buf()),
            Err(e) => problems.push(format!("{}: {e}", dest.display())),
        }
    }
}

/// Result of a codespace delete that took effect.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeleteOutcome {
    Deleted,
    /// Deleted and forgotten, but disconnecting the machine failed with this error.
    DeletedWithWarning(String),
}

/// A session's codespace and its (cached) state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionTarget {
    pub codespace: String,
    pub state: CloudState,
}

mod convert;
pub use convert::*;

// These fixtures execute the Linux host scripts against local Unix paths.
#[cfg(all(test, unix))]
mod convert_tests;
#[cfg(test)]
mod tests;
