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
//!   call from a worker thread on an explicit send/retry/stop (the GUI, a Mono or the app
//!   API), never from background polling. A sandbox session is woken through
//!   [`CloudSessions::reconnect_sandbox`] instead.
//!
//! * [`CloudSessions::pull_evidence`] `(session_id, Option<remote_path>) -> EvidencePull`:
//!   copy the files an agent left in the codespace or sandbox (default
//!   `/workspaces/<repo>/.evidence`) into the session's own folder
//!   `<db>.threads/<session>/evidence/` (a custom path lands in `evidence/<its name>`),
//!   merging over earlier pulls, and return the local
//!   file paths. A missing remote folder is `Ok` with no files and a `message`. This is a
//!   user action: copying starts a stopped codespace or sandbox. BLOCKS (gh or ssh).
//!
//! * Viewing a workspace file from the chat ([`view`], [`view_cache`]):
//!   [`CloudSessions::resolve_cloud_file`] / [`CloudSessions::open_cloud_file`] read a
//!   path the agent mentioned through the Host's `files.readBytes` over the live tunnel (never
//!   waking the workspace) and return it in memory when it is previewable and at most 5 MB;
//!   [`CloudSessions::download_cloud_file`] streams anything else into the session's
//!   `cloud-cache/` (verified, cancellable, capped at 2 GB over all sessions) and
//!   [`CloudSessions::clear_cloud_cache`] empties it on archive.
//!
//! * Non-blocking start (the GUI path; full description in [`startup`]):
//!   [`CloudSessions::begin_start`] `(local_cwd, Target, harness) -> (Session, StartRecord)`
//!   requests the codespace through the REST API and creates the session at once, on a
//!   provisional project; [`CloudSessions::run_start`] `(session_id, events, cancel) ->
//!   CloudBinding` BLOCKS on a worker thread, reports [`StartEvent`]s (step list, creation
//!   log, `Ready` or `Failed`) and is idempotent (first run, Retry, resume after a restart);
//!   [`CloudSessions::rebuild_start`] and [`CloudSessions::start_web_url`] serve the
//!   side-panel actions. Readers that need only a `&Store`: [`start_record`],
//!   [`pending_starts`], [`starting_refusal`], [`queue_start_message`],
//!   [`queued_start_messages`] and [`queued_start_message`]. [`SessionTarget::startup`] carries the phase to the app API.
//!   `prepare_target` keeps its blocking behaviour.
//!
//! * Shared instances: [`CloudSessions::sessions_on`] lists the live sessions of one
//!   codespace or sandbox (any project) and [`CloudSessions::attach_session`] starts another
//!   session on an instance that already has a ready one, without a start record.
//!
//! * Docker sandbox starts ([`docker_start`]): after [`CloudSessions::with_sandboxes`],
//!   [`CloudSessions::begin_sandbox_start`] `(local_cwd, &DockerProjectConfig, branch,
//!   harness)` creates the pending session and the sandbox (the start record is persisted
//!   before `sbx create`), and the same `run_start` / `rebuild_start` drive it through the
//!   same steps and events.
//!
//! Errors are user-facing `String`s. `CODESPACE_NOT_CONNECTED` from
//! `Connections::request` means "stopped / not connected": render it as a state
//! (see [`is_not_connected`]).
use crate::codespaces::{self, GhRunner};
use crate::remote::{self, Connections, Machine, SandboxTarget};
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

/// Drops the `\\?\` prefix of a Windows drive path (`\\?\D:\x` becomes `D:\x`).
/// `fs::canonicalize` returns that verbatim form there, and Git reads it as a remote host
/// when it is used as a URL. Every other path is returned unchanged.
pub(crate) fn plain_path(path: std::path::PathBuf) -> std::path::PathBuf {
    let plain = {
        let text = path.as_os_str().to_str().unwrap_or_default();
        text.strip_prefix(r"\\?\")
            .filter(|rest| {
                matches!(rest.as_bytes(), [drive, b':', b'\\', ..] if drive.is_ascii_alphabetic())
            })
            .map(std::path::PathBuf::from)
    };
    plain.unwrap_or(path)
}

/// True when `error` is `Connections::request`'s "codespace is not connected"
/// or "sandbox is not connected".
pub fn is_not_connected(error: &str) -> bool {
    error.starts_with("Codespace is not connected") || error.starts_with("Sandbox is not connected")
}

/// Error text for a start that failed after its codespace was created.
pub fn created_note(codespace: &str, error: &str) -> String {
    format!(
        "{error} (codespace {codespace} was created and kept; retry with target {{\"kind\":\"codespace\",\"instance\":\"{codespace}\"}} or remove it with cloud.delete: an operator must ask the user to, a Mono may use it on explicit request)"
    )
}

/// Error text for a start that failed after the instance `cloud` was created, worded for its
/// backend. A sandbox cannot be targeted again until its session is ready, so the advice is
/// to resume the pending session (kept by [`startup::pause_start`]) or remove the sandbox.
pub fn created_note_for(cloud: &CloudRef, error: &str) -> String {
    match cloud.kind {
        CloudBackendKind::Codespaces => created_note(&cloud.name, error),
        CloudBackendKind::DockerSandbox => format!(
            "{error} (sandbox {} was created and kept with its pending session; use Retry on that session in the app to continue, or remove it with cloud.delete: an operator must ask the user to, a Mono may use it on explicit request)",
            cloud.name
        ),
    }
}

const CODESPACE_NOTE_OPEN: &str = " (codespace ";
const SANDBOX_NOTE_OPEN: &str = " (sandbox ";
const NOTE_MARK: &str = " was created and kept";

/// Splits an error built by [`created_note_for`] into the plain error, the backend and the
/// name of the instance the failed start created. `None` for any other text. This is the one
/// parser of that format, so the wording cannot drift from the builders.
pub fn split_created_note(error: &str) -> Option<(&str, CloudBackendKind, &str)> {
    [
        (CODESPACE_NOTE_OPEN, CloudBackendKind::Codespaces),
        (SANDBOX_NOTE_OPEN, CloudBackendKind::DockerSandbox),
    ]
    .into_iter()
    .find_map(|(open, kind)| {
        let at = error.find(open)?;
        let (name, tail) = error[at + open.len()..].split_once(NOTE_MARK)?;
        (!name.is_empty() && !tail.is_empty() && !name.contains(' '))
            .then(|| (error[..at].trim(), kind, name))
    })
}

/// The dev container feature that installs an SSH server, which a codespace needs so the app
/// can connect to it.
pub const SSHD_FEATURE: &str = "ghcr.io/devcontainers/features/sshd:1";
/// GitHub's page on connecting to a codespace over SSH, which explains the requirement.
pub const SSHD_DOCS_URL: &str = "https://docs.github.com/en/codespaces/developing-in-a-codespace/using-github-codespaces-with-github-cli#ssh-into-a-codespace";

/// Whether a start error says the codespace has no SSH server: `gh codespace ssh` fails with
/// "failed to start SSH server: Please check if an SSH server is installed in the container."
/// when the dev container does not install one. The bare "failed to start SSH server" prefix
/// is not enough: an installed sshd that fails for another reason gives it too, and a rebuild
/// with the feature would not help.
pub fn is_missing_ssh_server(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    error.contains("ssh server") && error.contains("is installed in the container")
}

/// What to do about a start error, when the app recognises it. Only the missing SSH server
/// is recognised, the usual cause of a codespace failing at "connect".
pub fn start_error_hint(error: &str) -> Option<String> {
    is_missing_ssh_server(error).then(|| {
        format!(
            "This codespace has no SSH server, which the app needs to connect. Add the \"{SSHD_FEATURE}\": {{}} feature to the repository's devcontainer.json, then use Rebuild container."
        )
    })
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
    /// The workspace to open the project in; a codespace or a sandbox by name.
    cloud: &'a CloudRef,
    /// The transport of a sandbox; `None` for a codespace.
    sandbox: Option<&'a SandboxTarget>,
    display: &'a str,
    repo: &'a str,
    remote_cwd: &'a str,
    /// The sandbox's Claude prelude, so the Host upgrade restarts the Host with the same
    /// credential the connect step gave it. `None` for a codespace.
    auth: Option<remote::host_upgrade::SandboxAuth<'a>>,
    /// The Claude setup to run before the project is handed over, for a start that created
    /// its workspace. `None` for everything else (an attach, a reconnect, the kit off).
    setup: Option<claude_setup::SetupPlan>,
}

/// Step (c) of a start: connect (pair, or wake an already paired machine), open the project,
/// prepare the evidence folder and record the cloud binding. `halt` aborts between the
/// sub-steps with [`START_ABORTED`]; `step` hears `Connect` and `OpenProject` start and
/// `Connect` finish. Returns the binding and the stored project.
fn open_project(
    backend: &dyn CloudBackend,
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
        .find(|m| match request.sandbox {
            Some(sandbox) => m.sandbox.as_ref().is_some_and(|s| s.name == sandbox.name),
            None => m.codespace.as_deref() == Some(request.cloud.name.as_str()),
        }) {
        // A sandbox start has just restored the relay and the Host, so it pairs afresh
        // (waking is a codespace thing: `ensure` refuses sandboxes on purpose).
        Some(machine) if request.sandbox.is_some() => {
            progress("Reconnecting to the sandbox");
            host.disconnect(&machine.id)?;
            host.connect_sandbox(
                request.sandbox.cloned().unwrap(),
                request.display,
                halt,
                progress,
            )?
        }
        Some(machine) => {
            progress(&format!("Connecting to the {}", request.cloud.noun()));
            host.ensure(&machine.id, halt, progress)?;
            machine
        }
        None => match request.sandbox {
            Some(sandbox) => {
                host.connect_sandbox(sandbox.clone(), request.display, halt, progress)?
            }
            None => host.connect(&request.cloud.name, request.display, halt, progress)?,
        },
    };
    // Phase 2 of the Host upgrade, inside this user-initiated connect. Never fatal: a
    // deferral or a failure is a status line and the session goes on with the Host it has.
    host.upgrade_host(&machine.id, request.auth, halt, progress);
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
    if let Err(error) = backend.prepare_evidence(&request.cloud.name, request.remote_cwd) {
        progress(&format!("Could not prepare the evidence folder: {error}"));
    }
    if aborted() {
        return Err(START_ABORTED.into());
    }
    // One way, once, and only for a workspace this start created. It never fails the start:
    // `run` turns every problem into a warning.
    if let Some(plan) = &request.setup {
        step(startup::StepKind::ClaudeSetup, startup::StepStatus::Running);
        let env = claude_setup::Env {
            host,
            store,
            halt,
            now: crate::time::now_ms(),
        };
        let target = claude_setup::Target {
            local_project: request.local_cwd,
            cloud: request.cloud,
            machine_id: &machine.id,
            project_id: project.get("id").and_then(Value::as_str),
        };
        claude_setup::run(&env, &target, plan, progress);
        // A halted run may have stopped before anything was sent: leave the step Running so
        // a retry runs it again, instead of recording a setup that never happened.
        if aborted() {
            return Err(START_ABORTED.into());
        }
        step(startup::StepKind::ClaudeSetup, startup::StepStatus::Done);
    }
    let remembered = store
        .lock()
        .map_err(|_| "Native session store unavailable".to_string())?
        .remember_cloud_ref_project(
            &machine.environment_id,
            &project,
            request.local_cwd,
            request.cloud,
        )?;
    Ok((
        CloudBinding {
            local_project: request.local_cwd.into(),
            codespace: request.cloud.name.clone(),
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
    /// Machine type for a new codespace, used as is. `None` takes the project's saved machine
    /// while the repository allows it on that branch, else the smallest allowed machine
    /// ([`CloudSessions::default_machine`]), `basicLinux32gb` when GitHub does not say.
    pub machine: Option<String>,
    /// Minutes before an idle new codespace stops; `None` fills from the project's cloud
    /// settings, then the default of 30.
    pub idle_timeout_minutes: Option<u32>,
    /// Devcontainer path for a new codespace; `None` fills from the project's cloud settings,
    /// then the repository default.
    pub devcontainer_path: Option<String>,
    /// App Claude account of a per-session override; only read when
    /// [`Self::claude_account_pinned`]. `None` there means "no account" (the Codespaces
    /// secret) for this session.
    pub claude_account: Option<String>,
    /// This start carries an explicit per-session account (see [`claude_pin`]). Without it
    /// the codespace follows the project's current choice.
    pub claude_account_pinned: bool,
    /// Working branch of a new codespace: when it differs from [`Self::branch`] it is created
    /// in the codespace with `git switch -c` before the session opens. `None` or the same
    /// name keeps the codespace on `branch`.
    pub work_branch: Option<String>,
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
    /// Name of the workspace hosting the session: a codespace name or a sandbox name, see
    /// [`Self::cloud`].
    pub codespace: String,
    /// The workspace and the backend that owns it.
    pub cloud: CloudRef,
    pub environment_id: String,
    pub session_id: Option<String>,
    pub running: bool,
    pub needs_input: bool,
}

/// An instance (codespace or sandbox) that sessions of a project run in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectInstance {
    pub cloud: CloudRef,
    /// Live (not archived) sessions of the project on it.
    pub sessions: usize,
    /// One of them is past its start, so another session can be attached.
    pub ready: bool,
    /// The most recent update among those sessions.
    pub updated_at: u64,
}

/// A local session whose location is Cloud, as the sidebar needs it besides the
/// [`CloudSession`] row: the move point (rewind gating) and the durable sync error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocatedSession {
    pub id: i64,
    pub moved_at_row: usize,
    /// Why the session needs attention, which makes the shell offer "Reset to local": the
    /// stored row-append error, an unreadable location, or a missing Host record. A session
    /// that runs fine has none.
    pub sync_error: Option<String>,
    /// The stored location cannot be read (the session fails closed).
    pub unreadable: bool,
    /// The Host side is running a turn (or waits for input): resetting now would leave two
    /// writers.
    pub host_active: bool,
    /// The cloud copy may hold turns that were not copied here (see
    /// `Store::cloud_may_hold_unsynced`): Reset asks with the stronger text.
    pub unsynced: bool,
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
    /// Pair with a codespace (starting it via gh if needed); blocks until done. Waits for a
    /// free SSH setup slot instead of failing; setting `halt` stops the wait or the setup with
    /// [`START_ABORTED`].
    fn connect(
        &self,
        codespace: &str,
        display: &str,
        halt: &AtomicBool,
        progress: &mut dyn FnMut(&str),
    ) -> Result<Machine, String>;
    /// Pair with the Host in a Docker Sandbox's devcontainer; blocks until done. The relay
    /// on `sandbox.relay_port` must already run. A host without sandbox support refuses.
    fn connect_sandbox(
        &self,
        sandbox: SandboxTarget,
        display: &str,
        halt: &AtomicBool,
        progress: &mut dyn FnMut(&str),
    ) -> Result<Machine, String> {
        let _ = (sandbox, display, halt, progress);
        Err("This host cannot connect to a Docker sandbox.".into())
    }
    /// Wake the codespace and reconnect if needed (blocking, no-op when
    /// connected). `progress` fires only when a wake actually starts; `cancel`
    /// aborts the wait with `remote::WAKE_CANCELLED`. Concurrent calls for one
    /// machine share a single reconnect.
    ///
    /// BLOCKS for minutes: call it from a worker thread, never the UI thread. A Mono or the
    /// app API may call it for an explicit send or `cloud.resume`; background polling must
    /// not. A sandbox machine is refused (`SANDBOX_NOT_CONNECTED`) on purpose: sandboxes are
    /// woken by [`CloudSessions::reconnect_sandbox`], which also restores their relay and
    /// credentials.
    fn ensure(
        &self,
        machine_id: &str,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(&str),
    ) -> Result<(), String>;
    fn request(&self, machine_id: &str, method: &str, params: Value) -> Result<Value, String>;
    /// [`Self::request`] with a limit for a long operation (`None` = the default). Never wakes
    /// anything. The default ignores the limit.
    fn request_timeout(
        &self,
        machine_id: &str,
        method: &str,
        params: Value,
        timeout: Option<Duration>,
    ) -> Result<Value, String> {
        let _ = timeout;
        self.request(machine_id, method, params)
    }
    /// Flavor, version and capabilities of the machine's Host. The default asks the Host
    /// (`environment.describe`); [`Connections`] answers from its cache once it has one.
    fn host_info(&self, machine_id: &str) -> Result<crate::remote::host_info::HostInfo, String> {
        let described = self.request(machine_id, "environment.describe", serde_json::json!({}))?;
        Ok(crate::remote::host_info::HostInfo::from_descriptor(
            &described,
        ))
    }
    /// Where the Host update of this machine stands, for a file only a newer Host can send.
    /// The default knows nothing, so a cloud Host is said to update at its next connect.
    fn host_update(&self, machine_id: &str) -> view::HostUpdate {
        let _ = machine_id;
        view::HostUpdate::NextConnect
    }
    /// Phase 2 of the cloud Host upgrade (see [`remote::host_upgrade`]): replaces an older or
    /// foreign Host when nothing runs on it, else marks the machine for the next connect.
    /// Only a user-initiated connect or start flow may call it. Never fails: a warning is a
    /// `progress` line. The default does nothing.
    fn upgrade_host(
        &self,
        machine_id: &str,
        auth: Option<remote::host_upgrade::SandboxAuth<'_>>,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(&str),
    ) -> remote::host_upgrade::Outcome {
        let _ = (machine_id, auth, cancel, progress);
        remote::host_upgrade::Outcome::Current
    }
    fn disconnect(&self, machine_id: &str) -> Result<(), String>;
}

/// A halted wait for the SSH slot ends the start the way every other halted step does.
fn start_error(error: String) -> String {
    if error == remote::WAKE_CANCELLED {
        START_ABORTED.into()
    } else {
        error
    }
}

impl Host for Arc<Connections> {
    fn machines(&self) -> Result<Vec<Machine>, String> {
        self.list()
    }
    fn connect(
        &self,
        codespace: &str,
        display: &str,
        halt: &AtomicBool,
        progress: &mut dyn FnMut(&str),
    ) -> Result<Machine, String> {
        // A start that waited for the slot may find the codespace already paired by the job
        // it waited for. Pairing again would replace its device token and tunnel under
        // requests in flight, so that machine is woken (a no-op when connected) instead.
        let mut paired: Option<Machine> = None;
        let job = self
            .start_when_free(halt, remote::JOB_SLOT_WAIT, || {
                paired = self
                    .list()?
                    .into_iter()
                    .find(|m| m.codespace.as_deref() == Some(codespace));
                if paired.is_some() {
                    return Ok(String::new());
                }
                self.codespace_begin_for_start(codespace, Some(display))
            })
            .map_err(start_error)?;
        if let Some(machine) = paired {
            self.ensure_codespace_with(&machine.id, halt, progress)
                .map_err(start_error)?;
            return Ok(machine);
        }
        self.await_pairing(&job, "codespace", halt, progress)
    }
    // A sandbox always pairs afresh: `open_project` drops an existing machine first on
    // purpose (a start has just restored the relay and the Host), so a machine found here
    // would be the one that call meant to replace.
    fn connect_sandbox(
        &self,
        sandbox: SandboxTarget,
        display: &str,
        halt: &AtomicBool,
        progress: &mut dyn FnMut(&str),
    ) -> Result<Machine, String> {
        let job = self
            .start_when_free(halt, remote::JOB_SLOT_WAIT, || {
                self.sandbox_begin(sandbox.clone(), Some(display))
            })
            .map_err(start_error)?;
        self.await_pairing(&job, "sandbox", halt, progress)
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
    fn request_timeout(
        &self,
        machine_id: &str,
        method: &str,
        params: Value,
        timeout: Option<Duration>,
    ) -> Result<Value, String> {
        Connections::request_timeout(self, machine_id, method, params, timeout)
    }
    fn host_info(&self, machine_id: &str) -> Result<crate::remote::host_info::HostInfo, String> {
        match Connections::host_info(self, machine_id) {
            Some(info) => Ok(info),
            None => self.describe_host(machine_id),
        }
    }
    fn host_update(&self, machine_id: &str) -> view::HostUpdate {
        if let Some(warning) = self.host_upgrade_failure(machine_id) {
            view::HostUpdate::Failed(warning)
        } else if self.host_upgrade_pending(machine_id) {
            view::HostUpdate::Pending
        } else {
            view::HostUpdate::NextConnect
        }
    }
    fn upgrade_host(
        &self,
        machine_id: &str,
        auth: Option<remote::host_upgrade::SandboxAuth<'_>>,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(&str),
    ) -> remote::host_upgrade::Outcome {
        self.upgrade_host_if_needed(machine_id, auth, cancel, progress)
    }
    fn disconnect(&self, machine_id: &str) -> Result<(), String> {
        Connections::disconnect(self, machine_id)
    }
}

impl Connections {
    /// Follows the pairing job `job` to its end and returns the machine it paired. Setting
    /// `halt` cancels the job, waits for it to stop (bounded) and fails with [`START_ABORTED`].
    fn await_pairing(
        self: &Arc<Self>,
        job: &str,
        noun: &str,
        halt: &AtomicBool,
        progress: &mut dyn FnMut(&str),
    ) -> Result<Machine, String> {
        let mut last = String::new();
        loop {
            if halt.load(std::sync::atomic::Ordering::Relaxed) {
                let _ = self.ssh_cancel(job);
                for _ in 0..20 {
                    if self.ssh_poll(job).map_or(true, |view| view.done) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                return Err(START_ABORTED.into());
            }
            let view = self.ssh_poll(job)?;
            if view.message != last {
                progress(&view.message);
                last = view.message.clone();
            }
            if view.prompt.is_some() {
                let _ = self.ssh_cancel(job);
                return Err(format!(
                    "The {noun} connection asked an unexpected question"
                ));
            }
            if view.done {
                return match (view.error, view.machine) {
                    (Some(error), _) => Err(error),
                    (None, Some(machine)) => Ok(machine),
                    (None, None) => Err(format!(
                        "{} connection finished without a machine",
                        if noun == "sandbox" {
                            "Sandbox"
                        } else {
                            "Codespace"
                        }
                    )),
                };
            }
            std::thread::sleep(Duration::from_millis(100));
        }
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
    if session.project.cloud().is_none() {
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
    /// Codespaces-specific work (resolving, creating, waiting) still talks to gh directly.
    gh: Arc<dyn GhRunner>,
    /// Lifecycle operations by backend; [`Self::backend`] looks one up.
    backends: HashMap<CloudBackendKind, Arc<dyn CloudBackend>>,
    /// Docker sandbox seams, set by [`Self::with_sandboxes`].
    sandbox: Option<docker_start::SandboxDeps>,
    host: Arc<dyn Host>,
    store: Database,
    states: Mutex<HashMap<String, (Instant, String)>>,
    /// Codespaces gh reported as not found (or that were deleted here).
    gone: Mutex<std::collections::HashSet<String>>,
    /// Machine types each repository allows, asked once per repository and branch.
    machines: codespaces::MachineCache,
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
    /// Sandboxes with a user-initiated reconnect in flight, by name.
    reconnecting: Mutex<std::collections::HashSet<String>>,
    /// Where the Claude setup step reads the user's kit preferences and local setup.
    kit_seams: claude_setup::KitSeams,
    /// Moves (and rollbacks) running in this process, by local session id.
    moves: Mutex<HashMap<i64, Arc<move_session::RunState>>>,
    /// Instances (by cloud key) whose return cleanup runs in this process.
    cleaning: Mutex<std::collections::HashSet<String>>,
    /// Limits and pauses of the move's RestoreCode wait; tests shorten them.
    restore_timing: move_session::RestoreTiming,
    /// Test seam of the move flip: runs after the Host link and before the location.
    #[cfg(test)]
    flip_hook: Mutex<Option<move_session::FlipHook>>,
}

impl CloudSessions {
    pub fn new(gh: Arc<dyn GhRunner>, host: Arc<dyn Host>, store: Database) -> Self {
        let codespaces: Arc<dyn CloudBackend> = Arc::new(CodespacesBackend::new(gh.clone()));
        Self {
            gh,
            backends: HashMap::from([(CloudBackendKind::Codespaces, codespaces)]),
            sandbox: None,
            host,
            store,
            states: Mutex::new(HashMap::new()),
            gone: Mutex::new(Default::default()),
            machines: Default::default(),
            ttl: STATE_TTL,
            origin: Box::new(origin_url),
            claude_dir: convert::default_claude_dir(),
            workspace_root: "/workspaces".into(),
            ready_poll: codespaces::READY_POLL,
            sleep: Box::new(std::thread::sleep),
            clock: Box::new(crate::time::now_ms),
            follower_retry: Duration::from_secs(2),
            reconnecting: Mutex::new(Default::default()),
            kit_seams: claude_setup::KitSeams::system(),
            moves: Mutex::new(HashMap::new()),
            cleaning: Mutex::new(Default::default()),
            restore_timing: Default::default(),
            #[cfg(test)]
            flip_hook: Mutex::new(None),
        }
    }
    /// Replaces where the Claude setup reads the user's preferences and local setup.
    pub fn with_kit_seams(mut self, seams: claude_setup::KitSeams) -> Self {
        self.kit_seams = seams;
        self
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
    #[cfg(test)]
    fn with_restore_timing(mut self, timing: move_session::RestoreTiming) -> Self {
        self.restore_timing = timing;
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
            store.clone(),
        )
        .with_system_sandboxes(store))
    }
    /// Registers the Docker sandbox backend over the real `sbx`, `ssh` and the app's own
    /// secret store. Nothing runs until a sandbox session is used.
    ///
    /// The Docker accounts (`sbx` and ssh get the JWT of a project's account from
    /// [`crate::docker_accounts::shared`]) are NOT installed here: the app installs them once
    /// at startup ([`crate::docker_accounts::install_system`]), so building this service over
    /// a test database never puts the real Docker Hub exchange behind it.
    pub fn with_system_sandboxes(self, store: Database) -> Self {
        let secrets = Arc::new(docker_start::StoreSecrets::new(store));
        self.with_sandboxes(
            Arc::new(crate::sandboxes::SystemSbx),
            Arc::new(backend::docker::SystemShell::default()),
            secrets,
        )
    }
    /// Registers (or replaces) the backend for its kind.
    #[cfg(test)]
    fn with_backend(mut self, backend: Arc<dyn CloudBackend>) -> Self {
        self.backends.insert(backend.kind(), backend);
        self
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
    /// The Docker account sandbox `name` lives in: the one that created it (pinned per
    /// sandbox name), not the project's current choice. `None` is the machine's `sbx login`,
    /// also for a sandbox created before accounts existed.
    ///
    /// Locks the native store on a cache miss: call it WITHOUT holding the store guard. An
    /// answer that could not be read is `None` here; [`Self::try_sandbox_account`] tells
    /// that apart where it matters.
    pub fn sandbox_account(&self, name: &str) -> Option<String> {
        self.try_sandbox_account(name).ok().flatten()
    }

    /// [`Self::sandbox_account`], with `Err` when the account could not be read (it must not
    /// be taken for the machine login, which would report a sandbox as gone).
    pub fn try_sandbox_account(&self, name: &str) -> Result<Option<String>, String> {
        match &self.sandbox {
            Some(deps) => deps.backend.try_account_of(name),
            None => {
                let store = self.store()?;
                crate::sandboxes::account::pinned(&store, name)
            }
        }
    }

    /// How to name the Docker account of sandbox `name` to the user (its Docker ID), or
    /// `None` when the sandbox uses the machine login.
    pub fn sandbox_account_label(&self, name: &str) -> Option<String> {
        let account = self.sandbox_account(name)?;
        let store = self.store().ok()?;
        Some(crate::sandboxes::account::describe(&store, &account))
    }

    /// The codespace part of the project's cloud settings; the defaults when none is saved
    /// or the saved settings cannot be read (a start then behaves as before the settings
    /// existed).
    pub fn codespace_project_config(
        &self,
        local_cwd: &str,
    ) -> project_cloud::CodespaceProjectConfig {
        project_cloud::codespace_config_or_default(&self.store, local_cwd)
    }
    /// The registered backend for `kind`.
    pub(crate) fn backend(&self, kind: CloudBackendKind) -> Result<Arc<dyn CloudBackend>, String> {
        self.backends
            .get(&kind)
            .cloned()
            .ok_or_else(|| format!("No {} backend is available.", kind.noun()))
    }

    /// Browser URL of a workspace (a codespace's VS Code for the web), asked from its backend.
    /// `Err` when the backend has no web page or could not read it. Reads only: it never wakes
    /// the workspace.
    pub fn workspace_web_url(&self, cloud: &CloudRef) -> Result<String, String> {
        self.backend(cloud.kind)?
            .web_url(&cloud.name)
            .unwrap_or_else(|| Err(format!("A {} has no web page.", cloud.noun())))
    }

    /// `owner/repo` of the local project's `origin`.
    pub fn repo_for(&self, local_cwd: &str) -> Result<String, String> {
        codespaces::repo_from_remote(&(self.origin)(local_cwd)?)
            .ok_or_else(|| "The project's origin is not a GitHub repository.".to_string())
    }

    /// The machine types `repo` allows for a new codespace on `branch` (`None`: the default
    /// branch), cached for the life of the app. BLOCKS on the first call (one `gh api`). Only
    /// reads GitHub's catalogue: no codespace is touched or woken.
    pub fn allowed_machines(
        &self,
        repo: &str,
        branch: Option<&str>,
    ) -> Result<Vec<codespaces::MachineType>, String> {
        self.machines.allowed(&*self.gh, repo, branch)
    }

    /// The machine a new codespace of `repo` on `branch` gets when none is named: the smallest
    /// the repository allows (a devcontainer's `hostRequirements` can rule out the 2-core
    /// `basicLinux32gb`). A failed lookup keeps [`codespaces::DEFAULT_MACHINE`]. BLOCKS.
    pub fn default_machine(&self, repo: &str, branch: Option<&str>) -> String {
        self.pick_machine(repo, branch, None)
    }

    /// Like [`Self::default_machine`], but keeps `preferred` (a project's saved machine) while
    /// the repository still allows it on `branch`. BLOCKS.
    pub fn pick_machine(
        &self,
        repo: &str,
        branch: Option<&str>,
        preferred: Option<&str>,
    ) -> String {
        self.machines.pick(&*self.gh, repo, branch, preferred).name
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
        if let CodespaceChoice::Named(name) = &target.codespace {
            self.refuse_while_cleaning(&CloudRef::codespace(name))?;
        }
        // The Claude account of this codespace, validated before one is created for it.
        let claude = self.codespace_start_pin(local_cwd, &target)?;
        claude.validate(&*self.store()?)?;
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
        // Pinned before the project is opened, so its first pairing already uses the account.
        claude.write(&*self.store()?, &resolved.name)?;
        let (binding, _) = open_project(
            &*self.backend(CloudBackendKind::Codespaces)?,
            &*self.host,
            &self.store,
            &OpenRequest {
                local_cwd,
                cloud: &CloudRef::codespace(&resolved.name),
                sandbox: None,
                display: &resolved.display,
                repo: &resolved.repo,
                remote_cwd: &resolved.remote_cwd,
                auth: None,
                setup: None,
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
                // Whatever the caller left unset comes from the project's cloud settings.
                let project = self.codespace_project_config(local_cwd);
                // A blank target value counts as unset, so it falls back to the project's.
                let set = |value: &Option<String>| {
                    value
                        .as_deref()
                        .map(str::trim)
                        .filter(|v| !v.is_empty())
                        .map(str::to_owned)
                };
                // A machine the caller named is used as is (GitHub refuses one the repository
                // does not allow, and says so). Otherwise the project's saved machine is only a
                // preference: it is kept while the repository allows it on this branch, else the
                // smallest allowed machine is taken (a lookup that fails keeps the preference
                // or `basicLinux32gb`, so it never blocks a start).
                let machine = match set(&target.machine) {
                    Some(named) => named,
                    None => self.pick_machine(&repo, Some(branch), project.machine.as_deref()),
                };
                let devcontainer = set(&target.devcontainer_path)
                    .or_else(|| set(&Some(project.devcontainer_path.clone())));
                let options = codespaces::CreateOptions {
                    machine: Some(&machine),
                    idle_timeout_minutes: Some(
                        target
                            .idle_timeout_minutes
                            .unwrap_or(project.idle_timeout_minutes),
                    ),
                    devcontainer_path: devcontainer.as_deref(),
                };
                progress("Creating codespace");
                let started = codespaces::create_started(&*self.gh, &repo, branch, &options, None)?;
                Ok(ResolvedCodespace {
                    name: started.name,
                    display: codespaces::display_name_for(branch),
                    repo,
                    remote_cwd,
                    created: true,
                    web_url: started.web_url,
                    state: started.state,
                    branch: Some(branch.to_string()),
                    machine: Some(machine),
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

    /// Whether `local_cwd` has a cloud session, or a start in progress or failed, bound to
    /// a `kind` instance. Reads only the local store: it never runs a CLI.
    pub fn project_uses(&self, local_cwd: &str, kind: CloudBackendKind) -> bool {
        let session = self
            .cloud_sessions_for(local_cwd)
            .unwrap_or_default()
            .iter()
            .any(|s| s.cloud.kind == kind);
        session
            || self.store().is_ok_and(|store| {
                pending_starts(&store).iter().any(|r| {
                    r.local_project == local_cwd && r.cloud().is_some_and(|c| c.kind == kind)
                })
            })
    }

    /// Sessions of `local_cwd` that live in codespaces or Docker sandboxes, newest first.
    /// Two kinds: sessions born in the cloud (a Host record of their own), and local sessions
    /// whose location is Cloud (design 4.2.1). The Host record that backs a located session
    /// is not listed: the local session is the one row, and carries the activity flags.
    pub fn cloud_sessions_for(&self, local_cwd: &str) -> Result<Vec<CloudSession>, String> {
        let store = self.store()?;
        let mut found: Vec<CloudSession> = store
            .cloud_sessions(local_cwd)?
            .into_iter()
            .filter(|s| !store.host_record_is_hidden(s))
            .filter_map(|s| {
                let cloud = s.project.cloud()?;
                Some(CloudSession {
                    codespace: cloud.name.clone(),
                    cloud,
                    environment_id: s.project.environment_id.clone(),
                    session_id: s.session_id.clone(),
                    running: s.running,
                    needs_input: s.needs_input,
                    summary: s.summary,
                })
            })
            .collect();
        for (summary, location) in store.cloud_located_sessions(local_cwd)? {
            let backing = store.linked_remote_session(summary.id)?.map(|(_, r)| r);
            found.push(CloudSession {
                codespace: location.cloud.name.clone(),
                cloud: location.cloud,
                environment_id: location.environment_id,
                session_id: Some(location.host_session_id),
                running: backing.as_ref().is_some_and(|r| r.running),
                needs_input: backing.as_ref().is_some_and(|r| r.needs_input),
                summary,
            });
        }
        found.sort_by_key(|s| {
            (
                std::cmp::Reverse(s.summary.updated_at),
                std::cmp::Reverse(s.summary.id),
            )
        });
        Ok(found)
    }

    /// The instances of `kind` that the project's sessions run in, most recently used first,
    /// each once. Reads the store only (no gh or sbx call), so the state of an instance is not
    /// known here. Archived sessions do not count: an instance is only offered while a live
    /// session of the project is on it (see [`Self::attach_session`]).
    pub fn project_instances(
        &self,
        local_cwd: &str,
        kind: CloudBackendKind,
    ) -> Result<Vec<ProjectInstance>, String> {
        let mut found: Vec<ProjectInstance> = Vec::new();
        for session in self.cloud_sessions_for(local_cwd)? {
            if session.cloud.kind != kind || session.summary.archived {
                continue;
            }
            let ready = !crate::store::is_pending_cloud_environment(&session.environment_id);
            match found.iter_mut().find(|i| i.cloud == session.cloud) {
                Some(known) => {
                    known.sessions += 1;
                    known.ready |= ready;
                }
                None => found.push(ProjectInstance {
                    cloud: session.cloud,
                    sessions: 1,
                    ready,
                    updated_at: session.summary.updated_at,
                }),
            }
        }
        Ok(found)
    }

    /// The local sessions of `local_cwd` that have a cloud location (readable or not), with
    /// their move point and whatever needs the user's attention.
    pub fn located_sessions_for(&self, local_cwd: &str) -> Result<Vec<LocatedSession>, String> {
        let store = self.store()?;
        let located = store.cloud_located_raw()?;
        if located.is_empty() {
            return Ok(Vec::new());
        }
        let mut found = Vec::new();
        for summary in store.sessions()?.into_iter().filter(|s| s.cwd == local_cwd) {
            let Some((_, location)) = located.iter().find(|(id, _)| *id == summary.id) else {
                continue;
            };
            let backing = store.linked_remote_session(summary.id)?.map(|(_, r)| r);
            let stored = store.cloud_sync_error(summary.id).ok().flatten();
            let derived = match location {
                None => Some(crate::store::UNREADABLE_NOTICE.to_string()),
                Some(_) if store.cloud_host_record(summary.id).is_err() => {
                    Some(crate::store::MISSING_HOST.to_string())
                }
                Some(_) => None,
            };
            let sync_error = stored.or(derived);
            found.push(LocatedSession {
                id: summary.id,
                moved_at_row: location.as_ref().map_or(0, |l| l.moved_at_row),
                unreadable: location.is_none(),
                // Fresh `running` only, and never next to a problem: see `leave_cloud`.
                host_active: backing.as_ref().is_some_and(|r| r.running_now())
                    && sync_error.is_none(),
                unsynced: store.cloud_may_hold_unsynced(summary.id),
                sync_error,
            });
        }
        Ok(found)
    }

    /// Raw gh state of a codespace, cached for the TTL. Never wakes it.
    pub fn codespace_state(&self, name: &str) -> Result<String, String> {
        self.cloud_raw_state(&CloudRef::codespace(name))
    }
    /// Raw backend state of a workspace, cached for the TTL. Never wakes it.
    pub fn cloud_raw_state(&self, cloud: &CloudRef) -> Result<String, String> {
        let key = cloud.key();
        let cached = self
            .states
            .lock()
            .map_err(|_| "State cache poisoned")?
            .get(&key)
            .filter(|(at, _)| at.elapsed() < self.ttl)
            .map(|(_, state)| state.clone());
        if let Some(state) = cached {
            return Ok(state);
        }
        let state = self.backend(cloud.kind)?.state(&cloud.name)?;
        self.states
            .lock()
            .map_err(|_| "State cache poisoned")?
            .insert(key, (Instant::now(), state.clone()));
        Ok(state)
    }
    /// Infallible, displayable variant of [`Self::codespace_state`].
    pub fn cloud_state(&self, name: &str) -> CloudState {
        self.cloud_state_of(&CloudRef::codespace(name))
    }
    /// Infallible, displayable variant of [`Self::cloud_raw_state`].
    pub fn cloud_state_of(&self, cloud: &CloudRef) -> CloudState {
        match (self.backend(cloud.kind), self.cloud_raw_state(cloud)) {
            (Ok(backend), Ok(raw)) => backend.display_state(&raw),
            _ => CloudState::Unknown,
        }
    }
    /// Whether gh says the codespace no longer exists. A codespace that cannot be read for
    /// another reason (offline, not signed in) is not gone.
    pub fn codespace_gone(&self, name: &str) -> bool {
        self.cloud_gone(&CloudRef::codespace(name))
    }
    /// Whether the backend says the workspace no longer exists; see [`Self::codespace_gone`].
    pub fn cloud_gone(&self, cloud: &CloudRef) -> bool {
        if self.gone.lock().is_ok_and(|g| g.contains(&cloud.key())) {
            return true;
        }
        let Ok(backend) = self.backend(cloud.kind) else {
            return false;
        };
        match self.cloud_raw_state(cloud) {
            Ok(state) => backend.is_deleted_state(&state),
            Err(e) => {
                let missing = backend.is_missing_error(&e);
                if missing {
                    self.mark_gone(cloud);
                }
                missing
            }
        }
    }
    fn mark_gone(&self, cloud: &CloudRef) {
        if let Ok(mut gone) = self.gone.lock() {
            gone.insert(cloud.key());
        }
    }

    /// Converted local sessions and the codespace each one came from.
    pub fn converted_origins(&self) -> Vec<(i64, String)> {
        self.store()
            .and_then(|s| s.codespace_origins())
            .unwrap_or_default()
    }

    /// Ids of the sessions bound to `cloud`, read before a delete forgets them so the app can
    /// drop them from its window. The Host record behind a local session moved to `cloud` is
    /// left out: it is never listed as a session of its own, and the local session stays.
    pub fn session_ids_on(&self, cloud: &CloudRef) -> Vec<i64> {
        let Ok(store) = self.store() else {
            return vec![];
        };
        store
            .cloud_session_ids(cloud)
            .unwrap_or_default()
            .into_iter()
            .filter(|&id| {
                store
                    .remote_session(id)
                    .ok()
                    .flatten()
                    .is_none_or(|record| !store.host_record_is_hidden(&record))
            })
            .collect()
    }

    /// What deleting `codespace` would lose, as recorded when its sessions were saved.
    pub fn unsaved_for(&self, codespace: &str) -> Vec<String> {
        self.unsaved_for_cloud(&CloudRef::codespace(codespace))
    }
    /// [`Self::unsaved_for`] for any backend.
    pub fn unsaved_for_cloud(&self, cloud: &CloudRef) -> Vec<String> {
        self.store()
            .ok()
            .and_then(|s| s.setting(&convert::unsaved_key(cloud)).ok().flatten())
            .and_then(|v| serde_json::from_str(&v).ok())
            .unwrap_or_default()
    }
    /// Remembers `lines` for `codespace` (merged with what is already recorded), so every later
    /// delete of it asks again.
    pub fn record_unsaved(&self, codespace: &str, lines: &[String]) -> Result<(), String> {
        self.record_unsaved_cloud(&CloudRef::codespace(codespace), lines)
    }
    /// [`Self::record_unsaved`] for any backend.
    pub fn record_unsaved_cloud(&self, cloud: &CloudRef, lines: &[String]) -> Result<(), String> {
        if lines.is_empty() {
            return Ok(());
        }
        let mut all = self.unsaved_for_cloud(cloud);
        for line in lines {
            if !all.contains(line) {
                all.push(line.clone());
            }
        }
        let value = serde_json::to_string(&all).map_err(|e| e.to_string())?;
        self.store()?
            .save_setting(&convert::unsaved_key(cloud), &value)
    }
    pub fn invalidate(&self, name: &str) {
        self.invalidate_ref(&CloudRef::codespace(name));
    }
    pub fn invalidate_ref(&self, cloud: &CloudRef) {
        if let Ok(mut states) = self.states.lock() {
            states.remove(&cloud.key());
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

    /// Delete one session through the normal store cleanup. A missing Codespace
    /// only loses this session's local record; live hosts must accept the delete.
    pub fn delete_session(&self, id: i64) -> Result<(), String> {
        // A session in a Docker sandbox: ask whether the sandbox still exists BEFORE taking
        // the store, as the account it lives in. That runs `sbx` (and may exchange a Docker
        // Hub token), which must not happen while the store is held.
        let probe = self.store()?.sandbox_name_of_session(id);
        let exists = match probe {
            Some(name) => {
                let account = self.try_sandbox_account(&name)?;
                let runner: Arc<dyn crate::sandboxes::SbxRunner> = match &self.sandbox {
                    Some(deps) => deps.runner.clone(),
                    None => Arc::new(crate::sandboxes::SystemSbx),
                };
                Some(
                    crate::sandboxes::find_by_name(
                        &crate::sandboxes::as_account(&*runner, account.as_deref()),
                        &name,
                    )?
                    .is_some(),
                )
            }
            None => None,
        };
        self.store()?
            .delete_with_remote_known(id, &*self.gh, &*self.host, exists)
    }

    /// Fresh, read-only inventory check before an archive/delete can attempt a
    /// backup. Only confirmed absence permits forgetting bindings without a host.
    pub fn forget_missing_codespace(&self, name: &str) -> Result<bool, String> {
        self.forget_missing(&CloudRef::codespace(name))
    }
    /// [`Self::forget_missing_codespace`] for any backend.
    pub fn forget_missing(&self, cloud: &CloudRef) -> Result<bool, String> {
        if self.backend(cloud.kind)?.exists(&cloud.name)? {
            return Ok(false);
        }
        self.store()?.forget_cloud(cloud)?;
        self.invalidate_ref(cloud);
        self.mark_gone(cloud);
        Ok(true)
    }

    pub fn stop_codespace(&self, name: &str) -> Result<(), String> {
        self.stop_cloud(&CloudRef::codespace(name))
    }
    /// Stops the workspace and pauses the starts that were waiting on it.
    pub fn stop_cloud(&self, cloud: &CloudRef) -> Result<(), String> {
        let backend = self.backend(cloud.kind)?;
        {
            let store = self.store()?;
            for record in startup::pending_starts(&store)
                .into_iter()
                .filter(|r| r.cloud().as_ref() == Some(cloud))
            {
                startup::pause_start(
                    &store,
                    record.session_id,
                    &format!("{} stopped. Choose Retry to start it again.", cloud.title()),
                )?;
            }
        }
        backend.stop(&cloud.name)?;
        self.invalidate_ref(cloud);
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
        self.delete_cloud_outcome(&CloudRef::codespace(name))
    }
    /// [`Self::delete_codespace_outcome`] for any backend.
    pub fn delete_cloud_outcome(&self, cloud: &CloudRef) -> Result<DeleteOutcome, String> {
        let backend = self.backend(cloud.kind)?;
        if let Err(error) = backend.delete(&cloud.name) {
            if !backend.is_missing_error(&error) {
                return Err(error);
            }
        }
        self.invalidate_ref(cloud);
        let mut disconnect_error = None;
        match self.host.machines() {
            Ok(machines) => {
                for machine in machines {
                    let ours = match cloud.kind {
                        CloudBackendKind::Codespaces => {
                            machine.codespace.as_deref() == Some(&cloud.name)
                        }
                        CloudBackendKind::DockerSandbox => machine
                            .sandbox
                            .as_ref()
                            .is_some_and(|sandbox| sandbox.name == cloud.name),
                    };
                    if ours {
                        if let Err(e) = self.host.disconnect(&machine.id) {
                            disconnect_error.get_or_insert(e);
                        }
                    }
                }
            }
            Err(e) => disconnect_error = Some(e),
        }
        self.mark_gone(cloud);
        // The workspace is gone, so what it held is no longer a question.
        let _ = self
            .store()?
            .save_setting(&convert::unsaved_key(cloud), "[]");
        self.store()?.forget_cloud(cloud)?;
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
        let (cloud, workspace, thread) = {
            let store = self.store()?;
            let session = store
                .remote_session(session_id)?
                .ok_or("Session not found")?;
            let cloud = session
                .project
                .cloud()
                .ok_or("This session does not run in a cloud workspace.")?;
            let thread = store
                .thread_dir(session_id)?
                .ok_or("This store keeps no session folders, so evidence cannot be saved.")?;
            (cloud, session.project.cwd.clone(), thread)
        };
        let backend = self.backend(cloud.kind)?;
        self.prime_backend(&cloud);
        let instance = cloud.name.clone();
        let root = self.workspace_root_of(&cloud);
        let remote = evidence_remote_path(&root, &workspace, remote_path)?;
        let local_dir = thread.join(EVIDENCE_DIR);
        let mut pull = EvidencePull {
            session_id,
            kind: cloud.kind,
            instance: instance.clone(),
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
        let copied = backend.copy_from(&cloud.name, &remote, &staging);
        let notes = backend.take_copy_notes();
        // The copy may have started the workspace.
        self.invalidate_ref(&cloud);
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
        if !notes.is_empty() {
            let shown: Vec<&str> = notes.iter().take(3).map(String::as_str).collect();
            let extra = notes.len().saturating_sub(3);
            let mut text = format!("Not copied: {}", shown.join("; "));
            if extra > 0 {
                text.push_str(&format!("; and {extra} more"));
            }
            pull.warning = Some(match pull.warning.take() {
                Some(old) => format!("{old} {text}"),
                None => text,
            });
        }
        if pull.files.is_empty() {
            pull.message = Some(format!(
                "No evidence files found at {remote} in {} {instance}.",
                cloud.noun()
            ));
        }
        Ok(pull)
    }

    /// The live (non-archived) sessions bound to `cloud`, whatever project they belong to,
    /// newest first. A session whose start is still in progress is included (`pending`) and
    /// counts as busy until the start fails. Reads the store only.
    pub fn sessions_on(&self, cloud: &CloudRef) -> Vec<InstanceSession> {
        let Ok(store) = self.store() else {
            return vec![];
        };
        let mut found: Vec<InstanceSession> = store
            .cloud_session_ids(cloud)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|id| store.remote_session(id).ok().flatten())
            .filter(|s| !s.summary.archived)
            .map(|s| {
                let pending = crate::store::is_pending_cloud_environment(&s.project.environment_id);
                let starting =
                    pending && start_record(&store, s.summary.id).is_some_and(|r| !r.is_failed());
                InstanceSession {
                    id: s.summary.id,
                    title: s.summary.title.clone(),
                    local_project: s.project.local_project.clone(),
                    running: s.running,
                    needs_input: s.needs_input,
                    pending,
                    starting,
                    updated_at: s.summary.updated_at,
                }
            })
            .collect();
        found.sort_by_key(|s| (std::cmp::Reverse(s.updated_at), std::cmp::Reverse(s.id)));
        found
    }

    /// Starts a ready session on the existing workspace `cloud` of the local project: it
    /// reuses the remote project of an existing, connected session of that project on the same
    /// instance. No start record and no provisioning; the first send wakes a stopped
    /// instance. `Err` when the project has no ready session on `cloud`, because the remote
    /// project (and the pairing behind it) is only known through one. Claude only.
    pub fn attach_session(
        &self,
        local_cwd: &str,
        cloud: &CloudRef,
        harness: &str,
    ) -> Result<crate::store::Session, String> {
        require_claude(harness)?;
        self.refuse_while_cleaning(cloud)?;
        let store = self.store()?;
        let on_instance: Vec<RemoteSession> = store
            .cloud_sessions(local_cwd)?
            .into_iter()
            .filter(|s| s.project.cloud().as_ref() == Some(cloud) && !s.summary.archived)
            .collect();
        if on_instance.is_empty() {
            return Err(format!(
                "No session of this project runs in {} {}. Start a new {} with sessions.start target {{\"kind\":\"{}\",\"instance\":\"new\"}}.",
                cloud.noun(),
                cloud.name,
                cloud.noun(),
                cloud.kind.api_name()
            ));
        }
        let ready = on_instance
            .iter()
            .find(|s| !crate::store::is_pending_cloud_environment(&s.project.environment_id))
            .ok_or_else(|| {
                format!(
                    "The only sessions on {} {} are still starting. Try again when one is ready.",
                    cloud.noun(),
                    cloud.name
                )
            })?;
        self.refuse_while_cleaning(cloud)?;
        store.create(harness, &ready.project.key)
    }

    /// Codespace binding of one session (desktop session id), with its cached
    /// state. `None` for non-cloud sessions. May run `gh codespace view` on a
    /// cache miss; never wakes anything.
    pub fn target_for_session(&self, session_id: i64) -> Option<SessionTarget> {
        let cloud = self
            .store()
            .ok()?
            .remote_session(session_id)
            .ok()??
            .project
            .cloud()?;
        let startup = self
            .store()
            .ok()
            .and_then(|store| start_record(&store, session_id))
            .map(|record| StartupSummary::of(&record));
        Some(self.session_target(cloud, startup))
    }
}

/// The command of an interactive shell in a sandbox's container (or, without a devcontainer,
/// in its workspace): `ssh -t <destination> <remote>`. See [`CloudSessions::sandbox_terminal`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SandboxTerminal {
    /// `<id>@sbx_cloud`.
    pub destination: String,
    /// The single remote word: `devcontainer exec ... -- bash -l`, or `cd <workspace> && bash
    /// -l` for a sandbox without a devcontainer.
    pub remote: String,
    /// The Docker account the sandbox lives in; `None` is the machine's `sbx login`. The
    /// ssh needs its token in the environment: [`Self::env`].
    pub docker_account: Option<String>,
}

impl SandboxTerminal {
    /// The environment pairs the terminal's ssh must run with, resolved now (the JWT is
    /// short-lived, so call this when the terminal is spawned, not when the menu opens).
    /// Empty without an account. The token is in the environment only, never argv, and stays
    /// in a [`Secret`](crate::git_credentials::Secret) (wiped on drop, never printed) until
    /// the process environment is built.
    ///
    /// BLOCKS on Docker Hub when the account's JWT must be exchanged: call it on a worker,
    /// never on the UI thread.
    pub fn env(&self) -> Result<Vec<(String, crate::git_credentials::Secret)>, String> {
        let pair = crate::docker_accounts::env_for(self.docker_account.as_deref())
            .map_err(|e| e.to_string())?;
        Ok(pair
            .into_iter()
            .map(|(name, token)| (name.to_owned(), token))
            .collect())
    }

    /// The terminal's ssh ended with `code`: when that says the connection failed for the
    /// sandbox's account (ssh's 255), its cached JWT is dropped so the next terminal gets a
    /// fresh one. Returns whether it was dropped.
    pub fn note_exit(&self, code: Option<i32>) -> bool {
        crate::docker_accounts::invalidate_after_ssh_exit(self.docker_account.as_deref(), code)
    }
}

/// The Host worktree a session runs in, as the store recorded it, unless that worktree was
/// removed (then the session is back in the project folder). May still be a `remote://`
/// path: see [`confine_host_cwd`].
pub(crate) fn stored_worktree_cwd(settings: &crate::agent::Settings) -> Option<String> {
    if settings.worktree_removed {
        return None;
    }
    settings.worktree_cwd.clone().filter(|cwd| !cwd.is_empty())
}

/// A session's Host cwd: `cwd` (a Host path, or the `remote://<env>/<path>` form the store
/// keeps) when it lies strictly inside the workspace folder `workspace_root` and is a valid
/// remote path. Anything else is `None`, and the caller uses the project cwd.
pub(crate) fn confine_host_cwd(cwd: &str, workspace_root: &str) -> Option<String> {
    let cwd = match crate::remote::parse_remote_path(cwd.trim()) {
        Some(remote) => remote.host_path,
        None => cwd.to_owned(),
    };
    let cwd = cwd.trim_end_matches('/').to_owned();
    (cwd.starts_with(&format!("{}/", workspace_root.trim_end_matches('/')))
        && crate::codespaces::valid_remote_path(&cwd).is_ok())
    .then_some(cwd)
}

impl CloudSessions {
    /// The folder that holds the workspaces of `cloud`: `/workspaces`, or for a sandbox
    /// without a devcontainer the parent of its checkout in the outer sandbox. Reads the
    /// machine list only.
    pub(crate) fn workspace_root_of(&self, cloud: &CloudRef) -> String {
        if cloud.kind == CloudBackendKind::DockerSandbox {
            let plain = self
                .host
                .machines()
                .ok()
                .into_iter()
                .flatten()
                .filter_map(|m| m.sandbox)
                .find(|s| s.name == cloud.name && s.mode == crate::remote::SandboxMode::Plain);
            if let Some(target) = plain {
                if let Some((parent, _)) = target.workspace.trim_end_matches('/').rsplit_once('/') {
                    if !parent.is_empty() {
                        return parent.to_string();
                    }
                }
            }
        }
        self.workspace_root.clone()
    }

    /// How to open a terminal in the container of sandbox session `session_id`. Reads the
    /// store and the machine list only; the ssh that the caller then runs resumes a stopped
    /// sandbox, so call it for a user action (the session menu) and never from the background.
    pub fn sandbox_terminal(&self, session_id: i64) -> Result<SandboxTerminal, String> {
        let session = self
            .store()?
            .remote_session(session_id)?
            .ok_or("Session not found")?;
        let cloud = session
            .project
            .cloud()
            .filter(|c| c.kind == CloudBackendKind::DockerSandbox)
            .ok_or("This session does not run in a sandbox.")?;
        if crate::store::is_pending_cloud_environment(&session.project.environment_id) {
            return Err(startup::STILL_STARTING.into());
        }
        let target = self
            .host
            .machines()?
            .into_iter()
            .find(|m| {
                m.environment_id == session.project.environment_id
                    && m.sandbox.as_ref().is_some_and(|s| s.name == cloud.name)
            })
            .and_then(|m| m.sandbox)
            .ok_or("The sandbox connection was removed. Start a new session.")?;
        crate::remote::validate_sandbox_target(&target)?;
        Ok(SandboxTerminal {
            destination: crate::sandboxes::ssh_destination(&target.id),
            remote: target.exec_in_container(&["bash", "-l"]),
            // The pin follows its Docker ID to a re-added account; the machine record keeps
            // the id it was paired with.
            docker_account: self
                .try_sandbox_account(&cloud.name)?
                .or_else(|| target.docker_account.clone()),
        })
    }
}

impl CloudSessions {
    /// A session's target. While its codespace is starting the state is `Starting` without
    /// asking gh; a failed start shows the real gh state.
    pub(crate) fn session_target(
        &self,
        cloud: CloudRef,
        startup: Option<StartupSummary>,
    ) -> SessionTarget {
        let state = match &startup {
            Some(summary) if summary.error.is_none() => CloudState::Starting,
            _ => self.cloud_state_of(&cloud),
        };
        SessionTarget {
            codespace: cloud.name.clone(),
            cloud,
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
    /// The backend that hosts the session.
    pub kind: CloudBackendKind,
    /// Name of the codespace or sandbox the files came from.
    pub instance: String,
    /// The folder or file that was copied, as an absolute path in the workspace.
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
        return Err("This session has no cloud workspace to pull from.".into());
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

/// One live session on a cloud instance, see [`CloudSessions::sessions_on`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstanceSession {
    pub id: i64,
    pub title: String,
    /// The local project the session belongs to.
    pub local_project: Option<String>,
    pub running: bool,
    pub needs_input: bool,
    /// Still on the provisional project of a start.
    pub pending: bool,
    /// A start in progress (pending and not failed).
    pub starting: bool,
    pub updated_at: u64,
}
impl InstanceSession {
    /// Doing something that stopping or deleting the instance would interrupt: a turn is
    /// running, it waits for input, or its start is in progress.
    pub fn busy(&self) -> bool {
        self.running || self.needs_input || self.starting
    }
}

/// A session's codespace and its (cached) state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionTarget {
    /// Name of the workspace (a codespace or a sandbox), see [`Self::cloud`].
    pub codespace: String,
    /// The workspace and the backend that owns it.
    pub cloud: CloudRef,
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

pub mod auto_stop;
pub mod backend;
pub mod claude_pin;
pub mod claude_setup;
pub mod docker_config;
pub mod docker_start;
pub mod kit;
pub mod kit_pack;
pub mod kit_prefs;
pub mod move_out;
#[path = "cloud_sessions/move.rs"]
pub mod move_session;
pub mod move_ui;
pub mod project_cloud;
pub mod readiness;
pub use backend::{CloudBackend, CloudBackendKind, CloudRef, CodespacesBackend};

mod continue_local;
mod convert;
pub use convert::*;
pub mod return_cleanup;
pub use return_cleanup::{CleanupOutcome, HostCheck, ReturnCleanup};
pub mod transcript_sync;

pub mod startup;
pub mod view;
pub mod view_cache;
pub use startup::{
    pause_start, pending_starts, queue_start_message, queued_start_message, queued_start_messages,
    start_record, starting_refusal, StartEvent, StartRecord, Step, StepKind, StepStatus,
};

// These fixtures execute the Linux host scripts against local Unix paths.
#[cfg(test)]
mod auto_stop_tests;
#[cfg(test)]
mod claude_pin_tests;
#[cfg(all(test, unix))]
mod continue_tests;
#[cfg(all(test, unix))]
mod convert_tests;
#[cfg(test)]
mod docker_config_tests;
#[cfg(test)]
mod docker_start_tests;
#[cfg(test)]
mod kit_pack_tests;
#[cfg(test)]
mod kit_prefs_tests;
#[cfg(test)]
mod kit_tests;
#[cfg(test)]
mod move_out_tests;
#[cfg(test)]
mod move_tests;
#[cfg(test)]
mod move_ui_tests;
#[cfg(test)]
mod project_cloud_tests;
#[cfg(all(test, unix))]
mod return_cleanup_tests;
#[cfg(test)]
mod startup_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod view_tests;
