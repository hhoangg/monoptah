//! The backend seam of the cloud app API.
//!
//! The app API (`cloud.*` actions, `sessions.start` with a cloud `target`) is backend
//! neutral: validation, dispatch and JSON shaping live in [`super`] and [`super::validate`]
//! and know nothing about codespaces or sandboxes. Everything backend specific sits behind two
//! traits, so a new backend is one new impl of each plus one line in each registry:
//!
//! * [`CloudKindSpec`]: a stateless descriptor (instance name rules, accepted options,
//!   capabilities). `mono::api::validate` has no access to service instances, so it reads the
//!   static registry [`spec_of`] / [`specs`].
//! * [`CloudProvider`]: the runtime operations (list, status, create, ownership, start, wake)
//!   of one backend, registered in [`super::CloudApi`] by [`CloudBackendKind`].
//!
//! Stop, delete and the per-instance session bookkeeping are generic: they dispatch through
//! `CloudSessions` and its `CloudBackend`.
use crate::cloud_sessions::{
    CloudBackendKind, CloudBinding, CloudRef, CloudState, StartRecord, StepKind,
};
use crate::store::{RemoteSession, Session};
use serde_json::{Map, Value};

/// Stateless description of one backend kind, used before any service exists.
pub trait CloudKindSpec: Sync {
    fn kind(&self) -> CloudBackendKind;
    /// Checks the name (or id) of an existing instance. The error is shown to the caller.
    fn instance_name(&self, name: &str) -> Result<(), String>;
    /// Keys of a `sessions.start` target (besides `kind` and `instance`) that apply when the
    /// instance is `"new"`.
    fn start_options(&self) -> &'static [&'static str];
    /// Checks the value of one of [`Self::start_options`].
    fn check_start_option(&self, key: &str, value: &Value) -> Result<(), String>;
    /// Checks start options that only make sense together (or not at all), once each one is
    /// valid. `target` holds every key of the `sessions.start` target. Default: nothing.
    fn check_start_combination(&self, target: &Map<String, Value>) -> Result<(), String> {
        let _ = target;
        Ok(())
    }
    /// What `cloud.resume` tells the caller when no ready session of the project runs on the
    /// instance `name` (the instance is reconnected through such a session).
    fn resume_without_session_advice(&self, name: &str) -> String;
    /// Whether `cloud.create` can make an instance without a session. When false, `cloud.create`
    /// fails with a capability error naming the backend.
    fn supports_standalone_create(&self) -> bool {
        false
    }
    /// Keys `cloud.create` takes (besides `kind` and `project`); only meaningful when
    /// [`Self::supports_standalone_create`].
    fn create_options(&self) -> &'static [&'static str] {
        &[]
    }
    /// Checks the value of one of [`Self::create_options`].
    fn check_create_option(&self, key: &str, value: &Value) -> Result<(), String> {
        let _ = (key, value);
        Ok(())
    }
}

/// The descriptor of `kind`. The match is exhaustive on purpose: a new
/// [`CloudBackendKind`] does not compile until it has a spec.
pub fn spec_of(kind: CloudBackendKind) -> &'static dyn CloudKindSpec {
    match kind {
        CloudBackendKind::Codespaces => &super::codespace_provider::CodespaceSpec,
        CloudBackendKind::DockerSandbox => &super::sandbox_provider::SandboxSpec,
    }
}

/// Every descriptor, in [`CloudBackendKind::ALL`] order.
pub fn specs() -> Vec<&'static dyn CloudKindSpec> {
    CloudBackendKind::ALL.into_iter().map(spec_of).collect()
}

/// A backend does not support something. `Sandbox does not support X.`
pub fn unsupported(kind: CloudBackendKind, what: &str) -> String {
    format!("{} does not support {what}.", kind.title())
}

/// The capability error of `cloud.create` for a backend whose instances only exist together
/// with their first session.
pub fn standalone_create_unsupported(kind: CloudBackendKind) -> String {
    format!(
        "{} cannot be created on its own: it comes with its first session. Use sessions.start with target {{\"kind\":\"{}\",\"instance\":\"new\"}}.",
        kind.title(),
        kind.api_name()
    )
}

/// One instance as a provider reports it, before sessions are attached.
#[derive(Clone, Debug)]
pub struct CloudInstance {
    pub cloud: CloudRef,
    pub display_name: String,
    pub state: CloudState,
    /// The backend's own state word (gh `Available`, sbx `running`, ...).
    pub raw_state: String,
    pub last_used_at: Option<String>,
    /// Backend specific fields merged into the JSON row (`branch`, `machine`, `cpus`, ...).
    pub details: Map<String, Value>,
}

/// What `sessions.start` made.
// A short-lived value handed straight to the caller; boxing the record would only make every
// consumer unwrap it.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug)]
pub enum CloudStart {
    /// A new instance is being provisioned (or an existing codespace is being connected): the
    /// session is pending and the record drives `run_start`.
    Pending(Session, StartRecord),
    /// A ready session on an instance that already has one. Nothing to provision; the first
    /// send wakes the instance if it is stopped. The state was read from the cache.
    Attached(Session, CloudRef, CloudState),
}

impl CloudStart {
    pub fn session(&self) -> &Session {
        match self {
            Self::Pending(session, _) | Self::Attached(session, _, _) => session,
        }
    }
    pub fn into_session(self) -> Session {
        match self {
            Self::Pending(session, _) | Self::Attached(session, _, _) => session,
        }
    }
    /// The instance the session runs on.
    pub fn cloud_ref(&self) -> CloudRef {
        match self {
            Self::Pending(_, record) => record
                .cloud()
                .unwrap_or_else(|| CloudRef::codespace(record.codespace.clone())),
            Self::Attached(_, cloud, _) => cloud.clone(),
        }
    }
    /// Name of the codespace or sandbox.
    pub fn instance(&self) -> String {
        self.cloud_ref().name
    }
    /// `Starting` while a start is in progress, the cached state otherwise.
    pub fn initial_state(&self) -> CloudState {
        match self {
            Self::Pending(..) => CloudState::Starting,
            Self::Attached(_, _, state) => *state,
        }
    }
    /// The start step in progress, `None` for an attached session.
    pub fn startup_step(&self) -> Option<StepKind> {
        match self {
            Self::Pending(_, record) => crate::cloud_sessions::StartupSummary::of(record).step,
            Self::Attached(..) => None,
        }
    }
    /// The start record of a pending start.
    pub fn record(&self) -> Option<&StartRecord> {
        match self {
            Self::Pending(_, record) => Some(record),
            Self::Attached(..) => None,
        }
    }
}

/// Runtime operations of one backend. Every method may block on `gh`, `sbx` or ssh: call
/// from a worker thread.
pub trait CloudProvider: Send + Sync {
    fn spec(&self) -> &'static dyn CloudKindSpec;
    /// Whether this backend matters for `project`, so an unfiltered `cloud.list` or
    /// `cloud.status` should ask it: the project has a session or start bound to it, or is set
    /// up for it. Uses only local information and never runs the backend's CLI.
    fn relevant(&self, project: &str) -> bool;
    /// How the user makes this backend relevant for a project (shown when none is).
    fn setup_hint(&self) -> &'static str;
    /// The instances that belong to `project`. `Err` when the backend cannot be read (not
    /// signed in, tool missing, project not on GitHub, ...).
    fn list(&self, project: &str) -> Result<Vec<CloudInstance>, String>;
    /// Readiness of the backend for `project` as JSON that always has `ready`. Never contains
    /// secret values.
    fn status(&self, project: &str) -> Result<Value, String>;
    /// `cloud.create`: only called when the spec supports a standalone create.
    fn create(
        &self,
        project: &str,
        input: &Value,
        progress: &mut dyn FnMut(&str),
    ) -> Result<Value, String> {
        let _ = (project, input, progress);
        Err(standalone_create_unsupported(self.spec().kind()))
    }
    /// Checks that `instance` (a name, or for some backends an id) belongs to `project` and
    /// returns its canonical name.
    fn resolve_instance(&self, project: &str, instance: &str) -> Result<String, String>;
    /// `sessions.start`: `instance` is `None` for a new one, else the name of an existing
    /// one. `options` are the target options (only with a new instance). `account` is the app
    /// Claude account id of this session (a per-session override, pinned to the new instance);
    /// `None` follows the project's choice.
    fn start(
        &self,
        project: &str,
        harness: &str,
        instance: Option<&str>,
        options: &Map<String, Value>,
        kit: bool,
        account: Option<&str>,
    ) -> Result<CloudStart, String>;
    /// Blocking variant of a start that returns the ready binding.
    fn prepare(
        &self,
        project: &str,
        instance: Option<&str>,
        options: &Map<String, Value>,
        progress: &mut dyn FnMut(&str),
    ) -> Result<CloudBinding, String> {
        let _ = (project, instance, options, progress);
        Err(unsupported(
            self.spec().kind(),
            "preparing an instance without a session",
        ))
    }
    /// Wakes (reconnects) the instance of `session` for a user-initiated send. BLOCKS.
    fn wake(&self, session: &RemoteSession) -> Result<(), String>;
}

/// The project's current git branch, for a start that needs one.
pub fn current_branch(project: &str, noun: &str) -> Result<String, String> {
    let out = std::process::Command::new("git")
        .args(["-C", project, "rev-parse", "--abbrev-ref", "HEAD"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|e| format!("Cannot run git: {e}"))?;
    let branch = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !out.status.success() || branch.is_empty() || branch == "HEAD" {
        return Err(format!(
            "Provide branch: the project has no current branch to create a {noun} from"
        ));
    }
    Ok(branch)
}

/// Text of a target or option value, if it is text.
pub fn option_text<'a>(options: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    options.get(key).and_then(Value::as_str)
}
