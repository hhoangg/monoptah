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
//! Errors are user-facing `String`s. `CODESPACE_NOT_CONNECTED` from
//! `Connections::request` means "stopped / not connected": render it as a state
//! (see [`is_not_connected`]).
use crate::codespaces::{self, GhRunner};
use crate::remote::{self, Connections, Machine};
use crate::store::{RemoteSession, Store};
use serde_json::{json, Value};
use std::collections::HashMap;
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
    /// Wake the codespace and reconnect if needed (blocking, no-op when connected).
    fn ensure(&self, machine_id: &str) -> Result<(), String>;
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
    fn ensure(&self, machine_id: &str) -> Result<(), String> {
        self.ensure_codespace(machine_id)
    }
    fn request(&self, machine_id: &str, method: &str, params: Value) -> Result<Value, String> {
        Connections::request(self, machine_id, method, params)
    }
    fn disconnect(&self, machine_id: &str) -> Result<(), String> {
        Connections::disconnect(self, machine_id)
    }
}

/// Wake hook for the remote run path; see the module docs.
pub fn ensure_before_send(host: &dyn Host, session: &RemoteSession) -> Result<(), String> {
    if session.project.codespace.is_none() {
        return Ok(());
    }
    require_claude(&session.summary.provider)?;
    let machine = host
        .machines()?
        .into_iter()
        .find(|m| m.environment_id == session.project.environment_id)
        .ok_or("Codespace machine is no longer connected. Start a new cloud session.")?;
    host.ensure(&machine.id)
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
    ttl: Duration,
    origin: OriginFn,
}

impl CloudSessions {
    pub fn new(gh: Arc<dyn GhRunner>, host: Arc<dyn Host>, store: Database) -> Self {
        Self {
            gh,
            host,
            store,
            states: Mutex::new(HashMap::new()),
            ttl: STATE_TTL,
            origin: Box::new(origin_url),
        }
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
    pub fn prepare_target(
        &self,
        local_cwd: &str,
        target: Target,
        progress: &mut dyn FnMut(&str),
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
                self.host.ensure(&machine.id)?;
                machine
            }
            None => self.host.connect(&name, &display, progress)?,
        };
        progress("Opening the project");
        let project =
            self.host
                .request(&machine.id, "projects.open", json!({"cwd": remote_cwd}))?;
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
    pub fn delete_codespace(&self, name: &str) -> Result<(), String> {
        codespaces::delete(&*self.gh, name)?;
        self.invalidate(name);
        for machine in self.host.machines()? {
            if machine.codespace.as_deref() == Some(name) {
                self.host.disconnect(&machine.id)?;
            }
        }
        self.store()?.forget_codespace(name)
    }
}

#[cfg(test)]
mod tests;
