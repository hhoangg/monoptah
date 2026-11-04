//! A disposable transcript search cache beside the desktop database. Search
//! must not decode every saved transcript per keystroke, and the desktop schema
//! must never be altered, so searchable message text lives in a separate file
//! that is rebuilt whenever its version is unknown.
use rusqlite::{params, Connection, OpenFlags};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

/// Bump when the schema or the indexed text derivation changes.
const VERSION: u64 = 2;

/// Stored in `user_version`. It folds in the crate version so that a change to
/// the transcript projection or `searchable_text` in a new release rebuilds
/// the cache instead of serving rows derived by the old code.
fn stamp() -> i64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in VERSION
        .to_le_bytes()
        .iter()
        .chain(env!("CARGO_PKG_VERSION").as_bytes())
    {
        hash = (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3);
    }
    // Positive and never 0, which marks a database without a schema.
    ((hash >> 33) as i64).max(1)
}

/// One searchable message of a session: its first row index, role and text.
pub struct Message {
    pub row_ix: usize,
    pub role: String,
    pub text: String,
}

/// What the cache knows about one session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Doc {
    pub fingerprint: String,
    /// Some message exceeded the per-message cap when it was indexed.
    pub capped: bool,
}

pub struct Index {
    conn: Connection,
}

fn sql<T>(result: rusqlite::Result<T>) -> Result<T, String> {
    result.map_err(|e| e.to_string())
}

/// The cache file for a database, or None for in-memory databases.
pub fn path_for(database: &Path) -> Option<PathBuf> {
    if database == Path::new(":memory:") {
        return None;
    }
    let name = database.file_name()?.to_string_lossy();
    Some(database.with_file_name(format!("{name}.search.sqlite3")))
}

/// A quoted FTS5 phrase; embedded quotes are doubled.
fn phrase(folded: &str) -> String {
    format!("\"{}\"", folded.replace('"', "\"\""))
}

/// The text the FTS column holds and queries are matched against: Rust's own
/// lowercasing, the same fold the literal check applies.
fn fold(text: &str) -> String {
    text.to_lowercase()
}

/// Trigram matching needs at least three characters of the folded query.
pub fn can_prefilter(query: &str) -> bool {
    fold(query).chars().count() >= 3
}

fn is_corrupt(error: &str) -> bool {
    let error = error.to_lowercase();
    ["not a database", "malformed", "corrupt"]
        .iter()
        .any(|needle| error.contains(needle))
}

impl Index {
    /// Open the cache, recreating it once if the file is damaged. A search
    /// cancelled through `cancel` also interrupts this connection's statements.
    pub fn open(database: &Path, cancel: Arc<AtomicBool>) -> Result<Self, String> {
        let path = path_for(database).ok_or("No search index for in-memory databases")?;
        match Self::open_at(&path, cancel.clone()) {
            Err(error) if is_corrupt(&error) => {
                eprintln!("monoptah: search index is damaged ({error}); rebuilding it");
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                for suffix in ["", "-wal", "-shm"] {
                    let _ = std::fs::remove_file(path.with_file_name(format!("{name}{suffix}")));
                }
                Self::open_at(&path, cancel)
            }
            result => result,
        }
    }

    fn open_at(path: &Path, cancel: Arc<AtomicBool>) -> Result<Self, String> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(path) {
            Ok(_) => (),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(error) => return Err(error.to_string()),
        }
        let conn = sql(Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE,
        ))?;
        sql(conn.busy_timeout(std::time::Duration::from_secs(5)))?;
        sql(conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;"))?;
        // The connection is dropped with the search that opened it, so the
        // handler never outlives its flag.
        sql(conn.progress_handler(2000, Some(move || cancel.load(Ordering::Acquire))))?;
        let index = Self { conn };
        index.ensure_schema()?;
        Ok(index)
    }

    fn ensure_schema(&self) -> Result<(), String> {
        let version = || -> Result<i64, String> {
            sql(self
                .conn
                .query_row("PRAGMA user_version", [], |row| row.get(0)))
        };
        if version()? == stamp() {
            return Ok(());
        }
        sql(self.conn.execute_batch("BEGIN IMMEDIATE"))?;
        let result = (|| {
            // Another window may have rebuilt the cache while this one waited.
            if version()? == stamp() {
                return Ok(());
            }
            sql(self.conn.execute_batch(&format!(
                "DROP TABLE IF EXISTS rows_fts;
                 DROP TABLE IF EXISTS rows;
                 DROP TABLE IF EXISTS docs;
                 CREATE TABLE docs(
                     session_id TEXT PRIMARY KEY,
                     fingerprint TEXT NOT NULL,
                     capped INTEGER NOT NULL
                 );
                 CREATE TABLE rows(
                     id INTEGER PRIMARY KEY,
                     session_id TEXT NOT NULL,
                     row_ix INTEGER NOT NULL,
                     role TEXT NOT NULL,
                     text TEXT NOT NULL,
                     folded TEXT NOT NULL
                 );
                 CREATE INDEX rows_session_idx ON rows(session_id, row_ix);
                 CREATE VIRTUAL TABLE rows_fts USING fts5(
                     folded, content='rows', content_rowid='id',
                     tokenize='trigram case_sensitive 1'
                 );
                 CREATE TRIGGER rows_ai AFTER INSERT ON rows BEGIN
                     INSERT INTO rows_fts(rowid, folded) VALUES (new.id, new.folded);
                 END;
                 CREATE TRIGGER rows_ad AFTER DELETE ON rows BEGIN
                     INSERT INTO rows_fts(rows_fts, rowid, folded)
                         VALUES ('delete', old.id, old.folded);
                 END;
                 PRAGMA user_version={};",
                stamp()
            )))
        })();
        match result {
            Ok(()) => sql(self.conn.execute_batch("COMMIT")),
            Err(error) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }

    /// The indexed sessions and their fingerprints.
    pub fn docs(&self) -> Result<HashMap<String, Doc>, String> {
        let mut query = sql(self
            .conn
            .prepare("SELECT session_id, fingerprint, capped FROM docs"))?;
        let rows = sql(query.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                Doc {
                    fingerprint: row.get(1)?,
                    capped: row.get(2)?,
                },
            ))
        }))?;
        sql(rows.collect())
    }

    /// Replace one session's messages in a single transaction.
    pub fn replace(&self, session_id: &str, doc: &Doc, messages: &[Message]) -> Result<(), String> {
        sql(self.conn.execute_batch("BEGIN IMMEDIATE"))?;
        let result = (|| {
            sql(self
                .conn
                .execute("DELETE FROM rows WHERE session_id=?1", [session_id]))?;
            let mut insert = sql(self.conn.prepare(
                "INSERT INTO rows(session_id,row_ix,role,text,folded) VALUES(?1,?2,?3,?4,?5)",
            ))?;
            for message in messages {
                sql(insert.execute(params![
                    session_id,
                    message.row_ix as i64,
                    message.role,
                    message.text,
                    fold(&message.text)
                ]))?;
            }
            sql(self.conn.execute(
                "INSERT INTO docs(session_id,fingerprint,capped) VALUES(?1,?2,?3)
                 ON CONFLICT(session_id) DO UPDATE SET fingerprint=?2, capped=?3",
                params![session_id, doc.fingerprint, doc.capped],
            ))?;
            Ok(())
        })();
        match result {
            Ok(()) => sql(self.conn.execute_batch("COMMIT")),
            Err(error) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }

    /// Drop every session not in `keep`.
    pub fn retain(&self, keep: &HashSet<String>) -> Result<(), String> {
        for stale in self.docs()?.into_keys().filter(|id| !keep.contains(id)) {
            sql(self.conn.execute_batch("BEGIN IMMEDIATE"))?;
            let result = (|| {
                sql(self
                    .conn
                    .execute("DELETE FROM rows WHERE session_id=?1", [&stale]))?;
                sql(self
                    .conn
                    .execute("DELETE FROM docs WHERE session_id=?1", [&stale]))?;
                Ok::<_, String>(())
            })();
            match result {
                Ok(()) => sql(self.conn.execute_batch("COMMIT"))?,
                Err(error) => {
                    let _ = self.conn.execute_batch("ROLLBACK");
                    return Err(error);
                }
            }
        }
        Ok(())
    }

    /// Ids of the messages whose folded text contains the folded query, as
    /// `(row, id)` per session in row order. Text is fetched later with
    /// `text`, so a common query does not copy the corpus. Only valid when
    /// `can_prefilter(query)`; callers must still verify with the literal check.
    pub fn candidates(&self, query: &str) -> Result<HashMap<String, Vec<(usize, i64)>>, String> {
        let mut out: HashMap<String, Vec<(usize, i64)>> = HashMap::new();
        let mut statement = sql(self.conn.prepare(
            "SELECT session_id,row_ix,id FROM rows
             WHERE id IN (SELECT rowid FROM rows_fts WHERE rows_fts MATCH ?1)",
        ))?;
        let mut rows = sql(statement.query([phrase(&fold(query))]))?;
        while let Some(row) = sql(rows.next())? {
            out.entry(sql(row.get(0))?)
                .or_default()
                .push((sql(row.get::<_, i64>(1))? as usize, sql(row.get(2))?));
        }
        for list in out.values_mut() {
            list.sort_unstable();
        }
        Ok(out)
    }

    pub fn text(&self, id: i64) -> Result<String, String> {
        sql(self
            .conn
            .query_row("SELECT text FROM rows WHERE id=?1", [id], |row| row.get(0)))
    }

    /// Every indexed message of one session, in row order.
    pub fn session_messages(&self, session_id: &str) -> Result<Vec<(usize, String)>, String> {
        let mut statement = sql(self
            .conn
            .prepare("SELECT row_ix,text FROM rows WHERE session_id=?1 ORDER BY row_ix"))?;
        let rows = sql(statement.query_map([session_id], |row| {
            Ok((row.get::<_, i64>(0)? as usize, row.get(1)?))
        }))?;
        sql(rows.collect())
    }
}
