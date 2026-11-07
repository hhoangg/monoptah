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
//! * Non-blocking start (the GUI path; full description in [`startup`]):
//!   [`CloudSessions::begin_start`] `(local_cwd, Target, harness) -> (Session, StartRecord)`
//!   requests the codespace through the REST API and creates the session at once, on a
//!   provisional project; [`CloudSessions::run_start`] `(session_id, events, cancel) ->
//!   CloudBinding` BLOCKS on a worker thread, reports [`StartEvent`]s (step list, creation
//!   log, `Ready` or `Failed`) and is idempotent (first run, Retry, resume after a restart);
//!   [`CloudSessions::rebuild_start`] and [`CloudSessions::start_web_url`] serve the
//!   side-panel actions. Readers that need only a `&Store`: [`start_record`],
//!   [`pending_starts`], [`starting_refusal`], [`queue_start_message`] and
//!   [`queued_start_message`]. [`SessionTarget::startup`] carries the phase to the app API.
//!   `prepare_target` keeps its blocking behaviour (used by `sessions.start`).
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

/// Error text of a start that was cancelled or whose session disappeared. Not a failure: no
/// `Failed` event accompanies it.
pub const START_ABORTED: &str = "The cloud session start was cancelled.";

/// A codespace chosen for a start, before it is necessarily ready.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedCodespace {
    pub name: String,
    pub display: String,
    /// `owner/repo`.
    pub repo: String,
    /// `/workspaces/<repo-name>`.
    pub remote_cwd: String,
    /// This call created the codespace (it was not an existing one).
    pub created: bool,
    pub web_url: Option<String>,
    /// gh state at resolve time (`Provisioning` for a new codespace).
    pub state: String,
    pub branch: Option<String>,
    pub machine: Option<String>,
}

/// How [`CloudSessions::wait_available`] waits.
#[derive(Clone, Copy, Debug)]
pub struct WaitPlan {
    pub timeout: Duration,
    /// Treat `Shutdown` / `ShuttingDown` as "go on and let connecting start it" instead of
    /// an error.
    pub shutdown_is_ok: bool,
}

/// Why [`CloudSessions::wait_available`] stopped waiting without an error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Availability {
    Available,
    /// Shut down (or shutting down): connecting starts it.
    Stopped,
}

/// What [`open_project`] needs to know about the codespace.
struct OpenRequest<'a> {
    local_cwd: &'a str,
    codespace: &'a str,
    display: &'a str,
    repo: &'a str,
    remote_cwd: &'a str,
}

/// Step (c) of a start: connect (pair, or wake an already paired machine), open the project,
/// prepare the evidence folder and record the cloud binding. `halt` aborts between the
/// sub-steps with [`START_ABORTED`]; `step` hears `Connect` and `OpenProject` start and
/// `Connect` finish. Returns the binding and the stored project.
fn open_project(
    gh: &dyn GhRunner,
    host: &dyn Host,
    store: &Database,
    request: &OpenRequest,
    halt: &AtomicBool,
    step: &mut dyn FnMut(startup::StepKind, startup::StepStatus),
    progress: &mut dyn FnMut(&str),
) -> Result<(CloudBinding, crate::store::RemoteProject), String> {
    use startup::{StepKind, StepStatus};
    let aborted = || halt.load(std::sync::atomic::Ordering::Relaxed);
    step(StepKind::Connect, StepStatus::Running);
    let machine = match host
        .machines()?
        .into_iter()
        .find(|m| m.codespace.as_deref() == Some(request.codespace))
    {
        Some(machine) => {
            progress("Connecting to the codespace");
            host.ensure(&machine.id, halt, progress)?;
            machine
        }
        None => host.connect(request.codespace, request.display, progress)?,
    };
    step(StepKind::Connect, StepStatus::Done);
    if aborted() {
        return Err(START_ABORTED.into());
    }
    step(StepKind::OpenProject, StepStatus::Running);
    progress("Opening the project");
    let project = host.request(
        &machine.id,
        "projects.open",
        json!({"cwd": request.remote_cwd}),
    )?;
    if aborted() {
        return Err(START_ABORTED.into());
    }
    // Best effort: a failure here must not block the session, the agent just has no hint.
    progress("Preparing the evidence folder");
    if let Err(error) = codespaces::prepare_evidence(gh, request.codespace, request.remote_cwd) {
        progress(&format!("Could not prepare the evidence folder: {error}"));
    }
    if aborted() {
        return Err(START_ABORTED.into());
    }
    let remembered = store
        .lock()
        .map_err(|_| "Native session store unavailable".to_string())?
        .remember_cloud_project(
            &machine.environment_id,
            &project,
            request.local_cwd,
            request.codespace,
        )?;
    Ok((
        CloudBinding {
            local_project: request.local_cwd.into(),
            codespace: request.codespace.into(),
            repo: request.repo.into(),
            machine_id: machine.id,
            environment_id: remembered.environment_id.clone(),
            project_id: remembered.project_id.clone(),
            project_key: remembered.key.clone(),
            remote_cwd: remembered.cwd.clone(),
        },
        remembered,
    ))
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
///
/// A session whose codespace is still being started (its project is the provisional
/// `pending-` one) is refused here without a store lookup, so the text is generic; callers
/// that hold the store use [`startup::starting_refusal`] for the precise phase or error.
pub fn ensure_before_send(
    host: &dyn Host,
    session: &RemoteSession,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&str),
) -> Result<(), String> {
    if session.project.codespace.is_none() {
        return Ok(());
    }
    if crate::store::is_pending_cloud_environment(&session.project.environment_id) {
        return Err(startup::STILL_STARTING.into());
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
    /// Pause between two `gh codespace view` reads while waiting for Available.
    ready_poll: Duration,
    /// How [`Self::wait_available`] sleeps; tests make it instant.
    sleep: Box<dyn Fn(Duration) + Send + Sync>,
    /// Milliseconds since the epoch; tests make it deterministic.
    clock: Box<dyn Fn() -> u64 + Send + Sync>,
    /// Pause before the creation-log follower is started again after it ended.
    follower_retry: Duration,
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
            ready_poll: codespaces::READY_POLL,
            sleep: Box::new(std::thread::sleep),
            clock: Box::new(crate::time::now_ms),
            follower_retry: Duration::from_secs(2),
        }
    }
    #[cfg(test)]
    fn with_timing(
        mut self,
        poll: Duration,
        sleep: impl Fn(Duration) + Send + Sync + 'static,
        clock: impl Fn() -> u64 + Send + Sync + 'static,
    ) -> Self {
        self.ready_poll = poll;
        self.sleep = Box::new(sleep);
        self.clock = Box::new(clock);
        self.follower_retry = Duration::ZERO;
        self
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
    ///
    /// This is [`Self::resolve_codespace`] + [`Self::wait_available`] (only for a codespace
    /// just created) + [`open_project`]; the GUI drives the same pieces through
    /// [`Self::begin_start`] and [`Self::run_start`] instead, without blocking a session.
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
        let resolved = self.resolve_codespace(local_cwd, target, progress)?;
        if resolved.created {
            *created = Some(resolved.name.clone());
            progress("Waiting for the codespace to become available");
            self.wait_available(
                &resolved.name,
                &WaitPlan {
                    timeout: codespaces::READY_TIMEOUT,
                    shutdown_is_ok: false,
                },
                &mut |_| {},
                &|| false,
            )?;
        }
        self.invalidate(&resolved.name);
        let (binding, _) = open_project(
            &*self.gh,
            &*self.host,
            &self.store,
            &OpenRequest {
                local_cwd,
                codespace: &resolved.name,
                display: &resolved.display,
                repo: &resolved.repo,
                remote_cwd: &resolved.remote_cwd,
            },
            &AtomicBool::new(false),
            &mut |_, _| {},
            progress,
        )?;
        Ok(binding)
    }

    /// Step (a) of a start: check gh auth and the project's repository, then either request a
    /// new codespace through the REST API ([`codespaces::create_started`], which returns its
    /// name within seconds) or validate a named one. Never waits for the codespace.
    ///
    /// `Err` means no codespace was created by this call.
    pub fn resolve_codespace(
        &self,
        local_cwd: &str,
        target: Target,
        progress: &mut dyn FnMut(&str),
    ) -> Result<ResolvedCodespace, String> {
        codespaces::auth_status(&*self.gh)?.require_codespace_scope()?;
        let repo = self.repo_for(local_cwd)?;
        let remote_cwd = codespaces::workspace_path(&repo)
            .ok_or_else(|| format!("Invalid repository {repo}"))?;
        match target.codespace {
            CodespaceChoice::New => {
                let branch = target
                    .branch
                    .as_deref()
                    .map(str::trim)
                    .filter(|b| !b.is_empty())
                    .ok_or("A branch is required to create a codespace")?;
                progress("Creating codespace");
                let started = codespaces::create_started(
                    &*self.gh,
                    &repo,
                    branch,
                    target.machine.as_deref(),
                    None,
                )?;
                let machine = target
                    .machine
                    .as_deref()
                    .map(str::trim)
                    .filter(|m| !m.is_empty())
                    .unwrap_or(codespaces::DEFAULT_MACHINE);
                Ok(ResolvedCodespace {
                    name: started.name,
                    display: codespaces::display_name_for(branch),
                    repo,
                    remote_cwd,
                    created: true,
                    web_url: started.web_url,
                    state: started.state,
                    branch: Some(branch.to_string()),
                    machine: Some(machine.to_string()),
                })
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
                Ok(ResolvedCodespace {
                    display,
                    repo,
                    remote_cwd,
                    created: false,
                    web_url: None,
                    state: found.state.clone(),
                    branch: found.git_status.as_ref().map(|g| g.git_ref.clone()),
                    machine: Some(found.machine_name.clone()).filter(|m| !m.is_empty()),
                    name,
                })
            }
        }
    }

    /// Step (b): poll `gh codespace view` until the codespace is `Available`, sleeping
    /// [`Self::ready_poll`] between reads. `on_state` fires with the first state and on every
    /// change. `abort` is checked before every read; when it is true the wait ends with
    /// [`START_ABORTED`]. A failed read is retried a few times (a 30 minute wait must not die
    /// on one network blip) unless gh says the codespace does not exist.
    ///
    /// `Failed`, `Deleted` and `Unavailable` are errors. `Shutdown` and `ShuttingDown` are an
    /// error when `plan.shutdown_is_ok` is false (nothing starts it while we only read), and
    /// otherwise end the wait with [`Availability::Stopped`] so that connecting (which
    /// starts a stopped codespace through `gh codespace ssh`) can take over.
    pub fn wait_available(
        &self,
        name: &str,
        plan: &WaitPlan,
        on_state: &mut dyn FnMut(&str),
        abort: &dyn Fn() -> bool,
    ) -> Result<Availability, String> {
        let mut waited = Duration::ZERO;
        let mut last = String::new();
        let mut failures = 0;
        loop {
            if abort() {
                return Err(START_ABORTED.into());
            }
            let current = match codespaces::state(&*self.gh, name) {
                Ok(current) => {
                    failures = 0;
                    current
                }
                Err(error)
                    if failures < 24
                        && waited < plan.timeout
                        && !error.to_lowercase().contains("http 404") =>
                {
                    failures += 1;
                    (self.sleep)(self.ready_poll);
                    waited += self.ready_poll;
                    continue;
                }
                Err(error) => return Err(error),
            };
            if current != last {
                on_state(&current);
                last = current.clone();
            }
            match current.as_str() {
                "Available" => return Ok(Availability::Available),
                "Failed" | "Deleted" | "Unavailable" => {
                    return Err(format!("Codespace {name} is {current}"));
                }
                "Shutdown" | "ShuttingDown" if plan.shutdown_is_ok => {
                    return Ok(Availability::Stopped);
                }
                "Shutdown" | "ShuttingDown" => {
                    return Err(format!(
                        "Codespace {name} is {current}; start it first (gh codespace ssh starts it)"
                    ));
                }
                _ => {}
            }
            if waited >= plan.timeout {
                return Err(format!(
                    "Timed out waiting for codespace {name} (last state: {current})"
                ));
            }
            (self.sleep)(self.ready_poll);
            waited += self.ready_poll;
        }
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

    /// A provisional session has no remote transcript to save yet.
    pub fn is_pending_start(&self, id: i64) -> bool {
        self.store()
            .ok()
            .and_then(|store| store.remote_session(id).ok().flatten())
            .is_some_and(|remote| {
                crate::store::is_pending_cloud_environment(&remote.project.environment_id)
            })
    }

    pub fn stop_codespace(&self, name: &str) -> Result<(), String> {
        {
            let store = self.store()?;
            for record in startup::pending_starts(&store)
                .into_iter()
                .filter(|r| r.codespace == name)
            {
                startup::pause_start(
                    &store,
                    record.session_id,
                    "Codespace stopped. Choose Retry to start it again.",
                )?;
            }
        }
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
        if let Err(error) = codespaces::delete(&*self.gh, name) {
            if !error.to_lowercase().contains("http 404") {
                return Err(error);
            }
        }
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
        let startup = self
            .store()
            .ok()
            .and_then(|store| start_record(&store, session_id))
            .map(|record| StartupSummary::of(&record));
        Some(self.session_target(codespace, startup))
    }
}

impl CloudSessions {
    /// A session's target. While its codespace is starting the state is `Starting` without
    /// asking gh; a failed start shows the real gh state.
    pub(crate) fn session_target(
        &self,
        codespace: String,
        startup: Option<StartupSummary>,
    ) -> SessionTarget {
        let state = match &startup {
            Some(summary) if summary.error.is_none() => CloudState::Starting,
            _ => self.cloud_state(&codespace),
        };
        SessionTarget {
            codespace,
            state,
            startup,
        }
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
    /// Set while the session is still starting its codespace (or failed to).
    pub startup: Option<StartupSummary>,
}

/// The startup of a cloud session as the app API reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartupSummary {
    /// The step in progress (the one that failed when `error` is set).
    pub step: Option<startup::StepKind>,
    pub error: Option<String>,
}
impl StartupSummary {
    pub fn of(record: &StartRecord) -> Self {
        Self {
            step: if record.is_failed() {
                record.failed_at
            } else {
                record.current()
            },
            error: record.error.clone(),
        }
    }
}

mod convert;
pub use convert::*;

pub mod startup;
pub use startup::{
    pending_starts, queue_start_message, queued_start_message, start_record, starting_refusal,
    StartEvent, StartRecord, Step, StepKind, StepStatus,
};

// These fixtures execute the Linux host scripts against local Unix paths.
#[cfg(all(test, unix))]
mod convert_tests;
#[cfg(test)]
mod startup_tests;
#[cfg(test)]
mod tests;
