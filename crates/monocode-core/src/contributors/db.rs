//! The Contributors database: its own pair of connections on the shared
//! `monocode.db` file, like `automations::Service`. The tables are
//! `CREATE TABLE IF NOT EXISTS`, the same DDL the Tauri app runs, and the shell
//! store's lock is never taken: sync batches wait on this write connection only.

use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use rusqlite::Connection;

use super::store;

const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// A write connection and, for file databases, a query-only read connection so
/// a long read never waits for a sync batch (WAL lets both run at once).
pub struct ContributorsDb {
    write: Mutex<Connection>,
    read: Option<Mutex<Connection>>,
}

impl ContributorsDb {
    pub fn open(path: &Path) -> Result<Self, String> {
        // `Store` accepts `:memory:` (shell tests). Two connections to it would be two
        // unrelated databases, the read one without the tables.
        if is_in_memory(path) {
            return Self::open_in_memory();
        }
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
        }
        let write = Connection::open(path).map_err(|e| e.to_string())?;
        write
            .busy_timeout(BUSY_TIMEOUT)
            .map_err(|e| e.to_string())?;
        write
            .execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL;")
            .map_err(|e| e.to_string())?;
        store::ensure_tables(&write).map_err(|e| e.to_string())?;
        let read = Connection::open(path).map_err(|e| e.to_string())?;
        read.busy_timeout(BUSY_TIMEOUT).map_err(|e| e.to_string())?;
        read.execute_batch("PRAGMA query_only = ON;")
            .map_err(|e| e.to_string())?;
        Ok(Self {
            write: Mutex::new(write),
            read: Some(Mutex::new(read)),
        })
    }

    /// A private in-memory database; reads go through the write connection.
    pub fn open_in_memory() -> Result<Self, String> {
        let write = Connection::open_in_memory().map_err(|e| e.to_string())?;
        write
            .execute_batch("PRAGMA foreign_keys = ON;")
            .map_err(|e| e.to_string())?;
        store::ensure_tables(&write).map_err(|e| e.to_string())?;
        Ok(Self {
            write: Mutex::new(write),
            read: None,
        })
    }

    /// Runs `f` on the query-only read connection, so a long read never waits
    /// for or blocks the write connection. An in-memory database falls back to
    /// the write connection. Never call git or the network inside `f`.
    pub fn with_read_conn<T>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T, String>,
    ) -> Result<T, String> {
        match &self.read {
            Some(read) => {
                let conn = read
                    .lock()
                    .map_err(|_| "Contributors read store is locked".to_string())?;
                f(&conn)
            }
            None => {
                let conn = self.lock_conn()?;
                f(&conn)
            }
        }
    }

    pub fn lock_conn(&self) -> Result<MutexGuard<'_, Connection>, String> {
        self.write
            .lock()
            .map_err(|_| "Contributors store is locked".to_string())
    }

    /// For tests that must show a long operation runs without holding the lock.
    #[cfg(test)]
    pub(crate) fn conn_is_free(&self) -> bool {
        self.write.try_lock().is_ok()
    }
}

/// SQLite's private in-memory database names.
fn is_in_memory(path: &Path) -> bool {
    let text = path.to_string_lossy();
    text == ":memory:" || text.starts_with("file::memory:")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_memory_path_is_one_database_for_reads_and_writes() {
        let db = ContributorsDb::open(Path::new(":memory:")).unwrap();
        assert_eq!(db.with_read_conn(|c| Ok(tables(c))).unwrap(), EXPECTED);
    }

    fn tables(conn: &Connection) -> Vec<String> {
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name LIKE 'contrib_%' ORDER BY name")
            .unwrap();
        stmt.query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    const EXPECTED: [&str; 11] = [
        "contrib_commit_prs",
        "contrib_commits",
        "contrib_coverage",
        "contrib_prs",
        "contrib_repos",
        "contrib_reviews",
        "contrib_survival",
        "contrib_ticket_assessments",
        "contrib_ticket_coverage",
        "contrib_ticket_rows",
        "contrib_tickets",
    ];

    #[test]
    fn open_creates_every_table_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("monocode.db");
        drop(ContributorsDb::open(&path).unwrap());
        let db = ContributorsDb::open(&path).unwrap();
        let found = db.with_read_conn(|conn| Ok(tables(conn))).unwrap();
        assert_eq!(found, EXPECTED);
    }

    #[test]
    fn in_memory_has_the_same_tables_and_reads_through_the_write_connection() {
        let db = ContributorsDb::open_in_memory().unwrap();
        assert!(db.read.is_none());
        let found = db.with_read_conn(|conn| Ok(tables(conn))).unwrap();
        assert_eq!(found, EXPECTED);
        // The fallback must not wait on a free write lock it already holds.
        assert!(db.conn_is_free());
    }

    #[test]
    fn read_connection_is_query_only_and_sees_committed_writes() {
        let dir = tempfile::tempdir().unwrap();
        let db = ContributorsDb::open(&dir.path().join("monocode.db")).unwrap();
        db.lock_conn()
            .unwrap()
            .execute(
                "INSERT INTO contrib_repos (git_dir, slug) VALUES ('/r/.git', NULL)",
                [],
            )
            .unwrap();
        let count: i64 = db
            .with_read_conn(|conn| {
                conn.query_row("SELECT COUNT(*) FROM contrib_repos", [], |row| row.get(0))
                    .map_err(|e| e.to_string())
            })
            .unwrap();
        assert_eq!(count, 1);
        let err = db
            .with_read_conn(|conn| {
                conn.execute("DELETE FROM contrib_repos", [])
                    .map_err(|e| e.to_string())
            })
            .unwrap_err();
        assert!(err.contains("readonly"), "{err}");
    }

    #[test]
    fn a_read_does_not_hold_the_write_connection() {
        let dir = tempfile::tempdir().unwrap();
        let db = ContributorsDb::open(&dir.path().join("monocode.db")).unwrap();
        db.with_read_conn(|_| {
            assert!(db.conn_is_free());
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn opens_the_database_other_connections_wrote_to() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("monocode.db");
        let other = Connection::open(&path).unwrap();
        other
            .execute_batch(
                "CREATE TABLE sessions (id TEXT PRIMARY KEY); INSERT INTO sessions VALUES ('a');",
            )
            .unwrap();
        let db = ContributorsDb::open(&path).unwrap();
        let found = db.with_read_conn(|conn| Ok(tables(conn))).unwrap();
        assert_eq!(found, EXPECTED);
        let kept: i64 = other
            .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
            .unwrap();
        assert_eq!(kept, 1);
    }
}
