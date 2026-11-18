//! What a web device sees of where a session runs (H-09): the `target`,
//! `cloudState` and `startup` fields of a catalog card and the `cloud` field of
//! a conversation read or watch frame (architecture.md 6.6).
//!
//! Pure builders over plain data, so every shape is unit tested without a
//! shell. The shell fills [`Facts`] from cached state only (`STATE_TTL`), so
//! building a view never calls out and never wakes an instance. What never
//! goes in: tokens, environment, `web_url`, SSH targets, `remote_cwd`,
//! `gh_state`, `log_tail`, log lines and step `detail` (they can hold paths,
//! repositories and commands). `startup.error` goes through
//! `cloud_policy::sanitize_startup_error`. Every timestamp is integer
//! milliseconds since the Unix epoch (D2).
use monocode_core::cloud_sessions::{CloudBackendKind, CloudState, StartRecord, Step, StepStatus};
use monocode_core::mobile::cloud_policy::{WEB_ONLY, sanitize_startup_error};
use serde_json::{Map, Value, json};

/// A cloud session's binding and the cached state of its instance.
#[derive(Clone, Debug)]
pub(crate) struct Binding {
    /// The local project the session is listed under (its twin).
    pub local_project: String,
    /// Name of the codespace or sandbox.
    pub instance: String,
    pub kind: CloudBackendKind,
    /// From the shell's cache; `Unknown` until the first refresh.
    pub state: CloudState,
}

/// What the shell knows about one session's placement (see
/// `Shell::mobile_cloud_facts`).
#[derive(Clone, Debug)]
pub(crate) struct Facts {
    pub cwd: String,
    pub binding: Option<Binding>,
    /// The start in progress or failed, for a cloud session.
    pub start: Option<StartRecord>,
    /// A session on a `monocode-host` machine: a `remote://` cwd with no
    /// cloud binding.
    pub remote_machine: bool,
}

/// `target.kind` on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Placement {
    Local,
    Remote,
    Codespace,
    Sandbox,
}

impl Placement {
    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Remote => "remote",
            Self::Codespace => "codespace",
            Self::Sandbox => "sandbox",
        }
    }
}

/// `cloudState` on the wire (`cloud_api::state_id`).
fn state_id(state: CloudState) -> String {
    monocode_core::mono::cloud_api::state_id(state)
}

/// One startup step as a frame carries it: no `detail`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StepView {
    pub id: &'static str,
    pub status: &'static str,
    pub started_at: Option<u64>,
    pub finished_at: Option<u64>,
}

impl StepView {
    fn of(step: &Step) -> Self {
        Self {
            id: step.kind.id(),
            status: match step.status {
                StepStatus::Pending => "pending",
                StepStatus::Running => "running",
                StepStatus::Done => "done",
                StepStatus::Failed => "failed",
                StepStatus::Skipped => "skipped",
            },
            started_at: step.started_at,
            finished_at: step.finished_at,
        }
    }
}

/// The startup of a cloud session: the nine steps, the one in progress (the
/// one that failed when the start failed) and the sanitized error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StartupView {
    pub steps: Vec<StepView>,
    pub current: Option<&'static str>,
    pub error: Option<String>,
}

impl StartupView {
    pub(crate) fn of(record: &StartRecord) -> Self {
        let step = if record.is_failed() {
            record.failed_at
        } else {
            record.current()
        };
        Self {
            steps: record.steps.iter().map(StepView::of).collect(),
            current: step.map(|step| step.id()),
            error: record.error.as_deref().map(sanitize_startup_error),
        }
    }
}

/// What a catalog card or a `cloud` field says about a session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CardView {
    pub placement: Placement,
    /// The codespace or sandbox name, or the environment of a remote machine.
    pub instance: Option<String>,
    pub state: Option<CloudState>,
    pub startup: Option<StartupView>,
    /// Listed only for web devices: a phone never saw this session.
    pub web_only: bool,
}

impl CardView {
    pub(crate) fn local() -> Self {
        Self {
            placement: Placement::Local,
            instance: None,
            state: None,
            startup: None,
            web_only: false,
        }
    }

    /// A cloud session. The state is `starting` while its start runs and
    /// until it fails, as `cloud_api::annotate` reports it.
    pub(crate) fn cloud(binding: &Binding, start: Option<&StartRecord>, web_only: bool) -> Self {
        let startup = start.map(StartupView::of);
        let state = match &startup {
            Some(startup) if startup.error.is_none() => CloudState::Starting,
            _ => binding.state,
        };
        Self {
            placement: match binding.kind {
                CloudBackendKind::Codespaces => Placement::Codespace,
                CloudBackendKind::DockerSandbox => Placement::Sandbox,
            },
            instance: Some(binding.instance.clone()),
            state: Some(state),
            startup,
            web_only,
        }
    }

    /// A session on a remote machine; `environment` names the machine. The
    /// desktop does not track a machine's connection for the catalog, so no
    /// state is claimed.
    pub(crate) fn remote(environment: String, web_only: bool) -> Self {
        Self {
            placement: Placement::Remote,
            instance: Some(environment),
            state: None,
            startup: None,
            web_only,
        }
    }

    /// Whether the session runs in a codespace or a sandbox, the only kinds
    /// whose instance can be woken, stopped or started.
    pub(crate) fn is_cloud(&self) -> bool {
        matches!(self.placement, Placement::Codespace | Placement::Sandbox)
    }

    /// Adds `target`, `cloudState`, `startup` (and the [`WEB_ONLY`] marker the
    /// scope removes) to a catalog session card.
    pub(crate) fn annotate(&self, card: &mut Value) {
        let mut target = json!({"kind": self.placement.id()});
        if let Some(instance) = &self.instance {
            target["instance"] = json!(instance);
        }
        if let Some(state) = self.state {
            target["state"] = json!(state_id(state));
            card["cloudState"] = json!(state_id(state));
        }
        card["target"] = target;
        if let Some(startup) = &self.startup
            && let Some(step) = startup.current
        {
            let mut value = json!({"step": step});
            if let Some(error) = &startup.error {
                value["error"] = json!(error);
            }
            card["startup"] = value;
        }
        if self.web_only {
            card[WEB_ONLY] = json!(true);
        }
    }

    /// The `cloud` field of a read or watch frame. None for a local session.
    pub(crate) fn cloud_field(&self, extras: &Extras) -> Option<Value> {
        if self.placement == Placement::Local {
            return None;
        }
        let state = self.state.unwrap_or(CloudState::Unknown);
        let mut field = json!({
            "kind": self.placement.id(),
            "instance": self.instance.clone().unwrap_or_default(),
            "state": state_id(state),
        });
        if let Some(startup) = &self.startup {
            let steps: Vec<Value> = startup
                .steps
                .iter()
                .map(|step| {
                    json!({
                        "id": step.id,
                        "status": step.status,
                        "startedAt": step.started_at,
                        "finishedAt": step.finished_at,
                    })
                })
                .collect();
            let mut value = json!({"steps": steps, "current": startup.current});
            if let Some(error) = &startup.error {
                value["error"] = json!(error);
            }
            field["startup"] = value;
        }
        if extras.kept {
            field["kept"] = json!(true);
        }
        if let Some(auto_stop) = &extras.auto_stop {
            field["autoStop"] = json!({"at": auto_stop.at, "reason": auto_stop.reason});
        }
        if let Some(step) = &extras.moving {
            field["move"] = json!({"step": step.step, "status": step.status});
        }
        Some(field)
    }

    /// `{"cloud": ...}` for a read or watch frame, empty for a local session.
    pub(crate) fn fields(&self, extras: &Extras) -> Map<String, Value> {
        let mut fields = Map::new();
        if let Some(cloud) = self.cloud_field(extras) {
            fields.insert("cloud".into(), cloud);
        }
        fields
    }
}

/// An announced auto-stop of the instance (H-12). `at` is milliseconds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AutoStop {
    pub at: u64,
    pub reason: String,
}

/// A move in progress (H-14): one of the 7 step ids and its status.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MoveStep {
    pub step: &'static str,
    pub status: &'static str,
}

/// The parts of the `cloud` field that later tickets fill: they plug in by
/// setting these from `lifecycle::extras` (`kept`, `autoStop`, H-12) and
/// `moves::extras` (`move`, H-14).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Extras {
    /// A failed start left a billable instance (Stop is allowed on it).
    pub kept: bool,
    pub auto_stop: Option<AutoStop>,
    pub moving: Option<MoveStep>,
}

/// The view of a session for a web device, from the shell's cached facts.
/// `web_only` is whether a phone's catalog would not list it.
pub(crate) fn view_of(facts: &Facts, web_only: bool) -> CardView {
    if let Some(binding) = &facts.binding {
        CardView::cloud(binding, facts.start.as_ref(), web_only)
    } else if facts.remote_machine {
        let environment = monocode_core::remote::parse_remote_path(&facts.cwd)
            .map(|path| path.environment_id)
            .unwrap_or_default();
        CardView::remote(environment, web_only)
    } else {
        CardView::local()
    }
}
