//! "Continue in cloud": moves a local session to a cloud workspace (design 4.2.2 steps 1-7,
//! 4.9 R1, R4, R5, R6 and R9; rules D8, D9 and D10).
//!
//! # Flow
//!
//! 1. [`CloudSessions::begin_move`] `(MoveRequest, busy) -> MoveRecord` checks that the session
//!    is a local, idle Claude (or other) session and saves a [`MoveRecord`] (setting
//!    `cloud-move:<session id>`). It changes nothing else.
//! 2. [`CloudSessions::run_move`] `(session id, busy, events, cancel) -> MoveOutcome` BLOCKS (run
//!    it on a worker thread) and walks the seven steps of [`MoveStep::ALL`] in order. The record
//!    is saved after each step, so the call is idempotent: the first run, Retry after a failure
//!    and resuming after an app restart are the same call and continue at the step that did
//!    not finish. Steps that finished are never run again.
//! 3. [`CloudSessions::cancel_move`] rolls back what the move created (see below).
//!
//! [`move_record`] and [`pending_moves`] read the record with only a `&Store`. They are for
//! LISTING an unfinished move: nothing here resumes a move on its own, because
//! [`CloudSessions::run_move`] may wake a stopped workspace and so is only for a user action
//! (Retry), never for a background path.
//!
//! # Steps
//!
//! * **Snapshot** (local, read-only): fetch origin, choose the push target
//!   ([`move_out::push_target`]), build the patch and `tree` ([`move_out::snapshot`]) and copy
//!   the transcript ([`move_out::collect_transcript`]). The row count and the last row of the
//!   session are recorded as the anchor for the flip. LFS and dirty submodules become
//!   warnings, ignored files are only listed.
//! * **Push**: `<head>:refs/heads/<branch>` through [`crate::scm::push_commit_to_branch`]: never
//!   forced, never to origin's default branch. No push when `needs_push` is false or origin
//!   already has `head` there. A pushed branch is NEVER deleted by this module.
//! * **Workspace**: a new codespace on the pushed branch (`begin_start` / `run_start`), a new
//!   sandbox with `base_ref == branch` (the clone phase checks out the tracking branch), or an
//!   existing instance (the Host then restores into a new worktree). The start's own blank
//!   session is forgotten once the binding is known.
//! * **Upload**: the uncommitted patch and the transcript JSONL, through the Host's `uploads.*`.
//!   Nothing else is ever uploaded: ignored files stay on this computer. The folders next to the
//!   transcript (`<id>/`, `file-history/<id>/`) are collected for a later return but
//!   `sessions.import` carries only the JSONL.
//! * **RestoreCode**: `workspace.applySnapshot`, whose result must repeat `head`, `branch` and
//!   `tree`.
//! * **RestoreConversation**: first any older Host session that still binds the same Claude
//!   id is deleted (the Host refuses a second binding), then `sessions.import` with the same
//!   id and `fromCwds` = project cwd, session cwd and their real paths. A `forked` decision
//!   means the cloud copy diverged: the user is told and the NEW id is used from then on.
//!   A session that is not Claude (or has no usable transcript) takes the summary path: a
//!   blank Claude session is created on the Host and the local session gets a
//!   [`crate::agent::Handoff`] whose brief is [`crate::response::handoff`] of its rows (the same
//!   text a provider switch uses, built locally, no model request). The cloud Claude then
//!   starts with a new id.
//! * **Flip**: the Claude version check (the newest `version` in the JSONL against the remote
//!   `claude --version`; `claude update` runs there when it is older, a failure is only a
//!   warning), the moved-path note for the first cloud turn, then, under one store lock, the
//!   re-check of busy, HEAD and the last row, the Host record link, and only then the location
//!   with `moved_at_row` = the row count read at that instant. The link is written BEFORE the
//!   location, otherwise a session with a Cloud location and no Host record reads as a lost
//!   workspace.
//!
//! Until the flip the local session is untouched, usable and Local. A failure leaves it that
//! way; Retry continues at the failed step; a flip that finds HEAD or the last row changed
//! marks the move stale (Retry would fail again) and the user cancels and starts over.
//!
//! # Cancel
//!
//! [`CloudSessions::cancel_move`] deletes what the move created, best effort: the Host session
//! (`sessions.delete`), the Host worktree (the Host has no RPC for it, so a `git worktree
//! remove` runs through the backend, only for a worktree the move itself created), the
//! uploads, the staging folder and the moved-path note, and the workspace when the move
//! created it AND the user chose so. A branch the move created is only reported
//! ([`CancelReport::created_branch`]). Failures are listed in [`CancelReport::warnings`].
use super::backend::{CloudBackendKind, CloudRef};
use super::docker_config::DockerProjectConfig;
use super::move_out;
use super::{CloudSessions, CodespaceChoice, Target};
use crate::remote::uploads::{self, UploadOptions};
use crate::store::{CloudLocation, RemoteProject, RowAnchor, SessionLocation, Store};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// Store setting prefix of a move in progress: `cloud-move:<session id>`.
pub const SETTING_PREFIX: &str = "cloud-move:";
/// Store setting prefix of the moved-path note: `cloud-move-note:<session id>`.
pub const NOTE_PREFIX: &str = "cloud-move-note:";
/// Returned when `cancel` stopped a run. The record is kept so the move can be resumed.
pub const MOVE_ABORTED: &str = "The move to the cloud was cancelled.";
/// Why a move is refused while a turn runs.
pub const BUSY: &str =
    "A turn is running in this conversation. Wait for it to finish, then continue in the cloud.";
/// Why a flip is refused when the local session moved on after the snapshot.
pub const CHANGED_AFTER_START: &str =
    "Your local session changed after the move started. Cancel this move and move again.";
const ALREADY_MOVING: &str = "This conversation is already being moved to the cloud.";
const NO_MOVE: &str = "No move to the cloud is in progress for this conversation.";
const STAGING: &str = "cloud-move";
/// Capabilities the Host must announce (`environment.describe`) before a move uploads anything.
const CAPABILITIES: [&str; 3] = ["workspace.applySnapshot", "sessions.import", "uploads"];

// ------------------------------------------------------------------------ data

/// One step of a move, in execution order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MoveStep {
    Snapshot,
    Push,
    Workspace,
    Upload,
    RestoreCode,
    RestoreConversation,
    Flip,
}

impl MoveStep {
    pub const ALL: [MoveStep; 7] = [
        MoveStep::Snapshot,
        MoveStep::Push,
        MoveStep::Workspace,
        MoveStep::Upload,
        MoveStep::RestoreCode,
        MoveStep::RestoreConversation,
        MoveStep::Flip,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Snapshot => "Snapshot",
            Self::Push => "Push",
            Self::Workspace => "Workspace",
            Self::Upload => "Upload",
            Self::RestoreCode => "Restore code",
            Self::RestoreConversation => "Restore conversation",
            Self::Flip => "Finish",
        }
    }
}

/// Where the session goes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MoveTarget {
    /// A new codespace on the pushed branch.
    NewCodespace { machine: Option<String> },
    /// A new Docker sandbox; its `base_ref` is replaced by the pushed branch.
    NewSandbox { config: DockerProjectConfig },
    /// An existing instance that already runs a session of this project.
    Existing {
        backend: CloudBackendKind,
        name: String,
    },
}

/// How the conversation travels.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MoveKind {
    /// The Claude transcript moves and the cloud resumes the same session id.
    Resume,
    /// A summary and a new Claude session id. `reason` is shown to the user.
    Summary { reason: String },
}

/// The workspace of a move, as far as it is known.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceInfo {
    pub backend: CloudBackendKind,
    pub name: String,
    /// This move created the instance (a cancel may offer to delete it).
    pub created: bool,
    /// Pending session of the start that is still running or not yet cleaned up.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_session: Option<i64>,
    /// Known once the instance is connected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<RemoteProject>,
}

impl WorkspaceInfo {
    fn cloud(&self) -> CloudRef {
        CloudRef {
            kind: self.backend,
            name: self.name.clone(),
        }
    }
}

/// Everything known about one move. Persisted in the store (`cloud-move:<session id>`) from
/// [`CloudSessions::begin_move`] until the flip or a cancel. It holds no secret.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MoveRecord {
    pub session_id: i64,
    pub target: MoveTarget,
    pub kind: MoveKind,
    /// Claude config home of the session's account, when the caller named one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_home: Option<String>,
    /// The session's project folder.
    pub project_cwd: String,
    /// The folder the session works in (a session worktree, else the project folder).
    pub work_cwd: String,
    pub title: String,
    pub provider: String,
    /// The local Claude session id, when the session has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_id: Option<String>,
    pub started_at: u64,
    pub done: Vec<MoveStep>,
    pub current: Option<MoveStep>,
    pub failed_at: Option<MoveStep>,
    pub error: Option<String>,
    /// HEAD or the last row changed after the snapshot: Retry cannot succeed, cancel instead.
    #[serde(default)]
    pub stale: bool,
    // Snapshot.
    #[serde(default)]
    pub head: String,
    #[serde(default)]
    pub tree: String,
    #[serde(default)]
    pub branch: String,
    #[serde(default)]
    pub needs_push: bool,
    #[serde(default)]
    pub creates_branch: bool,
    #[serde(default)]
    pub has_patch: bool,
    #[serde(default)]
    pub patch_bytes: u64,
    #[serde(default)]
    pub transcript_bytes: u64,
    /// Warnings for the dialog: LFS, dirty submodules, ignored files that stay local.
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(default)]
    pub anchor: RowAnchor,
    /// The newest Claude Code version that wrote the transcript.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_version: Option<String>,
    // Push.
    #[serde(default)]
    pub pushed: bool,
    // Workspace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<WorkspaceInfo>,
    // Upload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch_upload: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_upload: Option<String>,
    /// An interrupted upload that can continue: (file name, upload id).
    #[serde(default)]
    pub partial_uploads: Vec<(String, String)>,
    // Restore.
    /// The Host working copy that holds the code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_cwd: Option<String>,
    /// `applySnapshot` created this worktree (a cancel removes it).
    #[serde(default)]
    pub created_worktree: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_session_id: Option<String>,
    /// The Claude id the cloud session uses; differs from `claude_id` only after a fork.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forked_from: Option<String>,
    /// Things the user must be told: a fork, a failed Claude update.
    #[serde(default)]
    pub notices: Vec<String>,
    /// Id of the Host `create` command of the summary path, so a retry lands on one session.
    #[serde(default)]
    pub create_command: String,
}

impl MoveRecord {
    pub fn cloud(&self) -> Option<CloudRef> {
        self.workspace.as_ref().map(WorkspaceInfo::cloud)
    }
    pub fn is_failed(&self) -> bool {
        self.failed_at.is_some()
    }
    fn fail(&mut self, step: MoveStep, error: String) {
        self.failed_at = Some(step);
        self.current = None;
        self.error = Some(error);
    }
}

/// What [`CloudSessions::run_move`] reports, in order.
#[derive(Clone, Debug)]
pub enum MoveEvent {
    /// The record changed (a step started or finished).
    Updated(MoveRecord),
    /// A progress line.
    Log(String),
    /// Something the user must be told (a warning, a fork).
    Notice(String),
    /// The move finished; the location is Cloud.
    Done(MoveOutcome),
    /// The move failed; the record (with `failed_at` and `error`) is persisted.
    Failed(MoveRecord),
}

/// The result of a finished move.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MoveOutcome {
    pub session_id: i64,
    pub cloud: CloudRef,
    pub host_session_id: String,
    /// The Claude id the cloud uses; `None` on the summary path (the Host picks a new one).
    pub provider_session_id: Option<String>,
    pub moved_at_row: usize,
    pub forked: bool,
    pub notices: Vec<String>,
}

/// What the user chose on Cancel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CancelChoice {
    /// Delete the workspace if the move created it.
    pub delete_workspace: bool,
}

/// What a cancel did and what is left for the user to decide.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CancelReport {
    /// The branch this move created on origin. It is never deleted automatically.
    pub created_branch: Option<String>,
    /// The workspace this move created and that was kept.
    pub kept_workspace: Option<CloudRef>,
    pub deleted_workspace: bool,
    /// Rollback steps that failed. The local session is Local either way.
    pub warnings: Vec<String>,
}

enum Stop {
    Aborted,
    Failed(String),
    Stale(String),
}
impl From<String> for Stop {
    fn from(error: String) -> Self {
        Stop::Failed(error)
    }
}
impl From<&str> for Stop {
    fn from(error: &str) -> Self {
        Stop::Failed(error.to_string())
    }
}

/// What a request to move names.
#[derive(Clone, Debug)]
pub struct MoveRequest {
    pub session_id: i64,
    pub target: MoveTarget,
    /// Claude config home of the session's account; defaults to the session's profile home.
    pub account_home: Option<PathBuf>,
}

// ------------------------------------------------------------- store accessors

fn key(session: i64) -> String {
    format!("{SETTING_PREFIX}{session}")
}

/// The unfinished move of a session, `None` when there is none (or it cannot be read).
pub fn move_record(store: &Store, session: i64) -> Option<MoveRecord> {
    serde_json::from_str(&store.setting(&key(session)).ok()??).ok()
}

/// Every unfinished move. A listing only: it never resumes anything.
pub fn pending_moves(store: &Store) -> Vec<MoveRecord> {
    store
        .cloud_move_settings()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(_, raw)| serde_json::from_str(&raw).ok())
        .collect()
}

fn save_record(store: &Store, record: &MoveRecord) -> Result<(), String> {
    store.save_setting(
        &key(record.session_id),
        &serde_json::to_string(record).map_err(|e| e.to_string())?,
    )
}

fn clear_record(store: &Store, session: i64) -> Result<(), String> {
    store.save_setting(&key(session), "")
}

/// The one-line note the first cloud turn of a moved session carries (design 4.9 R5), once.
pub fn moved_path_note(store: &Store, session: i64) -> Option<String> {
    store
        .setting(&format!("{NOTE_PREFIX}{session}"))
        .ok()
        .flatten()
        .filter(|note| !note.is_empty())
}

/// Forgets the note after the first cloud turn carried it.
pub fn clear_moved_path_note(store: &Store, session: i64) -> Result<(), String> {
    store.save_setting(&format!("{NOTE_PREFIX}{session}"), "")
}

/// Text for a moved session that appears in logs or errors: no URL credentials.
pub(super) fn scrub(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("://") {
        let (head, tail) = rest.split_at(at + 3);
        out.push_str(head);
        let end = tail
            .find(|c: char| c.is_whitespace() || c == '/')
            .unwrap_or(tail.len());
        match tail[..end].rfind('@') {
            Some(at) => rest = &tail[at + 1..],
            None => rest = tail,
        }
    }
    out.push_str(rest);
    out
}

fn local_busy(session: i64, busy: &dyn Fn() -> bool) -> bool {
    busy()
        || crate::process::agents()
            .iter()
            .any(|a| a.session == session)
}

/// The text of a version such as `2.1.321 (Claude Code)` as numbers.
fn version_parts(text: &str) -> Option<Vec<u64>> {
    let token = text
        .split_whitespace()
        .find(|t| t.starts_with(|c: char| c.is_ascii_digit()) && t.contains('.'))?;
    let parts: Vec<u64> = token
        .split('.')
        .map_while(|part| {
            let digits: String = part.chars().take_while(char::is_ascii_digit).collect();
            digits.parse().ok()
        })
        .collect();
    (!parts.is_empty()).then_some(parts)
}

/// The newest Claude Code `version` among the entries of a transcript.
fn newest_version(jsonl: &str) -> Option<String> {
    jsonl
        .lines()
        .filter(|line| line.contains("\"version\""))
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|entry| entry["version"].as_str().map(str::to_owned))
        .filter_map(|v| version_parts(&v).map(|parts| (parts, v)))
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, v)| v)
}

/// `remote` is older than `local`; an unreadable version is never "older".
pub(super) fn is_older(remote: &str, local: &str) -> bool {
    match (version_parts(remote), version_parts(local)) {
        (Some(remote), Some(local)) => remote < local,
        _ => false,
    }
}

fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

// ---------------------------------------------------------------------- service

/// Test seam: runs between writing the Host link and writing the location.
#[cfg(test)]
pub(super) type FlipHook = Box<dyn Fn(&Store) -> Result<(), String> + Send + Sync>;

impl CloudSessions {
    fn staging_dir(&self, session: i64) -> Result<PathBuf, String> {
        self.store()?
            .thread_dir(session)?
            .map(|dir| dir.join(STAGING))
            .ok_or_else(|| "This session store has no folder for the move".to_string())
    }

    /// Step 0: checks the session and saves the [`MoveRecord`]. Nothing else changes: no
    /// git command runs, nothing is pushed or created. `busy` tells whether the GUI runs a
    /// turn of this session; the process registry is asked too.
    pub fn begin_move(
        &self,
        request: MoveRequest,
        busy: &dyn Fn() -> bool,
    ) -> Result<MoveRecord, String> {
        let store = self.store()?;
        let id = request.session_id;
        if move_record(&store, id).is_some() {
            return Err(ALREADY_MOVING.into());
        }
        if store.remote_session(id)?.is_some() {
            return Err("This conversation already lives in the cloud.".into());
        }
        if store.cloud_location(id)?.is_cloud() {
            return Err("This conversation is already in the cloud.".into());
        }
        if local_busy(id, busy) {
            return Err(BUSY.into());
        }
        let session = store
            .sessions()?
            .into_iter()
            .find(|s| s.id == id)
            .ok_or("Session not found")?;
        let settings = store.agent_settings(id)?.unwrap_or_default();
        let work_cwd = settings
            .worktree_cwd
            .clone()
            .filter(|cwd| !cwd.is_empty() && !settings.worktree_removed)
            .filter(|cwd| !crate::files::remote::is_remote(Path::new(cwd)))
            .filter(|cwd| Path::new(cwd).is_dir())
            .unwrap_or_else(|| session.cwd.clone());
        if crate::files::remote::is_remote(Path::new(&session.cwd)) {
            return Err("A conversation on another machine cannot be moved from here.".into());
        }
        let kind = if session.provider == super::HARNESS && session.provider_id.is_some() {
            MoveKind::Resume
        } else if session.provider == super::HARNESS {
            MoveKind::Summary {
                reason: "This conversation has no Claude session to resume yet; the cloud gets its messages as a summary.".into(),
            }
        } else {
            MoveKind::Summary {
                reason: "The cloud runs Claude. It gets a summary of this conversation and a new Claude session.".into(),
            }
        };
        let record = MoveRecord {
            session_id: id,
            target: request.target,
            kind,
            account_home: request
                .account_home
                .map(|home| home.to_string_lossy().into_owned()),
            project_cwd: session.cwd.clone(),
            work_cwd,
            title: session.title.clone(),
            provider: session.provider.clone(),
            claude_id: session.provider_id.clone(),
            started_at: self.now(),
            done: vec![],
            current: None,
            failed_at: None,
            error: None,
            stale: false,
            head: String::new(),
            tree: String::new(),
            branch: String::new(),
            needs_push: false,
            creates_branch: false,
            has_patch: false,
            patch_bytes: 0,
            transcript_bytes: 0,
            warnings: vec![],
            anchor: RowAnchor::default(),
            local_version: None,
            pushed: false,
            workspace: None,
            patch_upload: None,
            transcript_upload: None,
            partial_uploads: vec![],
            host_cwd: None,
            created_worktree: false,
            host_session_id: None,
            provider_session_id: None,
            forked_from: None,
            notices: vec![],
            create_command: format!("move-{id}-{}", uuid::Uuid::new_v4()),
        };
        save_record(&store, &record)?;
        Ok(record)
    }

    /// `busy` is called while the store lock is held at the flip, so it must not take that lock.
    ///
    /// Runs the move from the first step that is not done. BLOCKS; see the module docs. May
    /// wake a stopped workspace, so call it only for a user action. `Err` is a failure (the
    /// record keeps the step and the text, and `events` ended with `Failed`), a refusal while
    /// busy, or [`MOVE_ABORTED`] (neither event; the record is kept).
    pub fn run_move(
        &self,
        session_id: i64,
        busy: &dyn Fn() -> bool,
        events: &mut dyn FnMut(MoveEvent),
        cancel: &AtomicBool,
    ) -> Result<MoveOutcome, String> {
        let mut record = move_record(&*self.store()?, session_id).ok_or(NO_MOVE)?;
        if record.stale {
            return Err(CHANGED_AFTER_START.into());
        }
        if local_busy(session_id, busy) {
            return Err(BUSY.into());
        }
        record.failed_at = None;
        record.error = None;
        self.publish_move(&record, events)?;
        let mut rewound = false;
        'run: loop {
            for step in MoveStep::ALL {
                if record.done.contains(&step) {
                    continue;
                }
                if cancel.load(Ordering::Relaxed) {
                    return Err(MOVE_ABORTED.into());
                }
                record.current = Some(step);
                self.publish_move(&record, events)?;
                match self.move_step(step, &mut record, busy, events, cancel) {
                    Ok(()) => {
                        record.done.push(step);
                        record.current = None;
                        if step == MoveStep::Flip {
                            break 'run;
                        }
                        self.publish_move(&record, events)?;
                    }
                    Err(Stop::Aborted) => {
                        record.current = None;
                        let _ = save_record(&*self.store()?, &record);
                        return Err(MOVE_ABORTED.into());
                    }
                    Err(Stop::Failed(error)) => {
                        let error = scrub(&error);
                        // A committed upload expires after an hour idle: upload again.
                        if !rewound
                            && matches!(step, MoveStep::RestoreCode | MoveStep::RestoreConversation)
                            && (error.contains("Upload not found")
                                || error.contains("Upload is not committed"))
                        {
                            rewound = true;
                            record.patch_upload = None;
                            record.transcript_upload = None;
                            record
                                .done
                                .retain(|s| !matches!(s, MoveStep::Upload | MoveStep::RestoreCode));
                            continue 'run;
                        }
                        return Err(self.fail_move(record, step, error, false, events));
                    }
                    Err(Stop::Stale(error)) => {
                        return Err(self.fail_move(record, step, error, true, events));
                    }
                }
            }
            break;
        }
        let outcome = self.finish_move(&record)?;
        events(MoveEvent::Done(outcome.clone()));
        Ok(outcome)
    }

    fn publish_move(
        &self,
        record: &MoveRecord,
        events: &mut dyn FnMut(MoveEvent),
    ) -> Result<(), String> {
        save_record(&*self.store()?, record)?;
        events(MoveEvent::Updated(record.clone()));
        Ok(())
    }

    fn fail_move(
        &self,
        mut record: MoveRecord,
        step: MoveStep,
        error: String,
        stale: bool,
        events: &mut dyn FnMut(MoveEvent),
    ) -> String {
        record.fail(step, error.clone());
        record.stale = stale;
        if let Ok(store) = self.store() {
            let _ = save_record(&store, &record);
        }
        events(MoveEvent::Failed(record));
        error
    }

    fn move_step(
        &self,
        step: MoveStep,
        record: &mut MoveRecord,
        busy: &dyn Fn() -> bool,
        events: &mut dyn FnMut(MoveEvent),
        cancel: &AtomicBool,
    ) -> Result<(), Stop> {
        match step {
            MoveStep::Snapshot => self.step_snapshot(record, events),
            MoveStep::Push => self.step_push(record, events),
            MoveStep::Workspace => self.step_workspace(record, events, cancel),
            MoveStep::Upload => self.step_upload(record, events, cancel),
            MoveStep::RestoreCode => self.step_restore_code(record, events),
            MoveStep::RestoreConversation => self.step_restore_conversation(record, events),
            MoveStep::Flip => self.step_flip(record, busy, events),
        }
    }

    // ------------------------------------------------------------- 1 snapshot

    fn step_snapshot(
        &self,
        record: &mut MoveRecord,
        events: &mut dyn FnMut(MoveEvent),
    ) -> Result<(), Stop> {
        let staging = self.staging_dir(record.session_id)?;
        let work = PathBuf::from(&record.work_cwd);
        events(MoveEvent::Log("Reading the local git state".into()));
        move_out::fetch_origin(&work)?;
        let target = move_out::push_target(&work)?;
        let snapshot = move_out::snapshot(&work, &staging)?;
        record.head = snapshot.head.clone();
        record.tree = snapshot.tree.clone();
        record.branch = target.branch;
        record.needs_push = target.needs_push;
        record.creates_branch = target.creates_branch;
        record.has_patch = snapshot.patch_path.is_some();
        record.patch_bytes = snapshot.sizes.patch;
        let mut warnings = vec![];
        if snapshot.warnings.lfs {
            warnings.push("This repository uses Git LFS. The push may not carry the LFS files; check that they are on the LFS server.".to_string());
        }
        for submodule in &snapshot.warnings.dirty_submodules {
            warnings.push(format!(
                "The submodule {submodule} has changes that do not travel."
            ));
        }
        if !snapshot.ignored.is_empty() {
            let names: Vec<&str> = snapshot
                .ignored
                .iter()
                .take(8)
                .map(|(name, _)| name.as_str())
                .collect();
            let more = snapshot.ignored.len().saturating_sub(names.len());
            warnings.push(format!(
                "Ignored files stay on this computer: {}{}",
                names.join(", "),
                if more > 0 {
                    format!(" and {more} more")
                } else {
                    String::new()
                }
            ));
        }
        record.warnings = warnings;

        if matches!(record.kind, MoveKind::Resume) {
            let id = record.claude_id.clone().unwrap_or_default();
            let home = match &record.account_home {
                Some(home) => PathBuf::from(home),
                None => {
                    let settings = self
                        .store()?
                        .agent_settings(record.session_id)?
                        .unwrap_or_default();
                    settings
                        .account
                        .home(crate::provider::Kind::Claude)
                        .unwrap_or_else(|_| self.claude_dir.clone())
                }
            };
            match move_out::collect_transcript(&home, &work, &id, &staging) {
                Ok(transcript) => {
                    record.transcript_bytes = transcript.total_bytes;
                    let text = std::fs::read_to_string(&transcript.jsonl)
                        .map_err(|e| format!("Cannot read the conversation: {e}"))?;
                    record.local_version = newest_version(&text);
                }
                // A conversation that is too large, or that Claude no longer has, is moved as a
                // summary with a new id (design 4.6), and the user is told why.
                Err(error)
                    if error.starts_with("The conversation is")
                        || error.starts_with("Could not find the Claude conversation") =>
                {
                    record.kind = MoveKind::Summary {
                        reason: format!(
                            "{error}. The cloud gets a summary and a new Claude session."
                        ),
                    };
                }
                Err(error) => return Err(error.into()),
            }
        }
        if let MoveKind::Summary { reason } = &record.kind {
            record.warnings.insert(0, reason.clone());
        }
        record.anchor = self.store()?.row_anchor(record.session_id)?;
        for warning in record.warnings.clone() {
            events(MoveEvent::Notice(warning));
        }
        Ok(())
    }

    // ------------------------------------------------------------------ 2 push

    fn step_push(
        &self,
        record: &mut MoveRecord,
        events: &mut dyn FnMut(MoveEvent),
    ) -> Result<(), Stop> {
        if !record.needs_push {
            events(MoveEvent::Log(format!(
                "Nothing to push: origin already has this commit on {}",
                record.branch
            )));
            return Ok(());
        }
        let work = PathBuf::from(&record.work_cwd);
        if move_out::origin_branch_tip(&work, &record.branch)?.as_deref()
            == Some(record.head.as_str())
        {
            record.pushed = true;
            return Ok(());
        }
        if move_out::may_be_default_branch(&work, &record.branch)? {
            return Err(Stop::Failed(format!(
                "Refusing to push to {}: it may be the default branch of origin.",
                record.branch
            )));
        }
        events(MoveEvent::Log(format!(
            "Pushing to origin/{}",
            record.branch
        )));
        crate::scm::push_commit_to_branch(
            &work,
            "origin",
            &record.head,
            &record.branch,
            record.creates_branch,
        )?;
        if move_out::origin_branch_tip(&work, &record.branch)?.as_deref()
            != Some(record.head.as_str())
        {
            return Err(Stop::Failed(
                "The push finished but origin does not have the commit.".into(),
            ));
        }
        record.pushed = true;
        Ok(())
    }

    // ------------------------------------------------------------- 3 workspace

    fn step_workspace(
        &self,
        record: &mut MoveRecord,
        events: &mut dyn FnMut(MoveEvent),
        cancel: &AtomicBool,
    ) -> Result<(), Stop> {
        if record.workspace.is_none() {
            let workspace = match record.target.clone() {
                MoveTarget::NewCodespace { machine } => {
                    let (session, start) = self.begin_start(
                        &record.project_cwd,
                        Target {
                            codespace: CodespaceChoice::New,
                            branch: Some(record.branch.clone()),
                            machine,
                        },
                        super::HARNESS,
                    )?;
                    WorkspaceInfo {
                        backend: CloudBackendKind::Codespaces,
                        name: start.codespace,
                        created: true,
                        start_session: Some(session.id),
                        project: None,
                    }
                }
                MoveTarget::NewSandbox { mut config } => {
                    config.base_ref = record.branch.clone();
                    let (session, start) = self.begin_sandbox_start(
                        &record.project_cwd,
                        &config,
                        &record.branch,
                        super::HARNESS,
                    )?;
                    WorkspaceInfo {
                        backend: CloudBackendKind::DockerSandbox,
                        name: start.sandbox.unwrap_or_default(),
                        created: true,
                        start_session: Some(session.id),
                        project: None,
                    }
                }
                MoveTarget::Existing { backend, name } => WorkspaceInfo {
                    backend,
                    name,
                    created: false,
                    start_session: None,
                    project: None,
                },
            };
            record.workspace = Some(workspace);
            // Saved before the start runs: a crash must still know what was created.
            save_record(&*self.store()?, record)?;
        }
        let workspace = record
            .workspace
            .clone()
            .ok_or("The move has no workspace")?;
        if workspace.project.is_some() {
            return Ok(());
        }
        let project = if let Some(start) = workspace.start_session {
            self.finish_start(start, events, cancel)?
        } else {
            self.existing_project(&workspace.cloud(), &record.project_cwd, cancel, events)?
        };
        if let Some(info) = record.workspace.as_mut() {
            info.project = Some(project);
        }
        save_record(&*self.store()?, record)?;
        // The blank session the start made is plumbing: the moved session is the one row.
        if let Some(start) = workspace.start_session {
            let store = self.store()?;
            let _ = store.forget_cloud_session(start);
            if let Some(info) = record.workspace.as_mut() {
                info.start_session = None;
            }
            save_record(&store, record)?;
        }
        Ok(())
    }

    /// Runs (or resumes) the start of `start_session` and returns the project it opened.
    fn finish_start(
        &self,
        start_session: i64,
        events: &mut dyn FnMut(MoveEvent),
        cancel: &AtomicBool,
    ) -> Result<RemoteProject, Stop> {
        let running = super::start_record(&*self.store()?, start_session).is_some();
        if running {
            let result = self.run_start(
                start_session,
                &mut |event| {
                    if let super::StartEvent::Log(line) = event {
                        events(MoveEvent::Log(scrub(&line)));
                    }
                },
                cancel,
            );
            match result {
                Ok(_) => {}
                Err(error) if error == super::START_ABORTED => return Err(Stop::Aborted),
                Err(error) => return Err(Stop::Failed(error)),
            }
        }
        // The start is ready (now, or on an earlier run that did not save its result).
        let session = self
            .store()?
            .remote_session(start_session)?
            .ok_or("The workspace start disappeared. Cancel the move and start again.")?;
        if crate::store::is_pending_cloud_environment(&session.project.environment_id) {
            return Err(Stop::Failed(
                "The workspace is not ready yet. Retry when it has started.".into(),
            ));
        }
        Ok(session.project)
    }

    /// The remote project of this local project on an instance that already runs one.
    fn existing_project(
        &self,
        cloud: &CloudRef,
        project_cwd: &str,
        cancel: &AtomicBool,
        events: &mut dyn FnMut(MoveEvent),
    ) -> Result<RemoteProject, Stop> {
        let project = self
            .store()?
            .remote_projects()?
            .into_values()
            .find(|p| {
                p.cloud().as_ref() == Some(cloud)
                    && p.local_project.as_deref() == Some(project_cwd)
                    && !crate::store::is_pending_cloud_environment(&p.environment_id)
            })
            .ok_or_else(|| {
                format!(
                    "No session of this project runs in {} {}.",
                    cloud.noun(),
                    cloud.name
                )
            })?;
        if cloud.kind == CloudBackendKind::Codespaces {
            if let Some(machine) = self
                .host
                .machines()?
                .into_iter()
                .find(|m| m.environment_id == project.environment_id)
            {
                // A user action: waking a stopped codespace is expected here.
                self.host
                    .ensure(&machine.id, cancel, &mut |line| {
                        events(MoveEvent::Log(scrub(line)))
                    })
                    .map_err(Stop::Failed)?;
            }
        }
        Ok(project)
    }

    fn workspace_of(record: &MoveRecord) -> Result<(WorkspaceInfo, RemoteProject), String> {
        let workspace = record
            .workspace
            .clone()
            .ok_or("The move has no workspace")?;
        let project = workspace
            .project
            .clone()
            .ok_or("The move has no workspace project")?;
        Ok((workspace, project))
    }

    fn machine_of(&self, project: &RemoteProject) -> Result<String, String> {
        self.host
            .machines()?
            .into_iter()
            .find(|m| m.environment_id == project.environment_id)
            .map(|m| m.id)
            .ok_or_else(|| {
                "The cloud workspace is not connected. Reconnect it, then Retry.".to_string()
            })
    }

    fn rpc(&self, record: &MoveRecord, method: &str, params: Value) -> Result<Value, String> {
        let (_, project) = Self::workspace_of(record)?;
        let machine = self.machine_of(&project)?;
        self.host.request(&machine, method, params)
    }

    // --------------------------------------------------------------- 4 upload

    fn step_upload(
        &self,
        record: &mut MoveRecord,
        events: &mut dyn FnMut(MoveEvent),
        cancel: &AtomicBool,
    ) -> Result<(), Stop> {
        let (_, project) = Self::workspace_of(record)?;
        let machine = self.machine_of(&project)?;
        let info = self.host.host_info(&machine)?;
        for capability in CAPABILITIES {
            if !info.supports(capability) {
                return Err(Stop::Failed(format!(
                    "The Host in the cloud workspace is too old for this ({capability} is missing). Update it in Settings, then Retry."
                )));
            }
        }
        let staging = self.staging_dir(record.session_id)?;
        let mut files: Vec<(&'static str, String, PathBuf)> = vec![];
        if record.has_patch && record.patch_upload.is_none() {
            files.push((
                "patch",
                "snapshot.patch".into(),
                staging.join("uncommitted.patch"),
            ));
        }
        if matches!(record.kind, MoveKind::Resume) && record.transcript_upload.is_none() {
            let id = record.claude_id.clone().unwrap_or_default();
            files.push((
                "transcript",
                format!("{id}.jsonl"),
                transcript_file(&staging, &id)?,
            ));
        }
        for (what, name, path) in files {
            if !path.is_file() {
                return Err(Stop::Failed(format!(
                    "The staged {what} is missing. Cancel the move and start again."
                )));
            }
            events(MoveEvent::Log(format!("Uploading the {what}")));
            let resume = record
                .partial_uploads
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, id)| id.clone());
            let mut request =
                |method: &str, params: Value| self.host.request(&machine, method, params);
            let result = uploads::upload_file(
                &mut request,
                &path,
                &name,
                UploadOptions {
                    resume_id: resume,
                    progress: None,
                    cancel: Some(cancel),
                },
            );
            match result {
                Ok(done) => {
                    record.partial_uploads.retain(|(n, _)| *n != name);
                    if what == "patch" {
                        record.patch_upload = Some(done.upload_id);
                    } else {
                        record.transcript_upload = Some(done.upload_id);
                    }
                    save_record(&*self.store()?, record)?;
                }
                Err(error) if error.cancelled => return Err(Stop::Aborted),
                Err(error) => {
                    if let Some(id) = error.upload_id.clone() {
                        record.partial_uploads.retain(|(n, _)| *n != name);
                        record.partial_uploads.push((name.clone(), id));
                        save_record(&*self.store()?, record)?;
                    }
                    return Err(Stop::Failed(error.message));
                }
            }
        }
        Ok(())
    }

    // ----------------------------------------------------------- 5 restore code

    fn step_restore_code(
        &self,
        record: &mut MoveRecord,
        events: &mut dyn FnMut(MoveEvent),
    ) -> Result<(), Stop> {
        let (workspace, project) = Self::workspace_of(record)?;
        events(MoveEvent::Log("Restoring the code in the workspace".into()));
        let mut params = json!({
            "projectId": project.project_id,
            "instance": if workspace.created { "new" } else { "shared" },
            "branch": record.branch,
            "head": record.head,
            "tree": record.tree,
        });
        if let Some(upload) = &record.patch_upload {
            params["patchUploadId"] = json!(upload);
        }
        let result = self.rpc(record, "workspace.applySnapshot", params)?;
        let text = |field: &str| result[field].as_str().unwrap_or_default().to_lowercase();
        if text("tree") != record.tree || text("head") != record.head {
            return Err(Stop::Failed(
                "The restored code does not match the snapshot (the Host reported another tree or commit).".into(),
            ));
        }
        if result["branch"].as_str() != Some(record.branch.as_str()) {
            return Err(Stop::Failed(
                "The Host restored the code on another branch than the pushed one.".into(),
            ));
        }
        let cwd = result["cwd"]
            .as_str()
            .filter(|cwd| !cwd.is_empty())
            .ok_or("The Host did not say where it restored the code")?;
        record.host_cwd = Some(cwd.to_string());
        record.created_worktree = result["createdWorktree"].as_bool().unwrap_or(false);
        Ok(())
    }

    // ------------------------------------------------------ 6 restore conversation

    fn step_restore_conversation(
        &self,
        record: &mut MoveRecord,
        events: &mut dyn FnMut(MoveEvent),
    ) -> Result<(), Stop> {
        let (_, project) = Self::workspace_of(record)?;
        let host_cwd = record
            .host_cwd
            .clone()
            .ok_or("The move has no restored working copy")?;
        let settings = self
            .store()?
            .agent_settings(record.session_id)?
            .unwrap_or_default();
        let configuration = crate::remote::protocol::settings_configuration("claude", &settings);
        let model = configuration["model"]
            .as_str()
            .unwrap_or("claude:default")
            .to_string();
        match record.kind.clone() {
            MoveKind::Resume => {
                let id = record.claude_id.clone().unwrap_or_default();
                self.retire_old_host_sessions(record, &id, events)?;
                let upload = record
                    .transcript_upload
                    .clone()
                    .ok_or("The conversation was not uploaded")?;
                events(MoveEvent::Log("Restoring the conversation".into()));
                let mut from: Vec<String> = vec![];
                for path in [&record.project_cwd, &record.work_cwd] {
                    from.push(path.clone());
                    if let Ok(real) = std::fs::canonicalize(path) {
                        from.push(real.to_string_lossy().into_owned());
                    }
                }
                from.dedup();
                let mut seen = std::collections::HashSet::new();
                from.retain(|p| seen.insert(p.clone()));
                let result = self.rpc(
                    record,
                    "sessions.import",
                    json!({
                        "projectId": project.project_id,
                        "providerSessionId": id,
                        "uploadId": upload,
                        "cwd": host_cwd,
                        "title": if record.title.trim().is_empty() { "Moved session".to_string() } else { record.title.chars().take(200).collect::<String>() },
                        "model": model,
                        "modelSettings": configuration["modelSettings"],
                        "runtimeMode": configuration["runtimeMode"],
                        "fromCwds": from,
                    }),
                )?;
                let host_session = result["sessionId"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .ok_or("The Host did not return the imported session")?
                    .to_string();
                let provider = result["providerSessionId"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .ok_or("The Host did not return the Claude session id")?
                    .to_string();
                match result["decision"].as_str() {
                    Some("installed" | "replaced" | "already_current") if provider == id => {}
                    Some("forked") => {
                        record.forked_from = Some(id.clone());
                        let notice = result["notice"]
                            .as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| super::transcript_sync::fork_notice(&id, &provider));
                        record.notices.push(notice.clone());
                        events(MoveEvent::Notice(notice));
                    }
                    other => {
                        return Err(Stop::Failed(format!(
                            "The Host answered the import with an unexpected decision ({}).",
                            other.unwrap_or("none")
                        )))
                    }
                }
                record.host_session_id = Some(host_session);
                record.provider_session_id = Some(provider);
            }
            MoveKind::Summary { .. } => {
                events(MoveEvent::Log("Creating the cloud session".into()));
                let receipt = self.rpc(
                    record,
                    "commands.dispatch",
                    json!({
                        "type": "create",
                        "commandId": record.create_command,
                        "projectId": project.project_id,
                        "worktreeCwd": host_cwd,
                        "harness": "claude",
                        "model": model,
                        "modelSettings": configuration["modelSettings"],
                        "runtimeMode": configuration["runtimeMode"],
                    }),
                )?;
                let host_session = receipt["sessionId"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .ok_or("The Host did not return the new session")?;
                record.host_session_id = Some(host_session.to_string());
                record.provider_session_id = None;
            }
        }
        save_record(&*self.store()?, record)?;
        Ok(())
    }

    /// Deletes the Host sessions, other than this move's own, that still bind `claude_id`.
    /// `sessions.import` refuses a conversation an earlier Host session with turns still
    /// binds, and returning to local only forgets that session in the app. A running one is
    /// never deleted: the cloud is still writing that conversation.
    fn retire_old_host_sessions(
        &self,
        record: &MoveRecord,
        claude_id: &str,
        events: &mut dyn FnMut(MoveEvent),
    ) -> Result<(), Stop> {
        let projects = self.rpc(record, "projects.list", json!({}))?;
        for project in projects.as_array().into_iter().flatten() {
            let Some(project_id) = project["id"].as_str() else {
                continue;
            };
            let sessions = self.rpc(record, "sessions.list", json!({"projectId": project_id}))?;
            for session in sessions.as_array().into_iter().flatten() {
                let bound = session["harness"].as_str() == Some("claude")
                    && session["providerSessionId"].as_str() == Some(claude_id);
                let Some(id) = session["id"].as_str().filter(|_| bound) else {
                    continue;
                };
                if record.host_session_id.as_deref() == Some(id) {
                    continue;
                }
                if session["status"].as_str() == Some("running") {
                    return Err(Stop::Failed(
                        "A turn is running in the cloud for this conversation. Wait for it to finish, then Retry.".into(),
                    ));
                }
                events(MoveEvent::Log(
                    "Removing the earlier cloud copy of this conversation".into(),
                ));
                self.rpc(
                    record,
                    "sessions.delete",
                    json!({"sessionId": id, "projectId": project_id}),
                )?;
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------------ 7 flip

    fn step_flip(
        &self,
        record: &mut MoveRecord,
        busy: &dyn Fn() -> bool,
        events: &mut dyn FnMut(MoveEvent),
    ) -> Result<(), Stop> {
        let (workspace, project) = Self::workspace_of(record)?;
        let host_session = record
            .host_session_id
            .clone()
            .ok_or("The move has no cloud session")?;
        let host_cwd = record.host_cwd.clone().unwrap_or_default();
        if matches!(record.kind, MoveKind::Resume) {
            self.check_claude_version(record, &workspace, events);
        }
        let note = format!(
            "This session moved from {} to {}; paths in earlier messages refer to the old location.",
            record.work_cwd, host_cwd
        );
        let store = self.store()?;
        let session = record.session_id;
        // A crash after the location was written: the flip is already done.
        if let SessionLocation::Cloud(location) = store.cloud_location(session)? {
            if location.host_session_id == host_session {
                return Ok(());
            }
            return Err(Stop::Failed(
                "This conversation is already in the cloud.".into(),
            ));
        }
        store.save_setting(&format!("{NOTE_PREFIX}{session}"), &note)?;
        // Everything below runs under the store lock, so no local write lands in between.
        if local_busy(session, busy) {
            return Err(Stop::Failed(BUSY.into()));
        }
        if move_out::head_commit(Path::new(&record.work_cwd)).as_deref()
            != Some(record.head.as_str())
        {
            return Err(Stop::Stale(CHANGED_AFTER_START.into()));
        }
        if store.row_anchor(session)? != record.anchor {
            return Err(Stop::Stale(CHANGED_AFTER_START.into()));
        }
        let provider = record.provider_session_id.clone();
        let flat = json!({
            "id": host_session,
            "title": record.title,
            "harness": "claude",
            "providerSessionId": provider,
            "status": "idle",
            "updatedAt": self.now(),
        });
        let remote = store.register_remote_summary(&project, &flat)?;
        // The link first: a Cloud location without a Host record reads as a lost workspace.
        store.link_remote_session(remote.id, Some(session))?;
        #[cfg(test)]
        if let Some(hook) = self.flip_hook.lock().map_err(|e| e.to_string())?.as_ref() {
            hook(&store)?;
        }
        let moved_at_row = store.row_anchor(session)?.count;
        let location = CloudLocation {
            cloud: workspace.cloud(),
            environment_id: project.environment_id.clone(),
            project_id: project.project_id.clone(),
            host_session_id: host_session,
            remote_cwd: host_cwd,
            local_cwd: record.project_cwd.clone(),
            moved_at_row,
            pushed_branch: Some(record.branch.clone()),
        };
        store.set_cloud_location(session, &SessionLocation::Cloud(location))?;
        if matches!(record.kind, MoveKind::Summary { .. }) {
            self.store_handoff(&store, record, events);
        }
        Ok(())
    }

    /// The summary for the first cloud turn: the same brief a provider switch writes.
    fn store_handoff(&self, store: &Store, record: &MoveRecord, events: &mut dyn FnMut(MoveEvent)) {
        let result = (|| -> Result<(), String> {
            let rows = store.load(record.session_id)?.rows;
            let mut settings = store.agent_settings(record.session_id)?.unwrap_or_default();
            settings.handoff = Some(crate::agent::Handoff {
                from: record.provider.clone(),
                brief: crate::response::handoff(&rows),
            });
            store.save_agent_settings(record.session_id, &settings)
        })();
        if let Err(error) = result {
            events(MoveEvent::Notice(format!(
                "The summary of this conversation could not be saved ({error}); the cloud session starts without it."
            )));
        }
    }

    /// R4: the cloud must run a Claude at least as new as the one that wrote the transcript.
    /// Tolerant: any failure is a warning, never an error.
    fn check_claude_version(
        &self,
        record: &mut MoveRecord,
        workspace: &WorkspaceInfo,
        events: &mut dyn FnMut(MoveEvent),
    ) {
        let Some(local) = record.local_version.clone() else {
            return;
        };
        let Ok(backend) = self.backend(workspace.backend) else {
            return;
        };
        const PATH: &str = "export PATH=\"$HOME/.local/bin:$HOME/.claude/local:$PATH\"\n";
        let read =
            || backend.run_script(&workspace.name, &format!("{PATH}claude --version"), false);
        fn warn(record: &mut MoveRecord, events: &mut dyn FnMut(MoveEvent), text: String) {
            record.notices.push(text.clone());
            events(MoveEvent::Notice(text));
        }
        let remote = match read() {
            Ok(out) => out.trim().to_string(),
            Err(error) => {
                warn(
                    record,
                    events,
                    format!(
                        "Could not read the Claude version in the cloud ({}).",
                        scrub(&error)
                    ),
                );
                return;
            }
        };
        if !is_older(&remote, &local) {
            return;
        }
        events(MoveEvent::Log(format!(
            "Updating Claude in the cloud ({remote} is older than {local})"
        )));
        let updated =
            backend.run_script(&workspace.name, &format!("{PATH}claude update 2>&1"), true);
        let now = read().unwrap_or_default();
        if updated.is_err() || is_older(now.trim(), &local) {
            warn(
                record,
                events,
                format!(
                    "Claude in the cloud ({}) is older than the one that wrote this conversation ({local}). If it cannot resume, run `claude update` there.",
                    if now.trim().is_empty() { remote } else { now.trim().to_string() }
                ),
            );
        }
    }

    /// After the flip: the move is over. Cleanup is best effort.
    fn finish_move(&self, record: &MoveRecord) -> Result<MoveOutcome, String> {
        let uploads: Vec<String> = [&record.patch_upload, &record.transcript_upload]
            .into_iter()
            .flatten()
            .cloned()
            .collect();
        for upload in uploads {
            let _ = self.rpc(record, "uploads.discard", json!({"uploadId": upload}));
        }
        if let Ok(dir) = self.staging_dir(record.session_id) {
            let _ = std::fs::remove_dir_all(dir);
        }
        let store = self.store()?;
        clear_record(&store, record.session_id)?;
        let location = store
            .cloud_location(record.session_id)?
            .cloud()
            .cloned()
            .ok_or("The location was not saved")?;
        Ok(MoveOutcome {
            session_id: record.session_id,
            cloud: location.cloud,
            host_session_id: location.host_session_id,
            provider_session_id: record.provider_session_id.clone(),
            moved_at_row: location.moved_at_row,
            forked: record.forked_from.is_some(),
            notices: record.notices.clone(),
        })
    }

    // ------------------------------------------------------------------ cancel

    /// Rolls the move back and removes its record, see the module docs. The local session is
    /// not touched. Never deletes a pushed branch. Call it when no `run_move` is running.
    pub fn cancel_move(
        &self,
        session_id: i64,
        choice: CancelChoice,
    ) -> Result<CancelReport, String> {
        let record = move_record(&*self.store()?, session_id).ok_or(NO_MOVE)?;
        let mut report = CancelReport {
            created_branch: (record.creates_branch && record.pushed).then(|| record.branch.clone()),
            ..Default::default()
        };
        let mut warnings: Vec<String> = vec![];
        let created = record.workspace.as_ref().filter(|w| w.created);
        let deleting = choice.delete_workspace && created.is_some();
        if record
            .workspace
            .as_ref()
            .is_some_and(|w| w.project.is_some())
        {
            for upload in [&record.patch_upload, &record.transcript_upload]
                .into_iter()
                .flatten()
            {
                if let Err(e) = self.rpc(&record, "uploads.discard", json!({"uploadId": upload})) {
                    warnings.push(format!("Could not discard an upload: {}", scrub(&e)));
                }
            }
            if let (Some(host_session), Some(project)) = (
                &record.host_session_id,
                record.workspace.as_ref().and_then(|w| w.project.as_ref()),
            ) {
                if let Err(e) = self.rpc(
                    &record,
                    "sessions.delete",
                    json!({"sessionId": host_session, "projectId": project.project_id}),
                ) {
                    warnings.push(format!("Could not delete the cloud session: {}", scrub(&e)));
                }
            }
            // A deleted workspace takes its working copy with it.
            if record.created_worktree && !deleting {
                if let Err(e) = self.remove_host_worktree(&record) {
                    warnings.push(format!(
                        "Could not remove the cloud working copy: {}",
                        scrub(&e)
                    ));
                }
            }
        }
        {
            let store = self.store()?;
            // A flip that failed after the Host record was linked leaves it behind. While the
            // session is still Local the record backs nothing, so it goes with the move.
            if !store.cloud_location(session_id)?.is_cloud() {
                let mut stale: Vec<i64> = vec![];
                if let Some((id, _)) = store.linked_remote_session(session_id)? {
                    stale.push(id);
                }
                if let (Some(host), Some(project)) = (
                    &record.host_session_id,
                    record.workspace.as_ref().and_then(|w| w.project.as_ref()),
                ) {
                    for remote in store.cloud_sessions(&record.project_cwd)? {
                        if remote.session_id.as_deref() == Some(host.as_str())
                            && remote.project.environment_id == project.environment_id
                        {
                            stale.push(remote.summary.id);
                        }
                    }
                }
                stale.sort_unstable();
                stale.dedup();
                for id in stale {
                    let _ = store.link_remote_session(id, None);
                    let _ = store.forget_cloud_session(id);
                }
            }
            if let Some(start) = record.workspace.as_ref().and_then(|w| w.start_session) {
                // A start that never finished: its record and blank session go with the move.
                let _ = super::startup::remove_start_record(&store, start);
                let _ = store.forget_cloud_session(start);
            }
            let _ = clear_moved_path_note(&store, session_id);
        }
        if let Ok(dir) = self.staging_dir(session_id) {
            let _ = std::fs::remove_dir_all(dir);
        }
        if let Some(workspace) = created {
            if deleting {
                match self.delete_cloud_outcome(&workspace.cloud()) {
                    Ok(_) => report.deleted_workspace = true,
                    Err(e) => {
                        warnings.push(format!("Could not delete the workspace: {}", scrub(&e)));
                        report.kept_workspace = Some(workspace.cloud());
                    }
                }
            } else {
                report.kept_workspace = Some(workspace.cloud());
            }
        }
        clear_record(&*self.store()?, session_id)?;
        report.warnings = warnings;
        Ok(report)
    }

    /// Removes the worktree `applySnapshot` created. The Host has no RPC for it, so a plain
    /// `git worktree remove` runs through the backend, on exactly that path.
    fn remove_host_worktree(&self, record: &MoveRecord) -> Result<(), String> {
        let (workspace, project) = Self::workspace_of(record)?;
        let path = record.host_cwd.clone().ok_or("No working copy to remove")?;
        if path == project.cwd || !path.starts_with('/') || path.contains('\n') {
            return Err("Refusing to remove the project folder".into());
        }
        let script = format!(
            "git -C {root} worktree remove --force -- {path} && git -C {root} worktree prune",
            root = shell_quote(&project.cwd),
            path = shell_quote(&path),
        );
        self.backend(workspace.backend)?
            .run_script(&workspace.name, &script, false)
            .map(|_| ())
    }
}

/// `<staging>/transcript/projects/<dir>/<id>.jsonl`, as [`move_out::collect_transcript`] lays
/// it out.
fn transcript_file(staging: &Path, id: &str) -> Result<PathBuf, String> {
    let projects = staging.join("transcript").join("projects");
    for entry in std::fs::read_dir(&projects)
        .map_err(|_| "The staged conversation is missing. Cancel the move and start again.")?
        .flatten()
    {
        let candidate = entry.path().join(format!("{id}.jsonl"));
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err("The staged conversation is missing. Cancel the move and start again.".into())
}
