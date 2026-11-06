//! Today persistence. Additive tables, created on first use, so an existing
//! database upgrades in place and an older build simply ignores them. Works on
//! the legacy store and on the shared desktop database alike (see `notes.rs`).
use super::Store;
use crate::today::statuses::{
    builtin, builtin_id, default_statuses, format_color, normalize, parse_color, resolve, DONE_ID,
    MAX_NAME, TODO_ID,
};
use crate::today::tags::{self, ID_PREFIX};
use crate::today::workday::{days_between, is_day, work_day};
use crate::today::{
    self, reconcile, Activity, Actor, CheckReason, ClosedBy, Day, DayMode, DayView, EventKind,
    Group, MonoCaller, NewTask, Note, Priority, Row, Source, Stage, Status, StatusPatch, Summary,
    SyncItem, SyncRequest, SyncStatus, SyncSummary, Tag, TagPatch, Task, TaskEvent, TaskPatch,
    WorkflowStatus, DEFAULT_ROLLOVER_MINUTES,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{de::DeserializeOwned, Serialize};
use std::collections::HashSet;

const ROLLOVER_KEY: &str = "today.rolloverMinutes";
const LAST_DAY_KEY: &str = "today.lastDay";
/// The ordered workflow statuses, JSON. Absent means the defaults.
const STATUSES_KEY: &str = "today.statuses";
/// The ordered tag definitions, JSON. Absent means no tags.
const TAGS_KEY: &str = "today.tags";
/// Set once free text task tags have become definitions (see `ensure_tags_migrated`).
const TAGS_MIGRATED_KEY: &str = "today.tagsMigrated";
const MAX_ROLLOVER_MINUTES: u32 = 720;
const NOTE_MAX: usize = 5_000;
const NOT_FOUND: &str = "Task not found";
const READ_ONLY: &str = "This Monoptah store is read-only";

fn sql(error: rusqlite::Error) -> String {
    error.to_string()
}

/// Tasks are JSON so a new field is safe to add; the other columns only index.
fn ensure_today_tables(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS today_tasks (
           id TEXT PRIMARY KEY,
           key TEXT NOT NULL UNIQUE,
           source_id TEXT,
           status TEXT NOT NULL,
           first_day TEXT NOT NULL,
           snoozed_until TEXT,
           closed_day TEXT,
           json TEXT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS today_tasks_source_idx ON today_tasks (source_id);
         CREATE TABLE IF NOT EXISTS today_days (
           day TEXT NOT NULL,
           task_id TEXT NOT NULL,
           grp TEXT NOT NULL,
           age_days INTEGER NOT NULL,
           json TEXT NOT NULL,
           status_json TEXT,
           PRIMARY KEY (day, task_id)
         );
         CREATE TABLE IF NOT EXISTS today_notes (
           id TEXT PRIMARY KEY,
           task_id TEXT NOT NULL,
           day TEXT NOT NULL,
           at INTEGER NOT NULL,
           mono_id TEXT,
           body TEXT NOT NULL,
           attachments_json TEXT,
           edited_at INTEGER,
           session_id INTEGER
         );
         CREATE INDEX IF NOT EXISTS today_notes_task_idx ON today_notes (task_id, at);
         CREATE TABLE IF NOT EXISTS today_events (
           id TEXT PRIMARY KEY,
           task_id TEXT NOT NULL,
           day TEXT NOT NULL,
           at INTEGER NOT NULL,
           json TEXT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS today_events_task_idx ON today_events (task_id, at);
         CREATE TABLE IF NOT EXISTS today_sources (
           id TEXT PRIMARY KEY,
           json TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS today_session_links (
           session_id INTEGER PRIMARY KEY,
           task_id TEXT NOT NULL,
           linked_at INTEGER NOT NULL
         );
         CREATE INDEX IF NOT EXISTS today_session_links_task_idx
           ON today_session_links (task_id, linked_at);",
    )?;
    // A table made before statuses existed lacks the frozen status column.
    if !has_column(conn, "today_days", "status_json")? {
        conn.execute("ALTER TABLE today_days ADD COLUMN status_json TEXT", [])?;
    }
    // A table made before note attachments existed lacks their column.
    if !has_column(conn, "today_notes", "attachments_json")? {
        conn.execute(
            "ALTER TABLE today_notes ADD COLUMN attachments_json TEXT",
            [],
        )?;
    }
    // A table made before notes could be edited lacks the edit time.
    if !has_column(conn, "today_notes", "edited_at")? {
        conn.execute("ALTER TABLE today_notes ADD COLUMN edited_at INTEGER", [])?;
    }
    // A table made before sessions could write notes lacks the author session.
    if !has_column(conn, "today_notes", "session_id")? {
        conn.execute("ALTER TABLE today_notes ADD COLUMN session_id INTEGER", [])?;
    }
    Ok(())
}

fn has_column(conn: &Connection, table: &str, column: &str) -> rusqlite::Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let names = stmt.query_map([], |row| row.get::<_, String>(1))?;
    for name in names {
        if name? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn text<T: Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

fn group_of(task: &Task, day: &str) -> Group {
    match task.status {
        Status::Open if task.first_day.as_str() < day => Group::CarriedOver,
        Status::Open => Group::Today,
        Status::NeedsCheck => Group::NeedsCheck,
        Status::Done => Group::Done,
        Status::Snoozed => Group::Snoozed,
        Status::Dismissed => Group::Dismissed,
    }
}

fn group_rank(group: Group) -> u8 {
    match group {
        Group::NeedsCheck => 0,
        Group::CarriedOver => 1,
        Group::Today => 2,
        Group::Done => 3,
        Group::Snoozed => 4,
        Group::Dismissed => 5,
    }
}

fn priority_rank(priority: Priority) -> u8 {
    match priority {
        Priority::Urgent => 0,
        Priority::High => 1,
        Priority::Normal => 2,
    }
}

/// Where a status sits in the current list; one the list no longer has (a past
/// day's frozen status) comes after the rest of its stage.
fn status_rank(status: &WorkflowStatus, list: &[WorkflowStatus]) -> usize {
    match list.iter().position(|s| s.id == status.id) {
        Some(index) => index * 2,
        None => {
            let last = list.iter().rposition(|s| s.stage == status.stage);
            last.map_or(list.len() * 2, |index| index * 2 + 1)
        }
    }
}

fn sort_rows(rows: &mut [Row], list: &[WorkflowStatus]) {
    rows.sort_by_cached_key(|row| {
        (
            status_rank(&row.status, list),
            group_rank(row.group),
            priority_rank(row.task.priority),
            row.task.first_day.clone(),
            row.task.title.clone(),
            row.task.id.clone(),
        )
    });
}

fn parse_task(json: &str) -> Option<Task> {
    serde_json::from_str(json).ok()
}

/// Every parseable task; a row that fails to parse is skipped, not fatal.
fn all_tasks(conn: &Connection) -> Result<Vec<Task>, String> {
    let mut stmt = conn
        .prepare("SELECT json FROM today_tasks ORDER BY rowid")
        .map_err(sql)?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(sql)?;
    let mut tasks = Vec::new();
    for json in rows {
        if let Some(task) = parse_task(&json.map_err(sql)?) {
            tasks.push(task);
        }
    }
    Ok(tasks)
}

fn load_task(conn: &Connection, id_or_key: &str) -> Result<Option<Task>, String> {
    let json: Option<String> = conn
        .query_row(
            "SELECT json FROM today_tasks WHERE id = ?1 OR key = ?1 ORDER BY id = ?1 DESC LIMIT 1",
            [id_or_key],
            |row| row.get(0),
        )
        .optional()
        .map_err(sql)?;
    Ok(json.and_then(|json| parse_task(&json)))
}

fn save_task(conn: &Connection, task: &Task) -> Result<(), String> {
    let json = serde_json::to_string(task).map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO today_tasks (id, key, source_id, status, first_day, snoozed_until, closed_day, json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(id) DO UPDATE SET key = excluded.key, source_id = excluded.source_id,
           status = excluded.status, first_day = excluded.first_day,
           snoozed_until = excluded.snoozed_until, closed_day = excluded.closed_day,
           json = excluded.json",
        params![
            task.id,
            task.key,
            task.source_id,
            text(&task.status),
            task.first_day,
            task.snoozed_until,
            task.closed_day,
            json
        ],
    )
    .map_err(sql)?;
    Ok(())
}

/// Freezes the task's state on `day`, with the workflow status it shows then.
/// Callers pass only the current day: a past day is never written.
fn write_record(
    conn: &Connection,
    day: &str,
    task: &Task,
    statuses: &[WorkflowStatus],
) -> Result<(), String> {
    // A done task belongs to the day it closed only.
    if task.status == Status::Done && task.closed_day.as_deref() != Some(day) {
        return Ok(());
    }
    let json = serde_json::to_string(task).map_err(|e| e.to_string())?;
    let status = serde_json::to_string(resolve(task, statuses)).map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO today_days (day, task_id, grp, age_days, json, status_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(day, task_id) DO UPDATE SET grp = excluded.grp,
           age_days = excluded.age_days, json = excluded.json,
           status_json = excluded.status_json",
        params![
            day,
            task.id,
            text(&group_of(task, day)),
            days_between(&task.first_day, day),
            json,
            status
        ],
    )
    .map_err(sql)?;
    Ok(())
}

// --- events ----------------------------------------------------------------------------------

/// Who did it: a Mono, a sync of a source, or (both empty) the user.
#[derive(Clone, Debug, Default)]
struct Who {
    mono_id: Option<String>,
    source_id: Option<String>,
}

fn who_of(actor: &Actor) -> Who {
    match actor {
        Actor::User | Actor::Session(_) => Who::default(),
        Actor::Mono(caller) => Who {
            mono_id: Some(caller.id.clone()),
            source_id: None,
        },
    }
}

/// A fresh event for `task` with the common fields filled.
fn event(task: &Task, kind: EventKind, who: &Who, day: &str, now: u64) -> TaskEvent {
    TaskEvent {
        id: format!("event-{}", uuid::Uuid::new_v4()),
        task_id: task.id.clone(),
        day: day.to_string(),
        at: now,
        kind,
        mono_id: who.mono_id.clone(),
        source_id: who.source_id.clone(),
        from: None,
        to: None,
        check_reason: None,
        detail: None,
        added_tags: Vec::new(),
        removed_tags: Vec::new(),
    }
}

/// The events a change implies. `old` and `new` are the status lists before and
/// after (they differ only while a status edit is in flight).
#[allow(clippy::too_many_arguments)]
fn diff_events(
    before: Option<&Task>,
    after: &Task,
    old: &[WorkflowStatus],
    new: &[WorkflowStatus],
    defs: &[Tag],
    who: &Who,
    day: &str,
    now: u64,
) -> Vec<TaskEvent> {
    let Some(before) = before else {
        return vec![event(after, EventKind::Added, who, day, now)];
    };
    let mut out = Vec::new();
    let (from, to) = (resolve(before, old), resolve(after, new));
    if from != to {
        let mut e = event(after, EventKind::StatusChanged, who, day, now);
        e.from = Some(from.clone());
        e.to = Some(to.clone());
        out.push(e);
    }
    if after.check_reason != before.check_reason {
        match after.check_reason {
            Some(reason) => {
                let mut e = event(after, EventKind::NeedsCheck, who, day, now);
                e.check_reason = Some(reason);
                out.push(e);
            }
            None if !(after.kept_done && !before.kept_done) => {
                out.push(event(after, EventKind::CheckCleared, who, day, now));
            }
            None => {}
        }
    }
    if after.status == Status::Done
        && before.status != Status::Done
        && after.closed_by == Some(ClosedBy::Source)
    {
        let mut e = event(after, EventKind::ClosedBySource, who, day, now);
        e.detail = after.close_reason.clone();
        out.push(e);
    }
    let reopened = matches!(before.status, Status::Done | Status::Dismissed)
        && after.status == Status::Open
        && who.source_id.is_some();
    if reopened {
        out.push(event(after, EventKind::ReopenedBySource, who, day, now));
    }
    if after.status == Status::Snoozed
        && (before.status != Status::Snoozed || before.snoozed_until != after.snoozed_until)
    {
        let mut e = event(after, EventKind::Snoozed, who, day, now);
        e.detail = after.snoozed_until.clone();
        out.push(e);
    }
    if after.status == Status::Dismissed && before.status != Status::Dismissed {
        out.push(event(after, EventKind::Dismissed, who, day, now));
    }
    if after.kept_done && !before.kept_done {
        out.push(event(after, EventKind::KeptDone, who, day, now));
    }
    // Tags compare as sets; the event holds them as the definitions read now.
    let entries = |from: &[String], not_in: &[String]| -> Vec<String> {
        from.iter()
            .filter(|t| !not_in.contains(t))
            .cloned()
            .collect()
    };
    let added = tags::resolve(&entries(&after.tags, &before.tags), defs);
    let removed = tags::resolve(&entries(&before.tags, &after.tags), defs);
    if !added.is_empty() || !removed.is_empty() {
        let mut e = event(after, EventKind::TagsChanged, who, day, now);
        e.added_tags = added;
        e.removed_tags = removed;
        out.push(e);
    }
    out
}

fn insert_events(conn: &Connection, events: &[TaskEvent]) -> Result<(), String> {
    for e in events {
        let json = serde_json::to_string(e).map_err(|err| err.to_string())?;
        conn.execute(
            "INSERT INTO today_events (id, task_id, day, at, json) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                e.id,
                e.task_id,
                e.day,
                e.at.min(i64::MAX as u64) as i64,
                json
            ],
        )
        .map_err(sql)?;
    }
    Ok(())
}

// --- lifecycle, in step with the workflow status ---------------------------------------------

/// The user's (or a Mono acting for them) close: the lifecycle half of Done.
fn finish_task(task: &mut Task, day: &str, now: u64) {
    task.status = Status::Done;
    task.closed_at = Some(now);
    task.closed_day = Some(day.to_string());
    task.close_reason = None;
    // A source never closed it, so a later sync that still lists it flags it
    // instead of reopening.
    task.closed_by = Some(ClosedBy::User);
    task.check_reason = None;
    task.snoozed_until = None;
}

/// The lifecycle half of taking a task back: it is open again.
fn reopen_task(task: &mut Task, day: &str) {
    task.status = Status::Open;
    task.closed_at = None;
    task.closed_day = None;
    task.close_reason = None;
    task.closed_by = None;
    task.check_reason = None;
    task.snoozed_until = None;
    task.source_closed_after_dismiss = false;
    task.kept_done = false;
    if task.first_day.as_str() > day {
        task.first_day = day.to_string();
    }
}

/// A finished status on a task that is leaving the finished state goes to To do.
fn leave_finished(task: &mut Task, statuses: &[WorkflowStatus]) {
    if resolve(task, statuses).stage == Stage::Finished {
        task.status_id = TODO_ID.into();
    }
}

fn load_source(conn: &Connection, id: &str) -> Result<Option<Source>, String> {
    let json: Option<String> = conn
        .query_row(
            "SELECT json FROM today_sources WHERE id = ?1",
            [id],
            |row| row.get(0),
        )
        .optional()
        .map_err(sql)?;
    // An unreadable row still has an owner we cannot see: refuse, never adopt.
    match json {
        Some(json) => serde_json::from_str(&json)
            .map(Some)
            .map_err(|_| format!("Source {id} is unreadable and cannot be synced")),
        None => Ok(None),
    }
}

fn save_source(conn: &Connection, source: &Source) -> Result<(), String> {
    let json = serde_json::to_string(source).map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO today_sources (id, json) VALUES (?1, ?2)
         ON CONFLICT(id) DO UPDATE SET json = excluded.json",
        params![source.id, json],
    )
    .map_err(sql)?;
    Ok(())
}

fn note_from_row(row: &rusqlite::Row) -> rusqlite::Result<Note> {
    Ok(Note {
        id: row.get(0)?,
        task_id: row.get(1)?,
        day: row.get(2)?,
        at: row.get::<_, i64>(3)?.max(0) as u64,
        mono_id: row.get(4)?,
        body: row.get(5)?,
        // Unreadable metadata reads as none rather than hiding the note.
        attachments: row
            .get::<_, Option<String>>(6)?
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default(),
        edited_at: row.get::<_, Option<i64>>(7)?.map(|at| at.max(0) as u64),
        session_id: row.get(8)?,
    })
}

const NOTE_COLUMNS: &str =
    "id, task_id, day, at, mono_id, body, attachments_json, edited_at, session_id";

fn latest_note(conn: &Connection, task_id: &str, day: &str) -> Result<Option<Note>, String> {
    conn.query_row(
        &format!(
            "SELECT {NOTE_COLUMNS} FROM today_notes WHERE task_id = ?1 AND day <= ?2
             ORDER BY at DESC, rowid DESC LIMIT 1"
        ),
        params![task_id, day],
        note_from_row,
    )
    .optional()
    .map_err(sql)
}

fn json_rows<T: DeserializeOwned>(
    conn: &Connection,
    query: &str,
    args: impl rusqlite::Params,
) -> Result<Vec<T>, String> {
    let mut stmt = conn.prepare(query).map_err(sql)?;
    let rows = stmt
        .query_map(args, |row| row.get::<_, String>(0))
        .map_err(sql)?;
    let mut out = Vec::new();
    for json in rows {
        if let Ok(value) = serde_json::from_str(&json.map_err(sql)?) {
            out.push(value);
        }
    }
    Ok(out)
}

/// Whether `actor` may see `task`. A session sees only the task it is linked
/// to (the same link `Store::today_session_task` reads).
fn actor_sees(conn: &Connection, actor: &Actor, task: &Task) -> bool {
    match actor {
        Actor::User => true,
        Actor::Mono(caller) => today::visible_to(task, caller),
        Actor::Session(id) => conn
            .query_row(
                "SELECT 1 FROM today_session_links WHERE session_id = ?1 AND task_id = ?2",
                params![id, task.id],
                |_| Ok(()),
            )
            .optional()
            .map(|found| found.is_some())
            .unwrap_or(false),
    }
}

fn mono_may_set(caller: &MonoCaller, project: &str) -> bool {
    let key = crate::files::path_key(project);
    caller
        .projects
        .iter()
        .any(|own| crate::files::path_key(own) == key)
}

fn check_project(actor: &Actor, project: Option<&str>) -> Result<(), String> {
    match (actor, project) {
        (Actor::Mono(caller), Some(project)) if !mono_may_set(caller, project) => {
            Err("That project is not one of this Mono's projects".into())
        }
        _ => Ok(()),
    }
}

fn clean_text(value: &str, what: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(format!("{what} cannot be empty"));
    }
    Ok(value.to_string())
}

fn valid_time(value: &str) -> bool {
    chrono::NaiveTime::parse_from_str(value, "%H:%M").is_ok() && value.len() == 5
}

fn check_due(due: &Option<String>, due_time: &Option<String>) -> Result<(), String> {
    if let Some(due) = due {
        if !is_day(due) {
            return Err("due must be a date like 2026-07-14".into());
        }
    }
    if let Some(time) = due_time {
        if due.is_none() {
            return Err("due_time needs a due date".into());
        }
        if !valid_time(time) {
            return Err("due_time must be HH:MM".into());
        }
    }
    Ok(())
}

impl Store {
    /// Today's tables need a writable database. A read-only shared store never
    /// creates them, so reads there see an empty ledger until the app has.
    pub(super) fn today_writable(&self) -> Result<(), String> {
        if self.shared.is_some() {
            self.shared_writable().map_err(|_| READ_ONLY.to_string())?;
        }
        Ok(())
    }
    /// `true` when the tables exist (creating them where the store can write).
    pub(super) fn today_ready(&self) -> Result<bool, String> {
        if self.today_writable().is_ok() {
            ensure_today_tables(&self.conn).map_err(sql)?;
            return Ok(true);
        }
        super::shared::has_table(&self.conn, "today_tasks")
    }
    pub(super) fn today_tx<T>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T, String>,
    ) -> Result<T, String> {
        // Immediate: take the write lock up front so the busy timeout applies.
        let tx = rusqlite::Transaction::new_unchecked(
            &self.conn,
            rusqlite::TransactionBehavior::Immediate,
        )
        .map_err(sql)?;
        let value = f(&tx)?;
        tx.commit().map_err(sql)?;
        Ok(value)
    }

    pub fn today_rollover_minutes(&self) -> Result<u32, String> {
        Ok(self
            .setting(ROLLOVER_KEY)?
            .and_then(|value| value.trim().parse::<u32>().ok())
            .filter(|minutes| *minutes <= MAX_ROLLOVER_MINUTES)
            .unwrap_or(DEFAULT_ROLLOVER_MINUTES))
    }
    pub fn save_today_rollover_minutes(&self, minutes: u32) -> Result<(), String> {
        if minutes > MAX_ROLLOVER_MINUTES {
            return Err(format!(
                "Rollover must be between 0 and {MAX_ROLLOVER_MINUTES} minutes"
            ));
        }
        self.save_setting(ROLLOVER_KEY, &minutes.to_string())
    }
    /// Advances to the current work day and returns it: freezes passed days,
    /// returns snoozed tasks, records today. Idempotent; every other method
    /// here calls it first.
    pub fn today_roll(&self, now: u64) -> Result<Day, String> {
        let computed = work_day(now, self.today_rollover_minutes()?);
        let last = self.setting(LAST_DAY_KEY)?.filter(|day| is_day(day));
        // A clock set back must not reopen a day that already passed.
        let day = match &last {
            Some(last) if last.as_str() > computed.as_str() => last.clone(),
            _ => computed,
        };
        if last.as_deref() == Some(day.as_str()) || self.today_writable().is_err() {
            return Ok(day);
        }
        self.today_ready()?;
        let statuses = self.today_statuses()?;
        self.today_tx(|conn| {
            for mut task in all_tasks(conn)? {
                if task.status == Status::Snoozed
                    && task
                        .snoozed_until
                        .as_deref()
                        .is_some_and(|until| until <= day.as_str())
                {
                    task.status = Status::Open;
                    task.snoozed_until = None;
                    task.updated_at = now;
                    save_task(conn, &task)?;
                }
                if matches!(task.status, Status::Open | Status::NeedsCheck) {
                    write_record(conn, &day, &task, &statuses)?;
                }
            }
            Ok(())
        })?;
        self.save_setting(LAST_DAY_KEY, &day)?;
        Ok(day)
    }
    pub fn today_view(&self, day: &str, now: u64) -> Result<DayView, String> {
        if !is_day(day) {
            return Err("Day must look like 2026-07-14".into());
        }
        let current = self.today_roll(now)?;
        let mode = match day.cmp(current.as_str()) {
            std::cmp::Ordering::Less => DayMode::Past,
            std::cmp::Ordering::Equal => DayMode::Current,
            std::cmp::Ordering::Greater => DayMode::Future,
        };
        let mut view = DayView {
            day: day.to_string(),
            current_day: current,
            mode,
            rows: Vec::new(),
            summary: Summary::default(),
            recorded: true,
        };
        if !self.today_ready()? {
            view.recorded = mode != DayMode::Past;
            return Ok(view);
        }
        let conn = &self.conn;
        let statuses = self.today_statuses()?;
        let defs = self.today_tags()?;
        match mode {
            DayMode::Past => {
                // A read-only store written before statuses existed has no column.
                let frozen = if has_column(conn, "today_days", "status_json").map_err(sql)? {
                    "status_json"
                } else {
                    "NULL"
                };
                let mut stmt = conn
                    .prepare(&format!(
                        "SELECT grp, age_days, json, {frozen} FROM today_days WHERE day = ?1"
                    ))
                    .map_err(sql)?;
                let found = stmt
                    .query_map([day], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, Option<String>>(3)?,
                        ))
                    })
                    .map_err(sql)?;
                let mut rows = Vec::new();
                for entry in found {
                    let (group, age, json, frozen) = entry.map_err(sql)?;
                    let (Some(task), Ok(group)) = (
                        parse_task(&json),
                        serde_json::from_value::<Group>(serde_json::Value::String(group)),
                    ) else {
                        continue;
                    };
                    // The status as that day froze it; a record from before statuses
                    // existed reads through the current list.
                    let status = frozen
                        .and_then(|json| serde_json::from_str::<WorkflowStatus>(&json).ok())
                        .unwrap_or_else(|| resolve(&task, &statuses).clone());
                    rows.push(Row {
                        tags: tags::resolve(&task.tags, &defs),
                        status,
                        latest_note: latest_note(conn, &task.id, day)?,
                        age_days: age.max(0) as u32,
                        group,
                        task,
                    });
                }
                view.recorded = !rows.is_empty();
                sort_rows(&mut rows, &statuses);
                view.rows = rows;
            }
            DayMode::Current => {
                let mut rows = Vec::new();
                for task in all_tasks(conn)? {
                    let shown = match task.status {
                        Status::Open | Status::NeedsCheck => true,
                        Status::Done => task.closed_day.as_deref() == Some(day),
                        Status::Snoozed | Status::Dismissed => false,
                    };
                    if !shown {
                        continue;
                    }
                    rows.push(Row {
                        tags: tags::resolve(&task.tags, &defs),
                        status: resolve(&task, &statuses).clone(),
                        latest_note: latest_note(conn, &task.id, day)?,
                        age_days: days_between(&task.first_day, day),
                        group: group_of(&task, day),
                        task,
                    });
                }
                for row in &rows {
                    match row.group {
                        Group::CarriedOver => {
                            view.summary.open += 1;
                            view.summary.carried_over += 1;
                        }
                        Group::Today => view.summary.open += 1,
                        Group::NeedsCheck => view.summary.to_check += 1,
                        _ => {}
                    }
                }
                sort_rows(&mut rows, &statuses);
                view.rows = rows;
            }
            DayMode::Future => {
                let mut rows = Vec::new();
                for task in all_tasks(conn)? {
                    if task.status == Status::Snoozed && task.snoozed_until.as_deref() == Some(day)
                    {
                        rows.push(Row {
                            tags: tags::resolve(&task.tags, &defs),
                            status: resolve(&task, &statuses).clone(),
                            latest_note: latest_note(conn, &task.id, day)?,
                            age_days: days_between(&task.first_day, day),
                            group: Group::Snoozed,
                            task,
                        });
                    }
                }
                sort_rows(&mut rows, &statuses);
                view.rows = rows;
            }
        }
        Ok(view)
    }
    /// The sidebar count: open plus needs check.
    pub fn today_open_count(&self, now: u64) -> Result<usize, String> {
        let current = self.today_roll(now)?;
        let summary = self.today_view(&current, now)?.summary;
        Ok(summary.open + summary.to_check)
    }
    pub fn today_task(&self, id_or_key: &str) -> Result<Option<Task>, String> {
        if !self.today_ready()? {
            return Ok(None);
        }
        load_task(&self.conn, id_or_key)
    }
    /// Live tasks in every status, for `today.list` filters and scoping.
    pub fn today_tasks(&self, now: u64) -> Result<Vec<Task>, String> {
        self.today_roll(now)?;
        if !self.today_ready()? {
            return Ok(Vec::new());
        }
        all_tasks(&self.conn)
    }
    pub fn today_sources(&self) -> Result<Vec<Source>, String> {
        if !self.today_ready()? {
            return Ok(Vec::new());
        }
        let mut sources: Vec<Source> = json_rows(
            &self.conn,
            "SELECT json FROM today_sources ORDER BY rowid",
            [],
        )?;
        sources.sort_by(|a, b| (&a.name, &a.id).cmp(&(&b.name, &b.id)));
        Ok(sources)
    }
    /// Reconciles one source in a single transaction. Refuses a source another
    /// Mono owns. A failed request changes no task.
    pub fn today_sync(
        &self,
        request: &SyncRequest,
        caller: &MonoCaller,
        now: u64,
    ) -> Result<SyncSummary, String> {
        self.today_writable()?;
        let source_id = clean_text(&request.source, "source")?;
        if caller.id.is_empty() {
            return Err("A sync needs a Mono".into());
        }
        if !request.failed && request.items.is_none() {
            return Err(
                "A sync with status ok needs items; send an empty list when the source has nothing"
                    .into(),
            );
        }
        let day = self.today_roll(now)?;
        self.today_ready()?;
        let statuses = self.today_statuses()?;
        let defs = self.today_tags()?;
        // The items name tags; reconcile gets ids, and a name no tag has is
        // dropped and reported, never created.
        let mut rejected_tags: Vec<String> = Vec::new();
        let items_by_id: Option<Vec<SyncItem>> = request.items.as_ref().map(|items| {
            items
                .iter()
                .map(|item| {
                    let mut item = item.clone();
                    if let Some(names) = &item.tags {
                        let mut ids: Vec<String> = Vec::new();
                        for name in names {
                            match defs.iter().find(|d| tags::same_name(&d.name, name)) {
                                Some(def) => {
                                    if !ids.contains(&def.id) {
                                        ids.push(def.id.clone());
                                    }
                                }
                                None => {
                                    let name = name.trim();
                                    if !rejected_tags.iter().any(|r| tags::same_name(r, name)) {
                                        rejected_tags.push(name.to_string());
                                    }
                                }
                            }
                        }
                        item.tags = Some(in_definition_order(&ids, &defs));
                    }
                    item
                })
                .collect()
        });
        self.today_tx(|conn| {
            let known = load_source(conn, &source_id)?;
            let mut source = match known {
                Some(source) if source.mono_id != caller.id => {
                    return Err("That source belongs to another Mono".into());
                }
                Some(source) => source,
                None => Source {
                    id: source_id.clone(),
                    name: source_id.clone(),
                    provider: String::new(),
                    mono_id: caller.id.clone(),
                    ..Source::default()
                },
            };
            let given = |value: &Option<String>| {
                value
                    .as_deref()
                    .map(str::trim)
                    .filter(|v| !v.is_empty())
                    .map(str::to_string)
            };
            if let Some(name) = given(&request.name) {
                source.name = name;
            }
            if let Some(provider) = given(&request.provider) {
                source.provider = provider;
            } else if source.provider.is_empty() {
                source.provider = source.name.clone();
            }
            if let Some(habit) = &caller.habit {
                source.habit_id = Some(habit.clone());
            }
            source.last_attempt_at = Some(now);

            let Some(items) = items_by_id.as_ref().filter(|_| !request.failed) else {
                source.last_status = SyncStatus::Failed;
                source.last_error =
                    Some(given(&request.error).unwrap_or_else(|| "The sync failed".to_string()));
                save_source(conn, &source)?;
                return Ok(SyncSummary::default());
            };

            let tasks = all_tasks(conn)?;
            let existing: Vec<Task> = tasks
                .into_iter()
                .filter(|task| task.source_id.as_deref() == Some(source_id.as_str()))
                .collect();
            // Keys of rows we cannot parse count as foreign: they still own the key.
            let owned: HashSet<&str> = existing.iter().map(|task| task.key.as_str()).collect();
            let mut stmt = conn.prepare("SELECT key FROM today_tasks").map_err(sql)?;
            let mut foreign = HashSet::new();
            for key in stmt
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(sql)?
            {
                let key = key.map_err(sql)?;
                if !owned.contains(key.as_str()) {
                    foreign.insert(key);
                }
            }
            drop(stmt);

            let result = reconcile::reconcile(
                &existing,
                &foreign,
                items,
                &reconcile::Context {
                    source_id: &source_id,
                    provider: &source.provider,
                    mono_id: &caller.id,
                    day: &day,
                    now,
                    statuses: &statuses,
                },
            );
            // `actions` already filled or dropped each item's project.
            let who = Who {
                mono_id: Some(caller.id.clone()),
                source_id: Some(source_id.clone()),
            };
            for mut task in result.tasks {
                let before = existing.iter().find(|old| old.id == task.id);
                // Added tags join in definition order.
                if before.is_some_and(|old| old.tags != task.tags) {
                    task.tags = in_definition_order(&task.tags, &defs);
                }
                let events =
                    diff_events(before, &task, &statuses, &statuses, &defs, &who, &day, now);
                save_task(conn, &task)?;
                write_record(conn, &day, &task, &statuses)?;
                insert_events(conn, &events)?;
            }

            let mut counted: HashSet<&str> = HashSet::new();
            for item in items {
                let key = item.key.as_str();
                if !key.is_empty() && !result.summary.conflicts.iter().any(|c| c == key) {
                    counted.insert(key);
                }
            }
            source.item_count = counted.len();
            source.last_sync_at = Some(now);
            source.last_sync_day = Some(day.clone());
            source.last_status = SyncStatus::Ok;
            source.last_error = None;
            for project in &request.projects {
                let key = crate::files::path_key(project);
                if !source
                    .projects
                    .iter()
                    .any(|known| crate::files::path_key(known) == key)
                {
                    source.projects.push(project.clone());
                }
            }
            save_source(conn, &source)?;
            let mut summary = result.summary;
            summary.rejected_tags = rejected_tags;
            Ok(summary)
        })
    }

    pub fn today_add(&self, new: &NewTask, actor: &Actor, now: u64) -> Result<Task, String> {
        self.today_writable()?;
        let title = clean_text(&new.title, "title")?;
        check_project(actor, new.project.as_deref())?;
        check_due(&new.due, &new.due_time)?;
        let current = self.today_roll(now)?;
        let day = new.day.clone().unwrap_or_else(|| current.clone());
        if !is_day(&day) {
            return Err("day must look like 2026-07-14".into());
        }
        if day < current {
            return Err("A task cannot be added to a past day".into());
        }
        let future = day > current;
        let defs = self.today_tags()?;
        let task_tags = check_task_tags(&new.tags, &defs, 0)?;
        let (mono_id, project) = match actor {
            Actor::User => (None, new.project.clone()),
            Actor::Session(_) => return Err("A session cannot add tasks".into()),
            Actor::Mono(caller) => (
                Some(caller.id.clone()),
                new.project
                    .clone()
                    .or_else(|| match caller.projects.as_slice() {
                        [only] => Some(only.clone()),
                        _ => None,
                    }),
            ),
        };
        let id = format!("task-{}", uuid::Uuid::new_v4());
        let task = Task {
            key: format!("manual:{id}"),
            id,
            title,
            kind: today::Kind::Manual,
            source_name: "Manual".into(),
            reason: new.reason.clone().unwrap_or_default(),
            project,
            mono_id,
            link: new.link.clone(),
            due: new.due.clone(),
            due_time: new.due_time.clone(),
            priority: new.priority,
            tags: task_tags,
            status: if future {
                Status::Snoozed
            } else {
                Status::Open
            },
            status_id: TODO_ID.into(),
            snoozed_until: future.then(|| day.clone()),
            first_seen: now,
            first_day: day,
            updated_at: now,
            ..Task::default()
        };
        self.today_ready()?;
        let statuses = self.today_statuses()?;
        self.today_tx(|conn| {
            save_task(conn, &task)?;
            write_record(conn, &current, &task, &statuses)?;
            let events = diff_events(
                None,
                &task,
                &statuses,
                &statuses,
                &defs,
                &who_of(actor),
                &current,
                now,
            );
            insert_events(conn, &events)
        })?;
        Ok(task)
    }

    /// The one path every task mutation takes: roll, scope, refuse, change,
    /// record today and its events, all in one transaction. `change` also sees
    /// the workflow statuses, to keep the lifecycle in step with them.
    fn today_mutate(
        &self,
        id: &str,
        actor: &Actor,
        now: u64,
        refuse_in_run: bool,
        change: impl FnOnce(&mut Task, &str, &[WorkflowStatus]) -> Result<(), String>,
    ) -> Result<Task, String> {
        self.today_writable()?;
        let day = self.today_roll(now)?;
        self.today_ready()?;
        let statuses = self.today_statuses()?;
        let defs = self.today_tags()?;
        self.today_tx(|conn| {
            let original = load_task(conn, id)?
                .filter(|task| actor_sees(conn, actor, task))
                .ok_or_else(|| NOT_FOUND.to_string())?;
            if let Actor::Mono(caller) = actor {
                if refuse_in_run && caller.in_run {
                    return Err("This is not allowed in a run: no user is present to decide".into());
                }
            }
            // A task saved before statuses existed gets its resolved id once it
            // really changes; the id alone is not a change.
            let mut baseline = original.clone();
            // Only an empty id is filled; an unknown one stays and resolves on read.
            if baseline.status_id.is_empty() {
                baseline.status_id = resolve(&original, &statuses).id.clone();
            }
            let mut task = baseline.clone();
            change(&mut task, &day, &statuses)?;
            if task != baseline {
                task.updated_at = now;
                let events = diff_events(
                    Some(&original),
                    &task,
                    &statuses,
                    &statuses,
                    &defs,
                    &who_of(actor),
                    &day,
                    now,
                );
                save_task(conn, &task)?;
                write_record(conn, &day, &task, &statuses)?;
                insert_events(conn, &events)?;
            }
            Ok(task)
        })
    }

    pub fn today_update(
        &self,
        id: &str,
        patch: &TaskPatch,
        actor: &Actor,
        now: u64,
    ) -> Result<Task, String> {
        if let Some(Some(project)) = &patch.project {
            check_project(actor, Some(project))?;
        }
        let user = matches!(actor, Actor::User);
        let defs = self.today_tags()?;
        self.today_mutate(id, actor, now, false, |task, _, _| {
            if let Some(title) = &patch.title {
                task.title = clean_text(title, "title")?;
            }
            if let Some(priority) = patch.priority {
                task.priority = priority;
                task.priority_pinned |= user;
            }
            if let Some(project) = &patch.project {
                task.project = project.clone();
                task.project_pinned |= user;
            }
            if let Some(due) = &patch.due {
                task.due = due.clone();
                if due.is_none() {
                    task.due_time = None;
                }
            }
            if let Some(due_time) = &patch.due_time {
                task.due_time = due_time.clone();
            }
            check_due(&task.due, &task.due_time)?;
            if let Some(tags) = &patch.tags {
                task.tags = check_task_tags(tags, &defs, task.tags.len())?;
            }
            if let Some(reason) = &patch.reason {
                task.reason = reason.clone();
            }
            Ok(())
        })
    }
    /// Puts one tag on a task or takes it off, whatever else the task holds now.
    /// Same validation, definition order and `TagsChanged` event as a patch with
    /// `tags`, but a concurrent change to other tags is never overwritten.
    pub fn today_toggle_tag(
        &self,
        id: &str,
        tag_id: &str,
        on: bool,
        actor: &Actor,
        now: u64,
    ) -> Result<Task, String> {
        let defs = self.today_tags()?;
        self.today_mutate(id, actor, now, false, |task, _, _| {
            // The user is editing the tags, so this is the moment to clean the
            // entries up: legacy names become the id of the definition they
            // resolve to, and orphans (ids with no definition, names matching
            // nothing) are dropped. A tag id sent by the caller is still
            // checked below and refused when unknown.
            let mut next: Vec<String> = tags::resolve(&task.tags, &defs)
                .into_iter()
                .map(|d| d.id)
                .filter(|id| id != tag_id)
                .collect();
            if on {
                next.push(tag_id.to_string());
            }
            task.tags = check_task_tags(&next, &defs, task.tags.len())?;
            Ok(())
        })
    }
    pub fn today_note(
        &self,
        id: &str,
        body: &str,
        actor: &Actor,
        now: u64,
    ) -> Result<Note, String> {
        self.today_note_with_files(id, body, &[], actor, now)
            .map(|saved| saved.note)
    }
    /// Oldest first.
    pub fn today_notes(&self, id: &str) -> Result<Vec<Note>, String> {
        if !self.today_ready()? {
            return Ok(Vec::new());
        }
        let task_id = load_task(&self.conn, id)?
            .map(|task| task.id)
            .unwrap_or_else(|| id.to_string());
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {NOTE_COLUMNS} FROM today_notes WHERE task_id = ?1 ORDER BY at, rowid"
            ))
            .map_err(sql)?;
        let notes = stmt
            .query_map([task_id], note_from_row)
            .map_err(sql)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sql)?;
        Ok(self.with_attachment_paths(notes))
    }
    pub fn today_complete(&self, id: &str, actor: &Actor, now: u64) -> Result<Task, String> {
        self.today_mutate(id, actor, now, true, |task, day, statuses| {
            match task.status {
                Status::Done => return Ok(()),
                Status::Dismissed => return Err("A dismissed task cannot be completed".into()),
                _ => {}
            }
            let finished = resolve(task, statuses).stage == Stage::Finished;
            finish_task(task, day, now);
            if !finished {
                task.status_id = DONE_ID.into();
            }
            Ok(())
        })
    }
    /// Takes back a done, snoozed or dismissed task: it is open again. The page calls it
    /// only to un-check a done row; nothing offers to reopen a dismissed task.
    pub fn today_reopen(&self, id: &str, now: u64) -> Result<Task, String> {
        self.today_mutate(id, &Actor::User, now, false, |task, day, statuses| {
            reopen_task(task, day);
            leave_finished(task, statuses);
            Ok(())
        })
    }
    pub fn today_snooze(
        &self,
        id: &str,
        until: &str,
        actor: &Actor,
        now: u64,
    ) -> Result<Task, String> {
        if !is_day(until) {
            return Err("until must look like 2026-07-14".into());
        }
        self.today_mutate(id, actor, now, true, |task, day, statuses| {
            if until <= day {
                return Err("A task can only be snoozed to a later day".into());
            }
            if matches!(task.status, Status::Done | Status::Dismissed) {
                return Err("Only an open task can be snoozed".into());
            }
            // A task planned for a later day has not counted yet.
            if task.first_day.as_str() > day {
                task.first_day = until.to_string();
            }
            task.status = Status::Snoozed;
            task.snoozed_until = Some(until.to_string());
            task.check_reason = None;
            leave_finished(task, statuses);
            Ok(())
        })
    }
    pub fn today_dismiss(&self, id: &str, actor: &Actor, now: u64) -> Result<Task, String> {
        self.today_mutate(id, actor, now, true, |task, _, statuses| {
            match task.status {
                Status::Dismissed => return Ok(()),
                Status::Done => return Err("A done task cannot be dismissed".into()),
                _ => {}
            }
            task.status = Status::Dismissed;
            task.snoozed_until = None;
            task.check_reason = None;
            task.source_closed_after_dismiss = false;
            leave_finished(task, statuses);
            Ok(())
        })
    }

    // --- workflow statuses -----------------------------------------------------------------

    /// The workflow statuses in order (stage-grouped), normalized. Absent or
    /// unreadable setting: the defaults (an unreadable one is never overwritten
    /// until the user edits statuses).
    pub fn today_statuses(&self) -> Result<Vec<WorkflowStatus>, String> {
        // A value that is not an array reads as the defaults; inside an array only
        // an unparseable entry is dropped and `normalize` repairs the built-ins.
        let entries = self
            .setting(STATUSES_KEY)?
            .and_then(|json| serde_json::from_str::<Vec<serde_json::Value>>(&json).ok());
        Ok(match entries {
            Some(entries) => normalize(
                entries
                    .into_iter()
                    .filter_map(|e| serde_json::from_value::<WorkflowStatus>(e).ok())
                    .collect(),
            ),
            None => default_statuses(),
        })
    }
    fn save_statuses(&self, list: &[WorkflowStatus]) -> Result<(), String> {
        let json = serde_json::to_string(list).map_err(|e| e.to_string())?;
        self.save_setting(STATUSES_KEY, &json)
    }
    /// Adds a custom status at the end of `stage`. `color` is `#rrggbb`.
    pub fn today_add_status(
        &self,
        name: &str,
        stage: Stage,
        color: &str,
    ) -> Result<WorkflowStatus, String> {
        self.today_writable()?;
        let mut list = self.today_statuses()?;
        let name = status_name(name, &list, None)?;
        let status = WorkflowStatus {
            id: format!("status-{}", uuid::Uuid::new_v4()),
            name,
            stage,
            color: status_color(color)?,
            builtin: false,
        };
        let at = list
            .iter()
            .rposition(|s| s.stage == stage)
            .map_or(list.len(), |index| index + 1);
        list.insert(at, status.clone());
        self.save_statuses(&list)?;
        Ok(status)
    }
    /// Renames or recolors any status, built-in included. Names are trimmed,
    /// non-empty, at most `statuses::MAX_NAME` characters and unique (ignoring case).
    pub fn today_update_status(
        &self,
        id: &str,
        patch: &StatusPatch,
        now: u64,
    ) -> Result<WorkflowStatus, String> {
        self.today_writable()?;
        let mut list = self.today_statuses()?;
        let at = list
            .iter()
            .position(|s| s.id == id)
            .ok_or(STATUS_NOT_FOUND)?;
        if let Some(name) = &patch.name {
            list[at].name = status_name(name, &list, Some(id))?;
        }
        if let Some(color) = &patch.color {
            list[at].color = status_color(color)?;
        }
        // Today's day records freeze the status as it stands now, so a rename
        // shows on today and never on a past day.
        let day = self.today_roll(now)?;
        {
            self.today_ready()?;
            self.today_tx(|conn| {
                for task in all_tasks(conn)? {
                    if resolve(&task, &list).id != id {
                        continue;
                    }
                    let recorded: bool = conn
                        .query_row(
                            "SELECT EXISTS(SELECT 1 FROM today_days WHERE day = ?1 AND task_id = ?2)",
                            params![day, task.id],
                            |row| row.get(0),
                        )
                        .map_err(sql)?;
                    if recorded {
                        write_record(conn, &day, &task, &list)?;
                    }
                }
                Ok(())
            })?;
        }
        // Saved after the task transaction commits: on the shared desktop database
        // the setting lives in another database and cannot join it, so a failed
        // commit leaves the list unchanged.
        self.save_statuses(&list)?;
        Ok(list[at].clone())
    }
    /// Moves a status to position `index` inside `stage` (clamped). A built-in
    /// may only move inside its own stage. Moving a custom status to another stage
    /// moves its tasks' lifecycle with it (into or out of Finished) in the same
    /// transaction, recording a status change event for each. Returns the new list.
    pub fn today_move_status(
        &self,
        id: &str,
        stage: Stage,
        index: usize,
        now: u64,
    ) -> Result<Vec<WorkflowStatus>, String> {
        self.today_writable()?;
        let old = self.today_statuses()?;
        let from = old
            .iter()
            .position(|s| s.id == id)
            .ok_or(STATUS_NOT_FOUND)?;
        let mut status = old[from].clone();
        if status.builtin && status.stage != stage {
            return Err("A built-in status cannot move to another stage".into());
        }
        let old_stage = status.stage;
        status.stage = stage;
        let mut list = old.clone();
        list.remove(from);
        let in_stage: Vec<usize> = (0..list.len())
            .filter(|&i| list[i].stage == stage)
            .collect();
        let at = match in_stage.get(index) {
            Some(&position) => position,
            None => in_stage.last().map_or(list.len(), |last| last + 1),
        };
        list.insert(at, status);
        if old_stage == stage {
            self.save_statuses(&list)?;
            return Ok(list);
        }
        let day = self.today_roll(now)?;
        self.today_ready()?;
        self.today_tx(|conn| {
            for mut task in all_tasks(conn)? {
                if task.status_id != id {
                    continue;
                }
                let before = task.clone();
                if stage == Stage::Finished {
                    if task.status == Status::Dismissed {
                        // A dismissed task is not finished: it leaves with the
                        // built-in of the stage the status came from.
                        task.status_id = builtin_id(old_stage).into();
                    } else if matches!(
                        task.status,
                        Status::Open | Status::NeedsCheck | Status::Snoozed
                    ) {
                        finish_task(&mut task, &day, now);
                    }
                } else if old_stage == Stage::Finished
                    && (task.status == Status::Done
                        || (task.status == Status::NeedsCheck
                            && task.check_reason == Some(CheckReason::DoneButSourceOpen)))
                {
                    reopen_task(&mut task, &day);
                }
                task.updated_at = now;
                let events = diff_events(
                    Some(&before),
                    &task,
                    &old,
                    &list,
                    &[],
                    &who_of(&Actor::User),
                    &day,
                    now,
                );
                save_task(conn, &task)?;
                write_record(conn, &day, &task, &list)?;
                insert_events(conn, &events)?;
            }
            Ok(())
        })?;
        // The list is saved after the task transaction commits: on the shared
        // desktop database the setting lives in another database, so it cannot
        // join the transaction. A failed commit leaves the list unchanged; a
        // failed setting write leaves tasks already moved, which is safe (they
        // sit on a status that still exists or resolves on read).
        self.save_statuses(&list)?;
        Ok(list)
    }
    /// Deletes a custom status; its tasks move to the built-in status of the same
    /// stage (an event each). A built-in cannot be deleted. Past day records keep
    /// the frozen status. Returns the new list.
    pub fn today_delete_status(&self, id: &str, now: u64) -> Result<Vec<WorkflowStatus>, String> {
        self.today_writable()?;
        let old = self.today_statuses()?;
        let status = old
            .iter()
            .find(|s| s.id == id)
            .ok_or(STATUS_NOT_FOUND)?
            .clone();
        if status.builtin {
            return Err("A built-in status cannot be deleted".into());
        }
        let list: Vec<WorkflowStatus> = old.iter().filter(|s| s.id != id).cloned().collect();
        let target = builtin(&list, status.stage).id.clone();
        let day = self.today_roll(now)?;
        self.today_ready()?;
        self.today_tx(|conn| {
            for mut task in all_tasks(conn)? {
                if task.status_id != id {
                    continue;
                }
                let before = task.clone();
                task.status_id = target.clone();
                task.updated_at = now;
                let events = diff_events(
                    Some(&before),
                    &task,
                    &old,
                    &list,
                    &[],
                    &who_of(&Actor::User),
                    &day,
                    now,
                );
                save_task(conn, &task)?;
                write_record(conn, &day, &task, &list)?;
                insert_events(conn, &events)?;
            }
            Ok(())
        })?;
        // The list is saved after the task transaction commits: on the shared
        // desktop database the setting lives in another database, so it cannot
        // join the transaction. A failed commit leaves the list unchanged; a
        // failed setting write leaves tasks already moved, which is safe (they
        // sit on a status that still exists or resolves on read).
        self.save_statuses(&list)?;
        Ok(list)
    }
    /// Sets a task's workflow status. Into Finished completes it (as
    /// `today_complete`); out of Finished reopens it (as `today_reopen`); inside
    /// a stage only the id changes. A user change pins the status
    /// (`status_pinned`). Records a status change event.
    pub fn today_set_status(
        &self,
        id: &str,
        status_id: &str,
        actor: &Actor,
        now: u64,
    ) -> Result<Task, String> {
        let user = matches!(actor, Actor::User);
        self.today_mutate(id, actor, now, false, |task, day, statuses| {
            let target = statuses
                .iter()
                .find(|s| s.id == status_id)
                .ok_or(STATUS_NOT_FOUND)?;
            let was_finished = resolve(task, statuses).stage == Stage::Finished;
            let to_finished = target.stage == Stage::Finished;
            if let Actor::Mono(caller) = actor {
                if caller.in_run && (was_finished || to_finished) {
                    return Err("This is not allowed in a run: no user is present to decide".into());
                }
            }
            match (was_finished, to_finished) {
                (false, true) => match task.status {
                    Status::Dismissed => {
                        return Err("A dismissed task cannot be completed".into());
                    }
                    Status::Done => {}
                    _ => finish_task(task, day, now),
                },
                (true, false)
                    if task.status == Status::Done
                        || (task.status == Status::NeedsCheck
                            && task.check_reason == Some(CheckReason::DoneButSourceOpen)) =>
                {
                    reopen_task(task, day);
                }
                _ => {}
            }
            task.status_id = target.id.clone();
            task.status_pinned |= user;
            Ok(())
        })
    }
    /// Answers a Done-but-source-open check with Keep done: done again, check
    /// cleared, `kept_done` set so a sync listing it open does not raise it again.
    pub fn today_keep_done(&self, id: &str, now: u64) -> Result<Task, String> {
        self.today_mutate(id, &Actor::User, now, false, |task, day, statuses| {
            if task.status != Status::NeedsCheck
                || task.check_reason != Some(CheckReason::DoneButSourceOpen)
            {
                return Err(
                    "Only a task you marked done that its source still lists open can be kept done"
                        .into(),
                );
            }
            let finished = resolve(task, statuses).stage == Stage::Finished;
            finish_task(task, day, now);
            task.kept_done = true;
            if !finished {
                task.status_id = DONE_ID.into();
            }
            Ok(())
        })
    }
    /// Events and notes of a task, oldest first. A task with no Added event (saved
    /// before events existed) gets one synthesized from `first_seen`.
    pub fn today_activity(&self, id: &str) -> Result<Vec<today::Activity>, String> {
        if !self.today_ready()? {
            return Err(NOT_FOUND.into());
        }
        let task = load_task(&self.conn, id)?.ok_or_else(|| NOT_FOUND.to_string())?;
        let mut events: Vec<TaskEvent> = if super::shared::has_table(&self.conn, "today_events")? {
            json_rows(
                &self.conn,
                "SELECT json FROM today_events WHERE task_id = ?1 ORDER BY at, rowid",
                [&task.id],
            )?
        } else {
            Vec::new()
        };
        // `at` then insertion; the sort is stable and rows come in insertion order.
        let notes = self.today_notes(&task.id)?;
        events.sort_by_key(|e| e.at);
        let mut timeline: Vec<Activity> = Vec::new();
        if !events.iter().any(|e| e.kind == EventKind::Added) {
            let mut added = event(
                &task,
                EventKind::Added,
                &Who {
                    mono_id: task.mono_id.clone(),
                    source_id: task.source_id.clone(),
                },
                &task.first_day,
                task.first_seen,
            );
            added.id = format!("added-{}", task.id);
            timeline.push(Activity::Event(added));
        }
        let mut rest: Vec<Activity> = events.into_iter().map(Activity::Event).collect();
        rest.extend(notes.into_iter().map(Activity::Note));
        // Events lead on a tie: they were written with the change a note answers.
        rest.sort_by_key(Activity::at);
        timeline.extend(rest);
        Ok(timeline)
    }
    // --- tags --------------------------------------------------------------------------------

    /// The stored definitions, normalized, without migrating anything.
    fn load_tags(&self) -> Result<Vec<Tag>, String> {
        let entries = self
            .setting(TAGS_KEY)?
            .and_then(|json| serde_json::from_str::<Vec<serde_json::Value>>(&json).ok());
        Ok(tags::normalize(
            entries
                .unwrap_or_default()
                .into_iter()
                .filter_map(|e| serde_json::from_value::<Tag>(e).ok())
                .collect(),
        ))
    }
    fn save_tags(&self, list: &[Tag]) -> Result<(), String> {
        let json = serde_json::to_string(list).map_err(|e| e.to_string())?;
        self.save_setting(TAGS_KEY, &json)
    }
    /// Appends a definition for every legacy name in `tasks` that no definition
    /// matches. Ids derive from the lowercase name, so they are stable.
    fn add_legacy_defs(defs: &mut Vec<Tag>, tasks: &[Task]) {
        for task in tasks {
            for entry in &task.tags {
                if tags::is_id(entry) {
                    continue;
                }
                let name = tags::legacy_key(entry);
                if name.is_empty() || defs.iter().any(|d| tags::same_name(&d.name, &name)) {
                    continue;
                }
                // FNV-1a over the lowercase name, as a UUID (no extra crate feature).
                let hash = name
                    .to_lowercase()
                    .bytes()
                    .fold(0x6c62272e07bb014262b821756295c58d_u128, |h, b| {
                        (h ^ u128::from(b)).wrapping_mul(0x0000000001000000000000000000013b)
                    });
                let id = uuid::Uuid::from_u128(hash);
                defs.push(Tag {
                    id: format!("{ID_PREFIX}{id}"),
                    name,
                    color: format_color(tags::preset_for(defs.len())),
                });
            }
        }
    }
    /// Turns the free text tags of tasks saved before tags had definitions into
    /// definitions, once. Order matters for a crash: the definitions are saved
    /// first, then the tasks are rewritten to ids, then the marker is saved; a
    /// run that stops anywhere repeats safely (names match existing definitions).
    /// An entry that is neither a legacy name nor a known id is kept as it is.
    /// A read-only store skips it; `today_tags` then shows the legacy names as
    /// in-memory definitions and writes nothing.
    fn ensure_tags_migrated(&self) -> Result<(), String> {
        if self.setting(TAGS_MIGRATED_KEY)?.is_some() || self.today_writable().is_err() {
            return Ok(());
        }
        self.today_ready()?;
        let mut defs = self.load_tags()?;
        let before = defs.len();
        Self::add_legacy_defs(&mut defs, &all_tasks(&self.conn)?);
        if defs.len() != before {
            self.save_tags(&defs)?;
        }
        self.today_tx(|conn| {
            for mut task in all_tasks(conn)? {
                let mut ids: Vec<String> = Vec::new();
                for entry in &task.tags {
                    let id = if tags::is_id(entry) {
                        entry.clone()
                    } else {
                        let name = tags::legacy_key(entry);
                        match defs.iter().find(|d| tags::same_name(&d.name, &name)) {
                            Some(def) => def.id.clone(),
                            // Blank: nothing to keep.
                            None => continue,
                        }
                    };
                    if !ids.contains(&id) {
                        ids.push(id);
                    }
                }
                // Neither updated_at nor an event: nothing the user did changed.
                if ids != task.tags {
                    task.tags = ids;
                    save_task(conn, &task)?;
                }
            }
            Ok(())
        })?;
        self.save_setting(TAGS_MIGRATED_KEY, "1")
    }

    /// The tag definitions in order, normalized. The first call on a writable
    /// store migrates free text task tags into definitions (see `tags`). On a
    /// store that cannot migrate, legacy names show as in-memory definitions.
    pub fn today_tags(&self) -> Result<Vec<Tag>, String> {
        self.ensure_tags_migrated()?;
        let mut defs = self.load_tags()?;
        if self.setting(TAGS_MIGRATED_KEY)?.is_none() && self.today_ready()? {
            Self::add_legacy_defs(&mut defs, &all_tasks(&self.conn)?);
        }
        Ok(defs)
    }
    /// Adds a tag at the end. Name trimmed, non-empty, at most `tags::MAX_NAME`
    /// characters and unique ignoring case; `color` is `#rrggbb`.
    pub fn today_add_tag(&self, name: &str, color: &str) -> Result<Tag, String> {
        self.today_writable()?;
        let mut list = self.today_tags()?;
        let tag = Tag {
            id: format!("{ID_PREFIX}{}", uuid::Uuid::new_v4()),
            name: tag_name(name, &list, None)?,
            color: status_color(color)?,
        };
        list.push(tag.clone());
        self.save_tags(&list)?;
        Ok(tag)
    }
    /// Renames or recolors a tag; same name rules as `today_add_tag`. Tasks hold
    /// ids, so the change shows on every task and every day at once.
    pub fn today_update_tag(&self, id: &str, patch: &TagPatch) -> Result<Tag, String> {
        self.today_writable()?;
        let mut list = self.today_tags()?;
        let at = list.iter().position(|t| t.id == id).ok_or(TAG_NOT_FOUND)?;
        if let Some(name) = &patch.name {
            list[at].name = tag_name(name, &list, Some(id))?;
        }
        if let Some(color) = &patch.color {
            list[at].color = status_color(color)?;
        }
        self.save_tags(&list)?;
        Ok(list[at].clone())
    }
    /// Moves a tag to position `index` (clamped). Returns the new list.
    pub fn today_move_tag(&self, id: &str, index: usize) -> Result<Vec<Tag>, String> {
        self.today_writable()?;
        let mut list = self.today_tags()?;
        let from = list.iter().position(|t| t.id == id).ok_or(TAG_NOT_FOUND)?;
        let tag = list.remove(from);
        list.insert(index.min(list.len()), tag);
        self.save_tags(&list)?;
        Ok(list)
    }
    /// How many live tasks (any lifecycle status) hold the tag.
    pub fn today_tag_usage(&self, id: &str) -> Result<usize, String> {
        let list = self.today_tags()?;
        let Some(tag) = list.into_iter().find(|t| t.id == id) else {
            return Ok(0);
        };
        if !self.today_ready()? {
            return Ok(0);
        }
        let only = [tag];
        Ok(all_tasks(&self.conn)?
            .iter()
            .filter(|task| !tags::resolve(&task.tags, &only).is_empty())
            .count())
    }
    /// Deletes a tag and takes it off every task holding it, a `TagsChanged`
    /// event each. Returns the new list.
    pub fn today_delete_tag(&self, id: &str, now: u64) -> Result<Vec<Tag>, String> {
        self.today_writable()?;
        let old = self.today_tags()?;
        let tag = old
            .iter()
            .find(|t| t.id == id)
            .ok_or(TAG_NOT_FOUND)?
            .clone();
        let list: Vec<Tag> = old.iter().filter(|t| t.id != id).cloned().collect();
        let only = [tag];
        let day = self.today_roll(now)?;
        self.today_ready()?;
        let statuses = self.today_statuses()?;
        self.today_tx(|conn| {
            for mut task in all_tasks(conn)? {
                if tags::resolve(&task.tags, &only).is_empty() {
                    continue;
                }
                let before = task.clone();
                task.tags
                    .retain(|entry| tags::resolve(std::slice::from_ref(entry), &only).is_empty());
                task.updated_at = now;
                let events = diff_events(
                    Some(&before),
                    &task,
                    &statuses,
                    &statuses,
                    &old,
                    &who_of(&Actor::User),
                    &day,
                    now,
                );
                save_task(conn, &task)?;
                if matches!(
                    task.status,
                    Status::Open | Status::NeedsCheck | Status::Done
                ) {
                    write_record(conn, &day, &task, &statuses)?;
                }
                insert_events(conn, &events)?;
            }
            Ok(())
        })?;
        // Saved after the task transaction commits, like the statuses: on the
        // shared desktop database the setting lives in another database. A failed
        // write leaves the tag defined and unused, which is safe.
        self.save_tags(&list)?;
        Ok(list)
    }
    /// Keys of every task that is not dismissed, without rolling the day (read
    /// only). For the composer's `@today/` highlighting.
    pub fn today_mention_keys(&self) -> Result<Vec<String>, String> {
        if !self.today_ready()? {
            return Ok(Vec::new());
        }
        Ok(all_tasks(&self.conn)?
            .into_iter()
            .filter(|task| task.status != Status::Dismissed)
            .map(|task| task.key)
            .collect())
    }
}

const STATUS_NOT_FOUND: &str = "Status not found";
const TAG_NOT_FOUND: &str = "Tag not found";

/// A tag name: trimmed, non-empty, short and unique ignoring case among the
/// others (`own` is the tag being renamed).
fn tag_name(name: &str, list: &[Tag], own: Option<&str>) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("A tag needs a name".into());
    }
    if name.chars().count() > tags::MAX_NAME {
        return Err(format!(
            "A tag name is at most {} characters",
            tags::MAX_NAME
        ));
    }
    if list
        .iter()
        .any(|t| Some(t.id.as_str()) != own && tags::same_name(&t.name, name))
    {
        return Err("Another tag already has that name".into());
    }
    Ok(name.to_string())
}

/// The tag ids a task may hold: each must be a known tag, no duplicates, stored
/// in definition order. At most `tags::MAX_PER_TASK`, unless the task already
/// holds more (a migration can leave that): then no more than it holds now, so
/// tags can still be removed one by one.
fn check_task_tags(
    entries: &[String],
    defs: &[Tag],
    current: usize,
) -> Result<Vec<String>, String> {
    for entry in entries {
        if !defs.iter().any(|d| &d.id == entry) {
            return Err(format!("Unknown tag: {entry}"));
        }
    }
    let ids = in_definition_order(entries, defs);
    if ids.len() > tags::MAX_PER_TASK.max(current) {
        return Err(format!("A task has at most {} tags", tags::MAX_PER_TASK));
    }
    Ok(ids)
}

/// Entries that name a definition first, in definition order; the rest keep
/// their order after them.
fn in_definition_order(entries: &[String], defs: &[Tag]) -> Vec<String> {
    let mut ids: Vec<String> = defs
        .iter()
        .filter(|d| entries.contains(&d.id))
        .map(|d| d.id.clone())
        .collect();
    for entry in entries {
        if !ids.contains(entry) {
            ids.push(entry.clone());
        }
    }
    ids
}

/// A status name: trimmed, non-empty, short and unique ignoring case among the
/// others (`own` is the status being renamed).
fn status_name(name: &str, list: &[WorkflowStatus], own: Option<&str>) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("A status needs a name".into());
    }
    if name.chars().count() > MAX_NAME {
        return Err(format!("A status name is at most {MAX_NAME} characters"));
    }
    let taken = list
        .iter()
        .any(|s| Some(s.id.as_str()) != own && s.name.to_lowercase() == name.to_lowercase());
    if taken {
        return Err("Another status already has that name".into());
    }
    Ok(name.to_string())
}

fn status_color(color: &str) -> Result<String, String> {
    parse_color(color.trim())
        .map(format_color)
        .ok_or_else(|| "A color looks like #rrggbb".to_string())
}

mod attachments;
mod note_edit;
pub use attachments::{NotePlan, NoteSaved};

#[cfg(test)]
mod tests;
