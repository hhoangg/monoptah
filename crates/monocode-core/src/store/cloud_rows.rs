//! Appends the rows of a cloud (Host) session to the LOCAL session it is linked to
//! (design 4.2.1 and the "F4-0 results" row-append approach).
//!
//! The Host snapshot is projected to rows `H`; the local `native_rows` are `L`. The longest
//! common prefix `p` (role and text) is kept, `H[p..]` is upserted at index `p`, and rows
//! at or after `H.len()` are deleted. Every step is idempotent, so a second sync of the same
//! snapshot changes nothing and a half-written earlier sync is repaired by the next one.
use super::{upsert_rows, SessionLocation, Store, CLOUD_OWNED};
use crate::transcript::Row;
use rusqlite::params;
use serde_json::Value;

/// What one append changed, in local row indexes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AppendOutcome {
    /// First row that was written or removed (the common prefix length).
    pub start: usize,
    /// Rows upserted from `start`.
    pub written: usize,
    /// Surplus rows deleted because the Host history is shorter.
    pub truncated: usize,
}

/// Why the append refuses a Host history that disagrees with rows written before the move.
pub const DIVERGED: &str = "The cloud conversation differs from the local conversation before the move point. Nothing was overwritten.";

impl Store {
    fn synced_key(environment: &str, host_session: &str) -> String {
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
        let frozen = match self.cloud_location(local_session)? {
            SessionLocation::Cloud(location) => location.moved_at_row,
            SessionLocation::Local => 0,
        };
        let local = self.native_rows(local_session)?;
        let prefix = local
            .iter()
            .zip(host)
            .take_while(|(a, b)| a.role == b.role && a.text == b.text)
            .count();
        // Divergence is never overwritten (design 4.9 R2): rows written before the move
        // stay as they are.
        if prefix < frozen.min(local.len()) {
            return Err(DIVERGED.into());
        }
        let truncated = local.len().saturating_sub(host.len());
        let tx = self.conn.transaction().map_err(|e| e.to_string())?;
        upsert_rows(&tx, local_session, prefix, &host[prefix..])?;
        if truncated > 0 {
            tx.execute(
                "DELETE FROM native_rows WHERE session_id=?1 AND row_ix>=?2",
                params![local_session, host.len() as i64],
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
        // Delivery receipt for a message that reached the cloud; only a new prompt row is one,
        // so a streaming reply does not add a key per revision.
        if host[prefix..].iter().any(Row::is_prompt) {
            tx.execute(
                "INSERT INTO native_settings(key,value) VALUES(?1,'1') ON CONFLICT(key) DO UPDATE SET value='1'",
                [format!(
                    "mono-delivered:{local_session}:cloud:{host_session}:{revision}"
                )],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(AppendOutcome {
            start: prefix,
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

    #[test]
    fn rows_before_the_move_point_are_never_overwritten() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let session = store.create("claude", "/Users/me/repo").unwrap();
        store
            .write_rows(
                session.id,
                0,
                &[
                    Row {
                        role: "user".into(),
                        text: "local".into(),
                        first: true,
                        sent_at: None,
                    },
                    Row {
                        role: "assistant".into(),
                        text: "answer".into(),
                        first: true,
                        sent_at: None,
                    },
                ],
            )
            .unwrap();
        let mut location = cloud_location();
        location.moved_at_row = 2;
        store
            .set_cloud_location(session.id, &SessionLocation::Cloud(location))
            .unwrap();
        let error = store
            .append_remote_rows(session.id, &snapshot(1, &[("user", "other")]))
            .unwrap_err();
        assert_eq!(error, DIVERGED);
        assert_eq!(
            texts(&store, session.id),
            ["user:local", "assistant:answer"]
        );
        // A history that keeps the local rows and adds the cloud turns is appended.
        let ok = store
            .append_remote_rows(
                session.id,
                &snapshot(
                    2,
                    &[
                        ("user", "local"),
                        ("assistant", "answer"),
                        ("user", "cloud"),
                    ],
                ),
            )
            .unwrap();
        assert_eq!((ok.start, ok.written), (2, 1));
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

    #[test]
    fn a_linked_host_session_is_recognisable_and_survives_registration() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let project = store
            .remember_remote_project("env", &json!({"id":"project","cwd":"/host/repo"}))
            .unwrap();
        let local = store.create("claude", "/Users/me/repo").unwrap();
        let host = json!({"projectId":"project","revision":1,"status":"idle","updatedAt":5,
            "session":{"id":"host-1","harness":"claude","title":"T","model":"claude:opus",
            "runtimeMode":"auto","blocks":[]}});
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
        // A later poll re-registers the record; the link must stay.
        store.register_remote_summary(&project, &host).unwrap();
        assert_eq!(
            store.remote_session(remote.id).unwrap().unwrap().linked_to,
            Some(local.id)
        );
        store.link_remote_session(remote.id, None).unwrap();
        assert!(store.linked_remote_session(local.id).unwrap().is_none());
    }
}
