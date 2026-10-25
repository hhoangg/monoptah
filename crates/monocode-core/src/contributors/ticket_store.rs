//! SQLite store for scored tickets (ClickUp) on the Contributors page.
//!
//! Tickets are not per repository: they hang off a ClickUp workspace, so these
//! tables are never touched by `store::clear_repo`. Fetch writes a
//! `StoredTicket` row for every ticket updated in the window, whatever its
//! status (`contrib_ticket_rows`); classifying a ticket as delivered or active
//! happens in the TypeScript model. Assessment writes the rubric result and the
//! manual override lives next to it, and `load_records` hands the TypeScript
//! model its `TicketRecord`s. The older `contrib_tickets` table (closed tickets
//! only) is left in place and no longer used.

use rusqlite::{params, Connection, Transaction};
use serde::{Deserialize, Serialize};

use super::store::{json, query_in, sql, unjson};

/// Every ticket in this plan comes from ClickUp.
const SOURCE: &str = "clickup";

const LEVELS: [&str; 5] = ["XS", "S", "M", "L", "XL"];

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS contrib_tickets (
  source TEXT NOT NULL,
  ticket_id TEXT NOT NULL,
  workspace_id TEXT NOT NULL,
  closed_epoch INTEGER NOT NULL,
  parent_id TEXT,
  ticket_json TEXT NOT NULL,
  PRIMARY KEY (source, ticket_id)
);
CREATE INDEX IF NOT EXISTS contrib_tickets_closed_idx
  ON contrib_tickets (workspace_id, closed_epoch);
CREATE INDEX IF NOT EXISTS contrib_tickets_parent_idx
  ON contrib_tickets (parent_id);
CREATE TABLE IF NOT EXISTS contrib_ticket_rows (
  source TEXT NOT NULL,
  ticket_id TEXT NOT NULL,
  workspace_id TEXT NOT NULL,
  updated_epoch INTEGER NOT NULL,
  parent_id TEXT,
  ticket_json TEXT NOT NULL,
  PRIMARY KEY (source, ticket_id)
);
CREATE INDEX IF NOT EXISTS contrib_ticket_rows_updated_idx
  ON contrib_ticket_rows (workspace_id, updated_epoch);
CREATE INDEX IF NOT EXISTS contrib_ticket_rows_parent_idx
  ON contrib_ticket_rows (parent_id);
CREATE TABLE IF NOT EXISTS contrib_ticket_assessments (
  source TEXT NOT NULL,
  ticket_id TEXT NOT NULL,
  assessment_json TEXT,
  override_level TEXT,
  override_note TEXT,
  PRIMARY KEY (source, ticket_id)
);
CREATE TABLE IF NOT EXISTS contrib_ticket_coverage (
  workspace_id TEXT NOT NULL,
  spaces_key TEXT NOT NULL,
  covered_since INTEGER NOT NULL,
  covered_until INTEGER NOT NULL,
  fetched_at INTEGER NOT NULL,
  PRIMARY KEY (workspace_id, spaces_key)
);
";

/// Creates the ticket tables. Safe to run on every start.
///
/// `contrib_ticket_coverage` first shipped keyed by workspace alone. It only
/// holds derived data (what the fetches have covered), so an older shape is
/// dropped and rebuilt; the next fetch of each window fills it again.
pub(super) fn ensure_schema(conn: &Connection) -> rusqlite::Result<()> {
    let count = |filter: &str| -> rusqlite::Result<i64> {
        conn.query_row(
            &format!("SELECT COUNT(*) FROM pragma_table_info('contrib_ticket_coverage'){filter}"),
            [],
            |row| row.get(0),
        )
    };
    if count("")? > 0 && count(" WHERE name = 'spaces_key'")? == 0 {
        conn.execute_batch("DROP TABLE contrib_ticket_coverage")?;
    }
    conn.execute_batch(SCHEMA)
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TicketAssignee {
    pub id: String,
    pub name: String,
    pub email: String,
}

/// A ticket as fetch stores it, including what the TypeScript model never
/// sees (`status_type`, `parent_id`, `description`).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StoredTicket {
    pub source: String,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_id: Option<String>,
    pub title: String,
    pub url: String,
    pub workspace_id: String,
    pub space_id: String,
    pub status: String,
    pub status_type: String,
    pub updated_at: String,
    pub updated_epoch: i64,
    /// When the ticket was done; only set for a done or closed status.
    pub done_at: Option<String>,
    /// When the ticket entered its current status, from ClickUp's status
    /// history; `None` when the history is not available.
    pub status_since: Option<String>,
    pub parent_id: Option<String>,
    pub assignees: Vec<TicketAssignee>,
    pub tags: Vec<String>,
    pub description: String,
}

/// Each criterion is scored 0 to 2 by the rubric.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Criteria {
    pub ambiguity: u8,
    pub investigation: u8,
    pub scope: u8,
    pub risk: u8,
    pub coordination: u8,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Reasons {
    pub ambiguity: String,
    pub investigation: String,
    pub scope: String,
    pub risk: String,
    pub coordination: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StoredAssessment {
    pub criteria: Criteria,
    pub reasons: Reasons,
    pub rubric_version: u32,
    pub model: String,
    pub runner: String,
    pub assessed_at: String,
}

/// Exactly the TypeScript `TicketRecord`. The description is not sent.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TicketRecord {
    pub source: String,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_id: Option<String>,
    pub title: String,
    pub url: String,
    pub workspace_id: String,
    pub space_id: String,
    pub status: String,
    pub status_type: String,
    pub updated_at: String,
    pub done_at: Option<String>,
    pub status_since: Option<String>,
    pub parent_id: Option<String>,
    /// Some stored ticket has this one as its parent.
    pub has_subtasks: bool,
    pub assignees: Vec<TicketAssignee>,
    pub tags: Vec<String>,
    pub assessment: Option<StoredAssessment>,
    pub override_level: Option<String>,
    pub override_note: Option<String>,
}

impl StoredTicket {
    fn into_record(
        self,
        has_subtasks: bool,
        assessment: Option<StoredAssessment>,
        override_level: Option<String>,
        override_note: Option<String>,
    ) -> TicketRecord {
        TicketRecord {
            source: self.source,
            id: self.id,
            custom_id: self.custom_id,
            title: self.title,
            url: self.url,
            workspace_id: self.workspace_id,
            space_id: self.space_id,
            status: self.status,
            status_type: self.status_type,
            updated_at: self.updated_at,
            done_at: self.done_at,
            status_since: self.status_since,
            parent_id: self.parent_id,
            has_subtasks,
            assignees: self.assignees,
            tags: self.tags,
            assessment,
            override_level,
            override_note,
        }
    }
}

/// Inserts or replaces tickets. Assessments live in their own table, so a
/// re-fetched ticket keeps its assessment and override.
pub(super) fn put_tickets(tx: &Transaction, tickets: &[StoredTicket]) -> Result<(), String> {
    let mut stmt = tx
        .prepare(
            "INSERT OR REPLACE INTO contrib_ticket_rows
               (source, ticket_id, workspace_id, updated_epoch, parent_id, ticket_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )
        .map_err(sql)?;
    for ticket in tickets {
        stmt.execute(params![
            ticket.source,
            ticket.id,
            ticket.workspace_id,
            ticket.updated_epoch,
            ticket.parent_id,
            json(ticket)?,
        ])
        .map_err(sql)?;
    }
    Ok(())
}

/// Key of a set of spaces in the coverage table: the sorted, comma-joined ids,
/// or `*` for every space (no spaces given, or an empty list).
pub(super) fn spaces_key(space_ids: Option<&[String]>) -> String {
    let mut ids: Vec<&str> = space_ids
        .unwrap_or_default()
        .iter()
        .map(String::as_str)
        .collect();
    if ids.is_empty() {
        return ALL_SPACES.to_string();
    }
    ids.sort_unstable();
    ids.dedup();
    ids.join(",")
}

/// The coverage key of every space.
const ALL_SPACES: &str = "*";

/// Records that a fetch of a workspace's `spaces_key` completed for everything
/// updated since `from` (epoch seconds), as of `until`: the time the listing
/// started, since a ticket updated after that may be missing. The fetch
/// covers `[from, until]`, not the time after it. When that touches or
/// overlaps the interval already recorded, the two are joined; otherwise a
/// gap would be reported as covered, so the new interval replaces the old.
pub(super) fn mark_covered(
    tx: &Transaction,
    workspace_id: &str,
    spaces_key: &str,
    from: i64,
    until: i64,
    fetched_at: i64,
) -> Result<(), String> {
    let existing: Option<(i64, i64)> = tx
        .query_row(
            "SELECT covered_since, covered_until FROM contrib_ticket_coverage
             WHERE workspace_id = ?1 AND spaces_key = ?2",
            params![workspace_id, spaces_key],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map(Some)
        .or_else(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other),
        })
        .map_err(sql)?;
    let (since, until) = match existing {
        Some((old_since, old_until)) if from <= old_until && until >= old_since => {
            (from.min(old_since), until.max(old_until))
        }
        _ => (from, until),
    };
    tx.execute(
        "INSERT INTO contrib_ticket_coverage
           (workspace_id, spaces_key, covered_since, covered_until, fetched_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(workspace_id, spaces_key) DO UPDATE SET
           covered_since = excluded.covered_since,
           covered_until = excluded.covered_until,
           fetched_at = excluded.fetched_at",
        params![workspace_id, spaces_key, since, until, fetched_at],
    )
    .map_err(sql)?;
    Ok(())
}

/// Whether the store holds every ticket of the workspace's spaces updated in
/// `[start, needed_until]`, as of the fetches that completed: some covered
/// interval of the same spaces (or of every space) starts at or before `start`
/// and runs to at least `needed_until`.
pub(super) fn window_covered(
    conn: &Connection,
    workspace_id: &str,
    space_ids: Option<&[String]>,
    start: i64,
    needed_until: i64,
) -> Result<bool, String> {
    conn.query_row(
        "SELECT EXISTS (
           SELECT 1 FROM contrib_ticket_coverage
           WHERE workspace_id = ?1 AND spaces_key IN (?2, ?3)
             AND covered_since <= ?4 AND covered_until >= ?5)",
        params![
            workspace_id,
            spaces_key(space_ids),
            ALL_SPACES,
            start,
            needed_until
        ],
        |row| row.get(0),
    )
    .map_err(sql)
}

/// Ids of the stored tickets of `workspace_id` (inside `space_ids` when it is
/// set) last updated at or after `from_epoch` that are not in `returned`. After
/// a completed fetch of that window these are the tickets the listing no longer
/// shows: archived, moved out of scope or deleted. A missing row proves
/// nothing, so the caller asks ClickUp about each before deleting any.
pub(super) fn unreturned_ids(
    conn: &Connection,
    workspace_id: &str,
    space_ids: Option<&[String]>,
    from_epoch: i64,
    returned: &[StoredTicket],
) -> Result<Vec<String>, String> {
    let keep: std::collections::HashSet<&str> = returned.iter().map(|t| t.id.as_str()).collect();
    let mut stmt = conn
        .prepare(
            "SELECT ticket_id, ticket_json FROM contrib_ticket_rows
             WHERE source = ?1 AND workspace_id = ?2 AND updated_epoch >= ?3",
        )
        .map_err(sql)?;
    let rows = stmt
        .query_map(params![SOURCE, workspace_id, from_epoch], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(sql)?;
    let mut stale = Vec::new();
    for row in rows {
        let (id, ticket_json) = row.map_err(sql)?;
        if keep.contains(id.as_str()) {
            continue;
        }
        if let Some(spaces) = space_ids {
            let ticket: StoredTicket = unjson(&ticket_json)?;
            if !spaces.contains(&ticket.space_id) {
                continue;
            }
        }
        stale.push(id);
    }
    Ok(stale)
}

/// Deletes tickets. Assessments and overrides stay: they survive a close,
/// reopen, close cycle.
pub(super) fn delete_tickets(tx: &Transaction, ids: &[String]) -> Result<(), String> {
    let mut delete = tx
        .prepare("DELETE FROM contrib_ticket_rows WHERE source = ?1 AND ticket_id = ?2")
        .map_err(sql)?;
    for id in ids {
        delete.execute(params![SOURCE, id]).map_err(sql)?;
    }
    Ok(())
}

/// Every ticket of the workspaces last updated at or after `since` (epoch
/// seconds), subtasks included, oldest update first, with its assessment and
/// manual override attached. `has_subtasks` says whether another stored ticket
/// has it as parent.
pub(super) fn load_records(
    conn: &Connection,
    workspace_ids: &[String],
    since: i64,
) -> Result<Vec<TicketRecord>, String> {
    let rows = query_in(
        conn,
        "SELECT t.updated_epoch, t.ticket_json, a.assessment_json, a.override_level, a.override_note,
                EXISTS (SELECT 1 FROM contrib_ticket_rows c
                        WHERE c.source = t.source AND c.parent_id = t.ticket_id)
         FROM contrib_ticket_rows t
         LEFT JOIN contrib_ticket_assessments a
           ON a.source = t.source AND a.ticket_id = t.ticket_id
         WHERE t.updated_epoch >= ?1 AND t.workspace_id IN ({in})",
        &[&since],
        workspace_ids,
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, bool>(5)?,
            ))
        },
    )?;
    let mut records = Vec::with_capacity(rows.len());
    for (epoch, ticket_json, assessment_json, level, note, has_subtasks) in rows {
        let ticket: StoredTicket = unjson(&ticket_json)?;
        let assessment = assessment_json.as_deref().map(unjson).transpose()?;
        records.push((
            epoch,
            ticket.into_record(has_subtasks, assessment, level, note),
        ));
    }
    records.sort_by(|a, b| (a.0, &a.1.id).cmp(&(b.0, &b.1.id)));
    Ok(records.into_iter().map(|(_, record)| record).collect())
}

/// The stored status and `status_since` of each ticket among `ids` that has a
/// date, keyed by ticket id.
pub(super) fn stored_status_since(
    conn: &Connection,
    ids: &[String],
) -> Result<std::collections::HashMap<String, (String, String)>, String> {
    let rows = query_in(
        conn,
        "SELECT ticket_json FROM contrib_ticket_rows
         WHERE source = ?1 AND ticket_id IN ({in})",
        &[&SOURCE],
        ids,
        |row| row.get::<_, String>(0),
    )?;
    let mut found = std::collections::HashMap::new();
    for ticket_json in rows {
        let ticket: StoredTicket = unjson(&ticket_json)?;
        if let Some(since) = ticket.status_since {
            found.insert(ticket.id, (ticket.status, since));
        }
    }
    Ok(found)
}

/// The tickets among `ids` that still need an assessment, each with the titles
/// of its subtasks, in the order of `ids`. A manual override is never
/// reassessed; an existing assessment is only redone when `force` is set.
pub(super) fn tickets_for_assessment(
    conn: &Connection,
    ids: &[String],
    force: bool,
) -> Result<Vec<(StoredTicket, Vec<String>)>, String> {
    let rows = query_in(
        conn,
        "SELECT t.ticket_json, a.assessment_json, a.override_level
         FROM contrib_ticket_rows t
         LEFT JOIN contrib_ticket_assessments a
           ON a.source = t.source AND a.ticket_id = t.ticket_id
         WHERE t.source = ?1 AND t.ticket_id IN ({in})",
        &[&SOURCE],
        ids,
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        },
    )?;
    let mut picked: Vec<StoredTicket> = Vec::new();
    for (ticket_json, assessment_json, level) in rows {
        if level.is_some() || (assessment_json.is_some() && !force) {
            continue;
        }
        picked.push(unjson(&ticket_json)?);
    }
    let position = |id: &str| ids.iter().position(|candidate| candidate == id);
    picked.sort_by_key(|ticket| position(&ticket.id));

    let picked_ids: Vec<String> = picked.iter().map(|ticket| ticket.id.clone()).collect();
    let children = query_in(
        conn,
        "SELECT parent_id, ticket_json FROM contrib_ticket_rows
         WHERE source = ?1 AND parent_id IN ({in})",
        &[&SOURCE],
        &picked_ids,
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    )?;
    let mut subtasks: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for (parent, ticket_json) in children {
        let child: StoredTicket = unjson(&ticket_json)?;
        subtasks.entry(parent).or_default().push(child.title);
    }
    Ok(picked
        .into_iter()
        .map(|ticket| {
            let titles = subtasks.remove(&ticket.id).unwrap_or_default();
            (ticket, titles)
        })
        .collect())
}

/// Upserts the assessment of each ticket id and leaves the override columns
/// alone: a manual level survives a reassessment.
pub(super) fn put_assessments(
    tx: &Transaction,
    rows: &[(String, StoredAssessment)],
) -> Result<(), String> {
    let mut stmt = tx
        .prepare(
            "INSERT INTO contrib_ticket_assessments (source, ticket_id, assessment_json)
             VALUES (?1, ?2, ?3)
             ON CONFLICT (source, ticket_id)
             DO UPDATE SET assessment_json = excluded.assessment_json",
        )
        .map_err(sql)?;
    for (ticket_id, assessment) in rows {
        stmt.execute(params![SOURCE, ticket_id, json(assessment)?])
            .map_err(sql)?;
    }
    Ok(())
}

/// Sets the manual level (and an optional note) of a ticket. `level` of `None`
/// clears both the level and the note.
pub(super) fn set_override(
    conn: &Connection,
    ticket_id: &str,
    level: Option<&str>,
    note: Option<&str>,
) -> Result<(), String> {
    let Some(level) = level else {
        conn.execute(
            "UPDATE contrib_ticket_assessments
             SET override_level = NULL, override_note = NULL
             WHERE source = ?1 AND ticket_id = ?2",
            params![SOURCE, ticket_id],
        )
        .map_err(sql)?;
        return Ok(());
    };
    if !LEVELS.contains(&level) {
        return Err(format!("Unknown ticket level: {level}"));
    }
    conn.execute(
        "INSERT INTO contrib_ticket_assessments (source, ticket_id, override_level, override_note)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (source, ticket_id)
         DO UPDATE SET override_level = excluded.override_level,
                       override_note = excluded.override_note",
        params![SOURCE, ticket_id, level, note],
    )
    .map_err(sql)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contributors::db::ContributorsDb;

    fn ticket(id: &str, workspace: &str, updated_epoch: i64, parent: Option<&str>) -> StoredTicket {
        StoredTicket {
            source: SOURCE.to_string(),
            id: id.to_string(),
            custom_id: Some(format!("CU-{id}")),
            title: format!("Title {id}"),
            url: format!("https://app.clickup.com/t/{id}"),
            workspace_id: workspace.to_string(),
            space_id: "space-1".to_string(),
            status: "closed".to_string(),
            status_type: "closed".to_string(),
            updated_at: format!("epoch-{updated_epoch}"),
            updated_epoch,
            done_at: Some(format!("done-{updated_epoch}")),
            status_since: None,
            parent_id: parent.map(str::to_string),
            assignees: vec![TicketAssignee {
                id: "u1".to_string(),
                name: "Alice".to_string(),
                email: "alice@example.com".to_string(),
            }],
            tags: vec!["bug".to_string()],
            description: "long text".to_string(),
        }
    }

    fn assessment(model: &str) -> StoredAssessment {
        StoredAssessment {
            criteria: Criteria {
                ambiguity: 1,
                investigation: 2,
                scope: 1,
                risk: 0,
                coordination: 2,
            },
            reasons: Reasons {
                ambiguity: "a".to_string(),
                investigation: "i".to_string(),
                scope: "s".to_string(),
                risk: "r".to_string(),
                coordination: "c".to_string(),
            },
            rubric_version: 1,
            model: model.to_string(),
            runner: "claude-code".to_string(),
            assessed_at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    fn put(store: &ContributorsDb, tickets: &[StoredTicket]) {
        let conn = store.lock_conn().unwrap();
        let tx = conn.unchecked_transaction().unwrap();
        put_tickets(&tx, tickets).unwrap();
        tx.commit().unwrap();
    }

    fn assess(store: &ContributorsDb, rows: &[(String, StoredAssessment)]) {
        let conn = store.lock_conn().unwrap();
        let tx = conn.unchecked_transaction().unwrap();
        put_assessments(&tx, rows).unwrap();
        tx.commit().unwrap();
    }

    fn ids(records: &[TicketRecord]) -> Vec<&str> {
        records.iter().map(|r| r.id.as_str()).collect()
    }

    fn workspaces() -> Vec<String> {
        vec!["w1".to_string()]
    }

    #[test]
    fn load_records_filters_by_updated_epoch() {
        let store = ContributorsDb::open_in_memory().unwrap();
        put(
            &store,
            &[
                ticket("before", "w1", 99, None),
                ticket("start", "w1", 100, None),
                ticket("inside", "w1", 150, None),
                ticket("late", "w1", 5_000_000_000, None),
                ticket("other", "w2", 150, None),
            ],
        );
        let records = store
            .with_read_conn(|conn| load_records(conn, &workspaces(), 100))
            .unwrap();
        assert_eq!(ids(&records), vec!["start", "inside", "late"]);
    }

    #[test]
    fn load_records_includes_subtasks_and_flags_parents() {
        let store = ContributorsDb::open_in_memory().unwrap();
        let mut custom = ticket("sub", "w1", 150, Some("parent"));
        custom.status = "ready for casium testing".to_string();
        custom.status_type = "custom".to_string();
        custom.done_at = None;
        custom.status_since = Some("2026-10-01T00:00:00.000Z".to_string());
        put(
            &store,
            &[
                ticket("parent", "w1", 140, None),
                custom,
                ticket("lonely", "w1", 160, None),
            ],
        );
        let records = store
            .with_read_conn(|conn| load_records(conn, &workspaces(), 0))
            .unwrap();
        assert_eq!(ids(&records), vec!["parent", "sub", "lonely"]);
        let flags: Vec<bool> = records.iter().map(|r| r.has_subtasks).collect();
        assert_eq!(flags, vec![true, false, false]);
        assert_eq!(records[1].parent_id.as_deref(), Some("parent"));
        assert_eq!(records[1].status_type, "custom");
        assert_eq!(records[1].done_at, None);
        assert_eq!(
            records[1].status_since.as_deref(),
            Some("2026-10-01T00:00:00.000Z")
        );
        assert_eq!(records[0].parent_id, None);
        assert_eq!(records[0].updated_at, "epoch-140");
    }

    #[test]
    fn assessments_and_overrides_attach_to_rows_in_the_new_table() {
        let store = ContributorsDb::open_in_memory().unwrap();
        // An assessment and an override written before the ticket is stored
        // (as after a delete or an older fetch) still attach to the same id.
        assess(&store, &[("a".to_string(), assessment("m1"))]);
        {
            let conn = store.lock_conn().unwrap();
            set_override(&conn, "b", Some("XL"), Some("big")).unwrap();
        }
        put(
            &store,
            &[ticket("a", "w1", 110, None), ticket("b", "w1", 120, None)],
        );
        let records = store
            .with_read_conn(|conn| load_records(conn, &workspaces(), 0))
            .unwrap();
        assert_eq!(records[0].assessment, Some(assessment("m1")));
        assert_eq!(records[0].override_level, None);
        assert_eq!(records[1].assessment, None);
        assert_eq!(records[1].override_level.as_deref(), Some("XL"));
        assert_eq!(records[1].override_note.as_deref(), Some("big"));
    }

    #[test]
    fn load_records_joins_assessment_and_override() {
        let store = ContributorsDb::open_in_memory().unwrap();
        put(
            &store,
            &[
                ticket("a", "w1", 110, None),
                ticket("b", "w1", 120, None),
                ticket("c", "w1", 130, None),
            ],
        );
        assess(&store, &[("a".to_string(), assessment("m1"))]);
        {
            let conn = store.lock_conn().unwrap();
            set_override(&conn, "b", Some("L"), Some("bigger than it looks")).unwrap();
        }
        let records = store
            .with_read_conn(|conn| load_records(conn, &workspaces(), 0))
            .unwrap();
        assert_eq!(records[0].assessment, Some(assessment("m1")));
        assert_eq!(records[0].override_level, None);
        assert_eq!(records[1].assessment, None);
        assert_eq!(records[1].override_level.as_deref(), Some("L"));
        assert_eq!(
            records[1].override_note.as_deref(),
            Some("bigger than it looks")
        );
        assert_eq!(records[2].assessment, None);
        assert_eq!(records[2].override_level, None);
        assert_eq!(records[2].override_note, None);
    }

    #[test]
    fn put_assessments_keeps_an_existing_override() {
        let store = ContributorsDb::open_in_memory().unwrap();
        put(&store, &[ticket("a", "w1", 110, None)]);
        {
            let conn = store.lock_conn().unwrap();
            set_override(&conn, "a", Some("XL"), Some("note")).unwrap();
        }
        assess(&store, &[("a".to_string(), assessment("m1"))]);
        assess(&store, &[("a".to_string(), assessment("m2"))]);
        let records = store
            .with_read_conn(|conn| load_records(conn, &workspaces(), 0))
            .unwrap();
        assert_eq!(records[0].assessment, Some(assessment("m2")));
        assert_eq!(records[0].override_level.as_deref(), Some("XL"));
        assert_eq!(records[0].override_note.as_deref(), Some("note"));
    }

    #[test]
    fn tickets_for_assessment_skips_assessed_unless_forced_and_always_skips_overridden() {
        let store = ContributorsDb::open_in_memory().unwrap();
        put(
            &store,
            &[
                ticket("fresh", "w1", 110, None),
                ticket("done", "w1", 120, None),
                ticket("manual", "w1", 130, None),
            ],
        );
        assess(&store, &[("done".to_string(), assessment("m1"))]);
        {
            let conn = store.lock_conn().unwrap();
            set_override(&conn, "manual", Some("S"), None).unwrap();
        }
        let all: Vec<String> = ["fresh", "done", "manual", "missing"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let pick = |force: bool| -> Vec<String> {
            store
                .with_read_conn(|conn| tickets_for_assessment(conn, &all, force))
                .unwrap()
                .into_iter()
                .map(|(t, _)| t.id)
                .collect()
        };
        assert_eq!(pick(false), vec!["fresh"]);
        assert_eq!(pick(true), vec!["fresh", "done"]);
    }

    #[test]
    fn tickets_for_assessment_returns_subtask_titles() {
        let store = ContributorsDb::open_in_memory().unwrap();
        put(
            &store,
            &[
                ticket("parent", "w1", 110, None),
                ticket("s1", "w1", 105, Some("parent")),
                ticket("s2", "w1", 106, Some("parent")),
                ticket("lonely", "w1", 120, None),
            ],
        );
        let wanted = vec!["parent".to_string(), "lonely".to_string()];
        let picked = store
            .with_read_conn(|conn| tickets_for_assessment(conn, &wanted, false))
            .unwrap();
        assert_eq!(picked.len(), 2);
        assert_eq!(picked[0].0.id, "parent");
        let mut titles = picked[0].1.clone();
        titles.sort();
        assert_eq!(titles, vec!["Title s1", "Title s2"]);
        assert_eq!(picked[1].0.id, "lonely");
        assert!(picked[1].1.is_empty());
    }

    #[test]
    fn set_override_none_clears_level_and_note() {
        let store = ContributorsDb::open_in_memory().unwrap();
        put(&store, &[ticket("a", "w1", 110, None)]);
        assess(&store, &[("a".to_string(), assessment("m1"))]);
        {
            let conn = store.lock_conn().unwrap();
            set_override(&conn, "a", Some("M"), Some("why")).unwrap();
            set_override(&conn, "a", None, Some("ignored")).unwrap();
            assert!(set_override(&conn, "a", Some("HUGE"), None).is_err());
        }
        let records = store
            .with_read_conn(|conn| load_records(conn, &workspaces(), 0))
            .unwrap();
        assert_eq!(records[0].override_level, None);
        assert_eq!(records[0].override_note, None);
        assert_eq!(records[0].assessment, Some(assessment("m1")));
    }

    fn unreturned(
        store: &ContributorsDb,
        spaces: Option<&[String]>,
        returned: &[StoredTicket],
    ) -> Vec<String> {
        let mut found = store
            .with_read_conn(|conn| unreturned_ids(conn, "w1", spaces, 100, returned))
            .unwrap();
        found.sort();
        found
    }

    #[test]
    fn unreturned_ids_lists_only_unreturned_tickets_updated_since_the_start() {
        let store = ContributorsDb::open_in_memory().unwrap();
        let mut elsewhere = ticket("other-space", "w1", 150, None);
        elsewhere.space_id = "space-2".to_string();
        put(
            &store,
            &[
                ticket("before", "w1", 99, None),
                ticket("kept", "w1", 150, None),
                ticket("reopened", "w1", 150, None),
                ticket("edge-start", "w1", 100, None),
                ticket("edge-end", "w1", 200, None),
                ticket("after", "w1", 201, None),
                ticket("other-ws", "w2", 150, None),
                elsewhere,
            ],
        );
        let returned = vec![ticket("kept", "w1", 150, None)];
        assert_eq!(
            unreturned(&store, None, &returned),
            vec!["after", "edge-end", "edge-start", "other-space", "reopened"]
        );
    }

    #[test]
    fn unreturned_ids_respects_narrowed_spaces() {
        let store = ContributorsDb::open_in_memory().unwrap();
        let mut elsewhere = ticket("other-space", "w1", 150, None);
        elsewhere.space_id = "space-2".to_string();
        put(&store, &[ticket("inside", "w1", 150, None), elsewhere]);
        let spaces = vec!["space-1".to_string()];
        assert_eq!(unreturned(&store, Some(&spaces), &[]), vec!["inside"]);
    }

    #[test]
    fn delete_tickets_removes_rows_and_keeps_assessments() {
        let store = ContributorsDb::open_in_memory().unwrap();
        put(
            &store,
            &[ticket("a", "w1", 150, None), ticket("b", "w1", 150, None)],
        );
        assess(&store, &[("a".to_string(), assessment("m1"))]);
        {
            let conn = store.lock_conn().unwrap();
            let tx = conn.unchecked_transaction().unwrap();
            delete_tickets(&tx, &["a".to_string()]).unwrap();
            tx.commit().unwrap();
        }
        let records = store
            .with_read_conn(|conn| load_records(conn, &workspaces(), 0))
            .unwrap();
        assert_eq!(ids(&records), vec!["b"]);
        // Closing the ticket again brings its assessment back.
        put(&store, &[ticket("a", "w1", 150, None)]);
        let records = store
            .with_read_conn(|conn| load_records(conn, &workspaces(), 0))
            .unwrap();
        assert_eq!(records[0].assessment, Some(assessment("m1")));
    }

    #[test]
    fn ticket_record_serializes_camel_case() {
        let stored = ticket("a", "w1", 110, None);
        let record = TicketRecord {
            source: stored.source.clone(),
            id: stored.id.clone(),
            custom_id: stored.custom_id.clone(),
            title: stored.title.clone(),
            url: stored.url.clone(),
            workspace_id: stored.workspace_id.clone(),
            space_id: stored.space_id.clone(),
            status: stored.status.clone(),
            status_type: stored.status_type.clone(),
            updated_at: stored.updated_at.clone(),
            done_at: stored.done_at.clone(),
            status_since: None,
            parent_id: None,
            has_subtasks: true,
            assignees: stored.assignees.clone(),
            tags: stored.tags.clone(),
            assessment: Some(assessment("m1")),
            override_level: None,
            override_note: None,
        };
        let value = serde_json::to_value(&record).unwrap();
        for key in [
            "customId",
            "statusType",
            "updatedAt",
            "doneAt",
            "statusSince",
            "parentId",
            "hasSubtasks",
            "workspaceId",
            "spaceId",
            "overrideLevel",
            "overrideNote",
        ] {
            assert!(value.get(key).is_some(), "missing {key}");
        }
        assert!(value["overrideLevel"].is_null());
        assert!(value["statusSince"].is_null());
        assert!(value["parentId"].is_null());
        assert_eq!(value["hasSubtasks"], true);
        assert_eq!(value["assessment"]["rubricVersion"], 1);
        assert_eq!(value["assessment"]["assessedAt"], "2026-01-01T00:00:00Z");
        assert!(value.get("description").is_none());

        let without = TicketRecord {
            custom_id: None,
            ..record
        };
        let value = serde_json::to_value(&without).unwrap();
        assert!(value.get("customId").is_none());
    }

    fn cover(store: &ContributorsDb, spaces: &str, from: i64, until: i64) {
        let conn = store.lock_conn().unwrap();
        let tx = conn.unchecked_transaction().unwrap();
        mark_covered(&tx, "w1", spaces, from, until, until).unwrap();
        tx.commit().unwrap();
    }

    fn interval(store: &ContributorsDb, spaces: &str) -> Option<(i64, i64)> {
        store
            .with_read_conn(|conn| {
                conn.query_row(
                    "SELECT covered_since, covered_until FROM contrib_ticket_coverage
                     WHERE workspace_id = 'w1' AND spaces_key = ?1",
                    params![spaces],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map(Some)
                .or_else(|error| match error {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    other => Err(other),
                })
                .map_err(sql)
            })
            .unwrap()
    }

    fn covers(
        store: &ContributorsDb,
        workspace: &str,
        spaces: Option<&[&str]>,
        start: i64,
        to: i64,
    ) -> bool {
        let spaces: Option<Vec<String>> =
            spaces.map(|ids| ids.iter().map(|id| id.to_string()).collect());
        store
            .with_read_conn(|conn| window_covered(conn, workspace, spaces.as_deref(), start, to))
            .unwrap()
    }

    #[test]
    fn the_spaces_key_is_sorted_and_joined_or_a_star_for_every_space() {
        let ids = |list: &[&str]| -> Vec<String> { list.iter().map(|s| s.to_string()).collect() };
        assert_eq!(spaces_key(None), "*");
        assert_eq!(spaces_key(Some(&[])), "*");
        assert_eq!(spaces_key(Some(&ids(&["b", "a"]))), "a,b");
        assert_eq!(spaces_key(Some(&ids(&["a", "b", "a"]))), "a,b");
        assert_eq!(spaces_key(Some(&ids(&["a"]))), "a");
    }

    #[test]
    fn nothing_is_covered_before_any_fetch_completed() {
        let store = ContributorsDb::open_in_memory().unwrap();
        assert_eq!(interval(&store, "*"), None);
        assert!(!covers(&store, "w1", None, 0, 0));
    }

    #[test]
    fn a_contiguous_fetch_extends_the_interval() {
        let store = ContributorsDb::open_in_memory().unwrap();
        // Aug fetched on the 15th, then Sep from the 1st on the 20th of Sep.
        cover(&store, "*", 100, 150);
        // From 140 reaches back into what is covered: the two join.
        cover(&store, "*", 140, 200);
        assert_eq!(interval(&store, "*"), Some((100, 200)));
        // An earlier start extends backwards.
        cover(&store, "*", 50, 210);
        assert_eq!(interval(&store, "*"), Some((50, 210)));
        // Touching counts as contiguous.
        cover(&store, "*", 210, 300);
        assert_eq!(interval(&store, "*"), Some((50, 300)));
    }

    #[test]
    fn a_fetch_after_a_gap_replaces_the_interval() {
        let store = ContributorsDb::open_in_memory().unwrap();
        cover(&store, "*", 100, 150);
        // From 200 starts after the covered range ended: 151..199 was never
        // fetched, so the old interval must not be kept.
        cover(&store, "*", 200, 260);
        assert_eq!(interval(&store, "*"), Some((200, 260)));
        assert!(!covers(&store, "w1", None, 100, 150));
        assert!(covers(&store, "w1", None, 200, 260));
    }

    #[test]
    fn a_fetch_that_ended_before_the_interval_does_not_join_it() {
        let store = ContributorsDb::open_in_memory().unwrap();
        cover(&store, "*", 500, 600);
        cover(&store, "*", 100, 200);
        assert_eq!(interval(&store, "*"), Some((100, 200)));
    }

    #[test]
    fn a_window_is_covered_only_when_the_interval_spans_all_of_it() {
        let store = ContributorsDb::open_in_memory().unwrap();
        cover(&store, "*", 100, 300);
        assert!(covers(&store, "w1", None, 100, 300));
        assert!(covers(&store, "w1", None, 150, 250));
        // Starts before the interval, or runs past its end.
        assert!(!covers(&store, "w1", None, 99, 300));
        assert!(!covers(&store, "w1", None, 100, 301));
        assert!(!covers(&store, "w2", None, 100, 300));
    }

    #[test]
    fn a_narrowed_fetch_covers_only_its_own_spaces() {
        let store = ContributorsDb::open_in_memory().unwrap();
        cover(&store, "a", 100, 300);
        assert!(covers(&store, "w1", Some(&["a"]), 100, 300));
        // Another set, a wider one and every space are not covered by it.
        assert!(!covers(&store, "w1", Some(&["a", "b"]), 100, 300));
        assert!(!covers(&store, "w1", Some(&["b"]), 100, 300));
        assert!(!covers(&store, "w1", None, 100, 300));
        // The order of the ids does not matter.
        cover(&store, "a,b", 100, 300);
        assert!(covers(&store, "w1", Some(&["b", "a"]), 100, 300));
    }

    #[test]
    fn an_every_space_interval_satisfies_a_narrower_key() {
        let store = ContributorsDb::open_in_memory().unwrap();
        cover(&store, "*", 100, 300);
        assert!(covers(&store, "w1", Some(&["a"]), 100, 300));
        assert!(covers(&store, "w1", Some(&["a", "b"]), 150, 250));
        assert!(!covers(&store, "w1", Some(&["a"]), 50, 300));
    }

    #[test]
    fn intervals_of_different_spaces_are_kept_apart() {
        let store = ContributorsDb::open_in_memory().unwrap();
        cover(&store, "a", 100, 300);
        cover(&store, "b", 500, 600);
        assert_eq!(interval(&store, "a"), Some((100, 300)));
        assert_eq!(interval(&store, "b"), Some((500, 600)));
    }

    #[test]
    fn an_older_coverage_table_is_rebuilt() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE contrib_ticket_coverage (
               workspace_id TEXT PRIMARY KEY,
               covered_since INTEGER NOT NULL,
               fetched_at INTEGER NOT NULL);
             INSERT INTO contrib_ticket_coverage VALUES ('w1', 1, 2);",
        )
        .unwrap();
        ensure_schema(&conn).unwrap();
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM contrib_ticket_coverage", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(rows, 0);
        conn.execute(
            "INSERT INTO contrib_ticket_coverage VALUES ('w1', '*', 1, 2, 3)",
            [],
        )
        .unwrap();
        // The current shape is left alone on the next start.
        ensure_schema(&conn).unwrap();
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM contrib_ticket_coverage", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(rows, 1);
    }
}
