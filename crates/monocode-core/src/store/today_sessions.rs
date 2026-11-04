//! Links between chat sessions and internal tasks (Today items).
//!
//! A session links to at most one internal task; a task can have many sessions.
//! `today_session_links.session_id` is the app-level session id (`Session.id`).
//! In the shared desktop database that id is a stable hash of the persisted
//! string id (see `shared::handle`), so it survives restarts and the table
//! needs no id translation. The table is additive, like the other Today tables.
//! External tasks (ClickUp, Jira, GitHub) are not involved; only the internal
//! task a session was started from or moved to.
use super::Store;
use crate::today::mention;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

/// Set once sessions started before links existed have been matched to tasks.
const BACKFILL_KEY: &str = "today.sessionLinksBackfilled";
const NOT_FOUND: &str = "Task not found";

fn sql(error: rusqlite::Error) -> String {
    error.to_string()
}

/// The key a message opens with (`@today/<key>`), if it starts with a mention.
fn leading_key(text: &str) -> Option<String> {
    let text = text.trim_start();
    if !text.starts_with(mention::PREFIX) {
        return None;
    }
    mention::keys_in_text(text).into_iter().next()
}

/// The key of the first user message in stored blocks: the first sent message,
/// or the first unsent draft when nothing was sent. Internal blocks are skipped.
fn first_message_key(blocks: &[Value]) -> Option<String> {
    let mut users = blocks
        .iter()
        .filter(|b| b["role"] == "user" && b["internal"] != true);
    let sent = users.clone().find(|b| b["draft"] != true);
    let first = sent.or_else(|| users.next())?;
    leading_key(first["text"].as_str()?)
}

impl Store {
    fn has_session_links(&self) -> Result<bool, String> {
        super::shared::has_table(&self.conn, "today_session_links")
    }

    /// Links `session_id` to the internal task `task` (id or key). A session has
    /// one task: linking it elsewhere moves it, and the previous task id is
    /// returned. Linking to the task it already has changes nothing and returns
    /// `None`.
    pub fn today_link_session(
        &self,
        session_id: i64,
        task: &str,
        now_ms: u64,
    ) -> Result<Option<String>, String> {
        self.today_writable()?;
        self.today_ready()?;
        let task = self
            .today_task(task)?
            .ok_or_else(|| NOT_FOUND.to_string())?;
        self.today_tx(|conn| {
            let current: Option<String> = conn
                .query_row(
                    "SELECT task_id FROM today_session_links WHERE session_id = ?1",
                    [session_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(sql)?;
            if current.as_deref() == Some(task.id.as_str()) {
                return Ok(None);
            }
            conn.execute(
                "INSERT INTO today_session_links (session_id, task_id, linked_at)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(session_id) DO UPDATE SET
                   task_id = excluded.task_id, linked_at = excluded.linked_at",
                params![session_id, task.id, now_ms.min(i64::MAX as u64) as i64],
            )
            .map_err(sql)?;
            Ok(current)
        })
    }

    /// Removes the session's link; a session with none is left as it is.
    pub fn today_unlink_session(&self, session_id: i64) -> Result<(), String> {
        self.today_writable()?;
        if !self.today_ready()? {
            return Ok(());
        }
        self.today_tx(|conn| {
            conn.execute(
                "DELETE FROM today_session_links WHERE session_id = ?1",
                [session_id],
            )
            .map_err(sql)?;
            Ok(())
        })
    }

    /// The id of the internal task a session is linked to.
    pub fn today_session_task(&self, session_id: i64) -> Result<Option<String>, String> {
        self.backfill_lazily();
        if !self.has_session_links()? {
            return Ok(None);
        }
        self.conn
            .query_row(
                "SELECT task_id FROM today_session_links WHERE session_id = ?1",
                [session_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(sql)
    }

    /// Ids of the sessions linked to an internal task (id or key), newest link
    /// first. An unknown task has none.
    pub fn today_task_session_ids(&self, task: &str) -> Result<Vec<i64>, String> {
        self.backfill_lazily();
        let Some(task) = self.today_task(task)? else {
            return Ok(Vec::new());
        };
        if !self.has_session_links()? {
            return Ok(Vec::new());
        }
        let mut stmt = self
            .conn
            .prepare(
                "SELECT session_id FROM today_session_links WHERE task_id = ?1
                 ORDER BY linked_at DESC, session_id DESC",
            )
            .map_err(sql)?;
        let ids = stmt
            .query_map([task.id], |row| row.get::<_, i64>(0))
            .map_err(sql)?;
        ids.collect::<Result<_, _>>().map_err(sql)
    }

    /// Every link as (session id, task id), for building a map in one read.
    pub fn today_session_links(&self) -> Result<Vec<(i64, String)>, String> {
        self.backfill_lazily();
        if !self.has_session_links()? {
            return Ok(Vec::new());
        }
        let mut stmt = self
            .conn
            .prepare(
                "SELECT session_id, task_id FROM today_session_links
                 ORDER BY linked_at DESC, session_id DESC",
            )
            .map_err(sql)?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(sql)?;
        rows.collect::<Result<_, _>>().map_err(sql)
    }

    /// Drops a deleted session's link. Called from `Store::delete`.
    pub(super) fn today_drop_session_link(&self, session_id: i64) -> Result<(), String> {
        if self.today_writable().is_err() || !self.has_session_links()? {
            return Ok(());
        }
        self.conn
            .execute(
                "DELETE FROM today_session_links WHERE session_id = ?1",
                [session_id],
            )
            .map_err(sql)?;
        Ok(())
    }

    /// One-time link of sessions started from a task before links existed.
    /// Those opened a chat whose first message is `@today/<key>`. Links only
    /// sessions whose first user message (or unsent draft, when nothing was
    /// sent) starts with the mention of an existing task, and never replaces a
    /// link. Returns how many were linked. A read-only store does nothing.
    pub fn today_backfill_session_links(&self, now_ms: u64) -> Result<usize, String> {
        if self.today_writable().is_err() || self.setting(BACKFILL_KEY)?.is_some() {
            return Ok(0);
        }
        self.today_ready()?;
        let mut linked = 0;
        // A session that cannot be read or linked is skipped: the scan must finish
        // and set the guard, or every later read would repeat it.
        for (session_id, key, at) in self.session_mention_keys()? {
            let Some(task) = self.today_task(&key).ok().flatten() else {
                continue;
            };
            // The session's own time keeps newest-first meaningful; `now` is the fallback.
            let linked_at = if at > 0 { at } else { now_ms };
            let done = self.today_tx(|conn| {
                conn.execute(
                    "INSERT OR IGNORE INTO today_session_links (session_id, task_id, linked_at)
                     VALUES (?1, ?2, ?3)",
                    params![session_id, task.id, linked_at.min(i64::MAX as u64) as i64],
                )
                .map_err(sql)
            });
            linked += done.unwrap_or(0);
        }
        self.save_setting(BACKFILL_KEY, "1")?;
        Ok(linked)
    }

    /// Readers run the backfill on a writable store; it is guarded, so it costs
    /// one settings read afterwards. A failure must not break a read, and the
    /// guard stays unset so the next read tries again.
    fn backfill_lazily(&self) {
        let _ = self.today_backfill_session_links(crate::response::now_ms());
    }

    /// (session id, key) for every session whose first message opens with a
    /// task mention. SQL keeps to sessions whose transcript contains `@today/`
    /// at all; only those are parsed.
    fn session_mention_keys(&self) -> Result<Vec<(i64, String, u64)>, String> {
        let mut found = Vec::new();
        if self.shared.is_some() {
            let mut stmt = self
                .conn
                .prepare("SELECT id, blocks_json, updated_at FROM sessions WHERE blocks_json LIKE '%@today/%'")
                .map_err(sql)?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                })
                .map_err(sql)?;
            for row in rows {
                let Ok((original, raw, at)) = row else {
                    continue;
                };
                let Ok(blocks) = serde_json::from_str::<Vec<Value>>(&raw) else {
                    continue;
                };
                if let Some(key) = first_message_key(&blocks) {
                    found.push((super::shared::handle(&original), key, at.max(0) as u64));
                }
            }
            return Ok(found);
        }
        let ids = legacy_candidates(&self.conn)?;
        for (id, at) in ids {
            // An unreadable record skips that session only.
            let Ok(sent) = self
                .conn
                .query_row(
                    "SELECT body FROM native_rows WHERE session_id = ?1 AND role = 'user'
                     ORDER BY row_ix LIMIT 1",
                    [id],
                    |row| row.get::<_, String>(0),
                )
                .optional()
            else {
                continue;
            };
            let text = match sent {
                Some(text) => text,
                None => match self.follow_ups(id).ok().and_then(|state| state.draft) {
                    Some(draft) => draft.text,
                    None => continue,
                },
            };
            if let Some(key) = leading_key(&text) {
                found.push((id, key, at));
            }
        }
        Ok(found)
    }
}

fn legacy_candidates(conn: &Connection) -> Result<Vec<(i64, u64)>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT s.id, s.updated_at FROM native_sessions s WHERE
               EXISTS(SELECT 1 FROM native_rows r WHERE r.session_id = s.id
                      AND r.role = 'user' AND r.body LIKE '%@today/%')
               OR EXISTS(SELECT 1 FROM native_settings
                         WHERE key = 'follow-up:' || s.id AND value LIKE '%@today/%')",
        )
        .map_err(sql)?;
    let ids = stmt
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?.max(0) as u64))
        })
        .map_err(sql)?;
    ids.collect::<Result<_, _>>().map_err(sql)
}

#[cfg(test)]
mod tests {
    use crate::follow_up::{Message, State};
    use crate::store::Store;
    use crate::today::{Actor, NewTask, Task};
    use crate::transcript::Row;
    use std::path::Path;

    const NOW: u64 = 1_800_000_000_000;

    /// The legacy store and the shared desktop database.
    fn stores() -> Vec<(&'static str, Store)> {
        vec![
            ("legacy", Store::open(Path::new(":memory:")).unwrap()),
            (
                "shared",
                Store::open_monocode(Path::new(":memory:")).unwrap(),
            ),
        ]
    }

    fn task(store: &Store, title: &str) -> Task {
        store
            .today_add(
                &NewTask {
                    title: title.into(),
                    ..NewTask::default()
                },
                &Actor::User,
                NOW,
            )
            .unwrap()
    }

    fn session(store: &Store) -> i64 {
        store.create("codex", "/project").unwrap().id
    }

    fn send(store: &mut Store, id: i64, texts: &[&str]) {
        let rows: Vec<Row> = texts
            .iter()
            .enumerate()
            .map(|(ix, text)| Row {
                role: if ix % 2 == 0 { "user" } else { "assistant" }.into(),
                text: (*text).into(),
                first: true,
                sent_at: None,
            })
            .collect();
        store.write_rows(id, 0, &rows).unwrap();
    }

    fn draft(store: &Store, id: i64, text: &str) {
        let state = State {
            draft: Some(Message {
                id: "draft-1".into(),
                text: text.into(),
                ..Default::default()
            }),
            ..Default::default()
        };
        store.save_follow_ups(id, &state).unwrap();
    }

    #[test]
    fn link_and_unlink_a_session() {
        for (mode, store) in stores() {
            let t = task(&store, "Report");
            let s = session(&store);
            assert_eq!(store.today_session_task(s).unwrap(), None, "{mode}");
            assert_eq!(store.today_link_session(s, &t.key, NOW).unwrap(), None);
            assert_eq!(store.today_session_task(s).unwrap(), Some(t.id.clone()));
            assert_eq!(store.today_task_session_ids(&t.id).unwrap(), vec![s]);
            assert_eq!(store.today_task_session_ids(&t.key).unwrap(), vec![s]);
            assert_eq!(
                store.today_session_links().unwrap(),
                vec![(s, t.id.clone())]
            );
            store.today_unlink_session(s).unwrap();
            assert_eq!(store.today_session_task(s).unwrap(), None, "{mode}");
            assert!(store.today_session_links().unwrap().is_empty());
            store.today_unlink_session(s).unwrap();
        }
    }

    #[test]
    fn a_task_has_many_sessions_newest_first_and_a_session_one_task() {
        for (mode, store) in stores() {
            let first = task(&store, "First");
            let second = task(&store, "Second");
            let (a, b) = (session(&store), session(&store));
            store.today_link_session(a, &first.id, NOW).unwrap();
            store.today_link_session(b, &first.id, NOW + 10).unwrap();
            assert_eq!(
                store.today_task_session_ids(&first.id).unwrap(),
                vec![b, a],
                "{mode}"
            );
            let moved = store.today_link_session(a, &second.id, NOW + 20).unwrap();
            assert_eq!(moved, Some(first.id.clone()), "{mode}");
            assert_eq!(store.today_task_session_ids(&first.id).unwrap(), vec![b]);
            assert_eq!(store.today_task_session_ids(&second.id).unwrap(), vec![a]);
            assert_eq!(store.today_session_task(a).unwrap(), Some(second.id));
        }
    }

    #[test]
    fn linking_to_the_same_task_again_is_a_no_op() {
        for (mode, store) in stores() {
            let t = task(&store, "Report");
            let (a, b) = (session(&store), session(&store));
            store.today_link_session(a, &t.id, NOW).unwrap();
            store.today_link_session(b, &t.id, NOW + 5).unwrap();
            assert_eq!(store.today_link_session(a, &t.key, NOW + 99).unwrap(), None);
            // linked_at was kept, so a is still older than b.
            assert_eq!(
                store.today_task_session_ids(&t.id).unwrap(),
                vec![b, a],
                "{mode}"
            );
        }
    }

    #[test]
    fn linking_to_an_unknown_task_fails() {
        for (mode, store) in stores() {
            let s = session(&store);
            let error = store
                .today_link_session(s, "task-missing", NOW)
                .unwrap_err();
            assert_eq!(error, "Task not found", "{mode}");
            assert_eq!(store.today_session_task(s).unwrap(), None);
            assert!(store
                .today_task_session_ids("task-missing")
                .unwrap()
                .is_empty());
        }
    }

    #[test]
    fn a_deleted_session_loses_its_link_and_an_archived_one_keeps_it() {
        for (mode, mut store) in stores() {
            let t = task(&store, "Report");
            let (kept, gone) = (session(&store), session(&store));
            store.today_link_session(kept, &t.id, NOW).unwrap();
            store.today_link_session(gone, &t.id, NOW + 1).unwrap();
            store.set_archived(kept, true).unwrap();
            assert_eq!(
                store.today_task_session_ids(&t.id).unwrap(),
                vec![gone, kept]
            );
            store.delete(gone).unwrap();
            assert_eq!(
                store.today_task_session_ids(&t.id).unwrap(),
                vec![kept],
                "{mode}"
            );
            assert_eq!(store.today_session_task(gone).unwrap(), None);
        }
    }

    #[test]
    fn backfill_links_sessions_whose_first_message_opens_with_the_mention() {
        for (mode, mut store) in stores() {
            let t = task(&store, "Report");
            let other = task(&store, "Other");
            let sent = session(&store);
            send(
                &mut store,
                sent,
                &[&format!("  @today/{} fix it", t.key), "ok"],
            );
            let later = session(&store);
            send(
                &mut store,
                later,
                &[&format!("see @today/{} please", t.key)],
            );
            let unknown = session(&store);
            send(&mut store, unknown, &["@today/nope-1 hello"]);
            let second = session(&store);
            send(
                &mut store,
                second,
                &["hello", "hi", &format!("@today/{} now", t.key)],
            );
            let existing = session(&store);
            send(&mut store, existing, &[&format!("@today/{} x", t.key)]);
            store.today_link_session(existing, &other.id, NOW).unwrap();
            let plain = session(&store);
            let unsent = session(&store);
            draft(&store, unsent, &format!("@today/{} ", other.key));
            let _ = plain;

            let linked = store.today_backfill_session_links(NOW + 1).unwrap();
            assert_eq!(linked, 2, "{mode}");
            assert_eq!(store.today_session_task(sent).unwrap(), Some(t.id.clone()));
            assert_eq!(
                store.today_session_task(unsent).unwrap(),
                Some(other.id.clone())
            );
            assert_eq!(store.today_session_task(later).unwrap(), None);
            assert_eq!(store.today_session_task(unknown).unwrap(), None);
            assert_eq!(store.today_session_task(second).unwrap(), None);
            assert_eq!(store.today_session_task(existing).unwrap(), Some(other.id));
        }
    }

    #[test]
    fn a_sent_message_wins_over_a_draft() {
        for (mode, mut store) in stores() {
            let t = task(&store, "Report");
            let o = task(&store, "Other");
            let s = session(&store);
            send(&mut store, s, &["no mention here"]);
            draft(&store, s, &format!("@today/{} ", t.key));
            assert_eq!(
                store.today_backfill_session_links(NOW).unwrap(),
                0,
                "{mode}"
            );
            assert_eq!(store.today_session_task(s).unwrap(), None);
            let _ = o;
        }
    }

    #[test]
    fn backfill_skips_an_unreadable_session_and_still_sets_the_guard() {
        // Only the legacy store keeps drafts in settings records that can be malformed.
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let t = task(&store, "Report");
        let bad = session(&store);
        store
            .save_setting(&format!("follow-up:{bad}"), "{ not json @today/x")
            .unwrap();
        let good = session(&store);
        send(&mut store, good, &[&format!("@today/{} go", t.key)]);
        assert_eq!(store.today_backfill_session_links(NOW).unwrap(), 1);
        assert_eq!(store.today_session_task(good).unwrap(), Some(t.id.clone()));
        assert_eq!(store.today_session_task(bad).unwrap(), None);
        assert!(
            store
                .setting("today.sessionLinksBackfilled")
                .unwrap()
                .is_some(),
            "the scan is done, so reads do not repeat it"
        );
    }

    #[test]
    fn backfilled_links_keep_the_sessions_own_order() {
        // Legacy store: its session times can be set directly.
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let t = task(&store, "Report");
        let old = session(&store);
        send(&mut store, old, &[&format!("@today/{} a", t.key)]);
        let new = session(&store);
        send(&mut store, new, &[&format!("@today/{} b", t.key)]);
        for (id, at) in [(old, 1_000), (new, 2_000)] {
            store
                .conn
                .execute(
                    "UPDATE native_sessions SET updated_at = ?2 WHERE id = ?1",
                    [id, at],
                )
                .unwrap();
        }
        store.today_backfill_session_links(NOW).unwrap();
        assert_eq!(store.today_task_session_ids(&t.id).unwrap(), vec![new, old]);
    }

    #[test]
    fn backfill_runs_once_and_readers_run_it_lazily() {
        for (mode, mut store) in stores() {
            let t = task(&store, "Report");
            let s = session(&store);
            send(&mut store, s, &[&format!("@today/{} go", t.key)]);
            // A reader links it without an explicit call.
            assert_eq!(
                store.today_session_task(s).unwrap(),
                Some(t.id.clone()),
                "{mode}"
            );
            store.today_unlink_session(s).unwrap();
            // The guard is set, so a second run does not bring it back.
            assert_eq!(store.today_backfill_session_links(NOW).unwrap(), 0);
            assert_eq!(store.today_session_task(s).unwrap(), None);
            assert!(store.today_session_links().unwrap().is_empty());
        }
    }

    #[test]
    fn a_read_only_store_reads_links_and_never_writes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shared.sqlite");
        let writer = Store::open_monocode(&path).unwrap();
        let s = session(&writer);
        // No Today tables yet: a read-only reader sees nothing and writes nothing.
        let reader = Store::read_monocode(&path).unwrap();
        assert!(reader.today_session_links().unwrap().is_empty());
        assert_eq!(reader.today_backfill_session_links(NOW).unwrap(), 0);
        assert_eq!(
            reader.setting("today.sessionLinksBackfilled").unwrap(),
            None
        );
        assert!(reader.today_link_session(s, "x", NOW).is_err());
        let t = task(&writer, "Report");
        writer.today_link_session(s, &t.id, NOW).unwrap();
        let reader = Store::read_monocode(&path).unwrap();
        assert_eq!(reader.today_session_links().unwrap(), vec![(s, t.id)]);
        assert!(reader.today_unlink_session(s).is_err());
    }

    #[test]
    fn the_shared_session_id_is_stable_across_reopening() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shared.sqlite");
        let store = Store::open_monocode(&path).unwrap();
        let t = task(&store, "Report");
        let s = session(&store);
        store.today_link_session(s, &t.id, NOW).unwrap();
        drop(store);
        let store = Store::open_monocode(&path).unwrap();
        let ids: Vec<i64> = store.sessions().unwrap().iter().map(|s| s.id).collect();
        assert_eq!(ids, vec![s]);
        assert_eq!(store.today_task_session_ids(&t.id).unwrap(), vec![s]);
    }
}
