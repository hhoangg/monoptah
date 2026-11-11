//! Where a local session lives: on this machine, or in a cloud workspace (design 4.2.1).
//!
//! The location is a per-session setting, `cloud-location:{id}`. While it is `Cloud`, the
//! session's runtime is the remote one and the only writer of its rows is the remote
//! row append in `cloud_rows.rs` (single writer, design 4.9 R2).
use super::Store;
use crate::cloud_sessions::{CloudBackendKind, CloudRef};
use serde::{Deserialize, Serialize};

/// A cloud workspace as stored: `CloudRef` has no serde support of its own.
mod cloud_ref {
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

/// The cloud side of a moved session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudLocation {
    #[serde(with = "cloud_ref")]
    pub cloud: CloudRef,
    pub environment_id: String,
    // Every field added later needs `#[serde(default)]`: an older stored value must keep
    // parsing after an upgrade, or its session would read as "unreadable cloud location".
    #[serde(default)]
    pub project_id: String,
    pub host_session_id: String,
    #[serde(default)]
    pub remote_cwd: String,
    #[serde(default)]
    pub local_cwd: String,
    /// Rows before this index were written locally; the row append never rewrites them.
    #[serde(default)]
    pub moved_at_row: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pushed_branch: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum SessionLocation {
    #[default]
    Local,
    Cloud(CloudLocation),
}

impl SessionLocation {
    pub fn is_cloud(&self) -> bool {
        matches!(self, Self::Cloud(_))
    }
    pub fn cloud(&self) -> Option<&CloudLocation> {
        match self {
            Self::Cloud(location) => Some(location),
            Self::Local => None,
        }
    }
}

/// Why the LOCAL runtime refuses to start a turn for a session that lives in the cloud
/// (design 4.9 R2: one writer at a time).
pub const LOCAL_RUN_REFUSED: &str =
    "This conversation is in the cloud. It cannot run on this computer until it is brought back.";

/// Why the local terminal (TUI) refuses to open a session that lives in the cloud.
pub const LOCAL_TUI_REFUSED: &str = "This conversation is in the cloud. Its local terminal is closed so two Claude processes never share one session. Bring it back to this computer first.";

/// Why Reset to local is refused while the cloud is working on the conversation.
pub const HOST_TURN_RUNNING: &str =
    "A turn is running in the cloud for this conversation. Wait for it to finish, then reset.";

/// Shown on a Cloud session whose workspace was deleted: its Host record went with it.
pub const WORKSPACE_LOST: &str = "The cloud workspace of this conversation was deleted. Messages written there after the last sync are lost. Reset to local to continue on this computer.";

/// Shown on a session whose stored location cannot be read.
pub const UNREADABLE_NOTICE: &str = "The cloud location of this conversation cannot be read, so it is closed for writing. Reset to local to continue on this computer.";

/// Why a session with a stored but unparsable location is refused.
pub const UNREADABLE_LOCATION: &str = "unreadable cloud location";

/// The local tree a move left behind. Kept after the move (the move record is cleared at the
/// flip) until the session returns, so "Continue locally" can tell whether the working copy
/// changed since (design 4.2.3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MovePoint {
    /// The Host session of the move this point belongs to (a reused session id never matches).
    pub host_session_id: String,
    pub head: String,
    pub tree: String,
    /// The folder the session worked in: a session worktree, else the project folder.
    pub work_cwd: String,
    /// The checked out branch ref at the move (`refs/heads/...`); empty when it was detached
    /// or unknown, which makes the in-place return refuse.
    #[serde(default)]
    pub branch: String,
}

impl Store {
    fn move_point_key(session: i64) -> String {
        format!("cloud-move-point:{session}")
    }
    pub fn set_cloud_move_point(&self, session: i64, point: &MovePoint) -> Result<(), String> {
        self.save_setting(
            &Self::move_point_key(session),
            &serde_json::to_string(point).map_err(|e| e.to_string())?,
        )
    }
    /// The recorded move point, only when it belongs to the session's current Cloud location.
    pub fn cloud_move_point(&self, session: i64) -> Option<MovePoint> {
        let SessionLocation::Cloud(location) = self.cloud_location(session).ok()? else {
            return None;
        };
        let raw = self.setting(&Self::move_point_key(session)).ok()??;
        let point: MovePoint = serde_json::from_str(&raw).ok()?;
        (point.host_session_id == location.host_session_id).then_some(point)
    }
    pub fn clear_cloud_move_point(&self, session: i64) -> Result<(), String> {
        self.save_setting(&Self::move_point_key(session), "")
    }
    fn broken_key(session: i64) -> String {
        format!("cloud-continue-broken:{session}")
    }
    /// A return that could not put the local folder back: where the backup of the
    /// uncommitted work is. Kept until the folder is verified or the user dealt with it.
    pub fn cloud_continue_broken(&self, session: i64) -> Option<String> {
        self.setting(&Self::broken_key(session))
            .ok()?
            .filter(|raw| !raw.is_empty())
    }
    /// Where the backup of an interrupted return is, when there is one.
    pub fn continue_backup_path(&self, session: i64) -> Option<String> {
        let raw = self.cloud_continue_broken(session)?;
        serde_json::from_str::<serde_json::Value>(&raw)
            .ok()
            .and_then(|v| v["backup"].as_str().map(str::to_owned))
            .or(Some(String::new()))
    }
    pub fn set_cloud_continue_broken(&self, session: i64, raw: &str) -> Result<(), String> {
        self.save_setting(&Self::broken_key(session), raw)
    }
    fn returned_gate_key(session: i64) -> String {
        format!("gate-before-row:{session}")
    }
    /// Rows before this index ran somewhere else than where the session runs now (it came
    /// back through a new worktree or a new Claude id): rewinding to them is disabled.
    pub fn returned_gate(&self, session: i64) -> Option<usize> {
        self.setting(&Self::returned_gate_key(session))
            .ok()?
            .and_then(|raw| raw.parse().ok())
    }
    /// Raises the gate; it never goes down (a later return must not re-enable turns an
    /// earlier one disabled). `None` is a no-op.
    pub fn set_returned_gate(&self, session: i64, row: Option<usize>) -> Result<(), String> {
        let Some(row) = row else { return Ok(()) };
        let row = row.max(self.returned_gate(session).unwrap_or(0));
        self.save_setting(&Self::returned_gate_key(session), &row.to_string())
    }
    /// The next turn starts a new Claude conversation with a summary of the rows: the Claude
    /// id is cleared (so a stale local file is never resumed), the summary is queued like a
    /// provider switch does, and older turns are gated (their files belong to another id).
    pub fn start_fresh_with_summary(&self, local: i64) -> Result<(), String> {
        let session = self
            .sessions()?
            .into_iter()
            .find(|s| s.id == local)
            .ok_or("The conversation no longer exists")?;
        let rows = self.load(local)?.rows;
        let mut settings = self.agent_settings(local)?.unwrap_or_default();
        settings.handoff = Some(crate::agent::Handoff {
            from: session.provider.clone(),
            brief: crate::response::handoff(&rows),
        });
        settings.revision += 1;
        self.save_agent_settings(local, &settings)?;
        self.reset_provider_id_if(local, session.provider_id.as_deref())?;
        self.set_returned_gate(local, Some(rows.len()))
    }
    fn continue_key(session: i64) -> String {
        format!("cloud-continue:{session}")
    }
    /// Progress of a "Continue locally" that stopped half way (see `continue_local`).
    pub fn cloud_continue_progress(&self, session: i64) -> Option<String> {
        self.setting(&Self::continue_key(session))
            .ok()?
            .filter(|raw| !raw.is_empty())
    }
    pub fn set_cloud_continue_progress(&self, session: i64, raw: &str) -> Result<(), String> {
        self.save_setting(&Self::continue_key(session), raw)
    }
    fn location_key(session: i64) -> String {
        format!("cloud-location:{session}")
    }
    /// The location of a local session. A missing or empty value is `Local`. A value that
    /// is present but unreadable fails closed: guessing Local could start a second writer
    /// on a session that lives in the cloud (design 4.9 R2).
    pub fn cloud_location(&self, session: i64) -> Result<SessionLocation, String> {
        match self
            .setting(&Self::location_key(session))?
            .filter(|raw| !raw.is_empty())
        {
            None => Ok(SessionLocation::Local),
            Some(raw) => serde_json::from_str::<CloudLocation>(&raw)
                .map(SessionLocation::Cloud)
                .map_err(|_| UNREADABLE_LOCATION.to_string()),
        }
    }
    /// Refuses a local turn or a local terminal for a session whose location is Cloud, with
    /// `message`. Fails closed on an unreadable location, like every other guard.
    fn require_local(&self, session: i64, message: &str) -> Result<(), String> {
        if self.cloud_location(session)?.is_cloud() {
            return Err(message.to_string());
        }
        Ok(())
    }
    /// Call before the local runtime starts a turn: nothing is written before it refuses.
    pub fn require_local_run(&self, session: i64) -> Result<(), String> {
        self.require_local(session, LOCAL_RUN_REFUSED)
    }
    /// Call before the local TUI is launched for the session.
    pub fn require_local_tui(&self, session: i64) -> Result<(), String> {
        self.require_local(session, LOCAL_TUI_REFUSED)
    }
    /// Every local session with a stored location value, by session id: `Some` when it reads as
    /// Cloud, `None` when the value is unreadable (such a session fails closed everywhere).
    pub fn cloud_located_raw(&self) -> Result<Vec<(i64, Option<CloudLocation>)>, String> {
        Ok(self
            .settings_with_prefix("cloud-location:")?
            .into_iter()
            .filter(|(_, raw)| !raw.is_empty())
            .map(|(id, raw)| (id, serde_json::from_str(&raw).ok()))
            .collect())
    }
    /// Every local session whose location is Cloud, with that location, by session id.
    /// Unreadable values are skipped here: they have no workspace to show.
    pub fn cloud_located(&self) -> Result<Vec<(i64, CloudLocation)>, String> {
        Ok(self
            .settings_with_prefix("cloud-location:")?
            .into_iter()
            .filter_map(|(id, raw)| Some((id, serde_json::from_str(&raw).ok()?)))
            .collect())
    }
    /// The Cloud-located local sessions of the project folder `project`, newest first. The
    /// sidebar shows these as cloud sessions (design 4.2.1).
    pub fn cloud_located_sessions(
        &self,
        project: &str,
    ) -> Result<Vec<(super::Session, CloudLocation)>, String> {
        let mut located: std::collections::HashMap<i64, CloudLocation> =
            self.cloud_located()?.into_iter().collect();
        if located.is_empty() {
            return Ok(Vec::new());
        }
        Ok(self
            .sessions()?
            .into_iter()
            .filter(|s| s.cwd == project)
            .filter_map(|s| located.remove(&s.id).map(|location| (s, location)))
            .collect())
    }
    /// Whether the Host record `record` only backs a local session that has a cloud location,
    /// and so must not be listed as a session of its own. A link whose local session is back
    /// to Local (or deleted) hides nothing. An unreadable value or a failed read hides it:
    /// a second row for the same conversation is the worse outcome.
    pub fn host_record_is_hidden(&self, record: &super::RemoteSession) -> bool {
        let Some(local) = record.linked_to else {
            return false;
        };
        match self.setting(&Self::location_key(local)) {
            Ok(raw) => raw.is_some_and(|raw| !raw.is_empty()),
            Err(_) => true,
        }
    }
    /// Whether the cloud copy may hold turns that were not copied here: the Host's last known
    /// revision differs from the synced cursor, or either is unknown (an unreachable Host, a
    /// recorded problem, a missing record). The Reset confirmation is stronger then.
    pub fn cloud_may_hold_unsynced(&self, session: i64) -> bool {
        if self.cloud_sync_error(session).ok().flatten().is_some() {
            return true;
        }
        let Ok((_, record)) = self.cloud_host_record(session) else {
            return true;
        };
        let known = record
            .configuration
            .as_ref()
            .and_then(|c| c["revision"].as_u64());
        let synced = self
            .cloud_synced_revision(
                &record.project.environment_id,
                record.session_id.as_deref().unwrap_or(""),
            )
            .ok()
            .flatten();
        match (known, synced) {
            (Some(a), Some(b)) => a != b,
            _ => true,
        }
    }
    /// The user's way back to Local ("Reset to local"), after the confirmation. Refuses while a
    /// Host-side turn runs (it would be a second writer). Unlike the bare reset it also
    /// retires the Host record: it is unlinked, remembered as converted so a poll never
    /// imports it again, and dropped locally, so nothing can send to it while the local
    /// runtime is allowed on the same Claude id. The copy in the workspace itself stays.
    /// It does not reconcile the transcript (F4-5).
    ///
    /// Choice: the local Host record is tombstoned and dropped, not kept as a read-only
    /// converted view. The converted mechanism only marks a Host session as already saved as a
    /// local copy; showing one would need the Host to be reachable and a new view, so it is not
    /// cheap. The data stays in the workspace, and the confirmation says so (stronger when
    /// `cloud_may_hold_unsynced`).
    pub fn leave_cloud(&self, session: i64) -> Result<(), String> {
        self.leave_cloud_inner(session, true)
    }
    /// [`Self::leave_cloud`] for the return that already brought the transcript back
    /// ("Continue locally"): the Claude id stays. Plain Reset does not reconcile the
    /// transcript, so it starts a new Claude conversation with a summary when the cloud may
    /// hold turns the local file lacks (design 4.9 R7, rule D10).
    pub(crate) fn leave_cloud_inner(&self, session: i64, fresh: bool) -> Result<(), String> {
        let backing = self.linked_remote_session(session)?;
        // Only a fresh `running` blocks. A Host waiting for input is not writing, and a flag
        // that is stale or sits next to a recorded problem (unreachable Host, lost workspace)
        // would block Reset exactly when it is the only way out.
        if let Some((_, record)) = &backing {
            if record.running_now() && self.cloud_sync_error(session)?.is_none() {
                return Err(HOST_TURN_RUNNING.to_string());
            }
        }
        if fresh {
            let cloud_rows = match self.cloud_location(session)? {
                SessionLocation::Cloud(location) => {
                    self.load(session)?.rows.len() > location.moved_at_row
                }
                SessionLocation::Local => false,
            };
            if cloud_rows || self.cloud_may_hold_unsynced(session) {
                self.start_fresh_with_summary(session)?;
            }
        }
        self.reset_cloud_location_to_local(session)?;
        if let Some((id, record)) = backing {
            if let Some(host) = record.session_id.as_deref() {
                self.set_cloud_converted(&record.project.environment_id, host, true)?;
            }
            self.forget_cloud_session(id)?;
        }
        Ok(())
    }
    /// Explicit way back to Local when the cloud side is gone (for example the linked Host
    /// record was deleted). Also clears the link and the sync cursor. It does NOT reconcile
    /// the transcript (F4-5 does): the UI asks for an explicit confirmation first, and
    /// nothing else may flip a Cloud session back to Local without that reconciliation.
    pub fn reset_cloud_location_to_local(&self, session: i64) -> Result<(), String> {
        if let Ok(SessionLocation::Cloud(location)) = self.cloud_location(session) {
            self.save_setting(
                &Self::synced_key(&location.environment_id, &location.host_session_id),
                "",
            )?;
        }
        let linked: Vec<i64> = self
            .remote_sessions()?
            .into_iter()
            .filter(|(_, r)| r.linked_to == Some(session))
            .map(|(id, _)| id)
            .collect();
        for id in linked {
            self.link_remote_session(id, None)?;
        }
        let _ = self.save_setting(&Self::sync_error_key(session), "");
        let _ = self.save_setting(&Self::move_point_key(session), "");
        let _ = self.save_setting(&Self::continue_key(session), "");
        self.save_setting(&Self::location_key(session), "")
    }
    /// Writes the Cloud location together with the settings the move needs in ONE transaction:
    /// either both are stored or neither (the settings carry the summary and the Claude-only
    /// model). Only the move may call this.
    pub fn set_cloud_location_with_settings(
        &self,
        session: i64,
        location: &SessionLocation,
        settings: Option<&crate::agent::Settings>,
    ) -> Result<(), String> {
        let Some(settings) = settings else {
            return self.set_cloud_location(session, location);
        };
        if self.shared.is_some() || session < 0 || self.remote_session(session)?.is_some() {
            self.save_agent_settings(session, settings)?;
            return self.set_cloud_location(session, location);
        }
        let raw = match location {
            SessionLocation::Local => String::new(),
            SessionLocation::Cloud(cloud) => {
                serde_json::to_string(cloud).map_err(|e| e.to_string())?
            }
        };
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| e.to_string())?;
        tx.execute(
            super::SAVE_AGENT_SETTINGS,
            rusqlite::params![
                format!("agent:{session}"),
                serde_json::to_string(settings).map_err(|e| e.to_string())?
            ],
        )
        .map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT INTO native_settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            rusqlite::params![Self::location_key(session), raw],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())
    }
    /// Every stored `cloud-move:<session id>` record (the move in progress), by session id.
    pub fn cloud_move_settings(&self) -> Result<Vec<(i64, String)>, String> {
        Ok(self
            .settings_with_prefix("cloud-move:")?
            .into_iter()
            .filter(|(_, raw)| !raw.is_empty())
            .collect())
    }
    /// Only the move (F4-3a) and the return (F4-5) may call this in production.
    pub fn set_cloud_location(
        &self,
        session: i64,
        location: &SessionLocation,
    ) -> Result<(), String> {
        let raw = match location {
            SessionLocation::Local => String::new(),
            SessionLocation::Cloud(cloud) => {
                serde_json::to_string(cloud).map_err(|e| e.to_string())?
            }
        };
        self.save_setting(&Self::location_key(session), &raw)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::path::Path;

    pub(crate) fn cloud_location() -> CloudLocation {
        CloudLocation {
            cloud: CloudRef::codespace("octo-abc"),
            environment_id: "env".into(),
            project_id: "project".into(),
            host_session_id: "host-1".into(),
            remote_cwd: "/workspaces/repo".into(),
            local_cwd: "/Users/me/repo".into(),
            moved_at_row: 2,
            pushed_branch: Some("feature".into()),
        }
    }

    #[test]
    fn location_defaults_to_local_and_flips_both_ways() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let session = store.create("claude", "/Users/me/repo").unwrap();
        assert_eq!(
            store.cloud_location(session.id).unwrap(),
            SessionLocation::Local
        );
        let cloud = SessionLocation::Cloud(cloud_location());
        store.set_cloud_location(session.id, &cloud).unwrap();
        assert_eq!(store.cloud_location(session.id).unwrap(), cloud);
        assert!(store.cloud_location(session.id).unwrap().is_cloud());
        store
            .set_cloud_location(session.id, &SessionLocation::Local)
            .unwrap();
        assert_eq!(
            store.cloud_location(session.id).unwrap(),
            SessionLocation::Local
        );
    }

    #[test]
    fn an_older_stored_location_still_parses() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let session = store.create("claude", "/Users/me/repo").unwrap();
        // Only the identifying fields, plus a field a newer app might have written.
        store
            .save_setting(
                &format!("cloud-location:{}", session.id),
                r#"{"cloud":{"backend":"codespaces","name":"octo"},"environmentId":"env","hostSessionId":"h","futureField":1}"#,
            )
            .unwrap();
        let location = store.cloud_location(session.id).unwrap();
        let cloud = location.cloud().unwrap();
        assert_eq!(
            (cloud.host_session_id.as_str(), cloud.moved_at_row),
            ("h", 0)
        );
    }

    #[test]
    fn location_is_per_session_and_sandbox_refs_round_trip() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let a = store.create("claude", "/Users/me/repo").unwrap();
        let b = store.create("claude", "/Users/me/repo").unwrap();
        let mut location = cloud_location();
        location.cloud = CloudRef::sandbox("box-1");
        store
            .set_cloud_location(a.id, &SessionLocation::Cloud(location.clone()))
            .unwrap();
        assert_eq!(store.cloud_location(a.id).unwrap().cloud(), Some(&location));
        assert_eq!(store.cloud_location(b.id).unwrap(), SessionLocation::Local);
    }

    #[test]
    fn unreadable_location_fails_closed_and_delete_forgets_it() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let session = store.create("claude", "/Users/me/repo").unwrap();
        let key = format!("cloud-location:{}", session.id);
        store.save_setting(&key, "not json").unwrap();
        assert_eq!(
            store.cloud_location(session.id).unwrap_err(),
            UNREADABLE_LOCATION
        );
        let row = crate::transcript::Row {
            role: "user".into(),
            text: "x".into(),
            first: true,
            sent_at: None,
        };
        assert!(store.write_rows(session.id, 0, &[row]).is_err());
        // Missing and empty values are Local.
        store.save_setting(&key, "").unwrap();
        assert_eq!(
            store.cloud_location(session.id).unwrap(),
            SessionLocation::Local
        );
        store
            .set_cloud_location(session.id, &SessionLocation::Cloud(cloud_location()))
            .unwrap();
        store.delete(session.id).unwrap();
        assert_eq!(store.setting(&key).unwrap(), None);
    }

    #[test]
    fn reset_to_local_clears_location_link_and_cursor() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let session = store.create("claude", "/Users/me/repo").unwrap();
        store
            .set_cloud_location(session.id, &SessionLocation::Cloud(cloud_location()))
            .unwrap();
        store.save_setting("cloud-synced:env:host-1", "4").unwrap();
        store.reset_cloud_location_to_local(session.id).unwrap();
        assert_eq!(
            store.cloud_location(session.id).unwrap(),
            SessionLocation::Local
        );
        assert_eq!(store.cloud_synced_revision("env", "host-1").unwrap(), None);
        // An unreadable value can be reset too.
        store
            .save_setting(&format!("cloud-location:{}", session.id), "junk")
            .unwrap();
        store.reset_cloud_location_to_local(session.id).unwrap();
        assert_eq!(
            store.cloud_location(session.id).unwrap(),
            SessionLocation::Local
        );
    }

    // The scenario of the tests below: a local session with 10 rows written before the move,
    // location Cloud with `moved_at_row` 10, and a Host record linked to it.
    fn moved_session(store: &mut Store) -> (i64, i64) {
        let local = store.create("claude", "/Users/me/repo").unwrap();
        let rows: Vec<crate::transcript::Row> = (0..10)
            .map(|i| crate::transcript::Row {
                role: if i % 2 == 0 { "user" } else { "assistant" }.into(),
                text: format!("row {i}"),
                first: i % 2 == 0,
                sent_at: None,
            })
            .collect();
        store.write_rows(local.id, 0, &rows).unwrap();
        let mut location = cloud_location();
        location.moved_at_row = 10;
        store
            .set_cloud_location(local.id, &SessionLocation::Cloud(location))
            .unwrap();
        let project = store
            .remember_remote_project(
                "env",
                &serde_json::json!({"id":"project","cwd":"/host/repo"}),
            )
            .unwrap();
        let host = serde_json::json!({"projectId":"project","revision":1,"status":"idle",
            "updatedAt":5,"session":{"id":"host-1","harness":"claude","title":"T",
            "model":"claude:opus","runtimeMode":"auto","blocks":[]}});
        let remote = store.register_remote_summary(&project, &host).unwrap();
        store
            .link_remote_session(remote.id, Some(local.id))
            .unwrap();
        (local.id, remote.id)
    }

    fn listed(store: &Store) -> Vec<i64> {
        store
            .sessions()
            .unwrap()
            .into_iter()
            .map(|s| s.id)
            .collect()
    }

    #[test]
    fn the_linked_host_record_is_hidden_while_cloud_and_an_unlinked_one_is_not() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let (local, linked) = moved_session(&mut store);
        // A session born in the cloud: a Host record with no link.
        let project = store
            .remember_remote_project(
                "env",
                &serde_json::json!({"id":"project2","cwd":"/host/other"}),
            )
            .unwrap();
        let born = store
            .register_remote_summary(
                &project,
                &serde_json::json!({"projectId":"project2","revision":1,"status":"idle",
                "updatedAt":6,"session":{"id":"host-2","harness":"claude","title":"B",
                "model":"claude:opus","runtimeMode":"auto","blocks":[]}}),
            )
            .unwrap();
        let ids = listed(&store);
        assert!(ids.contains(&local) && ids.contains(&born.id));
        assert!(!ids.contains(&linked), "the linked Host record is plumbing");
        // Back to Local: the link is cleared and the Host record is a session of its own.
        store.reset_cloud_location_to_local(local).unwrap();
        assert!(listed(&store).contains(&linked));
        // A link whose local session has no cloud location hides nothing.
        store.link_remote_session(linked, Some(local)).unwrap();
        assert!(listed(&store).contains(&linked));
    }

    #[test]
    fn located_sessions_are_listed_by_project_with_their_move_point() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let (local, _) = moved_session(&mut store);
        let found = store.cloud_located_sessions("/Users/me/repo").unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0.id, local);
        assert_eq!(found[0].1.moved_at_row, 10);
        assert!(store
            .cloud_located_sessions("/elsewhere")
            .unwrap()
            .is_empty());
        store.reset_cloud_location_to_local(local).unwrap();
        assert!(store
            .cloud_located_sessions("/Users/me/repo")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn the_local_runtime_and_terminal_are_refused_while_cloud() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let plain = store.create("claude", "/Users/me/repo").unwrap();
        store.require_local_run(plain.id).unwrap();
        store.require_local_tui(plain.id).unwrap();
        let (local, _) = moved_session(&mut store);
        assert_eq!(
            store.require_local_run(local).unwrap_err(),
            LOCAL_RUN_REFUSED
        );
        assert_eq!(
            store.require_local_tui(local).unwrap_err(),
            LOCAL_TUI_REFUSED
        );
        assert_eq!(
            crate::tui_launch::refuse_while_cloud(&store, local).unwrap_err(),
            LOCAL_TUI_REFUSED
        );
        // An unreadable location fails closed too.
        store
            .save_setting(&format!("cloud-location:{}", plain.id), "junk")
            .unwrap();
        assert_eq!(
            store.require_local_run(plain.id).unwrap_err(),
            UNREADABLE_LOCATION
        );
        assert!(crate::tui_launch::refuse_while_cloud(&store, plain.id).is_err());
        // Back to Local, both are allowed again.
        store.reset_cloud_location_to_local(local).unwrap();
        store.require_local_run(local).unwrap();
        crate::tui_launch::refuse_while_cloud(&store, local).unwrap();
    }

    #[test]
    fn deleting_a_cloud_session_does_not_resurrect_its_host_record() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let (local, linked) = moved_session(&mut store);
        store.delete(local).unwrap();
        assert!(!listed(&store).contains(&linked));
        assert!(store.remote_session(linked).unwrap().is_none());
    }

    #[test]
    fn reset_to_local_clears_the_sync_error_link_and_cursor_of_the_real_scenario() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let (local, linked) = moved_session(&mut store);
        store.save_setting("cloud-synced:env:host-1", "7").unwrap();
        store
            .record_cloud_sync_error(local, "Host unreachable")
            .unwrap();
        assert_eq!(
            store.cloud_sync_error(local).unwrap().as_deref(),
            Some("Host unreachable")
        );
        store.reset_cloud_location_to_local(local).unwrap();
        assert_eq!(store.cloud_location(local).unwrap(), SessionLocation::Local);
        assert_eq!(store.cloud_sync_error(local).unwrap(), None);
        assert_eq!(store.cloud_synced_revision("env", "host-1").unwrap(), None);
        assert!(store.linked_remote_session(local).unwrap().is_none());
        // The 10 local rows are untouched.
        assert_eq!(store.load(local).unwrap().rows.len(), 10);
        assert!(store.remote_session(linked).unwrap().is_some());
    }

    #[test]
    fn leave_cloud_retires_the_host_record_and_refuses_while_the_cloud_runs() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let (local, linked) = moved_session(&mut store);
        store.save_setting("cloud-synced:env:host-1", "3").unwrap();
        // The linked record needs a Host session id to be tombstoned.
        let mut record = store.remote_session(linked).unwrap().unwrap();
        record.running = true;
        record.flags_at = crate::time::now_ms();
        store.save_remote_session(&record).unwrap();
        assert_eq!(store.leave_cloud(local).unwrap_err(), HOST_TURN_RUNNING);
        assert!(
            store.cloud_location(local).unwrap().is_cloud(),
            "nothing changed"
        );
        record.running = false;
        store.save_remote_session(&record).unwrap();
        store.leave_cloud(local).unwrap();
        assert_eq!(store.cloud_location(local).unwrap(), SessionLocation::Local);
        assert_eq!(store.cloud_synced_revision("env", "host-1").unwrap(), None);
        // The Host record is not a session that can be sent to, and a poll cannot bring it back.
        assert!(store.remote_session(linked).unwrap().is_none());
        assert!(!listed(&store).contains(&linked));
        assert!(store.is_cloud_converted("env", "host-1").unwrap());
        assert_eq!(store.load(local).unwrap().rows.len(), 10);
    }

    #[test]
    fn a_late_sync_error_is_a_no_op_once_local_and_a_reset_clears_a_stale_one() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let (local, _) = moved_session(&mut store);
        store
            .record_cloud_sync_error(local, "Host unreachable")
            .unwrap();
        store.leave_cloud(local).unwrap();
        assert_eq!(store.cloud_sync_error(local).unwrap(), None);
        store.record_cloud_sync_error(local, "late").unwrap();
        assert_eq!(store.cloud_sync_error(local).unwrap(), None);
    }

    #[test]
    fn an_unreadable_location_can_carry_a_notice_and_deleting_forgets_its_host_record() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let (local, linked) = moved_session(&mut store);
        store
            .save_setting(&format!("cloud-location:{local}"), "junk")
            .unwrap();
        assert!(store.cloud_located().unwrap().is_empty());
        assert_eq!(store.cloud_located_raw().unwrap(), vec![(local, None)]);
        store
            .record_cloud_sync_error(local, UNREADABLE_NOTICE)
            .unwrap();
        assert_eq!(
            store.cloud_sync_error(local).unwrap().as_deref(),
            Some(UNREADABLE_NOTICE)
        );
        store.delete(local).unwrap();
        assert!(store.remote_session(linked).unwrap().is_none());
        assert!(!listed(&store).contains(&linked));
    }

    #[test]
    fn only_a_fresh_running_flag_without_a_problem_blocks_reset() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let (local, linked) = moved_session(&mut store);
        let mut record = store.remote_session(linked).unwrap().unwrap();
        // A Host waiting for input is not a second writer.
        record.needs_input = true;
        record.flags_at = crate::time::now_ms();
        store.save_remote_session(&record).unwrap();
        assert!(!store.remote_session(linked).unwrap().unwrap().running_now());
        // Stale running (the Host went unreachable mid-turn) does not block.
        record.needs_input = false;
        record.running = true;
        record.flags_at = crate::time::now_ms() - 10 * 60_000;
        store.save_remote_session(&record).unwrap();
        assert!(!store.remote_session(linked).unwrap().unwrap().running_now());
        // Fresh running blocks...
        record.flags_at = crate::time::now_ms();
        store.save_remote_session(&record).unwrap();
        assert_eq!(store.leave_cloud(local).unwrap_err(), HOST_TURN_RUNNING);
        // ...unless a problem is recorded: the flag cannot be trusted then.
        store
            .record_cloud_sync_error(local, "Host unreachable")
            .unwrap();
        store.leave_cloud(local).unwrap();
        assert_eq!(store.cloud_location(local).unwrap(), SessionLocation::Local);
    }

    #[test]
    fn the_warning_variant_follows_what_may_be_unsynced() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let (local, linked) = moved_session(&mut store);
        // The Host is at revision 1 (its last snapshot); nothing synced yet: unknown, so strong.
        assert!(store.cloud_may_hold_unsynced(local));
        store.save_setting("cloud-synced:env:host-1", "1").unwrap();
        let mut record = store.remote_session(linked).unwrap().unwrap();
        record.configuration = Some(serde_json::json!({"revision": 1}));
        store.save_remote_session(&record).unwrap();
        assert!(!store.cloud_may_hold_unsynced(local), "in step: plain text");
        store.save_setting("cloud-synced:env:host-1", "0").unwrap();
        assert!(
            store.cloud_may_hold_unsynced(local),
            "cursor behind the Host"
        );
        store.save_setting("cloud-synced:env:host-1", "1").unwrap();
        store
            .record_cloud_sync_error(local, "Host unreachable")
            .unwrap();
        assert!(
            store.cloud_may_hold_unsynced(local),
            "unreachable: cannot know"
        );
    }

    #[test]
    fn a_deleted_workspace_marker_survives_and_blocks_appends_and_journals() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let (local, linked) = moved_session(&mut store);
        let stale = store.remote_session(linked).unwrap().unwrap();
        store.save_setting("cloud-synced:env:host-1", "4").unwrap();
        store.forget_cloud(&cloud_location().cloud).unwrap();
        // The fixture's Host record is not bound to the workspace, so drop it as the delete would.
        store.forget_cloud_session(linked).unwrap();
        assert_eq!(
            store.cloud_sync_error(local).unwrap().as_deref(),
            Some(WORKSPACE_LOST)
        );
        // A vaguer later failure does not replace it.
        store
            .record_cloud_sync_error(local, crate::store::MISSING_HOST)
            .unwrap();
        assert_eq!(
            store.cloud_sync_error(local).unwrap().as_deref(),
            Some(WORKSPACE_LOST)
        );
        // An append writes nothing and leaves the marker and the cursor alone.
        let snapshot = serde_json::json!({"revision": 9, "status": "idle",
            "session": {"id": "host-1", "blocks": [{"id": "b0", "role": "user", "text": "x"}]}});
        assert_eq!(
            store.append_remote_rows(local, &snapshot).unwrap_err(),
            WORKSPACE_LOST
        );
        assert_eq!(
            store.cloud_sync_error(local).unwrap().as_deref(),
            Some(WORKSPACE_LOST)
        );
        assert_eq!(
            store.cloud_synced_revision("env", "host-1").unwrap(),
            Some(4)
        );
        assert_eq!(store.load(local).unwrap().rows.len(), 10);
        // A worker that journals its old record after the delete cannot bring the record back.
        assert_eq!(
            store.save_remote_session(&stale).unwrap_err(),
            WORKSPACE_LOST
        );
        assert!(store.remote_session(linked).unwrap().is_none());
        assert!(!listed(&store).contains(&linked));
    }
}
