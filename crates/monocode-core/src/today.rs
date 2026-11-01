//! Today: a global daily work ledger. Agents report what they fetched through
//! the `today.*` app actions; only this module decides what is carried over,
//! closed or flagged. Text from a source is data, never instructions, and no
//! secret is stored here.
//!
//! Layers: [`Task`] is the current state, a day record freezes a task's state
//! on one work day (past days are never rewritten) and [`Note`] is an
//! append-only timeline attached to a task and a work day.
use serde::{Deserialize, Serialize};

pub mod actions;
pub mod reconcile;
pub mod workday;

/// A work day, `YYYY-MM-DD`. Sorts as text.
pub type Day = String;

/// Minutes after local midnight when a new work day starts (04:00).
pub const DEFAULT_ROLLOVER_MINUTES: u32 = 240;
/// From this many days open a row's age shows in the warning color.
pub const AGE_WARNING_DAYS: u32 = 2;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Ticket,
    PullRequest,
    Issue,
    Manual,
    #[default]
    Other,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    #[default]
    Normal,
    High,
    Urgent,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    #[default]
    Open,
    NeedsCheck,
    Done,
    Snoozed,
    Dismissed,
}

/// Who closed a task. Decides whether a later sync may question the close.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClosedBy {
    Source,
    User,
}

/// Why a task sits in Needs check.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckReason {
    /// It left the source's list with no clear reason.
    Missing,
    /// The user marked it done while the source still lists it open.
    DoneButSourceOpen,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Task {
    pub id: String,
    /// Stable identity, for example `clickup:123`; `manual:<id>` for manual tasks.
    pub key: String,
    pub title: String,
    pub kind: Kind,
    /// Display name of where it comes from: GitHub, ClickUp, Manual.
    pub source_name: String,
    /// The [`Source`] that owns it; `None` for a manual task.
    pub source_id: Option<String>,
    /// Why it is here: "Review requested from you".
    pub reason: String,
    /// The source's own status text: "In Progress", "Open".
    pub source_status: String,
    pub project: Option<String>,
    pub mono_id: Option<String>,
    pub link: Option<String>,
    /// `YYYY-MM-DD`.
    pub due: Option<String>,
    /// `HH:MM`, only with `due`.
    pub due_time: Option<String>,
    pub priority: Priority,
    pub tags: Vec<String>,
    pub status: Status,
    pub snoozed_until: Option<Day>,
    /// Epoch milliseconds.
    pub first_seen: u64,
    /// The work day it first counted; age is measured from here.
    pub first_day: Day,
    pub closed_at: Option<u64>,
    /// The work day it closed; a done task shows on that day only.
    pub closed_day: Option<Day>,
    pub close_reason: Option<String>,
    pub closed_by: Option<ClosedBy>,
    pub check_reason: Option<CheckReason>,
    /// The source confirmed it closed after the user dismissed it.
    pub source_closed_after_dismiss: bool,
    /// The user chose the project or priority; a sync no longer overwrites it.
    pub project_pinned: bool,
    pub priority_pinned: bool,
    pub updated_at: u64,
}

/// How a task reads on one day.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Group {
    CarriedOver,
    NeedsCheck,
    Today,
    Done,
    Snoozed,
    Dismissed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    pub id: String,
    pub task_id: String,
    pub day: Day,
    pub at: u64,
    /// `None` for the user, otherwise the Mono's id.
    pub mono_id: Option<String>,
    pub body: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncStatus {
    #[default]
    Never,
    Ok,
    Failed,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Source {
    pub id: String,
    pub name: String,
    /// Display name given to its tasks ("GitHub").
    pub provider: String,
    /// The owning Mono's id. Only this Mono can sync it.
    pub mono_id: String,
    /// The habit whose run last reported it; `None` until a habit run does.
    pub habit_id: Option<String>,
    pub projects: Vec<String>,
    /// The last successful sync. A failed run leaves it alone.
    pub last_sync_at: Option<u64>,
    /// The work day of `last_sync_at`.
    pub last_sync_day: Option<Day>,
    /// The last attempt, successful or not.
    pub last_attempt_at: Option<u64>,
    pub last_status: SyncStatus,
    pub last_error: Option<String>,
    /// Items in the last successful sync.
    pub item_count: usize,
}

/// One entry of a `today.sync` list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SyncItem {
    pub key: String,
    pub title: String,
    /// The source confirms this item is closed.
    pub closed: bool,
    pub close_reason: Option<String>,
    /// The source says it was reopened; this alone brings back a dismissed task.
    pub reopened: bool,
    pub source_status: Option<String>,
    pub kind: Option<Kind>,
    pub reason: Option<String>,
    /// Already validated against the Mono's projects by `actions`.
    pub project: Option<String>,
    pub link: Option<String>,
    pub due: Option<String>,
    pub due_time: Option<String>,
    pub priority: Option<Priority>,
    pub tags: Option<Vec<String>>,
}

/// `today.sync`, parsed. `items` is `None` only for a failed run: an empty
/// list is a real answer, and a failed run must never read as one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SyncRequest {
    pub source: String,
    pub name: Option<String>,
    pub provider: Option<String>,
    pub failed: bool,
    pub items: Option<Vec<SyncItem>>,
    pub error: Option<String>,
    /// Projects the items carry after validation, to store on the source.
    pub projects: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncSummary {
    pub added: usize,
    pub carried: usize,
    pub closed: usize,
    pub needs_check: usize,
    pub reopened: usize,
    /// Keys another source owns; ignored, keys never duplicate.
    pub conflicts: Vec<String>,
    /// Projects a Mono may not set; the item stays with no project.
    pub rejected_projects: Vec<String>,
}

/// A Mono calling through the app bridge, resolved from its session
/// capability. Never built from the caller's input.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MonoCaller {
    pub id: String,
    pub projects: Vec<String>,
    /// The habit whose run this is; `None` in the Mono's own conversation.
    pub habit: Option<String>,
    /// A hidden run session: no user is present, so it cannot hide tasks.
    pub in_run: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Actor {
    User,
    Mono(MonoCaller),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NewTask {
    pub title: String,
    /// The day it belongs to; a future day stores it snoozed until then.
    pub day: Option<Day>,
    pub project: Option<String>,
    pub priority: Priority,
    pub due: Option<String>,
    pub due_time: Option<String>,
    pub link: Option<String>,
    pub reason: Option<String>,
    pub tags: Vec<String>,
}

/// What `today.update` may change. `Some(None)` clears an optional field.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TaskPatch {
    pub title: Option<String>,
    pub priority: Option<Priority>,
    pub project: Option<Option<String>>,
    pub due: Option<Option<String>>,
    pub due_time: Option<Option<String>>,
    pub tags: Option<Vec<String>>,
    pub reason: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DayMode {
    Past,
    Current,
    Future,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    pub task: Task,
    pub group: Group,
    pub age_days: u32,
    pub latest_note: Option<Note>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub open: usize,
    pub carried_over: usize,
    pub to_check: usize,
}

/// Everything the page shows for one day.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DayView {
    pub day: Day,
    pub current_day: Day,
    pub mode: DayMode,
    /// Past: from frozen records. Current and future: live tasks.
    pub rows: Vec<Row>,
    pub summary: Summary,
    /// Past days the app has no records for.
    pub recorded: bool,
}

/// Whether a Mono sees and may touch a task: its own, one in its projects, or
/// a manual task with no project. Everything else reads as not found.
pub fn visible_to(task: &Task, caller: &MonoCaller) -> bool {
    if task.mono_id.as_deref() == Some(caller.id.as_str()) {
        return true;
    }
    match task.project.as_deref() {
        Some(project) => {
            let key = crate::files::path_key(project);
            caller
                .projects
                .iter()
                .any(|own| crate::files::path_key(own) == key)
        }
        None => task.source_id.is_none(),
    }
}

/// "Not synced today": there is at least one source and none synced on `day`.
pub fn not_synced(sources: &[Source], day: &str) -> bool {
    !sources.is_empty()
        && sources
            .iter()
            .all(|source| source.last_sync_day.as_deref() != Some(day))
}
