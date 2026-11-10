//! Appends the rows of a cloud (Host) session to the LOCAL session it is linked to
//! (design 4.2.1 and the "F4-0 results" row-append approach).
//!
//! The Host snapshot is projected to rows `H`; the local `native_rows` are `L`. A Host
//! session created by a move holds only the turns that ran in the cloud, so Host row `i`
//! is local row `moved_at_row + i`. The longest common prefix `p` (role and text) of
//! `L[moved_at_row..]` and `H` is kept, `H[p..]` is upserted at `moved_at_row + p`, and rows
//! from `moved_at_row + H.len()` are deleted. Rows before `moved_at_row` are never read,
//! written or deleted. Every step is idempotent, so a second sync of the same snapshot
//! changes nothing and a half-written earlier sync is repaired by the next one.
//!
//! Preconditions: the Host session starts with zero blocks (`sessions.import` must not
//! project the imported JSONL), and the first sync (no cursor) finds exactly `moved_at_row`
//! local rows. The append writes only while the stored location is Cloud for the same
//! environment and Host session.
use super::{upsert_rows, SessionLocation, Store, CLOUD_OWNED};
use crate::transcript::Row;
use rusqlite::params;
use serde_json::Value;

/// What one append changed, in local row indexes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AppendOutcome {
    /// First local row that was written or removed.
    pub start: usize,
    /// Rows upserted from `start`.
    pub written: usize,
    /// Surplus rows deleted because the Host history is shorter.
    pub truncated: usize,
}

/// Why the append refuses: the local rows written before the move are missing, so the Host
/// rows have no place to go. Nothing is written.
pub const NOT_CLOUD: &str =
    "This conversation is no longer in that cloud session. Nothing was written.";

/// Why the append refuses: the local rows do not match the move point (too few rows, or at
/// the first sync a different count), so the Host rows have no safe place. Nothing is written.
pub const DIVERGED: &str = "The local conversation is shorter than its move point, so the cloud messages cannot be placed. Nothing was overwritten.";

impl Store {
    pub(super) fn synced_key(environment: &str, host_session: &str) -> String {
        format!("cloud-synced:{environment}:{host_session}")
    }
    /// The Host revision last appended for this Host session, if any.
    pub fn cloud_synced_revision(
        &self,
        environment: &str,
        host_session: &str,
    ) -> Result<Option<u64>, String> {
        Ok(self
            .setting(&Self::synced_key(environment, host_session))?
            .and_then(|raw| raw.parse().ok()))
    }
    pub(super) fn sync_error_key(session: i64) -> String {
        format!("cloud-sync-error:{session}")
    }
    /// The last row-append failure of a Cloud session, until the next successful append.
    pub fn cloud_sync_error(&self, session: i64) -> Result<Option<String>, String> {
        Ok(self
            .setting(&Self::sync_error_key(session))?
            .filter(|message| !message.is_empty()))
    }
    /// Remembers why the row append failed, so the UI can say so after a reload. Does
    /// nothing when the session is not in the cloud (for example it was just deleted).
    pub fn record_cloud_sync_error(&self, session: i64, message: &str) -> Result<(), String> {
        // A Local session (or a deleted one) has nothing to report. An unreadable location is
        // still a location: the session fails closed and the notice must say so.
        if matches!(self.cloud_location(session), Ok(SessionLocation::Local)) {
            return Ok(());
        }
        self.save_setting(&Self::sync_error_key(session), message)
    }
    /// Deletes the rows of a local session from index `from` on.
    pub fn truncate_rows(&mut self, session: i64, from: usize) -> Result<(), String> {
        if self.remote_session(session)?.is_some() {
            return Err("Remote transcripts are owned by the host".into());
        }
        if self.cloud_location(session)?.is_cloud() {
            return Err(CLOUD_OWNED.into());
        }
        self.require_native_rows()?;
        self.conn
            .execute(
                "DELETE FROM native_rows WHERE session_id=?1 AND row_ix>=?2",
                params![session, from as i64],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    fn require_native_rows(&self) -> Result<(), String> {
        if self.shared.is_some() {
            return Err("Cloud sessions need the native database".into());
        }
        Ok(())
    }
    /// Projects a Host snapshot and appends it to the Cloud-located local session.
    pub fn append_remote_rows(
        &mut self,
        local_session: i64,
        snapshot: &Value,
    ) -> Result<AppendOutcome, String> {
        let SessionLocation::Cloud(location) = self.cloud_location(local_session)? else {
            return Err("This conversation is not in the cloud".into());
        };
        let (history, _) = Self::project_remote_history(snapshot)?;
        self.append_projected_rows(
            local_session,
            &location.environment_id,
            &location.host_session_id,
            snapshot["revision"].as_u64().unwrap_or(0),
            &history.rows,
        )
    }
    /// The idempotent append of already projected Host rows `host` to a local session.
    /// One transaction covers the upsert, the truncation and the cursor.
    pub fn append_projected_rows(
        &mut self,
        local_session: i64,
        environment: &str,
        host_session: &str,
        revision: u64,
        host: &[Row],
    ) -> Result<AppendOutcome, String> {
        if self.remote_session(local_session)?.is_some() {
            return Err("Remote transcripts are owned by the host".into());
        }
        self.require_native_rows()?;
        // A Host session created by a move holds only the turns that ran in the cloud, so
        // Host row `i` is local row `moved_at_row + i`. Rows before `moved_at_row` are never
        // compared, written or deleted.
        let SessionLocation::Cloud(location) = self.cloud_location(local_session)? else {
            return Err(NOT_CLOUD.into());
        };
        if location.environment_id != environment || location.host_session_id != host_session {
            return Err(NOT_CLOUD.into());
        }
        let base = location.moved_at_row;
        let local = self.native_rows(local_session)?;
        let first_sync = self
            .cloud_synced_revision(environment, host_session)?
            .is_none();
        if local.len() < base || (first_sync && local.len() != base) {
            return Err(DIVERGED.into());
        }
        let tail = &local[base..];
        let prefix = tail
            .iter()
            .zip(host)
            .take_while(|(a, b)| a.role == b.role && a.text == b.text)
            .count();
        let truncated = tail.len().saturating_sub(host.len());
        let tx = self.conn.transaction().map_err(|e| e.to_string())?;
        upsert_rows(&tx, local_session, base + prefix, &host[prefix..])?;
        if truncated > 0 {
            tx.execute(
                "DELETE FROM native_rows WHERE session_id=?1 AND row_ix>=?2",
                params![local_session, (base + host.len()) as i64],
            )
            .map_err(|e| e.to_string())?;
        }
        if prefix < host.len() || truncated > 0 {
            tx.execute(
                "UPDATE native_sessions SET updated_at=?2 WHERE id=?1",
                params![
                    local_session,
                    crate::response::now_ms().min(i64::MAX as u64) as i64
                ],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.execute(
            "INSERT INTO native_settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![
                Self::synced_key(environment, host_session),
                revision.to_string()
            ],
        )
        .map_err(|e| e.to_string())?;
        tx.execute(
            "DELETE FROM native_settings WHERE key=?1",
            [Self::sync_error_key(local_session)],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(AppendOutcome {
            start: base + prefix,
            written: host.len() - prefix,
            truncated,
        })
    }
    fn native_rows(&self, session: i64) -> Result<Vec<Row>, String> {
        let mut stmt = self
            .conn
            .prepare("SELECT role,body,first,sent_at FROM native_rows WHERE session_id=?1 ORDER BY row_ix")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([session], |r| {
                Ok(Row {
                    role: r.get(0)?,
                    text: r.get(1)?,
                    first: r.get(2)?,
                    sent_at: r
                        .get::<_, Option<i64>>(3)?
                        .and_then(|at| u64::try_from(at).ok()),
                })
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::location::tests::cloud_location;
    use serde_json::json;
    use std::path::Path;

    fn snapshot(revision: u64, blocks: &[(&str, &str)]) -> Value {
        let blocks: Vec<Value> = blocks
            .iter()
            .enumerate()
            .map(|(i, (role, text))| json!({"id": format!("b{i}"), "role": role, "text": text}))
            .collect();
        json!({"revision": revision, "status": "idle", "session": {"id": "host-1", "blocks": blocks}})
    }
    fn cloud_session(store: &Store, moved_at_row: usize) -> i64 {
        let session = store.create("claude", "/Users/me/repo").unwrap();
        let mut location = cloud_location();
        location.moved_at_row = moved_at_row;
        store
            .set_cloud_location(session.id, &SessionLocation::Cloud(location))
            .unwrap();
        session.id
    }
    fn texts(store: &Store, session: i64) -> Vec<String> {
        store
            .native_rows(session)
            .unwrap()
            .into_iter()
            .map(|r| format!("{}:{}", r.role, r.text))
            .collect()
    }

    #[test]
    fn rows_are_appended_once_and_a_second_sync_changes_nothing() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let id = cloud_session(&store, 0);
        let snap = snapshot(3, &[("user", "hi"), ("assistant", "hello")]);
        let first = store.append_remote_rows(id, &snap).unwrap();
        assert_eq!((first.start, first.written, first.truncated), (0, 2, 0));
        let after_first = texts(&store, id);
        assert_eq!(after_first, ["user:hi", "assistant:hello"]);
        for _ in 0..2 {
            let again = store.append_remote_rows(id, &snap).unwrap();
            assert_eq!((again.written, again.truncated), (0, 0));
            assert_eq!(texts(&store, id), after_first);
        }
        assert_eq!(
            store.cloud_synced_revision("env", "host-1").unwrap(),
            Some(3)
        );
        // The load path sees one history.
        assert_eq!(store.load(id).unwrap().rows.len(), 2);
    }

    #[test]
    fn new_host_blocks_extend_the_local_rows() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let id = cloud_session(&store, 0);
        store
            .append_remote_rows(id, &snapshot(1, &[("user", "hi")]))
            .unwrap();
        let next = store
            .append_remote_rows(id, &snapshot(2, &[("user", "hi"), ("assistant", "yo")]))
            .unwrap();
        assert_eq!((next.start, next.written), (1, 1));
        assert_eq!(texts(&store, id), ["user:hi", "assistant:yo"]);
    }

    #[test]
    fn a_partial_earlier_write_is_repaired() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let id = cloud_session(&store, 0);
        let snap = snapshot(
            5,
            &[
                ("user", "a"),
                ("assistant", "b"),
                ("user", "c"),
                ("assistant", "d"),
            ],
        );
        let (history, _) = Store::project_remote_history(&snap).unwrap();
        // A crash left the first row, and a torn second row with the wrong text and role.
        {
            let tx = store.conn.transaction().unwrap();
            upsert_rows(&tx, id, 0, &history.rows[..1]).unwrap();
            upsert_rows(
                &tx,
                id,
                1,
                &[Row {
                    role: "user".into(),
                    text: "torn".into(),
                    first: true,
                    sent_at: None,
                }],
            )
            .unwrap();
            tx.commit().unwrap();
        }
        // An earlier sync got as far as its cursor (the first sync checks the row count).
        store.save_setting("cloud-synced:env:host-1", "4").unwrap();
        let repaired = store.append_remote_rows(id, &snap).unwrap();
        assert_eq!(repaired.start, 1);
        assert_eq!(
            texts(&store, id),
            ["user:a", "assistant:b", "user:c", "assistant:d"]
        );
        assert_eq!(store.native_rows(id).unwrap(), history.rows);
    }

    #[test]
    fn a_rewritten_earlier_host_row_rewrites_the_tail() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let id = cloud_session(&store, 0);
        store
            .append_remote_rows(
                id,
                &snapshot(
                    1,
                    &[("user", "a"), ("assistant", "streaming"), ("user", "c")],
                ),
            )
            .unwrap();
        let next = store
            .append_remote_rows(
                id,
                &snapshot(2, &[("user", "a"), ("assistant", "final"), ("user", "c")]),
            )
            .unwrap();
        assert_eq!((next.start, next.written, next.truncated), (1, 2, 0));
        assert_eq!(texts(&store, id), ["user:a", "assistant:final", "user:c"]);
    }

    #[test]
    fn a_shorter_host_history_truncates_the_surplus() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let id = cloud_session(&store, 0);
        store
            .append_remote_rows(
                id,
                &snapshot(1, &[("user", "a"), ("assistant", "b"), ("user", "c")]),
            )
            .unwrap();
        let next = store
            .append_remote_rows(id, &snapshot(2, &[("user", "a")]))
            .unwrap();
        assert_eq!((next.written, next.truncated), (0, 2));
        assert_eq!(texts(&store, id), ["user:a"]);
        // The public helper behaves the same on a plain local session.
        let plain = store.create("claude", "/p").unwrap();
        store
            .write_rows(
                plain.id,
                0,
                &[
                    Row {
                        role: "user".into(),
                        text: "x".into(),
                        first: true,
                        sent_at: Some(7),
                    },
                    Row {
                        role: "assistant".into(),
                        text: "y".into(),
                        first: true,
                        sent_at: None,
                    },
                ],
            )
            .unwrap();
        store.truncate_rows(plain.id, 1).unwrap();
        assert_eq!(store.load(plain.id).unwrap().rows.len(), 1);
    }

    fn local_history(store: &mut Store, count: usize) -> (i64, Vec<Row>) {
        let session = store.create("claude", "/Users/me/repo").unwrap();
        let rows: Vec<Row> = (0..count)
            .map(|i| Row {
                role: if i % 2 == 0 { "user" } else { "assistant" }.into(),
                text: format!("local {i}"),
                first: true,
                sent_at: Some(i as u64 + 1),
            })
            .collect();
        store.write_rows(session.id, 0, &rows).unwrap();
        let mut location = cloud_location();
        location.moved_at_row = count;
        store
            .set_cloud_location(session.id, &SessionLocation::Cloud(location))
            .unwrap();
        (session.id, rows)
    }

    #[test]
    fn a_moved_session_appends_cloud_turns_after_the_local_rows() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let (id, before) = local_history(&mut store, 10);
        // The Host session made by the move holds only the turns that ran in the cloud.
        let first = snapshot(1, &[("user", "cloud q"), ("assistant", "cloud a")]);
        let out = store.append_remote_rows(id, &first).unwrap();
        assert_eq!((out.start, out.written, out.truncated), (10, 2, 0));
        assert_eq!(&store.native_rows(id).unwrap()[..10], &before[..]);
        assert_eq!(store.native_rows(id).unwrap().len(), 12);
        // Re-sync changes nothing.
        let again = store.append_remote_rows(id, &first).unwrap();
        assert_eq!((again.written, again.truncated), (0, 0));
        // A rewritten cloud row rewrites only the cloud tail.
        let next = snapshot(
            2,
            &[
                ("user", "cloud q"),
                ("assistant", "cloud a2"),
                ("user", "more"),
            ],
        );
        let out = store.append_remote_rows(id, &next).unwrap();
        assert_eq!((out.start, out.written), (11, 2));
        // A shorter Host history truncates only cloud rows.
        let out = store.append_remote_rows(id, &snapshot(3, &[])).unwrap();
        assert_eq!(out.truncated, 3);
        assert_eq!(store.native_rows(id).unwrap(), before);
    }

    #[test]
    fn a_host_that_replays_pre_move_history_is_not_required_to_match() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let (id, before) = local_history(&mut store, 4);
        let replay = snapshot(1, &[("user", "unrelated"), ("assistant", "unrelated too")]);
        store.append_remote_rows(id, &replay).unwrap();
        let rows = store.native_rows(id).unwrap();
        assert_eq!(&rows[..4], &before[..]);
        assert_eq!(rows.len(), 6);
    }

    #[test]
    fn missing_pre_move_rows_refuse_without_writing() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let (id, _) = local_history(&mut store, 4);
        store
            .conn
            .execute(
                "DELETE FROM native_rows WHERE session_id=?1 AND row_ix>=2",
                [id],
            )
            .unwrap();
        let error = store
            .append_remote_rows(id, &snapshot(1, &[("user", "q")]))
            .unwrap_err();
        assert_eq!(error, DIVERGED);
        assert_eq!(store.native_rows(id).unwrap().len(), 2);
    }

    #[test]
    fn edit_and_other_row_writers_are_refused_while_cloud() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let (id, rows) = local_history(&mut store, 2);
        let snapshot_edit = store.prepare_edit_last_turn(id, &rows, None).unwrap_err();
        assert_eq!(snapshot_edit, CLOUD_OWNED);
        let edit = crate::store::EditSnapshot {
            start: 0,
            prompt: String::new(),
            attachments: vec![],
            provider_turn_id: None,
            provider_id: String::new(),
            account_id: String::new(),
            cwd: String::new(),
            rows: vec![],
            blocks: None,
            block_start: 0,
            provider: "claude".into(),
        };
        assert_eq!(
            store
                .finish_edit_last_turn(id, &edit, "thread")
                .unwrap_err(),
            CLOUD_OWNED
        );
        assert_eq!(store.native_rows(id).unwrap(), rows);
    }

    #[test]
    fn a_location_flip_or_delete_mid_observation_writes_nothing() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let (id, before) = local_history(&mut store, 10);
        let rows = [Row {
            role: "user".into(),
            text: "cloud".into(),
            first: true,
            sent_at: None,
        }];
        store
            .append_projected_rows(id, "env", "host-1", 1, &rows)
            .unwrap();
        // The location flips to Local: the next poll must not touch the ten local rows.
        store.reset_cloud_location_to_local(id).unwrap();
        let err = store
            .append_projected_rows(id, "env", "host-1", 2, &[])
            .unwrap_err();
        assert_eq!(err, NOT_CLOUD);
        assert_eq!(&store.native_rows(id).unwrap()[..10], &before[..]);
        assert_eq!(store.native_rows(id).unwrap().len(), 11);
        // A different Host session than the location names is refused too.
        let mut other = cloud_location();
        other.moved_at_row = 10;
        store
            .set_cloud_location(id, &SessionLocation::Cloud(other))
            .unwrap();
        assert!(store
            .append_projected_rows(id, "env", "another-host", 3, &[])
            .is_err());
        assert!(store
            .append_projected_rows(id, "other-env", "host-1", 3, &[])
            .is_err());
        assert_eq!(store.native_rows(id).unwrap().len(), 11);
        // A deleted session gets neither rows nor a cursor back.
        store.delete(id).unwrap();
        assert!(store
            .append_projected_rows(id, "env", "host-1", 4, &rows)
            .is_err());
        assert!(store.native_rows(id).unwrap().is_empty());
        assert_eq!(store.cloud_synced_revision("env", "host-1").unwrap(), None);
    }

    #[test]
    fn the_first_sync_needs_exactly_the_move_point_row_count() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let (id, before) = local_history(&mut store, 4);
        // A local turn left an extra row between the move point and the flip.
        store
            .write_rows(
                id,
                4,
                &[Row {
                    role: "assistant".into(),
                    text: "interrupted".into(),
                    first: true,
                    sent_at: None,
                }],
            )
            .unwrap_err(); // refused: already Cloud
        store.reset_cloud_location_to_local(id).unwrap();
        store
            .write_rows(
                id,
                4,
                &[Row {
                    role: "assistant".into(),
                    text: "interrupted".into(),
                    first: true,
                    sent_at: None,
                }],
            )
            .unwrap();
        let mut location = cloud_location();
        location.moved_at_row = 4;
        store
            .set_cloud_location(id, &SessionLocation::Cloud(location))
            .unwrap();
        assert_eq!(
            store
                .append_remote_rows(id, &snapshot(1, &[("user", "q")]))
                .unwrap_err(),
            DIVERGED
        );
        assert_eq!(store.native_rows(id).unwrap().len(), 5);
        assert_eq!(&store.native_rows(id).unwrap()[..4], &before[..]);
        assert_eq!(store.cloud_synced_revision("env", "host-1").unwrap(), None);
    }

    #[test]
    fn a_sync_failure_is_remembered_until_the_next_successful_append() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let (id, _) = local_history(&mut store, 4);
        assert_eq!(store.cloud_sync_error(id).unwrap(), None);
        store.record_cloud_sync_error(id, "boom").unwrap();
        assert_eq!(store.cloud_sync_error(id).unwrap().as_deref(), Some("boom"));
        store
            .append_remote_rows(id, &snapshot(1, &[("user", "q")]))
            .unwrap();
        assert_eq!(store.cloud_sync_error(id).unwrap(), None);
        // A session that is not in the cloud (or was deleted) keeps no marker.
        store.record_cloud_sync_error(id, "again").unwrap();
        store.delete(id).unwrap();
        store.record_cloud_sync_error(id, "late").unwrap();
        assert_eq!(store.cloud_sync_error(id).unwrap(), None);
    }

    #[test]
    fn deleting_the_session_forgets_the_sync_cursor() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let id = cloud_session(&store, 0);
        store
            .append_remote_rows(id, &snapshot(7, &[("user", "a")]))
            .unwrap();
        assert_eq!(
            store.cloud_synced_revision("env", "host-1").unwrap(),
            Some(7)
        );
        store.delete(id).unwrap();
        assert_eq!(store.cloud_synced_revision("env", "host-1").unwrap(), None);
    }

    #[test]
    fn local_writers_are_refused_while_cloud_and_allowed_again_after() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let id = cloud_session(&store, 0);
        let row = Row {
            role: "user".into(),
            text: "x".into(),
            first: true,
            sent_at: None,
        };
        assert_eq!(
            store
                .write_rows(id, 0, std::slice::from_ref(&row))
                .unwrap_err(),
            CLOUD_OWNED
        );
        assert_eq!(store.truncate_rows(id, 0).unwrap_err(), CLOUD_OWNED);
        store
            .set_cloud_location(id, &SessionLocation::Local)
            .unwrap();
        store.write_rows(id, 0, &[row]).unwrap();
    }

    #[test]
    fn append_needs_a_cloud_location_and_a_local_target() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let local = store.create("claude", "/p").unwrap();
        assert!(store
            .append_remote_rows(local.id, &snapshot(1, &[("user", "a")]))
            .is_err());
        let project = store
            .remember_remote_project("env", &json!({"id":"project","cwd":"/host/repo"}))
            .unwrap();
        let remote = store.create("claude", &project.key).unwrap();
        assert!(store
            .append_projected_rows(remote.id, "env", "host-1", 1, &[])
            .is_err());
    }

    fn host_value(id: &str) -> Value {
        json!({"projectId":"project","revision":1,"status":"idle","updatedAt":5,
            "session":{"id":id,"harness":"claude","title":"T","model":"claude:opus",
            "runtimeMode":"auto","blocks":[]}})
    }

    #[test]
    fn a_linked_host_session_is_recognisable_and_survives_registration() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let project = store
            .remember_remote_project("env", &json!({"id":"project","cwd":"/host/repo"}))
            .unwrap();
        let local = store.create("claude", "/Users/me/repo").unwrap();
        let host = host_value("host-1");
        let remote = store.register_remote_summary(&project, &host).unwrap();
        assert!(store.linked_remote_session(local.id).unwrap().is_none());
        assert!(!store
            .remote_session(remote.id)
            .unwrap()
            .unwrap()
            .is_linked());
        store
            .link_remote_session(remote.id, Some(local.id))
            .unwrap();
        let (found, session) = store.linked_remote_session(local.id).unwrap().unwrap();
        assert_eq!(found, remote.id);
        assert!(session.is_linked());
        store.register_remote_summary(&project, &host).unwrap();
        assert_eq!(
            store.remote_session(remote.id).unwrap().unwrap().linked_to,
            Some(local.id)
        );
        store.link_remote_session(remote.id, None).unwrap();
        assert!(store.linked_remote_session(local.id).unwrap().is_none());
    }

    #[test]
    fn linking_replaces_older_links_and_a_stale_journal_cannot_restore_one() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let project = store
            .remember_remote_project("env", &json!({"id":"project","cwd":"/host/repo"}))
            .unwrap();
        let local = store.create("claude", "/Users/me/repo").unwrap();
        let old = store
            .register_remote_summary(&project, &host_value("old"))
            .unwrap();
        let new = store
            .register_remote_summary(&project, &host_value("new"))
            .unwrap();
        store.link_remote_session(old.id, Some(local.id)).unwrap();
        // A worker loaded the record while it was linked.
        let stale = store.remote_session(old.id).unwrap().unwrap();
        store.link_remote_session(new.id, Some(local.id)).unwrap();
        assert_eq!(
            store.remote_session(old.id).unwrap().unwrap().linked_to,
            None
        );
        // The stale worker journals its copy: the link must not come back.
        store.save_remote_session(&stale).unwrap();
        assert_eq!(
            store.remote_session(old.id).unwrap().unwrap().linked_to,
            None
        );
        assert_eq!(
            store.linked_remote_session(local.id).unwrap().unwrap().0,
            new.id
        );
        // Unlink during an observation: a journal of the linked copy keeps it unlinked.
        let linked_copy = store.remote_session(new.id).unwrap().unwrap();
        store.link_remote_session(new.id, None).unwrap();
        store.save_remote_session(&linked_copy).unwrap();
        assert!(store.linked_remote_session(local.id).unwrap().is_none());
    }

    #[test]
    fn the_host_record_is_found_by_the_location_not_by_the_first_link() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let project = store
            .remember_remote_project("env", &json!({"id":"project","cwd":"/host/repo"}))
            .unwrap();
        let local = cloud_session(&store, 0);
        let stale = store
            .register_remote_summary(&project, &host_value("stale"))
            .unwrap();
        let wanted = store
            .register_remote_summary(&project, &host_value("host-1"))
            .unwrap();
        assert_eq!(
            store.cloud_host_record(local).unwrap_err(),
            crate::store::MISSING_HOST
        );
        store.link_remote_session(wanted.id, Some(local)).unwrap();
        // A stale link on another record is ignored (and cleared by the new link).
        store.link_remote_session(stale.id, Some(local)).unwrap();
        assert_eq!(
            store.cloud_host_record(local).unwrap_err(),
            crate::store::MISSING_HOST
        );
        store.link_remote_session(wanted.id, Some(local)).unwrap();
        assert_eq!(store.cloud_host_record(local).unwrap().0, wanted.id);
        // A plain local session has no host record.
        let plain = store.create("claude", "/p").unwrap();
        assert!(store.cloud_host_record(plain.id).is_err());
    }
}
