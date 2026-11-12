//! Deleting the cloud instance after "Continue locally" succeeded (design 4.2.3).
//!
//! The return is one-way: once the code is placed, the transcript installed and the location
//! flipped (or the cloud-born session adopted), the instance has nothing the user needs, unless
//! something else uses it or it holds files that did not come back. Deleting is destructive, so
//! every check fails closed: when a question cannot be answered, the instance is kept.
//!
//! * [`CloudSessions::continue_locally`] ARMS a durable record per instance
//!   (`cloud-return-cleanup:<cloud key>`) after a full conversion, never after an error and never
//!   after a return that lost the workspace. Arming only reads (the Host is still connected from
//!   the conversion); it can never turn the successful return into an error.
//! * [`CloudSessions::run_return_cleanup`] BLOCKS (worker thread). It re-checks everything fresh,
//!   stores that an attempt started, then calls [`CloudSessions::delete_cloud_outcome`]. It never
//!   wakes a stopped instance: a Host that cannot be asked is answered from the recorded check.
//! * A failed or interrupted delete keeps the record with a message; the user chooses Retry or
//!   Keep ([`CloudSessions::dismiss_return_cleanup`]). Nothing retries by itself.
use super::*;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

const PREFIX: &str = "cloud-return-cleanup:";

/// How long after the return a recorded "no other session" answer is still trusted when the
/// Host cannot be asked any more (the instance went to sleep). Older than this, the answer is
/// treated as unknown and the user must have the instance running for a fresh check.
pub const CLEAR_TRUST_MS: u64 = 15 * 60 * 1000;

/// Refusal of a second delete of the same instance in this process.
pub const ALREADY_DELETING: &str = "already deleting";

/// `CloudRef` has no serde support of its own.
mod cloud_wire {
    use super::{CloudBackendKind, CloudRef};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[derive(Serialize, Deserialize)]
    struct Wire {
        backend: CloudBackendKind,
        name: String,
    }
    pub fn serialize<S: Serializer>(cloud: &CloudRef, s: S) -> Result<S::Ok, S::Error> {
        Wire {
            backend: cloud.kind,
            name: cloud.name.clone(),
        }
        .serialize(s)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<CloudRef, D::Error> {
        let wire = Wire::deserialize(d)?;
        Ok(CloudRef {
            kind: wire.backend,
            name: wire.name,
        })
    }
}

/// What the Host of the instance said about other sessions when the return was armed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HostCheck {
    /// No other live session.
    Clear,
    /// Other sessions (titles, else ids) that were not saved as local copies.
    Others(Vec<String>),
    /// The Host could not be asked (the error).
    Unknown(String),
}

/// A delete that follows a successful return, kept until it is done or the user keeps the
/// instance.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReturnCleanup {
    /// The local session that came back.
    pub local_id: i64,
    #[serde(with = "cloud_wire")]
    pub cloud: CloudRef,
    pub environment_id: String,
    /// The Host session the returned conversation ran as.
    pub host_session_id: Option<String>,
    pub title: String,
    pub created_at: u64,
    pub host_check: HostCheck,
    /// Set just before the delete is called; still set when the app closed during it.
    pub attempt_started_at: Option<u64>,
    pub last_error: Option<String>,
    /// The instance is known to be gone already; only the bookkeeping is left.
    #[serde(default)]
    pub gone: bool,
}

impl ReturnCleanup {
    /// The settings key suffix and the key the GUI passes to
    /// [`CloudSessions::run_return_cleanup`].
    pub fn key(&self) -> String {
        self.cloud.key()
    }
    /// The user must choose Retry or Keep.
    pub fn needs_retry(&self) -> bool {
        self.last_error.is_some()
    }
    /// Never attempted and without a problem: the GUI runs it by itself.
    pub fn auto_run(&self) -> bool {
        self.last_error.is_none() && self.attempt_started_at.is_none()
    }
}

/// How [`CloudSessions::run_return_cleanup`] ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CleanupOutcome {
    /// No such record.
    Nothing,
    /// The instance was already gone; its bindings were forgotten.
    AlreadyGone,
    /// Deleted. `warning` is set when disconnecting its machine failed.
    Deleted { warning: Option<String> },
    /// Not deleted, on purpose: the instance is still running and the record is gone.
    Kept { reason: String },
}

/// What the caller of the conversion knows about the instance before it is forgotten.
pub(super) struct ReturnSource {
    pub(super) cloud: CloudRef,
    pub(super) environment_id: String,
    pub(super) host_session_id: Option<String>,
}

/// Holds the in-process lock of one instance.
struct Running<'a> {
    set: &'a Mutex<HashSet<String>>,
    key: String,
}
impl Drop for Running<'_> {
    fn drop(&mut self) {
        if let Ok(mut set) = self.set.lock() {
            set.remove(&self.key);
        }
    }
}

fn setting_key(key: &str) -> String {
    format!("{PREFIX}{key}")
}

impl CloudSessions {
    /// Every return whose instance is still to be deleted or decided on, oldest first.
    pub fn return_cleanups(&self) -> Vec<ReturnCleanup> {
        let mut all: Vec<ReturnCleanup> = self
            .store()
            .and_then(|s| s.settings_with_key_prefix(PREFIX))
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(_, raw)| serde_json::from_str(&raw).ok())
            .collect();
        all.sort_by_key(|r| (r.created_at, r.local_id));
        all
    }

    /// The records of the returned session `local_id`, oldest first. A session can have
    /// several: one per instance it came back from.
    ///
    /// Limitation: a session id can be reused after the session was deleted, and a session has
    /// no creation time to tell the two apart, so a record that outlived its session could be
    /// attributed to a new one with the same id. Callers also match the instance.
    pub fn return_cleanups_for_session(&self, local_id: i64) -> Vec<ReturnCleanup> {
        self.return_cleanups()
            .into_iter()
            .filter(|r| r.local_id == local_id)
            .collect()
    }

    /// The pending record of session `local_id` for exactly `cloud`.
    pub fn return_cleanup_for(&self, local_id: i64, cloud: &CloudRef) -> Option<ReturnCleanup> {
        self.return_cleanups_for_session(local_id)
            .into_iter()
            .find(|r| r.cloud == *cloud)
    }

    /// The record the session menu acts on: the newest one that needs a Retry, else the
    /// newest. Deterministic when a session has several.
    pub fn return_cleanup_for_session(&self, local_id: i64) -> Option<ReturnCleanup> {
        let all = self.return_cleanups_for_session(local_id);
        all.iter()
            .rev()
            .find(|r| r.needs_retry())
            .or(all.last())
            .cloned()
    }

    /// The user keeps the instance: the record goes, nothing is deleted.
    pub fn dismiss_return_cleanup(&self, cloud_key: &str) -> Result<(), String> {
        self.remove_cleanup(cloud_key)
    }

    /// The delete could not start (another action works on the instance): the record becomes a
    /// Retry with `reason`, so it does not run silently at the next start.
    /// `false` when the record could not be saved as a Retry: it then stays as it was.
    pub fn defer_return_cleanup(&self, cloud_key: &str, reason: &str) -> bool {
        let Ok(Some(mut record)) = self.load_cleanup(cloud_key) else {
            return false;
        };
        record.attempt_started_at = None;
        record.last_error = Some(reason.to_string());
        self.save_cleanup(&record).is_ok()
    }

    /// For app start: a delete that was in progress when the app closed is not run again by
    /// itself. It becomes a retry the user chooses.
    pub fn mark_interrupted_return_cleanups(&self) {
        let running = self
            .cleaning
            .lock()
            .map(|set| set.clone())
            .unwrap_or_default();
        for mut record in self.return_cleanups() {
            if record.attempt_started_at.is_none() || running.contains(&record.key()) {
                continue;
            }
            record.attempt_started_at = None;
            record.last_error = Some(format!(
                "The app closed while deleting the {} {}; choose Retry to finish.",
                record.cloud.noun(),
                record.cloud.name
            ));
            let _ = self.save_cleanup(&record);
        }
    }

    /// Refuses to put new work on `cloud` while its return cleanup is deleting it.
    pub(super) fn refuse_while_cleaning(&self, cloud: &CloudRef) -> Result<(), String> {
        let busy = self
            .cleaning
            .lock()
            .map(|set| set.contains(&cloud.key()))
            .unwrap_or(true);
        if busy {
            return Err(format!(
                "The {} {} is being deleted because a session came back from it. Start on a new {} instead.",
                cloud.noun(),
                cloud.name,
                cloud.noun()
            ));
        }
        Ok(())
    }

    fn load_cleanup(&self, key: &str) -> Result<Option<ReturnCleanup>, String> {
        Ok(self
            .store()?
            .setting(&setting_key(key))?
            .filter(|raw| !raw.is_empty())
            .and_then(|raw| serde_json::from_str(&raw).ok()))
    }

    fn save_cleanup(&self, record: &ReturnCleanup) -> Result<(), String> {
        let raw = serde_json::to_string(record).map_err(|e| e.to_string())?;
        self.store()?
            .save_setting(&setting_key(&record.key()), &raw)
    }

    fn remove_cleanup(&self, key: &str) -> Result<(), String> {
        self.store()?.save_setting(&setting_key(key), "")
    }

    /// Whether the record stored under the key is still the one `record` was loaded from. A
    /// different return of the same instance may have replaced it meanwhile; that newer
    /// record is never touched by an older run.
    fn is_stored(&self, record: &ReturnCleanup) -> bool {
        self.load_cleanup(&record.key())
            .ok()
            .flatten()
            .is_some_and(|stored| {
                stored.local_id == record.local_id && stored.created_at == record.created_at
            })
    }

    fn save_if_stored(&self, record: &ReturnCleanup) -> Result<(), String> {
        if self.is_stored(record) {
            self.save_cleanup(record)
        } else {
            Ok(())
        }
    }

    fn remove_if_stored(&self, record: &ReturnCleanup) -> Result<(), String> {
        if self.is_stored(record) {
            self.remove_cleanup(&record.key())
        } else {
            Ok(())
        }
    }

    /// Whether a session, a start or a move of the app still refers to `cloud`.
    fn instance_listed(&self, cloud: &CloudRef) -> bool {
        self.cleanup_blockers(cloud, false)
            .map(|reasons| !reasons.is_empty())
            .unwrap_or(true)
    }

    /// What to tell the user about a kept instance: the path to delete it that exists in the
    /// app for this case, else plainly that the app has none. `local_id` is the session that
    /// came back from it.
    pub fn kept_instance_hint(&self, cloud: &CloudRef, local_id: Option<i64>) -> String {
        if self.instance_listed(cloud) {
            return "It is still listed under the sessions that use it; delete it from the session menu or the archive dialog of one of them when they are done.".into();
        }
        // A cloud-born codespace leaves an origin on its local copy, and that session's menu
        // offers "Delete codespace <name>", which also warns about unsaved files.
        if cloud.kind == CloudBackendKind::Codespaces {
            let origin = local_id.and_then(|id| {
                let raw = self
                    .store()
                    .ok()?
                    .setting(&origin_key(CloudBackendKind::Codespaces, id))
                    .ok()??;
                let value: Value = serde_json::from_str(&raw).ok()?;
                (value["codespace"].as_str() == Some(cloud.name.as_str())).then_some(())
            });
            if origin.is_some() {
                return format!(
                    "Use \"Delete codespace {}\" in the menu of the session that came back; it also warns about files that are not on this computer.",
                    cloud.name
                );
            }
        }
        match cloud.kind {
            CloudBackendKind::Codespaces => format!(
                "Monoptah no longer lists it, so it stays until you delete it yourself, for example with `gh codespace delete -c {}`.",
                cloud.name
            ),
            CloudBackendKind::DockerSandbox => format!(
                "Monoptah no longer lists it, so it stays until you delete the sandbox {} yourself with Docker's sandbox tools (sbx).",
                cloud.name
            ),
        }
    }

    /// Records that the instance of a successful return is to be deleted, or says why it is
    /// kept. Called by `continue_locally` after a full conversion; reads and one write, and
    /// the result of the return is never changed except for a warning.
    pub(super) fn arm_return_cleanup(&self, source: ReturnSource, converted: &mut Converted) {
        let ReturnSource {
            cloud,
            environment_id,
            host_session_id,
        } = source;
        let what = format!("{} {}", cloud.noun(), cloud.name);
        let lines = unsaved_lines(std::slice::from_ref(converted));
        if !lines.is_empty() {
            if let Err(e) = self.record_unsaved_cloud(&cloud, &lines) {
                converted.warnings.push(format!(
                    "The {what} was kept, but what it holds that is not on this computer could not be recorded ({e})."
                ));
            }
            converted.warnings.push(format!(
                "The {what} was kept because it holds files that are not on this computer. {} {}",
                lines.join(" "),
                self.kept_instance_hint(&cloud, Some(converted.local_id))
            ));
            return;
        }
        // A Claude conversation whose raw transcript was not copied: the cloud copy may be the
        // only one left.
        if converted.transcript_lost {
            let why = match &converted.resume {
                Resume::Missing(why) => why.clone(),
                _ => "it was not copied".into(),
            };
            converted.warnings.push(format!(
                "The {what} was kept because the Claude transcript did not come back ({why}). {}",
                self.kept_instance_hint(&cloud, Some(converted.local_id))
            ));
            return;
        }
        // Another return from the same instance is deciding right now: keep this decision as
        // a Retry so it is not lost.
        let busy = self
            .cleaning
            .lock()
            .map(|set| set.contains(&cloud.key()))
            .unwrap_or(true);
        let host_check = match self.other_host_sessions(&environment_id, host_session_id.as_deref())
        {
            Ok(others) if others.is_empty() => HostCheck::Clear,
            Ok(others) => HostCheck::Others(others),
            Err(e) => HostCheck::Unknown(e),
        };
        let record = ReturnCleanup {
            local_id: converted.local_id,
            cloud,
            environment_id,
            host_session_id,
            title: converted.title.clone(),
            created_at: (self.clock)(),
            host_check,
            attempt_started_at: None,
            last_error: busy
                .then(|| format!("another return was deciding on the {what}; choose Retry")),
            gone: false,
        };
        if busy {
            converted.warnings.push(format!(
                "Another return was deciding on the {what}; delete it later with Retry in the session menu."
            ));
        }
        if let Err(e) = self.save_cleanup(&record) {
            converted.warnings.push(format!(
                "The {what} was kept because deleting it could not be scheduled ({e}). {}",
                self.kept_instance_hint(&record.cloud, Some(record.local_id))
            ));
        }
    }

    /// The live Host sessions of the instance behind `environment_id` other than `own` that
    /// were not saved as local copies, labelled by title (else id). Asks the Host only: a
    /// stopped or unreachable instance is an `Err`, never woken.
    fn other_host_sessions(
        &self,
        environment_id: &str,
        own: Option<&str>,
    ) -> Result<Vec<String>, String> {
        let machine = self
            .host
            .machines()?
            .into_iter()
            .find(|m| m.environment_id == environment_id)
            .ok_or("The workspace is not connected.")?;
        let mut listed: Vec<(String, String)> = vec![];
        let projects = self.host.request(&machine.id, "projects.list", json!({}))?;
        for project in projects.as_array().into_iter().flatten() {
            let Some(project_id) = project["id"].as_str() else {
                continue;
            };
            let sessions = self.host.request(
                &machine.id,
                "sessions.list",
                json!({"projectId": project_id}),
            )?;
            for session in sessions.as_array().into_iter().flatten() {
                let Some(id) = session["id"].as_str() else {
                    continue;
                };
                let title = session["title"].as_str().unwrap_or("").trim();
                let label = if title.is_empty() { id } else { title };
                listed.push((id.to_string(), label.to_string()));
            }
        }
        let store = self.store()?;
        let mut others = vec![];
        for (id, label) in listed {
            if Some(id.as_str()) == own || store.is_cloud_converted(environment_id, &id)? {
                continue;
            }
            others.push(label);
        }
        Ok(others)
    }

    /// Why the instance `cloud` must not be deleted now, from fresh reads of what the app
    /// knows. Empty when nothing is in the way. `with_unsaved` adds the files that never came
    /// back (a delete is blocked by them, the listing question is not).
    fn cleanup_blockers(
        &self,
        cloud: &CloudRef,
        with_unsaved: bool,
    ) -> Result<Vec<String>, String> {
        let mut reasons = vec![];
        {
            let store = self.store()?;
            let mut sessions = vec![];
            for id in store.cloud_session_ids(cloud)? {
                if let Some(remote) = store.remote_session(id)? {
                    if !store.host_record_is_hidden(&remote) {
                        sessions.push(quoted(&remote.summary.title));
                    }
                }
            }
            if !sessions.is_empty() {
                reasons.push(format!("it still holds {}", sessions.join(", ")));
            }
            let located: Vec<i64> = store
                .cloud_located_raw()?
                .into_iter()
                .filter(|(_, location)| location.as_ref().is_none_or(|l| l.cloud == *cloud))
                .map(|(id, _)| id)
                .collect();
            if !located.is_empty() {
                let titles: Vec<String> = store
                    .sessions()?
                    .into_iter()
                    .filter(|s| located.contains(&s.id))
                    .map(|s| quoted(&s.title))
                    .collect();
                reasons.push(format!(
                    "{} {} running in it",
                    if titles.is_empty() {
                        "a session is".to_string()
                    } else {
                        titles.join(", ")
                    },
                    if titles.len() > 1 { "are" } else { "is" }
                ));
            }
            if pending_starts(&store)
                .iter()
                .any(|s| s.cloud().as_ref() == Some(cloud))
            {
                reasons.push("a session is still starting in it".into());
            }
            // A move names its target before it has a workspace (`workspace` is set at the
            // Workspace step), so the target counts too.
            if move_session::pending_moves(&store).iter().any(|m| {
                m.workspace
                    .as_ref()
                    .is_some_and(|w| w.backend == cloud.kind && w.name == cloud.name)
                    || matches!(
                        &m.target,
                        move_session::MoveTarget::Existing { backend, name }
                            if *backend == cloud.kind && *name == cloud.name
                    )
            }) {
                reasons.push("a move to it is not finished".into());
            }
        }
        if with_unsaved {
            let unsaved = self.unsaved_for_cloud(cloud);
            if !unsaved.is_empty() {
                reasons.push(format!(
                    "it holds files that are not on this computer ({})",
                    unsaved.join(" ")
                ));
            }
        }
        Ok(reasons)
    }

    /// Deletes the instance of the return recorded under `cloud_key` when that is still safe.
    /// BLOCKS; call from a worker thread. See the module docs for the guards. `Err` keeps the
    /// record (with `last_error`) for a Retry; `Kept` removes it.
    pub fn run_return_cleanup(&self, cloud_key: &str) -> Result<CleanupOutcome, String> {
        {
            let mut set = self.cleaning.lock().map_err(|_| "internal: lock")?;
            if !set.insert(cloud_key.to_string()) {
                return Err(ALREADY_DELETING.into());
            }
        }
        let _running = Running {
            set: &self.cleaning,
            key: cloud_key.to_string(),
        };
        let Some(mut record) = self.load_cleanup(cloud_key)? else {
            return Ok(CleanupOutcome::Nothing);
        };
        let cloud = record.cloud.clone();
        let what = format!("{} {}", cloud.noun(), cloud.name);
        // Every error from here on keeps the record with the message, so Retry shows.
        let fail = |this: &Self, record: &mut ReturnCleanup, error: String| {
            record.attempt_started_at = None;
            record.last_error = Some(error.clone());
            let _ = this.save_if_stored(record);
            Err(error)
        };
        // Not deleting: the record goes. If it cannot be removed it is marked as kept so it
        // never runs by itself later.
        let keep = |this: &Self, record: &mut ReturnCleanup, text: String| {
            if this.remove_if_stored(record).is_err() {
                record.attempt_started_at = None;
                record.last_error = Some(text.clone());
                let _ = this.save_if_stored(record);
            }
            Ok(CleanupOutcome::Kept { reason: text })
        };
        let kept_text = |this: &Self, record: &ReturnCleanup, reasons: Vec<String>| {
            format!(
                "The {what} was kept because {}. {}",
                reasons.join("; "),
                this.kept_instance_hint(&cloud, Some(record.local_id))
            )
        };

        let backend = match self.backend(cloud.kind) {
            Ok(backend) => backend,
            Err(e) => return fail(self, &mut record, e),
        };
        // A read-only look first: an instance that is gone needs no question to the Host.
        let seen = backend.exists(&cloud.name);
        let gone = matches!(seen, Ok(false));
        if let (true, Err(e)) = (record.gone, &seen) {
            // It was gone before and cannot be confirmed now (offline): no question to the
            // Host, just another try.
            return fail(
                self,
                &mut record,
                format!(
                    "could not confirm that the {what} is gone ({e}); choose Finish cleanup again"
                ),
            );
        }
        if gone {
            record.gone = true;
            match self.cleanup_blockers(&cloud, false) {
                Ok(reasons) if reasons.is_empty() => {}
                Ok(reasons) => {
                    let text = format!(
                        "The {what} no longer exists, so nothing was deleted. Sessions that still refer to it: {}.",
                        reasons.join("; ")
                    );
                    return keep(self, &mut record, text);
                }
                Err(e) => {
                    return fail(
                        self,
                        &mut record,
                        format!("could not check whether the {what} is still in use ({e})"),
                    )
                }
            }
            return match self.delete_cloud_outcome(&cloud) {
                Ok(_) => {
                    let _ = self.remove_if_stored(&record);
                    Ok(CleanupOutcome::AlreadyGone)
                }
                Err(e) => fail(self, &mut record, e),
            };
        }

        let mut reasons = match self.cleanup_blockers(&cloud, true) {
            Ok(reasons) => reasons,
            Err(e) => {
                return fail(
                    self,
                    &mut record,
                    format!("could not check whether the {what} is still in use ({e})"),
                )
            }
        };
        // Fresh answer when the Host can be asked; else the one recorded at the return, while
        // it is recent. An unreachable (stopped) instance is never woken to ask.
        let others = match self
            .other_host_sessions(&record.environment_id, record.host_session_id.as_deref())
        {
            Ok(others) => others,
            Err(live) => match &record.host_check {
                HostCheck::Clear
                    if (self.clock)().saturating_sub(record.created_at) <= CLEAR_TRUST_MS =>
                {
                    vec![]
                }
                HostCheck::Clear => {
                    return fail(
                        self,
                        &mut record,
                        format!(
                            "the {what} must be running so Monoptah can check that no other session uses it ({live}); start it, then choose Retry"
                        ),
                    )
                }
                HostCheck::Others(others) => others.clone(),
                HostCheck::Unknown(_) => {
                    return fail(
                        self,
                        &mut record,
                        format!(
                            "could not check whether other sessions use the {what} ({live}); start it, then choose Retry"
                        ),
                    )
                }
            },
        };
        if !others.is_empty() {
            reasons.push(format!(
                "other sessions use it ({})",
                others
                    .iter()
                    .map(|o| quoted(o))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !reasons.is_empty() {
            let text = kept_text(self, &record, reasons);
            return keep(self, &mut record, text);
        }
        // The last look before the delete: work the app began meanwhile (a move or an attach
        // onto this instance) is refused while the instance is in the cleaning set, and work
        // that began before is visible here.
        match self.cleanup_blockers(&cloud, true) {
            Ok(reasons) if reasons.is_empty() => {}
            Ok(reasons) => {
                let text = kept_text(self, &record, reasons);
                return keep(self, &mut record, text);
            }
            Err(e) => {
                return fail(
                    self,
                    &mut record,
                    format!("could not check whether the {what} is still in use ({e})"),
                )
            }
        }
        record.attempt_started_at = Some((self.clock)());
        record.last_error = None;
        if let Err(e) = self.save_if_stored(&record) {
            return fail(self, &mut record, e);
        }
        match self.delete_cloud_outcome(&cloud) {
            Ok(outcome) => {
                let _ = self.remove_if_stored(&record);
                Ok(CleanupOutcome::Deleted {
                    warning: match outcome {
                        DeleteOutcome::Deleted => None,
                        DeleteOutcome::DeletedWithWarning(w) => {
                            Some(format!("disconnecting its machine failed: {w}"))
                        }
                    },
                })
            }
            // The backend delete may have worked and only the bookkeeping failed: when the
            // instance is confirmed gone, say so and keep a record for a Retry.
            Err(e) if matches!(backend.exists(&cloud.name), Ok(false)) => {
                let warning = format!(
                    "Monoptah could not finish forgetting it ({e}); choose Retry to clean up"
                );
                record.attempt_started_at = None;
                record.last_error = Some(warning.clone());
                record.gone = true;
                let _ = self.save_if_stored(&record);
                Ok(CleanupOutcome::Deleted {
                    warning: Some(warning),
                })
            }
            Err(e) => fail(self, &mut record, e),
        }
    }
}

fn quoted(title: &str) -> String {
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    format!("\u{201c}{title}\u{201d}")
}
