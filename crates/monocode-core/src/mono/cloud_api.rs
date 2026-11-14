//! App API for cloud instances: the backend-neutral `cloud.*` actions and `sessions.start`
//! with a cloud `target`.
//!
//! One service shared by the Operator dispatcher and the Mono dispatcher so both behave the
//! same. [`CloudApi`] is a thin facade over [`CloudSessions`] and a registry of
//! [`CloudProvider`]s (one per [`CloudBackendKind`]): it owns validation-independent dispatch,
//! instance/session bookkeeping and JSON shaping, and knows nothing about codespaces or
//! sandboxes. Every call BLOCKS on gh, sbx or ssh, so callers run it on a worker thread.
//!
//! # Actions
//!
//! * `cloud.list {project?, kind?}` -> `{instances:[...], backends:{<kind>:{ok,error?}}, note?}`;
//!   without `kind` only the backends relevant to the project are asked.
//!   A row is `{kind, instance, displayName, state, rawState, lastUsedAt?, <backend details>,
//!   sessions:[{sessionId,title,busy}]}`. One failing backend does not hide the others; with
//!   `kind` given its error is returned. Sandbox names carry no repository, so a sandbox
//!   belongs to a project when one of the project's cloud sessions (or pending starts) is bound
//!   to it. `sessions` lists every live session on the instance, whatever its project.
//! * `cloud.status {project?, kind?}` -> `{backends:{<kind>:{ready,...}}}`, never secret values.
//! * `cloud.create {kind, project?, ...}`: backends with `supports_standalone_create` only.
//! * `cloud.stop|resume|delete {sessionId | kind+instance, project?}` -> `{kind, instance,
//!   stopped|resumed|deleted:true, sessions, warning?}`. Stop proceeds with busy sessions and
//!   warns, but is refused while the app archives or converts a session on the instance;
//!   delete is refused in that case too, while a session is busy, and when the local copy lacks
//!   files. A session counts as busy from the stored flags (ignored while the instance is
//!   stopped) or from the caller's live view ([`AppBusy::session_busy`]).
//!
//! Waking a stopped instance happens only for an explicit caller: a user-initiated send
//! ([`CloudApi::wake`]), `cloud.resume`, or preparing a session target the caller asked for.
//! Listings only read cached or read-only state.
//!
//! Secret values never pass through this module.
use crate::cloud_sessions::kit_prefs::ApiKit;
use crate::cloud_sessions::workspaces::WorkspaceTarget;
use crate::cloud_sessions::{
    CloudBackendKind, CloudBinding, CloudRef, CloudSessions, CloudState, DeleteOutcome,
    InstanceSession, SessionTarget, StartupSummary,
};
use crate::cloud_sessions::{Host, StartRecord};
use crate::codespaces::{self, GhRunner};
use crate::store::{Session, Store};
use crate::today::{visible_to, Actor, MonoCaller, Task};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

mod codespace_provider;
pub mod provider;
mod sandbox_provider;
pub mod validate;

pub use provider::{
    spec_of, specs, standalone_create_unsupported, unsupported, CloudInstance, CloudKindSpec,
    CloudProvider, CloudStart,
};
pub use validate::parse_session_ref;

type Database = Arc<Mutex<Store>>;

static SHARED: Mutex<Option<Arc<CloudApi>>> = Mutex::new(None);

/// What only the caller knows about busy work, for `cloud.stop` and `cloud.delete`.
///
/// The GUI knows when an archive is converting a workspace and which sessions are mid-turn
/// right now (the stored `running` flag lags behind a send and outlives a stop). A plain
/// `Fn(&CloudRef) -> bool` is an `AppBusy` that only answers [`Self::working_on`].
pub trait AppBusy {
    /// The app is archiving or converting a session on `cloud`. An archive resumes a stopped
    /// instance, so `cloud.stop` is refused meanwhile, and `cloud.delete` too.
    fn working_on(&self, cloud: &CloudRef) -> bool;
    /// Session `id` is working or waiting for input in the live view. Added to the stored
    /// flags; the default knows nothing.
    fn session_busy(&self, id: i64) -> bool {
        let _ = id;
        false
    }
}

impl<F: Fn(&CloudRef) -> bool> AppBusy for F {
    fn working_on(&self, cloud: &CloudRef) -> bool {
        self(cloud)
    }
}

/// The caller's busy state, see [`AppBusy`].
pub type ExtraBusy<'a> = &'a dyn AppBusy;

/// How [`CloudApi::delete_instance`] differs from the plain `cloud.delete`.
#[derive(Clone, Debug, Default)]
pub struct DeleteOptions {
    /// What the user was shown that would be lost and confirmed ([`DeletePreview::lines`]).
    /// `None` is the plain `cloud.delete`: any file the local copy lacks refuses it. `Some`
    /// reads what would be lost again right before the delete (the saved list and, for a
    /// codespace, its git state) and refuses when that holds a line the user did not confirm.
    ///
    /// [`DeletePreview::lines`]: crate::cloud_sessions::workspaces::DeletePreview::lines
    pub confirmed: Option<Vec<String>>,
    /// The caller showed the user that the workspace was not made by this app
    /// ([`DeletePreview::ours`]) and the user said they understood. Only read with
    /// [`Self::confirmed`]: a workspace this app's store does not know is refused without it,
    /// whatever `confirmed` holds, so the acknowledgement is enforced here and not only by the
    /// dialog. It means nothing for a workspace the store knows.
    ///
    /// [`DeletePreview::ours`]: crate::cloud_sessions::workspaces::DeletePreview::ours
    pub acknowledged: bool,
    /// Stop a sandbox and wait until it is stopped first, retrying a refused delete
    /// ([`CloudSessions::delete_settled`]).
    pub settle: bool,
}

/// The extra check of a caller that has no extra busy state.
pub fn never_busy(_: &CloudRef) -> bool {
    false
}

pub struct CloudApi {
    cloud: Arc<CloudSessions>,
    store: Database,
    providers: HashMap<CloudBackendKind, Arc<dyn CloudProvider>>,
}

/// The lifecycle state as it is reported to agents.
pub fn state_id(state: CloudState) -> String {
    state.label().to_lowercase()
}

/// `target` of a session: `{kind:"local"}` or `{kind:"codespace"|"sandbox", instance, state}`.
pub fn target_json(target: Option<&SessionTarget>) -> Value {
    match target {
        None => json!({"kind":"local"}),
        Some(t) => json!({
            "kind":t.cloud.kind.api_name(),
            "instance":t.cloud.name,
            "state":state_id(t.state),
        }),
    }
}

/// Adds `target` and, for a cloud session, `cloudState` to a session object. A session whose
/// instance is still starting (or failed to) also gets `startup {step, error?}`, and its
/// `cloudState` is `starting` until the start fails.
pub fn annotate(session: &mut Value, target: Option<&SessionTarget>) {
    session["target"] = target_json(target);
    if let Some(t) = target {
        session["cloudState"] = json!(state_id(t.state));
        if let Some(startup) = &t.startup {
            let mut value = json!({"step": startup.step.map(|s| s.id())});
            if let Some(error) = &startup.error {
                value["error"] = json!(error);
            }
            session["startup"] = value;
        }
    }
}

/// Which instance a `sessions.start` target names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstanceChoice {
    /// Create one.
    New,
    /// An existing instance, by name (or, for a sandbox, id).
    Named(String),
    /// The instance an existing cloud session of the project runs on.
    OfSession(i64),
}

/// The parsed, already validated `target` of `sessions.start` (see `validate::validate_target`).
#[derive(Clone, Debug, PartialEq)]
pub struct CloudTarget {
    pub kind: CloudBackendKind,
    pub instance: InstanceChoice,
    /// Backend options (`branch`, `machine`, `cpus`, ...), only with [`InstanceChoice::New`].
    pub options: Map<String, Value>,
    /// `target.kit`. The API can only turn the user's kit off, never widen it.
    pub kit: ApiKit,
    /// The app Claude account id this session runs under, resolved by the GUI from the
    /// request's `account` (never parsed from the input here). Pinned to the new instance as
    /// a per-session override; `None` follows the project's default. Only with
    /// [`InstanceChoice::New`].
    pub account: Option<String>,
}

impl CloudTarget {
    pub fn new(kind: CloudBackendKind, instance: InstanceChoice) -> Self {
        Self {
            kind,
            instance,
            options: Map::new(),
            kit: ApiKit::Default,
            account: None,
        }
    }
    /// Text option `key`, e.g. `branch`.
    pub fn option(&self, key: &str) -> Option<&str> {
        self.options.get(key).and_then(Value::as_str)
    }
}

/// The parsed `target` of sessions.start; `Ok(None)` when the input has none.
pub fn target_from_input(input: &Value) -> Result<Option<CloudTarget>, String> {
    let Some(target) = input.get("target") else {
        return Ok(None);
    };
    let map = target.as_object().ok_or("target must be an object")?;
    let kind = map
        .get("kind")
        .and_then(Value::as_str)
        .and_then(CloudBackendKind::from_api_name)
        .ok_or("target.kind is missing or unknown")?;
    let instance = match map.get("instance") {
        None => InstanceChoice::New,
        Some(Value::String(name)) if name == "new" => InstanceChoice::New,
        Some(Value::String(name)) => InstanceChoice::Named(name.clone()),
        Some(Value::Object(session)) => InstanceChoice::OfSession(
            session
                .get("session")
                .and_then(parse_session_ref)
                .ok_or("target.instance.session must be a session ID")?,
        ),
        Some(_) => return Err("target.instance is not valid".into()),
    };
    let kit = map
        .get(validate::KIT_OPTION)
        .map(ApiKit::parse)
        .transpose()?
        .unwrap_or(ApiKit::Default);
    let options = map
        .iter()
        .filter(|(key, _)| !matches!(key.as_str(), "kind" | "instance" | validate::KIT_OPTION))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    Ok(Some(CloudTarget {
        kind,
        instance,
        options,
        kit,
        account: None,
    }))
}

/// True when session `id` is a cloud session bound to the local `project`.
pub fn is_cloud_session_of(store: &Store, id: i64, project: &str) -> bool {
    let key = crate::files::path_key(project);
    // A local session whose location is Cloud is a cloud session of its own project, served by
    // its linked Host record (design 4.2.1).
    if let Ok((_, host)) = store.cloud_host_record(id) {
        return host.project.cloud().is_some()
            && store.sessions().ok().is_some_and(|all| {
                all.iter()
                    .any(|s| s.id == id && crate::files::path_key(&s.cwd) == key)
            });
    }
    store.remote_session(id).ok().flatten().is_some_and(|s| {
        s.project.cloud().is_some()
            && s.project
                .local_project
                .as_deref()
                .is_some_and(|p| crate::files::path_key(p) == key)
    })
}

/// The Host record that serves session `id`: the linked record of a Cloud-located local
/// session, or the session's own record for one born in the cloud.
fn serving_record(store: &Store, id: i64) -> Option<(i64, crate::store::RemoteSession)> {
    store
        .cloud_host_record(id)
        .ok()
        .or_else(|| store.remote_session(id).ok().flatten().map(|s| (id, s)))
}

pub use crate::cloud_sessions::created_note;

/// Who is pulling evidence, which decides the Today tasks it may annotate.
#[derive(Clone, Debug)]
pub enum EvidenceScope {
    /// The Operator of a local project: tasks of that project, or tasks with no project that
    /// no source owns. Notes are written as the user.
    Operator,
    /// A Mono: the tasks it can see, as for `today.note`.
    Mono(MonoCaller),
}

impl EvidenceScope {
    fn sees(&self, task: &Task, project: &str) -> bool {
        match self {
            Self::Mono(caller) => visible_to(task, caller),
            Self::Operator => match task.project.as_deref() {
                Some(own) => crate::files::path_key(own) == crate::files::path_key(project),
                None => task.source_id.is_none(),
            },
        }
    }
    fn actor(&self) -> Actor {
        match self {
            Self::Operator => Actor::User,
            Self::Mono(caller) => Actor::Mono(caller.clone()),
        }
    }
}

/// Character budget of a note's file list; the store caps a note at 5000.
const NOTE_BUDGET: usize = 4500;

fn evidence_note(title: &str, cloud: &CloudRef, urls: &[String], now_ms: u64) -> String {
    let when = chrono::DateTime::from_timestamp_millis(now_ms as i64)
        .map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_default();
    let title: String = title.chars().take(80).collect();
    let mut body = format!(
        "Evidence from cloud session \"{title}\" ({} {}), pulled {when}:",
        cloud.noun(),
        cloud.name
    );
    let mut listed = 0;
    for url in urls {
        // Whole URLs only: a clipped file URL would point nowhere.
        if body.chars().count() + url.chars().count() + 1 > NOTE_BUDGET {
            break;
        }
        body.push('\n');
        body.push_str(url);
        listed += 1;
    }
    if listed < urls.len() {
        body.push_str(&format!("\n... and {} more", urls.len() - listed));
    }
    body
}

/// Whether a user-initiated send to a cloud session should (re)connect its instance.
///
/// A codespace or sandbox can be Available yet have no live tunnel (after an app restart or a
/// dropped tunnel), and background loads never reconnect, so the session's history stays
/// unloaded and the send would wait forever. The send therefore calls [`CloudApi::wake`] for
/// every state except Unavailable (failed or deleted: nothing to wake); waking a connected
/// instance is a no-op. Drafts, listings and other background work never call it. The GUI can
/// use the same rule for its own sends.
pub fn should_wake(state: CloudState) -> bool {
    state != CloudState::Unavailable
}

/// `{sessionId, title, busy}` rows of the sessions on an instance.
pub fn sessions_json(sessions: &[InstanceSession]) -> Vec<Value> {
    sessions
        .iter()
        .map(|s| json!({"sessionId":s.id,"title":s.title,"busy":s.busy()}))
        .collect()
}

fn busy_names(sessions: &[InstanceSession]) -> String {
    sessions
        .iter()
        .filter(|s| s.busy())
        .map(|s| format!("\"{}\" ({})", s.title, s.id))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Where a `cloud.stop|resume|delete` call points: the instance and, when the call named a
/// session, that session.
struct Address {
    cloud: CloudRef,
    session: Option<i64>,
}

impl CloudApi {
    /// A service over fakes or real seams for codespaces. Sandboxes stay unavailable (their
    /// provider reports "Docker sandboxes are not available.") until
    /// [`Self::with_sandboxes`].
    pub fn new(gh: Arc<dyn GhRunner>, host: Arc<dyn Host>, store: Database) -> Self {
        let cloud = CloudSessions::new(gh.clone(), host.clone(), store.clone());
        Self::assemble(cloud, gh, host, store)
    }
    /// [`Self::new`] with the Docker sandbox seams registered (the `sbx` runner, the ssh
    /// seam and the secret source), as `CloudSessions::with_sandboxes`. Used by tests.
    pub fn with_sandboxes(
        gh: Arc<dyn GhRunner>,
        host: Arc<dyn Host>,
        store: Database,
        runner: Arc<dyn crate::sandboxes::SbxRunner>,
        shell: Arc<dyn crate::cloud_sessions::backend::docker::SandboxShell>,
        secrets: Arc<dyn crate::cloud_sessions::docker_start::SandboxSecrets>,
    ) -> Self {
        let cloud = CloudSessions::new(gh.clone(), host.clone(), store.clone())
            .with_sandboxes(runner, shell, secrets);
        Self::assemble(cloud, gh, host, store)
    }
    /// Real `gh`, `sbx` and the app's default connections.
    pub fn system(store: Database) -> Result<Self, String> {
        let gh: Arc<dyn GhRunner> = Arc::new(codespaces::SystemGh);
        let host: Arc<dyn Host> = Arc::new(crate::remote::default_connections()?);
        let cloud = CloudSessions::new(gh.clone(), host.clone(), store.clone())
            .with_system_sandboxes(store.clone());
        Ok(Self::assemble(cloud, gh, host, store))
    }
    /// The provider registry: one entry per [`CloudBackendKind`]. Adding a backend means one
    /// more entry here and one more arm in [`provider::spec_of`].
    fn assemble(
        cloud: CloudSessions,
        gh: Arc<dyn GhRunner>,
        host: Arc<dyn Host>,
        store: Database,
    ) -> Self {
        let cloud = Arc::new(cloud);
        let providers: HashMap<CloudBackendKind, Arc<dyn CloudProvider>> = HashMap::from([
            (
                CloudBackendKind::Codespaces,
                Arc::new(codespace_provider::CodespaceProvider::new(
                    gh,
                    host.clone(),
                    cloud.clone(),
                )) as Arc<dyn CloudProvider>,
            ),
            (
                CloudBackendKind::DockerSandbox,
                Arc::new(sandbox_provider::SandboxProvider::new(
                    cloud.clone(),
                    store.clone(),
                )) as Arc<dyn CloudProvider>,
            ),
        ]);
        Self {
            cloud,
            store,
            providers,
        }
    }
    /// User-initiated or app API: reconnects the sandbox of cloud session `session_id` (see
    /// [`CloudSessions::reconnect_sandbox`]), resuming it when stopped. BLOCKS for minutes, so
    /// call it from a worker thread, never the UI thread. A Mono or the app API may call it
    /// (that is what [`Self::wake`] does for a send), background polling must not.
    pub fn reconnect_sandbox(
        &self,
        session_id: i64,
        progress: &mut dyn FnMut(&str),
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<(), String> {
        self.cloud.reconnect_sandbox(session_id, progress, cancel)
    }
    /// The process-wide service (it owns the state cache). `install` replaces it.
    pub fn shared(store: &Database) -> Result<Arc<Self>, String> {
        let mut shared = SHARED.lock().map_err(|_| "Cloud service unavailable")?;
        if let Some(api) = shared.as_ref() {
            return Ok(api.clone());
        }
        let api = Arc::new(Self::system(store.clone())?);
        *shared = Some(api.clone());
        Ok(api)
    }
    /// Replaces the shared service (tests and embedders with their own seams).
    pub fn install(api: Arc<Self>) {
        if let Ok(mut shared) = SHARED.lock() {
            *shared = Some(api);
        }
    }

    fn provider(&self, kind: CloudBackendKind) -> Result<&Arc<dyn CloudProvider>, String> {
        self.providers
            .get(&kind)
            .ok_or_else(|| format!("{} is not available.", kind.title()))
    }
    /// The optional `kind` of an input, and the kinds it selects.
    fn selected(
        &self,
        input: &Value,
    ) -> Result<(Option<CloudBackendKind>, Vec<CloudBackendKind>), String> {
        let kind = input
            .get("kind")
            .map(|value| {
                value
                    .as_str()
                    .and_then(CloudBackendKind::from_api_name)
                    .ok_or("kind is not a known cloud backend")
            })
            .transpose()?;
        let kinds = CloudBackendKind::ALL
            .into_iter()
            .filter(|k| kind.is_none_or(|wanted| wanted == *k) && self.providers.contains_key(k))
            .collect();
        Ok((kind, kinds))
    }
    /// The kinds an unfiltered `cloud.list` / `cloud.status` asks: those relevant to `project`.
    /// With a `wanted` kind, exactly that kind. `Err` carries the note for "none relevant".
    fn queried(
        &self,
        project: &str,
        wanted: Option<CloudBackendKind>,
        kinds: Vec<CloudBackendKind>,
    ) -> Result<Vec<CloudBackendKind>, String> {
        if wanted.is_some() {
            return Ok(kinds);
        }
        let mut relevant = vec![];
        let mut hints = vec![];
        for kind in kinds {
            let provider = self.provider(kind)?;
            if provider.relevant(project) {
                relevant.push(kind);
            } else {
                hints.push(provider.setup_hint());
            }
        }
        if relevant.is_empty() {
            Err(format!(
                "No cloud backend is set up for this project. To enable one: {}. Pass kind to ask a backend anyway.",
                hints.join("; ")
            ))
        } else {
            Ok(relevant)
        }
    }
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Store>, String> {
        self.store
            .lock()
            .map_err(|_| "Native session store unavailable".to_string())
    }

    /// The cloud service behind this API (the cloud workspaces overview reads through it).
    pub fn cloud(&self) -> &Arc<CloudSessions> {
        &self.cloud
    }

    /// The live sessions on `cloud`, any project. See [`CloudSessions::sessions_on`].
    pub fn sessions_on(&self, cloud: &CloudRef) -> Vec<InstanceSession> {
        self.cloud.sessions_on(cloud)
    }

    /// [`Self::sessions_on`] with `running` / `needs_input` made trustworthy. The stored flags
    /// lag a send and outlive a stop, so: a stopped or unavailable instance runs nothing (the
    /// flags are ignored, which also covers an instance stopped outside the app), and a
    /// session the caller's live view calls busy is busy. BLOCKS when the state is not cached.
    fn busy_sessions_on(
        &self,
        cloud: &CloudRef,
        state: CloudState,
        busy: ExtraBusy,
    ) -> Vec<InstanceSession> {
        let mut sessions = self.cloud.sessions_on(cloud);
        let idle = matches!(state, CloudState::Stopped | CloudState::Unavailable);
        for session in &mut sessions {
            if idle {
                session.running = false;
                session.needs_input = false;
            } else if busy.session_busy(session.id) {
                session.running = true;
            }
        }
        sessions
    }

    fn instance_json(&self, instance: CloudInstance) -> Value {
        let sessions = self.busy_sessions_on(&instance.cloud, instance.state, &never_busy);
        let mut row = json!({
            "kind":instance.cloud.kind.api_name(),
            "instance":instance.cloud.name,
            "displayName":instance.display_name,
            "state":state_id(instance.state),
            "rawState":instance.raw_state,
        });
        if let Some(last) = &instance.last_used_at {
            row["lastUsedAt"] = json!(last);
        }
        for (key, value) in instance.details {
            row[key] = value;
        }
        row["sessions"] = json!(sessions_json(&sessions));
        row
    }

    /// `cloud.list {project?, kind?}`.
    pub fn list(&self, project: &str, input: &Value) -> Result<Value, String> {
        let (wanted, kinds) = self.selected(input)?;
        let kinds = match self.queried(project, wanted, kinds) {
            Ok(kinds) => kinds,
            Err(note) => return Ok(json!({"instances":[],"backends":{},"note":note})),
        };
        let mut instances = vec![];
        let mut backends = Map::new();
        for kind in kinds {
            match self.provider(kind)?.list(project) {
                Ok(rows) => {
                    backends.insert(kind.api_name().into(), json!({"ok":true}));
                    instances.extend(rows.into_iter().map(|row| self.instance_json(row)));
                }
                Err(error) if wanted.is_some() => return Err(error),
                Err(error) => {
                    backends.insert(kind.api_name().into(), json!({"ok":false,"error":error}));
                }
            }
        }
        Ok(json!({"instances":instances,"backends":backends}))
    }

    /// `cloud.status {project?, kind?}`: readiness of each backend, never secret values.
    pub fn status(&self, project: &str, input: &Value) -> Result<Value, String> {
        let (wanted, kinds) = self.selected(input)?;
        let kinds = match self.queried(project, wanted, kinds) {
            Ok(kinds) => kinds,
            Err(note) => return Ok(json!({"backends":{},"note":note})),
        };
        let mut backends = Map::new();
        for kind in kinds {
            match self.provider(kind)?.status(project) {
                Ok(status) => {
                    backends.insert(kind.api_name().into(), status);
                }
                Err(error) if wanted.is_some() => return Err(error),
                Err(error) => {
                    backends.insert(kind.api_name().into(), json!({"ready":false,"error":error}));
                }
            }
        }
        Ok(json!({"backends":backends}))
    }

    /// `cloud.create {kind, project?, ...options}`: backends that can make an instance without
    /// a session (a codespace) create it and wait until it is available; the others return a
    /// capability error pointing at `sessions.start`.
    pub fn create(
        &self,
        project: &str,
        input: &Value,
        progress: &mut dyn FnMut(&str),
    ) -> Result<Value, String> {
        let (kind, _) = self.selected(input)?;
        let kind = kind.ok_or("Provide kind")?;
        let provider = self.provider(kind)?;
        if !provider.spec().supports_standalone_create() {
            return Err(standalone_create_unsupported(kind));
        }
        provider.create(project, input, progress)
    }

    /// Resolves the instance a `stop|resume|delete` input points at, checking it belongs to
    /// `project`.
    fn address(&self, project: &str, input: &Value) -> Result<Address, String> {
        if let Some(id) = input.get("sessionId") {
            let id = parse_session_ref(id).ok_or("sessionId must be an ID")?;
            let cloud = self.instance_of_session(project, id)?;
            return Ok(Address {
                cloud,
                session: Some(id),
            });
        }
        let (kind, _) = self.selected(input)?;
        let kind = kind.ok_or("Provide sessionId, or kind and instance")?;
        let instance = input
            .get("instance")
            .and_then(Value::as_str)
            .ok_or("Provide sessionId, or kind and instance")?;
        let name = self.provider(kind)?.resolve_instance(project, instance)?;
        Ok(Address {
            cloud: CloudRef { kind, name },
            session: None,
        })
    }
    /// The instance cloud session `id` of `project` runs on.
    fn instance_of_session(&self, project: &str, id: i64) -> Result<CloudRef, String> {
        let store = self.lock()?;
        if !is_cloud_session_of(&store, id, project) {
            return Err("Session is not a cloud session of this project.".into());
        }
        serving_record(&store, id)
            .and_then(|(_, s)| s.project.cloud())
            .ok_or_else(|| "Session is not a cloud session of this project.".into())
    }
    fn outcome(&self, cloud: &CloudRef, flag: &str, sessions: &[InstanceSession]) -> Value {
        let mut result = json!({
            "kind":cloud.kind.api_name(),
            "instance":cloud.name,
            "sessions":sessions_json(sessions),
        });
        result[flag] = json!(true);
        result
    }

    /// `cloud.stop {sessionId | kind+instance}`: stops the instance (state is kept; a send to
    /// a session on it wakes it). Proceeds while sessions are busy and says so in `warning`.
    pub fn stop(&self, project: &str, input: &Value, busy: ExtraBusy) -> Result<Value, String> {
        let address = self.address(project, input)?;
        self.stop_target(&address.cloud, busy)
    }

    /// [`Self::stop`] for a workspace of the cloud workspaces overview, as the Docker account its
    /// row was listed under.
    pub fn stop_instance(
        &self,
        target: &WorkspaceTarget,
        busy: ExtraBusy,
    ) -> Result<Value, String> {
        self.cloud.use_listed_account(target)?;
        self.stop_target(&target.cloud, busy)
    }

    fn stop_target(&self, cloud: &CloudRef, busy: ExtraBusy) -> Result<Value, String> {
        if busy.working_on(cloud) {
            return Err(format!(
                "Cannot stop {} {}: the app is archiving or converting a session on it, which would start it again. Try again when that finishes.",
                cloud.noun(),
                cloud.name
            ));
        }
        let sessions = self.busy_sessions_on(cloud, self.cloud.cloud_state_of(cloud), busy);
        self.cloud.stop_cloud(cloud)?;
        let mut result = self.outcome(cloud, "stopped", &sessions);
        let names = busy_names(&sessions);
        if !names.is_empty() {
            result["warning"] = json!(format!(
                "Sessions that were working on this {} were interrupted: {names}.",
                cloud.noun()
            ));
        }
        Ok(result)
    }

    /// `cloud.resume {sessionId | kind+instance}`: reconnects the instance through a session
    /// on it (a codespace is started, a sandbox resumed and reconnected). BLOCKS for minutes.
    pub fn resume(&self, project: &str, input: &Value) -> Result<Value, String> {
        let address = self.address(project, input)?;
        let sessions = self.cloud.sessions_on(&address.cloud);
        let session = match address.session {
            Some(id) => id,
            None => {
                let key = crate::files::path_key(project);
                sessions
                    .iter()
                    .find(|s| {
                        !s.pending
                            && s.local_project
                                .as_deref()
                                .is_some_and(|p| crate::files::path_key(p) == key)
                    })
                    .map(|s| s.id)
                    .ok_or_else(|| {
                        format!(
                            "No ready session of this project runs on {} {}. {}",
                            address.cloud.noun(),
                            address.cloud.name,
                            spec_of(address.cloud.kind)
                                .resume_without_session_advice(&address.cloud.name)
                        )
                    })?
            }
        };
        self.wake(session, &self.store)?;
        self.cloud.invalidate_ref(&address.cloud);
        Ok(self.outcome(&address.cloud, "resumed", &sessions))
    }

    /// `cloud.delete {sessionId | kind+instance}`: irreversible; also forgets the bindings of
    /// its sessions. Refused while a session on it is busy (or `busy` says the app is working
    /// on it) and while the local copy lacks files that only exist on the instance.
    pub fn delete(&self, project: &str, input: &Value, busy: ExtraBusy) -> Result<Value, String> {
        let address = self.address(project, input)?;
        self.delete_target(&address.cloud, busy, &DeleteOptions::default())
    }

    /// [`Self::delete`] for a workspace of the cloud workspaces overview, as the Docker account its
    /// row was listed under, with the caller's [`DeleteOptions`].
    pub fn delete_instance(
        &self,
        target: &WorkspaceTarget,
        busy: ExtraBusy,
        options: DeleteOptions,
    ) -> Result<Value, String> {
        self.cloud.use_listed_account(target)?;
        self.delete_target(&target.cloud, busy, &options)
    }

    /// Why `cloud` must not be deleted right now. The store and the backend are read fresh on
    /// every call: sessions working on it, local sessions moved into it, whether this app knows
    /// the workspace at all (and the caller acknowledged it when it does not), and what would be
    /// lost that the caller did not confirm. `woke` also refuses a sandbox that is running again
    /// after it was stopped (a send to a session on it wakes it). Returns the sessions on it.
    ///
    /// What is NOT fresh is `busy`: the caller's [`AppBusy`], which for the overview is a copy
    /// the window took when the user clicked Delete. During a sandbox's wait (up to minutes) an
    /// archive that started after that click is not seen here; what protects the delete then is
    /// the `woke` state check, which refuses a workspace that is running again.
    fn delete_refusal(
        &self,
        cloud: &CloudRef,
        busy: ExtraBusy,
        options: &DeleteOptions,
        woke: bool,
    ) -> Result<Vec<InstanceSession>, String> {
        let state = if woke {
            // The state cached from before the wait says nothing about now.
            self.cloud.invalidate_ref(cloud);
            let state = self.cloud.cloud_state_of(cloud);
            if matches!(state, CloudState::Available | CloudState::Starting) {
                return Err(format!(
                    "{} started again while Monoptah waited for it to stop (a session or another app woke it), so it was not deleted. Check what is using it, then try again.",
                    cloud.name
                ));
            }
            state
        } else {
            self.cloud.cloud_state_of(cloud)
        };
        let sessions = self.busy_sessions_on(cloud, state, busy);
        let names = busy_names(&sessions);
        if !names.is_empty() {
            return Err(format!(
                "Cannot delete {} {}: sessions are working on it: {names}. Stop them or wait until they finish.",
                cloud.noun(),
                cloud.name
            ));
        }
        if busy.working_on(cloud) {
            return Err(format!(
                "Cannot delete {} {}: the app is archiving or converting a session on it. Try again when that finishes.",
                cloud.noun(),
                cloud.name
            ));
        }
        // Only the overview refuses on a conversation moved into the workspace: the app's own
        // `cloud.delete` keeps such a session and marks it lost (its callers know the cost).
        if options.confirmed.is_some()
            && !options.acknowledged
            && !self.cloud.known_to_this_app(cloud)
        {
            return Err(format!(
                "Not deleted: Monoptah did not create {} {}, and the confirmation did not say the user was told so. Open Delete again and tick the acknowledgement.",
                cloud.noun(),
                cloud.name
            ));
        }
        let blockers = if options.confirmed.is_some() {
            self.cloud.delete_blockers(cloud)
        } else {
            Vec::new()
        };
        if !blockers.is_empty() {
            return Err(format!(
                "Cannot delete {} {}: {}",
                cloud.noun(),
                cloud.name,
                blockers.join(" ")
            ));
        }
        match &options.confirmed {
            None => {
                let unsaved = self.cloud.unsaved_for_cloud(cloud);
                if !unsaved.is_empty() {
                    return Err(format!(
                        "Deleting {} would lose files that are not in the local copy. {} Delete it from the app to confirm.",
                        cloud.name,
                        unsaved.join(" ")
                    ));
                }
            }
            Some(confirmed) => {
                let unseen: Vec<String> = self
                    .cloud
                    .loss_lines(cloud)?
                    .into_iter()
                    .filter(|line| !confirmed.contains(line))
                    .collect();
                if !unseen.is_empty() {
                    return Err(format!(
                        "Not deleted: {} has something you were not shown when you confirmed. {} Open Delete again to review it.",
                        cloud.name,
                        unseen.join(" ")
                    ));
                }
            }
        }
        Ok(sessions)
    }

    fn delete_target(
        &self,
        cloud: &CloudRef,
        busy: ExtraBusy,
        options: &DeleteOptions,
    ) -> Result<Value, String> {
        let sessions = self.delete_refusal(cloud, busy, options, false)?;
        let removed = self.cloud.session_ids_on(cloud);
        let outcome = if options.settle {
            // The wait can take minutes; everything is checked again right before the delete.
            self.cloud.delete_settled(cloud, &|| {
                let woke = cloud.kind == CloudBackendKind::DockerSandbox;
                self.delete_refusal(cloud, busy, options, woke).map(|_| ())
            })?
        } else {
            self.cloud.delete_cloud_outcome(cloud)?
        };
        let mut result = self.outcome(cloud, "deleted", &sessions);
        // The sessions the delete removed, so the app drops them from its window and panels.
        result["removedSessionIds"] = json!(removed);
        if let DeleteOutcome::DeletedWithWarning(error) = outcome {
            result["warning"] = json!(format!(
                "Deleted, but disconnecting its machine failed: {error}"
            ));
        }
        Ok(result)
    }

    /// The instance a start target names, for a provider (`None` = create).
    fn instance_name(&self, project: &str, target: &CloudTarget) -> Result<Option<String>, String> {
        match &target.instance {
            InstanceChoice::New => Ok(None),
            InstanceChoice::Named(name) => Ok(Some(name.clone())),
            InstanceChoice::OfSession(id) => {
                let cloud = self.instance_of_session(project, *id)?;
                if cloud.kind != target.kind {
                    return Err(format!(
                        "Session {id} runs in a {}, not a {}.",
                        cloud.kind.noun(),
                        target.kind.noun()
                    ));
                }
                Ok(Some(cloud.name))
            }
        }
    }

    /// Prepares the instance for a blocking start: creating one needs a branch (default: the
    /// project's current branch). BLOCKS for minutes when creating or booting. Backends that
    /// cannot do it return a capability error.
    ///
    /// A codespace created here is never deleted on failure; the error names it so the caller
    /// retries with `instance:"<name>"` instead of creating a second one.
    pub fn prepare(
        &self,
        project: &str,
        target: CloudTarget,
        progress: &mut dyn FnMut(&str),
    ) -> Result<CloudBinding, String> {
        let instance = self.instance_name(project, &target)?;
        self.provider(target.kind)?
            .prepare(project, instance.as_deref(), &target.options, progress)
    }
    /// `sessions.start` with a cloud target: a pending session when an instance is created or
    /// connected, a ready one when the target is an instance a session of the project already
    /// runs on (see [`CloudStart`]).
    pub fn begin_start(
        &self,
        project: &str,
        target: CloudTarget,
        harness: &str,
    ) -> Result<CloudStart, String> {
        let instance = self.instance_name(project, &target)?;
        // `kit: "none"` can only narrow the user's choice (rule D11). It goes into the first
        // record the start persists, so no later write can turn the kit back on.
        self.provider(target.kind)?.start(
            project,
            harness,
            instance.as_deref(),
            &target.options,
            target.kit != ApiKit::None,
            target.account.as_deref(),
        )
    }
    /// A blank session on the binding (Claude only).
    pub fn create_session(&self, binding: &CloudBinding, harness: &str) -> Result<Session, String> {
        self.cloud.create_session(binding, harness)
    }

    /// Cloud sessions of `project` with their cached state. Never wakes anything.
    pub fn session_targets(&self, project: &str) -> HashMap<i64, SessionTarget> {
        let mut states: HashMap<String, CloudState> = HashMap::new();
        let starts: HashMap<i64, StartupSummary> = self
            .store
            .lock()
            .map(|store| {
                crate::cloud_sessions::pending_starts(&store)
                    .iter()
                    .map(|r: &StartRecord| (r.session_id, StartupSummary::of(r)))
                    .collect()
            })
            .unwrap_or_default();
        self.cloud
            .cloud_sessions_for(project)
            .unwrap_or_default()
            .into_iter()
            .map(|s| {
                let id = s.summary.id;
                let startup = starts.get(&id).cloned();
                let target = match startup {
                    // Starting: no gh read needed, the state is `starting`.
                    Some(startup) if startup.error.is_none() => SessionTarget {
                        codespace: s.codespace,
                        cloud: s.cloud,
                        state: CloudState::Starting,
                        startup: Some(startup),
                    },
                    startup => SessionTarget {
                        state: *states
                            .entry(s.cloud.key())
                            .or_insert_with(|| self.cloud.cloud_state_of(&s.cloud)),
                        codespace: s.codespace,
                        cloud: s.cloud,
                        startup,
                    },
                };
                (id, target)
            })
            .collect()
    }
    /// Wakes the instance of a cloud session for an explicit send or `cloud.resume`: a
    /// codespace is started and reconnected, a sandbox resumed and reconnected. No-op when it
    /// is connected or the session is not a cloud session. Refuses a session whose instance is
    /// still starting, or failed to start (see `cloud_sessions::starting_refusal`).
    /// BLOCKS for minutes: call it from a worker thread, never the UI thread. Background
    /// polling must never call it (the Host itself refuses to wake on its own).
    pub fn wake(&self, session_id: i64, store: &Database) -> Result<(), String> {
        let (session, refusal) = {
            let store = store
                .lock()
                .map_err(|_| "Native session store unavailable")?;
            (
                store.remote_session(session_id)?,
                crate::cloud_sessions::starting_refusal(&store, session_id),
            )
        };
        // A session whose instance is still starting (or failed to) cannot take a message.
        if let Some(refusal) = refusal {
            return Err(refusal);
        }
        let Some(session) = session else {
            return Ok(());
        };
        let Some(cloud) = session.project.cloud() else {
            return Ok(());
        };
        self.provider(cloud.kind)?.wake(&session)
    }
    /// `sessions.pullEvidence`: copies the files a cloud session left in its codespace or
    /// sandbox (default `.evidence/`, or `path`) to the session's local folder and returns
    /// their paths. With a task (`taskId`, a Today task id or key the caller may see) it also
    /// appends a note listing the files as `file://` URLs, so a local agent can write the report
    /// later; no note is added when nothing was pulled. A user action: it may start a stopped
    /// instance. BLOCKS.
    pub fn pull_evidence(
        &self,
        store: &Database,
        project: &str,
        session_id: i64,
        input: &Value,
        scope: &EvidenceScope,
        now: u64,
    ) -> Result<Value, String> {
        let lock = || store.lock().map_err(|_| "Native session store unavailable");
        if !is_cloud_session_of(&*lock()?, session_id, project) {
            return Err("Session is not a cloud session of this project.".into());
        }
        let task = match input.get("taskId") {
            None | Some(Value::Null) => None,
            Some(Value::String(id)) if !id.trim().is_empty() && id.len() <= 200 => {
                let id = id.trim();
                let found = lock()?
                    .today_task(id)?
                    .filter(|task| scope.sees(task, project))
                    .ok_or("Task not found")?;
                Some(found.id)
            }
            Some(_) => return Err("taskId must be a Today task id or key".into()),
        };
        let path = input.get("path").and_then(Value::as_str);
        let serving =
            serving_record(&*lock()?, session_id).map_or(session_id, |(host_id, _)| host_id);
        let pull = self.cloud.pull_evidence(serving, path)?;
        let files: Vec<String> = pull
            .files
            .iter()
            .map(|f| f.to_string_lossy().into_owned())
            .collect();
        let urls: Vec<String> = pull
            .files
            .iter()
            .filter_map(|f| url::Url::from_file_path(f).ok().map(String::from))
            .collect();
        let mut value = json!({
            "sessionId":session_id,
            "kind":pull.kind.api_name(),
            "instance":pull.instance,
            "remotePath":pull.remote_path,
            "localDir":pull.local_dir,
            "files":files,
            "fileUrls":urls,
        });
        if let Some(message) = &pull.message {
            value["message"] = json!(message);
        }
        if let Some(warning) = &pull.warning {
            value["warning"] = json!(warning);
        }
        if let (Some(task), false) = (task, urls.is_empty()) {
            let title = lock()?
                .remote_session(session_id)?
                .map(|s| s.summary.title)
                .unwrap_or_default();
            let cloud = CloudRef {
                kind: pull.kind,
                name: pull.instance.clone(),
            };
            let body = evidence_note(&title, &cloud, &urls, now);
            // The files are already local: a note that cannot be written is reported, not fatal.
            match lock()?.today_note(&task, &body, &scope.actor(), now) {
                Ok(note) => value["note"] = json!({"id":note.id,"taskId":note.task_id}),
                Err(e) => value["noteError"] = json!(e),
            }
        }
        Ok(value)
    }
    /// One session's target, `None` for a local session. Never wakes anything.
    pub fn target_of(&self, session_id: i64) -> Option<SessionTarget> {
        self.cloud.target_for_session(session_id)
    }
}

#[cfg(test)]
mod tests;
