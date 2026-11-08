//! Non-blocking start of a cloud session: the session exists at once, its codespace comes up
//! in the background and the UI follows a fixed list of steps.
//!
//! # Flow
//!
//! 1. [`CloudSessions::begin_start`] `(local_cwd, Target, harness) -> (Session, StartRecord)`:
//!    seconds. Checks gh auth and the repository, requests the codespace through the REST API
//!    (the name is known immediately, a slow devcontainer build can no longer lose it), then
//!    creates a *pending* session on a provisional project (environment `pending-<codespace>`,
//!    see [`crate::store::pending_cloud_project`]) and saves a [`StartRecord`].
//! 2. [`CloudSessions::run_start`] `(session_id, events, cancel) -> CloudBinding`: BLOCKS for
//!    minutes, run it on a worker thread. Polls the codespace until Available, follows its
//!    creation log, connects, opens the project and rebinds the session to the real project.
//!    It is idempotent: the first run, Retry after a failure and resuming after an app restart
//!    are the same call (see [`pending_starts`]). Only one `run_start` per session may run at
//!    a time; the caller enforces that. `events` hears [`StartEvent`]s in order and ends with
//!    exactly one of `Ready` or `Failed`; a cancelled start (`cancel`, or the session or
//!    codespace was deleted) returns `Err` with neither.
//! 3. [`CloudSessions::rebuild_start`] `(session_id)` rebuilds the container and resets the
//!    steps; call `run_start` again afterwards. [`CloudSessions::start_web_url`] gives the
//!    "Open in browser" URL.
//!
//! Readers need only a `&Store`: [`start_record`], [`pending_starts`], [`starting_refusal`]
//! (the text for a send to a starting session) and [`queued_start_message`]. A message typed
//! while starting is kept with [`queue_start_message`] as the session's first prompt.
//!
//! # Steps
//!
//! [`StepKind::ALL`] is the display order. `Create`, `Provision`, `Connect` and `OpenProject`
//! are timed with the wall clock. `BuildContainer`, `BlockingCommands`, `ConfigureCodespace`
//! and `LifecycleCommands` come from the codespace's creation log (`log_step`): the log is
//! replayed from the top once the codespace is Available, so their durations are real even
//! though they arrive late. `LifecycleCommands` runs in the background after Available and
//! never blocks `Ready`. Blocking lifecycle commands (up to the devcontainer `waitFor`) run
//! before Available, so connecting at Available respects `waitFor`.
//!
//! # Measured gh behaviour (gh 2.95.0)
//!
//! * `gh codespace create` polls for at most two minutes and prints the name only at the end;
//!   a slow build fails with "context deadline exceeded" and the name is lost. The REST call
//!   ([`codespaces::create_started`]) returns `201` with the name in seconds.
//! * `gh codespace logs` is unusable (plain `ssh` without gh's key, and it starts a stopped
//!   codespace). `gh codespace ssh -- tail -n +1 -F <creation.log>` works once Available.
//! * A real log: `Configuration starting...`, `Cloning...`, `Creating container...`,
//!   `Running blocking commands...`, `Configuring codespace...`, `Running oryx...`,
//!   `Running commands...`, `Running the postCreateCommand from Feature '...'...`,
//!   `Finished configuring codespace.`, each prefixed `2026-11-07 04:01:54.456Z: `.
use super::backend::{CloudBackendKind, CloudRef};
use super::docker_start::SandboxStart;
use super::{
    created_note, require_claude, CloudBinding, CloudSessions, ResolvedCodespace, Target, WaitPlan,
    START_ABORTED,
};
use crate::codespaces::{self, LogLine};
use crate::store::{pending_cloud_project, RemoteProject, Session, Store};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

/// Store setting holding the JSON map `session id -> StartRecord`.
const SETTING: &str = "cloud-starts.v1";
/// Same map for sandbox starts. A separate key keeps `cloud-starts.v1` readable and
/// untouched for an older app that does not know sandboxes.
const SETTING_SANDBOX: &str = "cloud-starts-sbx.v1";
/// How long [`CloudSessions::run_start`] tracks a codespace that is still provisioning.
pub const START_READY_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// Log lines older than the record's `started_at` by more than this do not update steps.
const LOG_STALE_MARGIN_MS: u64 = 60_000;
/// Prefix of connection progress lines in [`StartEvent::Log`], to tell them from creation-log lines.
pub const PROGRESS_PREFIX: &str = "[monoptah] ";
/// Number of log lines kept in a [`StartRecord`] and persisted with a failure.
pub const LOG_TAIL_LINES: usize = 40;
/// Prefix of the id of the draft [`queue_start_message`] stores.
const APP_PROMPT: &str = "app-prompt-";
/// How often the creation-log follower is (re)started while one start runs.
const FOLLOW_ATTEMPTS: usize = 4;
/// Refusal for a send to a session whose codespace is still starting, when the phase is not
/// known (see [`starting_refusal`] for the precise text).
pub const STILL_STARTING: &str =
    "This cloud session is still starting its codespace. Send again when it is ready.";
const NOT_STARTING: &str = "This cloud session is not starting a codespace.";
const NO_WORKSPACE: &str = "This cloud session start has no workspace.";

/// One step of starting a cloud session, in display order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StepKind {
    Create,
    Provision,
    BuildContainer,
    BlockingCommands,
    ConfigureCodespace,
    Connect,
    OpenProject,
    LifecycleCommands,
}

impl StepKind {
    /// Every step in display order. `LifecycleCommands` is last: it runs in the background
    /// after Available and never blocks `Ready`.
    pub const ALL: [StepKind; 8] = [
        StepKind::Create,
        StepKind::Provision,
        StepKind::BuildContainer,
        StepKind::BlockingCommands,
        StepKind::ConfigureCodespace,
        StepKind::Connect,
        StepKind::OpenProject,
        StepKind::LifecycleCommands,
    ];
    /// Name in the step list.
    pub fn label(self) -> &'static str {
        match self {
            Self::Create => "Create",
            Self::Provision => "Provision",
            Self::BuildContainer => "Build container",
            Self::BlockingCommands => "Lifecycle commands (waitFor)",
            Self::ConfigureCodespace => "Configure codespace",
            Self::Connect => "Connect over SSH",
            Self::OpenProject => "Open project",
            Self::LifecycleCommands => "Background lifecycle commands",
        }
    }
    /// Text for the composer chip and the transcript status row while the step runs.
    pub fn running_label(self) -> &'static str {
        match self {
            Self::Create => "Creating codespace",
            Self::Provision => "Provisioning",
            Self::BuildContainer => "Building container",
            Self::BlockingCommands => "Running lifecycle commands",
            Self::ConfigureCodespace => "Configuring codespace",
            Self::Connect => "Connecting",
            Self::OpenProject => "Opening project",
            Self::LifecycleCommands => "Running background commands",
        }
    }
    /// [`Self::label`] as `backend` words it. Differs from the codespace text only where
    /// the step is about the codespace itself.
    pub fn label_for(self, backend: CloudBackendKind) -> &'static str {
        match (backend, self) {
            (CloudBackendKind::DockerSandbox, Self::ConfigureCodespace) => "Configure environment",
            _ => self.label(),
        }
    }
    /// [`Self::running_label`] as `backend` words it.
    pub fn running_label_for(self, backend: CloudBackendKind) -> &'static str {
        match (backend, self) {
            (CloudBackendKind::DockerSandbox, Self::Create) => "Creating sandbox",
            (CloudBackendKind::DockerSandbox, Self::ConfigureCodespace) => {
                "Configuring environment"
            }
            _ => self.running_label(),
        }
    }
    /// camelCase identifier, as in the app API (`startup.step`).
    pub fn id(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Provision => "provision",
            Self::BuildContainer => "buildContainer",
            Self::BlockingCommands => "blockingCommands",
            Self::ConfigureCodespace => "configureCodespace",
            Self::Connect => "connect",
            Self::OpenProject => "openProject",
            Self::LifecycleCommands => "lifecycleCommands",
        }
    }
    /// Steps whose progress comes from the creation log.
    fn is_log_derived(self) -> bool {
        matches!(
            self,
            Self::BuildContainer
                | Self::BlockingCommands
                | Self::ConfigureCodespace
                | Self::LifecycleCommands
        )
    }
    fn index(self) -> usize {
        Self::ALL.iter().position(|k| *k == self).unwrap_or(0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StepStatus {
    Pending,
    Running,
    Done,
    Failed,
    /// Did not happen (an existing codespace is not created; a log marker never appeared).
    Skipped,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Step {
    pub kind: StepKind,
    pub status: StepStatus,
    /// Unix milliseconds. Log-derived steps use the log's own timestamps.
    pub started_at: Option<u64>,
    pub finished_at: Option<u64>,
    /// Create: the codespace name. Provision: the gh state (`Provisioning`, `Starting`...).
    /// LifecycleCommands: the last `Running the postCreateCommand from ...` line.
    pub detail: Option<String>,
}

impl Step {
    pub(super) fn pending(kind: StepKind) -> Self {
        Self {
            kind,
            status: StepStatus::Pending,
            started_at: None,
            finished_at: None,
            detail: None,
        }
    }
}

/// What one creation-log line means for the steps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogMark {
    /// `Configuration starting`, `Cloning`: part of Provision, which the wall clock times.
    Provision,
    /// This step starts; the previous log-derived running step ends at the same instant.
    Start(StepKind),
    /// `Finished configuring codespace`: every running log-derived step is done.
    Finished,
}

/// Maps the text of a creation-log line (see [`codespaces::parse_log_line`]) to its meaning.
pub fn log_step(text: &str) -> Option<LogMark> {
    let text = text.trim();
    if text.starts_with("Configuration starting") || text.starts_with("Cloning") {
        Some(LogMark::Provision)
    } else if text.starts_with("Creating container") {
        Some(LogMark::Start(StepKind::BuildContainer))
    } else if text.starts_with("Running blocking commands") {
        Some(LogMark::Start(StepKind::BlockingCommands))
    } else if text.starts_with("Configuring codespace") {
        Some(LogMark::Start(StepKind::ConfigureCodespace))
    } else if text.starts_with("Running commands")
        || (text.starts_with("Running the ") && text.contains("Command"))
    {
        Some(LogMark::Start(StepKind::LifecycleCommands))
    } else if text.starts_with("Finished configuring codespace") {
        Some(LogMark::Finished)
    } else {
        None
    }
}

/// Everything known about one session that is starting its codespace. Persisted in the store
/// (setting `cloud-starts.v1`) from [`CloudSessions::begin_start`] until the start is ready,
/// so a start survives an app restart and a failure can be retried.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartRecord {
    pub session_id: i64,
    pub local_project: String,
    /// Name of the codespace; empty for a sandbox start (see [`Self::sandbox`]).
    pub codespace: String,
    /// Name of the Docker sandbox. Set only on records stored under [`SETTING_SANDBOX`],
    /// which an older app never reads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<String>,
    /// `owner/repo`.
    pub repo: String,
    pub branch: Option<String>,
    pub machine: Option<String>,
    pub display: String,
    /// `/workspaces/<repo-name>`.
    pub remote_cwd: String,
    pub web_url: Option<String>,
    /// This app created the codespace (so a failure keeps it and says so).
    pub created: bool,
    /// Last gh state seen while provisioning.
    pub gh_state: Option<String>,
    /// Always [`StepKind::ALL`], in order.
    pub steps: Vec<Step>,
    pub error: Option<String>,
    pub failed_at: Option<StepKind>,
    /// The last [`LOG_TAIL_LINES`] log lines, persisted with a failure.
    pub log_tail: Vec<String>,
    pub started_at: u64,
    pub finished_at: Option<u64>,
    /// `run_start` polls the codespace until Available before connecting. True for a
    /// codespace this app created or rebuilt; an existing one is connected to directly
    /// (connecting starts a stopped codespace).
    #[serde(default)]
    pub track_ready: bool,
    /// What only a sandbox start has. Set exactly when [`Self::sandbox`] is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox_start: Option<SandboxStart>,
}

impl StartRecord {
    pub(super) fn fresh(
        resolved: &ResolvedCodespace,
        session_id: i64,
        local_cwd: &str,
        now: u64,
    ) -> Self {
        Self {
            session_id,
            local_project: local_cwd.into(),
            codespace: resolved.name.clone(),
            sandbox: None,
            repo: resolved.repo.clone(),
            branch: resolved.branch.clone(),
            machine: resolved.machine.clone(),
            display: resolved.display.clone(),
            remote_cwd: resolved.remote_cwd.clone(),
            web_url: resolved.web_url.clone(),
            created: resolved.created,
            gh_state: Some(resolved.state.clone()),
            steps: StepKind::ALL.iter().map(|k| Step::pending(*k)).collect(),
            error: None,
            failed_at: None,
            log_tail: vec![],
            started_at: now,
            finished_at: None,
            track_ready: resolved.created,
            sandbox_start: None,
        }
    }
    /// The backend this start runs on.
    pub fn backend(&self) -> CloudBackendKind {
        if self.sandbox.is_some() {
            CloudBackendKind::DockerSandbox
        } else {
            CloudBackendKind::Codespaces
        }
    }
    /// The workspace being started, `None` for a record that names none.
    pub fn cloud(&self) -> Option<CloudRef> {
        match &self.sandbox {
            Some(name) => Some(CloudRef::sandbox(name)),
            None => Some(CloudRef::codespace(&self.codespace)).filter(|c| !c.name.is_empty()),
        }
    }
    /// The step called `kind`. A record always has all of them; one read from damaged
    /// storage is repaired by [`CloudSessions::run_start`] before use.
    pub fn step(&self, kind: StepKind) -> &Step {
        &self.steps[kind.index()]
    }
    pub(super) fn step_mut(&mut self, kind: StepKind) -> &mut Step {
        &mut self.steps[kind.index()]
    }
    /// Ensures `steps` holds every kind in order (damaged or older storage).
    fn normalize(&mut self) {
        let mut steps: Vec<Step> = StepKind::ALL.iter().map(|k| Step::pending(*k)).collect();
        for old in std::mem::take(&mut self.steps) {
            let at = old.kind.index();
            steps[at] = old;
        }
        self.steps = steps;
    }
    /// The step to show in the composer chip: the running main-flow step (Connect or
    /// OpenProject), else the latest running log-derived step, else Provision while it
    /// waits. `None` when nothing is running.
    pub fn current(&self) -> Option<StepKind> {
        let running = |kind: StepKind| self.step(kind).status == StepStatus::Running;
        [StepKind::Connect, StepKind::OpenProject]
            .into_iter()
            .find(|k| running(*k))
            .or_else(|| {
                StepKind::ALL
                    .iter()
                    .rev()
                    .copied()
                    .find(|k| k.is_log_derived() && running(*k))
            })
            .or_else(|| running(StepKind::Provision).then_some(StepKind::Provision))
            .or_else(|| running(StepKind::Create).then_some(StepKind::Create))
    }
    pub fn is_failed(&self) -> bool {
        self.error.is_some()
    }
    /// Time the start took (or has taken so far), in milliseconds.
    pub fn elapsed_ms(&self, now: u64) -> u64 {
        self.finished_at
            .unwrap_or(now)
            .saturating_sub(self.started_at)
    }
    pub(super) fn start_step(&mut self, kind: StepKind, at: u64) {
        let step = self.step_mut(kind);
        if step.status == StepStatus::Pending {
            step.status = StepStatus::Running;
            step.started_at = Some(at);
            step.finished_at = None;
        }
    }
    pub(super) fn finish_step(&mut self, kind: StepKind, at: u64) {
        let step = self.step_mut(kind);
        if matches!(step.status, StepStatus::Pending | StepStatus::Running) {
            step.started_at.get_or_insert(at);
            step.status = StepStatus::Done;
            step.finished_at = Some(at.max(step.started_at.unwrap_or(at)));
        }
    }
    fn finish_running_log_steps(&mut self, except: Option<StepKind>, at: u64) -> bool {
        let mut changed = false;
        for kind in StepKind::ALL {
            if kind.is_log_derived()
                && Some(kind) != except
                && self.step(kind).status == StepStatus::Running
            {
                self.finish_step(kind, at);
                changed = true;
            }
        }
        changed
    }
    /// Feeds one parsed creation-log line into the log-derived steps; true when a step
    /// changed. `fallback_at` stamps a marker line that carries no timestamp.
    pub(super) fn apply_log(&mut self, line: &LogLine, fallback_at: u64) -> bool {
        // The log is replayed from the top: lines older than this start belong to an earlier
        // build of the codespace. They stay in the displayed log but never move a step.
        if line
            .at_ms
            .is_some_and(|t| t.saturating_add(LOG_STALE_MARGIN_MS) < self.started_at)
        {
            return false;
        }
        let at = line.at_ms.unwrap_or(fallback_at);
        match log_step(&line.text) {
            None | Some(LogMark::Provision) => false,
            Some(LogMark::Finished) => self.finish_running_log_steps(None, at),
            Some(LogMark::Start(kind)) => {
                let before = self.step(kind).clone();
                if before.status == StepStatus::Pending {
                    self.finish_running_log_steps(Some(kind), at);
                    self.start_step(kind, at);
                }
                if kind == StepKind::LifecycleCommands
                    && self.step(kind).status == StepStatus::Running
                {
                    self.step_mut(kind).detail = Some(line.text.clone());
                }
                *self.step(kind) != before
            }
        }
    }
    pub(super) fn push_log(&mut self, text: &str) {
        self.log_tail.push(text.to_string());
        let extra = self.log_tail.len().saturating_sub(LOG_TAIL_LINES);
        self.log_tail.drain(..extra);
    }
    /// Prepares a run: a new attempt starts clean. Finished log-derived steps and Provision
    /// stay (a replayed log does not redo them); anything failed or interrupted, and the
    /// cheap connection steps, run again.
    pub(super) fn reset_for_run(&mut self) {
        self.error = None;
        self.failed_at = None;
        self.finished_at = None;
        for step in &mut self.steps {
            let redo = matches!(step.status, StepStatus::Failed | StepStatus::Running)
                || (step.status == StepStatus::Skipped && step.started_at.is_some())
                || matches!(step.kind, StepKind::Connect | StepKind::OpenProject);
            if redo {
                let detail = step
                    .detail
                    .take()
                    .filter(|_| step.kind == StepKind::Provision);
                *step = Step {
                    detail,
                    ..Step::pending(step.kind)
                };
            }
        }
        if self.step(StepKind::Provision).status == StepStatus::Pending {
            // Interrupted while waiting: keep the original start so durations stay honest.
            let at = self.started_at;
            self.start_step(StepKind::Provision, at);
        }
    }
    /// Marks the failing step: a running main-flow step wins over a running log step.
    pub(super) fn fail(&mut self, error: String, at: u64) {
        let main = [
            StepKind::Connect,
            StepKind::OpenProject,
            StepKind::Provision,
            StepKind::Create,
        ];
        let running = |s: &Self, k: StepKind| s.step(k).status == StepStatus::Running;
        let failing = main.into_iter().find(|k| running(self, *k)).or_else(|| {
            StepKind::ALL
                .iter()
                .rev()
                .copied()
                .find(|k| k.is_log_derived() && running(self, *k))
        });
        if let Some(kind) = failing {
            let step = self.step_mut(kind);
            step.status = StepStatus::Failed;
            step.finished_at = Some(at);
        }
        for step in &mut self.steps {
            if step.status == StepStatus::Running {
                step.status = StepStatus::Skipped;
                step.finished_at = Some(at);
            }
        }
        self.failed_at = failing;
        self.error = Some(error);
        self.finished_at = Some(at);
    }
}

/// What [`CloudSessions::run_start`] reports, in order.
#[derive(Clone, Debug)]
pub enum StartEvent {
    /// The record changed (a step moved, the gh state changed).
    Updated(StartRecord),
    /// A creation-log line (timestamp removed) or a connection progress line.
    Log(String),
    /// The codespace is open; the session now lives on the real project.
    Ready {
        binding: CloudBinding,
        session: Session,
    },
    /// The start failed; the record (with `error`, `failed_at`, `log_tail`) is persisted.
    Failed(StartRecord),
}

fn load_key(store: &Store, key: &str) -> BTreeMap<i64, StartRecord> {
    store
        .setting(key)
        .ok()
        .flatten()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn save_key(store: &Store, key: &str, all: &BTreeMap<i64, StartRecord>) -> Result<(), String> {
    store.save_setting(key, &serde_json::to_string(all).map_err(|e| e.to_string())?)
}

/// The setting a record is stored under.
fn key_of(record: &StartRecord) -> &'static str {
    match record.backend() {
        CloudBackendKind::Codespaces => SETTING,
        CloudBackendKind::DockerSandbox => SETTING_SANDBOX,
    }
}

/// Every stored record, codespace and sandbox starts together.
fn load_all(store: &Store) -> BTreeMap<i64, StartRecord> {
    let mut all = load_key(store, SETTING);
    all.extend(load_key(store, SETTING_SANDBOX));
    all
}

/// The start record of a session, `None` once it is ready (or never started this way).
pub fn start_record(store: &Store, id: i64) -> Option<StartRecord> {
    load_all(store).remove(&id).map(|mut record| {
        record.normalize();
        record
    })
}

/// Every start in progress or failed, oldest first. After an app restart these are the
/// sessions to resume or offer a Retry for.
pub fn pending_starts(store: &Store) -> Vec<StartRecord> {
    let mut all: Vec<_> = load_all(store)
        .into_values()
        .map(|mut record| {
            record.normalize();
            record
        })
        .collect();
    all.sort_by_key(|r| (r.started_at, r.session_id));
    all
}

/// Saves (inserts or replaces) a record.
pub fn save_start_record(store: &Store, record: &StartRecord) -> Result<(), String> {
    let key = key_of(record);
    let mut all = load_key(store, key);
    all.insert(record.session_id, record.clone());
    save_key(store, key, &all)
}

/// Removes a record; no-op when there is none. Only the setting that held it is written.
pub fn remove_start_record(store: &Store, id: i64) -> Result<(), String> {
    for key in [SETTING, SETTING_SANDBOX] {
        let mut all = load_key(store, key);
        if all.remove(&id).is_some() {
            save_key(store, key, &all)?;
        }
    }
    Ok(())
}

/// Pauses a pending session before archive or an explicit stop. Retry remains available.
pub(crate) fn pause_start(store: &Store, id: i64, reason: &str) -> Result<(), String> {
    if let Some(mut record) = start_record(store, id) {
        record.fail(reason.into(), crate::time::now_ms());
        save_start_record(store, &record)?;
    }
    Ok(())
}

/// Drops the records of a workspace that no longer exists (called by `forget_cloud`).
pub(crate) fn forget_cloud_starts(store: &Store, cloud: &CloudRef) -> Result<(), String> {
    for key in [SETTING, SETTING_SANDBOX] {
        let mut all = load_key(store, key);
        let before = all.len();
        all.retain(|_, record| record.cloud().as_ref() != Some(cloud));
        if all.len() != before {
            save_key(store, key, &all)?;
        }
    }
    Ok(())
}

/// Drops the record of a session that was forgotten (called by `forget_cloud_session`).
pub(crate) fn forget_session_start(store: &Store, id: i64) -> Result<(), String> {
    remove_start_record(store, id)
}

/// Why a send to a starting session is refused, `None` when the session is not starting.
pub fn starting_refusal(store: &Store, id: i64) -> Option<String> {
    let record = start_record(store, id)?;
    Some(match &record.error {
        Some(error) => {
            format!("This cloud session could not start: {error}. Choose Retry in the session.")
        }
        None => format!(
            "This cloud session is still starting its codespace ({}). Send again when it is ready.",
            record
                .current()
                .unwrap_or(StepKind::Provision)
                .running_label_for(record.backend())
        ),
    })
}

/// Keeps `text` as the session's first prompt while its codespace starts: an unsent draft
/// whose id starts with `app-prompt-` (the same mechanism as an app-launched prompt). When
/// such a draft exists the text is appended after a blank line. An unrelated draft of the
/// user is never touched: that is an `Err`.
pub fn queue_start_message(store: &Store, session_id: i64, text: &str) -> Result<(), String> {
    let mut state = store.follow_ups(session_id)?;
    match state.draft.as_mut() {
        Some(draft) if draft.id.starts_with(APP_PROMPT) => {
            draft.text.push_str("\n\n");
            draft.text.push_str(text);
            state.revision += 1;
            store.save_follow_ups(session_id, &state)
        }
        Some(_) => Err("This session already has a draft. Send or clear it first.".into()),
        None => store.prepare_app_prompt(
            session_id,
            &format!("cloud-start-{}", uuid::Uuid::new_v4()),
            text,
        ),
    }
}

/// The text [`queue_start_message`] kept for a session, if any.
pub fn queued_start_message(store: &Store, id: i64) -> Option<String> {
    let draft = store.follow_ups(id).ok()?.draft?;
    draft.id.starts_with(APP_PROMPT).then_some(draft.text)
}

/// Messages from the helper threads to the thread that owns the record.
enum Msg {
    /// A raw creation-log line.
    Log(String),
    /// A connection progress line.
    Progress(String),
    Step(StepKind, StepStatus),
    Done(Box<Result<(CloudBinding, RemoteProject), String>>),
}

/// Why a run ended without `Ready`.
enum Stop {
    /// Cancelled, or the session / codespace disappeared. Not reported as a failure.
    Aborted,
    Failed(String),
}
impl From<String> for Stop {
    fn from(error: String) -> Self {
        if error == START_ABORTED {
            Stop::Aborted
        } else {
            Stop::Failed(error)
        }
    }
}

/// The creation-log follower: a thread running `gh codespace ssh ... tail -F`. It must never
/// fail a start, so errors only make it try again (a few times) and then give up silently.
/// Dropping it stops and joins the thread.
struct Follower {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}
impl Follower {
    fn start(
        gh: Arc<dyn codespaces::GhRunner>,
        codespace: String,
        tx: mpsc::Sender<Msg>,
        retry: Duration,
        started_at: u64,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let handle = std::thread::spawn(move || {
            // Every attempt replays the log from the top: skip what was already delivered.
            let mut delivered = 0usize;
            for _ in 0..FOLLOW_ATTEMPTS {
                if flag.load(Ordering::Relaxed) {
                    return;
                }
                let mut seen = 0usize;
                let _ = codespaces::follow_creation_log(&*gh, &codespace, &flag, &mut |raw| {
                    seen += 1;
                    if seen <= delivered {
                        return;
                    }
                    delivered = seen;
                    let _ = tx.send(Msg::Log(raw.to_string()));
                    let finished = codespaces::parse_log_line(raw).is_some_and(|l| {
                        l.at_ms
                            .is_none_or(|at| at.saturating_add(LOG_STALE_MARGIN_MS) >= started_at)
                            && log_step(&l.text) == Some(LogMark::Finished)
                    });
                    if finished {
                        // Nothing more to learn from the log.
                        flag.store(true, Ordering::Relaxed);
                    }
                });
                let until = Instant::now() + retry;
                while Instant::now() < until && !flag.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        });
        Self {
            stop,
            handle: Some(handle),
        }
    }
}
impl Drop for Follower {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl CloudSessions {
    pub(super) fn now(&self) -> u64 {
        (self.clock)()
    }

    /// Step 1 of a non-blocking start, see the module docs. `Err` before a codespace exists
    /// creates nothing; `Err` after it was created names it ([`created_note`]).
    pub fn begin_start(
        &self,
        local_cwd: &str,
        target: Target,
        harness: &str,
    ) -> Result<(Session, StartRecord), String> {
        require_claude(harness)?;
        let began = self.now();
        let resolved = self.resolve_codespace(local_cwd, target, &mut |_| {})?;
        self.invalidate(&resolved.name);
        self.record_start(local_cwd, harness, &resolved, began)
            .map_err(|error| {
                if resolved.created {
                    created_note(&resolved.name, &error)
                } else {
                    error
                }
            })
    }
    fn record_start(
        &self,
        local_cwd: &str,
        harness: &str,
        resolved: &ResolvedCodespace,
        began: u64,
    ) -> Result<(Session, StartRecord), String> {
        let now = self.now();
        let store = self.store()?;
        let project = pending_cloud_project(
            local_cwd,
            &CloudRef::codespace(&resolved.name),
            &resolved.remote_cwd,
        );
        let session = store.create_pending_cloud_session(harness, project)?;
        let mut record = StartRecord::fresh(resolved, session.id, local_cwd, began);
        let create = record.step_mut(StepKind::Create);
        if resolved.created {
            create.status = StepStatus::Done;
            create.started_at = Some(began);
            create.finished_at = Some(now);
            create.detail = Some(resolved.name.clone());
        } else {
            create.status = StepStatus::Skipped;
        }
        let provision = record.step_mut(StepKind::Provision);
        provision.status = StepStatus::Running;
        provision.started_at = Some(now);
        provision.detail = Some(resolved.state.clone());
        if let Err(error) = save_start_record(&store, &record) {
            let _ = store.forget_cloud_session(session.id);
            return Err(error);
        }
        Ok((session, record))
    }

    pub(super) fn start_aborted(&self, session_id: i64, cancel: &AtomicBool) -> bool {
        cancel.load(Ordering::Relaxed)
            || self.store().is_ok_and(|store| {
                start_record(&store, session_id).is_none_or(|record| record.is_failed())
            })
    }
    pub(super) fn persist_start(&self, record: &StartRecord) {
        if let Ok(store) = self.store() {
            if start_record(&store, record.session_id)
                .is_some_and(|saved| !saved.is_failed() || record.is_failed())
            {
                let _ = save_start_record(&store, record);
            }
        }
    }
    pub(super) fn publish(&self, record: &StartRecord, events: &mut dyn FnMut(StartEvent)) {
        self.persist_start(record);
        events(StartEvent::Updated(record.clone()));
    }

    /// Resumes on launch without waking a stopped or archived codespace.
    pub fn resume_start(
        &self,
        session_id: i64,
        events: &mut dyn FnMut(StartEvent),
        cancel: &AtomicBool,
    ) -> Result<CloudBinding, String> {
        let mut record = start_record(&*self.store()?, session_id).ok_or(NOT_STARTING)?;
        let session = self.store()?.remote_session(session_id)?;
        let reason = match session {
            None => {
                remove_start_record(&*self.store()?, session_id)?;
                return Err(START_ABORTED.into());
            }
            Some(session) if session.summary.archived => {
                Some("Session is archived. Choose Retry to start it again.".to_string())
            }
            Some(_) => match self.cloud_start_state(&record) {
                Ok(state) if self.start_is_stopped(&record, &state) => Some(format!(
                    "{} stopped. Choose Retry to start it again.",
                    record.backend().title()
                )),
                // Resuming runs ssh, which resumes (and bills) a sandbox: only a sandbox that
                // is running now may continue on its own; every other state waits for Retry.
                Ok(state)
                    if record.backend() == CloudBackendKind::DockerSandbox
                        && !self.backend(record.backend()).is_ok_and(|backend| {
                            backend.display_state(&state) == super::CloudState::Available
                        }) =>
                {
                    Some(format!(
                        "Sandbox is {}. Choose Retry to start it again.",
                        state.trim().to_ascii_lowercase()
                    ))
                }
                Ok(_) => None,
                Err(error) => Some(error),
            },
        };
        if let Some(error) = reason {
            record.fail(error.clone(), self.now());
            self.persist_start(&record);
            events(StartEvent::Failed(record));
            return Err(error);
        }
        self.run_start(session_id, events, cancel)
    }

    /// Step 2 of a non-blocking start; BLOCKS, see the module docs.
    pub fn run_start(
        &self,
        session_id: i64,
        events: &mut dyn FnMut(StartEvent),
        cancel: &AtomicBool,
    ) -> Result<CloudBinding, String> {
        let mut record = start_record(&*self.store()?, session_id).ok_or(NOT_STARTING)?;
        if record.backend() == CloudBackendKind::DockerSandbox {
            return self.run_sandbox_start(record, events, cancel);
        }
        record.reset_for_run();
        if cancel.load(Ordering::Relaxed) {
            return Err(START_ABORTED.into());
        }
        {
            let store = self.store()?;
            if start_record(&store, session_id).is_none() {
                return Err(START_ABORTED.into());
            }
            save_start_record(&store, &record)?;
        }
        record.log_tail.clear();
        self.publish(&record, events);
        match self.drive_start(&mut record, events, cancel) {
            Ok((binding, session)) => {
                events(StartEvent::Ready {
                    binding: binding.clone(),
                    session,
                });
                Ok(binding)
            }
            Err(Stop::Aborted) => Err(START_ABORTED.into()),
            Err(Stop::Failed(error)) => {
                let error = if record.created {
                    created_note(&record.codespace, &error)
                } else {
                    error
                };
                record.fail(error.clone(), self.now());
                self.persist_start(&record);
                events(StartEvent::Failed(record));
                Err(error)
            }
        }
    }

    fn drive_start(
        &self,
        record: &mut StartRecord,
        events: &mut dyn FnMut(StartEvent),
        cancel: &AtomicBool,
    ) -> Result<(CloudBinding, Session), Stop> {
        let id = record.session_id;
        let name = record.codespace.clone();
        let aborted = || self.start_aborted(id, cancel);
        if record.step(StepKind::Provision).status != StepStatus::Done {
            if record.track_ready {
                let plan = WaitPlan {
                    timeout: START_READY_TIMEOUT,
                    shutdown_is_ok: false,
                };
                self.wait_available(
                    &name,
                    &plan,
                    &mut |state| {
                        record.gh_state = Some(state.into());
                        record.step_mut(StepKind::Provision).detail = Some(state.into());
                        self.publish(record, events);
                    },
                    &aborted,
                )?;
            }
            record.finish_step(StepKind::Provision, self.now());
            self.publish(record, events);
        }
        self.invalidate(&name);
        if aborted() {
            return Err(Stop::Aborted);
        }

        let (tx, rx) = mpsc::channel::<Msg>();
        let _follower = Follower::start(
            self.gh.clone(),
            name.clone(),
            tx.clone(),
            self.follower_retry,
            record.started_at,
        );
        let halt = Arc::new(AtomicBool::new(false));
        {
            let (backend, host, store, halt, tx) = (
                self.backend(record.backend())?,
                self.host.clone(),
                self.store.clone(),
                halt.clone(),
                tx,
            );
            let record = record.clone();
            std::thread::spawn(move || {
                let request = super::OpenRequest {
                    local_cwd: &record.local_project,
                    cloud: &CloudRef::codespace(&record.codespace),
                    sandbox: None,
                    display: &record.display,
                    repo: &record.repo,
                    remote_cwd: &record.remote_cwd,
                };
                let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    super::open_project(
                        &*backend,
                        &*host,
                        &store,
                        &request,
                        &halt,
                        &mut |kind, status| {
                            let _ = tx.send(Msg::Step(kind, status));
                        },
                        &mut |line| {
                            let _ = tx.send(Msg::Progress(line.to_string()));
                        },
                    )
                }));
                let result = run.unwrap_or_else(|_| Err("Opening the project crashed".to_string()));
                let _ = tx.send(Msg::Done(Box::new(result)));
            });
        }

        let mut last_check = Instant::now();
        let opened = loop {
            let received = rx.recv_timeout(Duration::from_millis(150));
            if last_check.elapsed() >= Duration::from_millis(500) {
                last_check = Instant::now();
                if aborted() {
                    halt.store(true, Ordering::Relaxed);
                    return Err(Stop::Aborted);
                }
            }
            match received {
                Ok(Msg::Log(raw)) => {
                    if let Some(line) = codespaces::parse_log_line(&raw) {
                        record.push_log(&line.text);
                        events(StartEvent::Log(line.text.clone()));
                        if record.apply_log(&line, self.now()) {
                            self.publish(record, events);
                        }
                    }
                }
                Ok(Msg::Progress(line)) => {
                    let line = format!("{PROGRESS_PREFIX}{line}");
                    record.push_log(&line);
                    events(StartEvent::Log(line));
                }
                Ok(Msg::Step(kind, StepStatus::Running)) => {
                    record.start_step(kind, self.now());
                    self.publish(record, events);
                }
                Ok(Msg::Step(kind, _)) => {
                    record.finish_step(kind, self.now());
                    self.publish(record, events);
                }
                Ok(Msg::Done(result)) => break *result,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(Stop::Failed("The connection helper stopped".into()));
                }
            }
        };
        let (binding, project) = opened?;
        if aborted() {
            return Err(Stop::Aborted);
        }
        let session = {
            let store = self.store()?;
            match store.rebind_cloud_session(id, project) {
                Ok(session) => session,
                Err(_) if start_record(&store, id).is_none() => return Err(Stop::Aborted),
                Err(error) => return Err(Stop::Failed(error)),
            }
        };
        let at = self.now();
        record.finish_step(StepKind::OpenProject, at);
        for kind in StepKind::ALL {
            if kind.is_log_derived()
                && kind != StepKind::LifecycleCommands
                && record.step(kind).status == StepStatus::Pending
            {
                record.step_mut(kind).status = StepStatus::Skipped;
            }
        }
        if record.step(StepKind::LifecycleCommands).status == StepStatus::Running {
            let step = record.step_mut(StepKind::LifecycleCommands);
            step.status = StepStatus::Skipped;
            step.finished_at = Some(at);
        }
        record.finished_at = Some(at);
        // The final list (all main-flow steps done, absent log steps skipped) is shown once
        // before `Ready`; it is not persisted because the record is removed next.
        events(StartEvent::Updated(record.clone()));
        remove_start_record(&*self.store()?, id)?;
        self.invalidate(&name);
        Ok((binding, session))
    }

    /// Rebuilds the container of the start's codespace (`gh codespace rebuild`) and resets
    /// its steps (except Create) so that the next [`Self::run_start`] tracks the rebuild
    /// from Provision. `Err` when the session has no start record. Returns the new record.
    pub fn rebuild_start(&self, session_id: i64) -> Result<StartRecord, String> {
        let mut record = start_record(&*self.store()?, session_id).ok_or(NOT_STARTING)?;
        if record.backend() == CloudBackendKind::DockerSandbox {
            return self.rebuild_sandbox_start(record);
        }
        let cloud = record.cloud().ok_or(NO_WORKSPACE)?;
        self.backend(cloud.kind)?.rebuild(&cloud.name)?;
        self.invalidate_ref(&cloud);
        let now = self.now();
        for step in &mut record.steps {
            if step.kind != StepKind::Create {
                *step = Step::pending(step.kind);
            }
        }
        record.error = None;
        record.failed_at = None;
        record.finished_at = None;
        record.gh_state = None;
        record.log_tail.clear();
        record.track_ready = true;
        // The log restarts: only lines from now on describe the rebuild.
        record.started_at = now;
        record.start_step(StepKind::Provision, now);
        record.step_mut(StepKind::Provision).detail = Some("Rebuilding".into());
        let store = self.store()?;
        if start_record(&store, session_id).is_none() {
            return Err(START_ABORTED.into());
        }
        save_start_record(&store, &record)?;
        Ok(record)
    }

    /// Browser URL of the start's workspace: the one returned at creation, else asked from
    /// its backend. `Err` when the session has no start record, or the backend has no URL.
    pub fn start_web_url(&self, session_id: i64) -> Result<String, String> {
        let record = start_record(&*self.store()?, session_id).ok_or(NOT_STARTING)?;
        match record.web_url {
            Some(url) => Ok(url),
            None => {
                let cloud = record.cloud().ok_or(NO_WORKSPACE)?;
                self.backend(cloud.kind)?
                    .web_url(&cloud.name)
                    .unwrap_or_else(|| Err(format!("A {} has no web page.", cloud.noun())))
            }
        }
    }
    fn start_is_stopped(&self, record: &StartRecord, raw: &str) -> bool {
        self.backend(record.backend())
            .is_ok_and(|backend| backend.is_stopped(raw))
    }
    /// Raw backend state of the workspace a start runs on. Never wakes it.
    fn cloud_start_state(&self, record: &StartRecord) -> Result<String, String> {
        let cloud = record.cloud().ok_or(NO_WORKSPACE)?;
        self.backend(cloud.kind)?.state(&cloud.name)
    }
}
