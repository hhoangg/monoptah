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

impl Store {
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
    /// The user's way back to Local ("Reset to local"), after the confirmation. Refuses while a
    /// Host-side turn runs (it would be a second writer). Unlike the bare reset it also
    /// retires the Host record: it is unlinked, remembered as converted so a poll never
    /// imports it again, and dropped locally, so nothing can send to it while the local
    /// runtime is allowed on the same Claude id. The copy in the workspace itself stays.
    /// It does not reconcile the transcript (F4-5).
    pub fn leave_cloud(&self, session: i64) -> Result<(), String> {
        let backing = self.linked_remote_session(session)?;
        if let Some((_, record)) = &backing {
            if record.running || record.needs_input {
                return Err(HOST_TURN_RUNNING.to_string());
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
        self.save_setting(&Self::location_key(session), "")
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
}
