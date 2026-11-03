//! The reconcile rules, pure: tasks and a report in, changed tasks out.
use super::statuses::{self, builtin_id, resolve, stage_of_source_status};
use super::{
    CheckReason, ClosedBy, Kind, Priority, Stage, Status, SyncItem, SyncSummary, Task,
    WorkflowStatus,
};
use std::collections::{HashMap, HashSet};

/// What the reconcile needs to know besides the tasks.
pub struct Context<'a> {
    pub source_id: &'a str,
    /// Becomes `Task::source_name` on new tasks.
    pub provider: &'a str,
    pub mono_id: &'a str,
    pub day: &'a str,
    pub now: u64,
    /// The workflow statuses (normalized), to read a task's current stage.
    pub statuses: &'a [WorkflowStatus],
}
pub struct Reconciled {
    /// Every new or changed task; the others are untouched.
    pub tasks: Vec<Task>,
    pub summary: SyncSummary,
}

const DEFAULT_CLOSE_REASON: &str = "Closed by source";
const UNKNOWN_STATUS: &str = "Unknown";

/// `existing` are this source's tasks, `foreign` the keys other owners hold.
/// A key repeated inside `items` counts once: the first entry wins.
pub fn reconcile(
    existing: &[Task],
    foreign: &HashSet<String>,
    items: &[SyncItem],
    context: &Context,
) -> Reconciled {
    let mut summary = SyncSummary::default();
    let mut out: Vec<Task> = Vec::new();
    let by_key: HashMap<&str, &Task> = existing
        .iter()
        .map(|task| (task.key.as_str(), task))
        .collect();
    let mut seen: HashSet<&str> = HashSet::new();

    for item in items {
        let key = item.key.as_str();
        if key.is_empty() || !seen.insert(key) {
            continue;
        }
        if foreign.contains(key) || key.starts_with("manual:") {
            if !summary.conflicts.iter().any(|known| known == key) {
                summary.conflicts.push(key.to_string());
            }
            continue;
        }
        match by_key.get(key) {
            Some(task) => {
                // A task saved before statuses existed gets its resolved id the
                // first time it changes; on its own that is not a change.
                let mut next = (*task).clone();
                if next.status_id.is_empty() {
                    next.status_id = resolve(task, context.statuses).id.clone();
                }
                let baseline = next.clone();
                listed(&mut next, item, context, &mut summary);
                if next != baseline {
                    next.updated_at = context.now;
                    out.push(next);
                }
            }
            None if !item.closed => {
                summary.added += 1;
                out.push(new_task(item, context));
            }
            // A closed item nobody knew has nothing to carry over.
            None => {}
        }
    }

    for task in existing {
        if seen.contains(task.key.as_str()) || task.status != Status::Open {
            continue;
        }
        let mut next = task.clone();
        if next.status_id.is_empty() {
            next.status_id = resolve(task, context.statuses).id.clone();
        }
        next.status = Status::NeedsCheck;
        next.check_reason = Some(CheckReason::Missing);
        next.source_status = UNKNOWN_STATUS.into();
        next.updated_at = context.now;
        summary.needs_check += 1;
        out.push(next);
    }
    Reconciled {
        tasks: out,
        summary,
    }
}

/// The stage an item states or implies. A Finished stage on an open item is
/// ignored: `closed` stays the only authority for closing a task.
fn item_stage(item: &SyncItem) -> Option<Stage> {
    let stage = item.stage.or_else(|| {
        item.source_status
            .as_deref()
            .and_then(stage_of_source_status)
    });
    stage.filter(|stage| item.closed || *stage != Stage::Finished)
}

fn new_task(item: &SyncItem, context: &Context) -> Task {
    let stage = item_stage(item).unwrap_or(Stage::NotStarted);
    Task {
        status_id: builtin_id(stage).into(),
        summary: item.summary.clone().unwrap_or_default(),
        id: format!("task-{}", uuid::Uuid::new_v4()),
        key: item.key.clone(),
        title: item.title.clone(),
        kind: item.kind.unwrap_or(Kind::Other),
        source_name: context.provider.to_string(),
        source_id: Some(context.source_id.to_string()),
        reason: item.reason.clone().unwrap_or_default(),
        source_status: item.source_status.clone().unwrap_or_default(),
        project: item.project.clone(),
        mono_id: Some(context.mono_id.to_string()),
        link: item.link.clone(),
        due: item.due.clone(),
        due_time: item.due_time.clone(),
        priority: item.priority.unwrap_or(Priority::Normal),
        tags: item.tags.clone().unwrap_or_default(),
        status: Status::Open,
        first_seen: context.now,
        first_day: context.day.to_string(),
        updated_at: context.now,
        ..Task::default()
    }
}

/// Fields the item supplies; a user's pinned project or priority stays.
fn apply_fields(task: &mut Task, item: &SyncItem) {
    if !item.title.trim().is_empty() {
        task.title = item.title.clone();
    }
    if let Some(value) = &item.source_status {
        task.source_status = value.clone();
    }
    if let Some(value) = item.kind {
        task.kind = value;
    }
    if let Some(value) = &item.reason {
        task.reason = value.clone();
    }
    if let (Some(value), false) = (&item.project, task.project_pinned) {
        task.project = Some(value.clone());
    }
    if let Some(value) = &item.link {
        task.link = Some(value.clone());
    }
    if let Some(value) = &item.due {
        task.due = Some(value.clone());
    }
    if let Some(value) = &item.due_time {
        task.due_time = Some(value.clone());
    }
    if let (Some(value), false) = (item.priority, task.priority_pinned) {
        task.priority = value;
    }
    if let Some(value) = &item.tags {
        task.tags = value.clone();
    }
    if let Some(value) = &item.summary {
        task.summary = value.clone();
    }
}

/// An unpinned task listed open follows the stage the source states, unless it
/// is finished (the lifecycle rules own that) or already in that stage.
fn follow_stage(task: &mut Task, item: &SyncItem, context: &Context) {
    if task.status_pinned || task.status == Status::Dismissed {
        return;
    }
    let current = resolve(task, context.statuses).stage;
    if let Some(stage) = item_stage(item) {
        if stage != Stage::Finished && current != Stage::Finished && stage != current {
            task.status_id = builtin_id(stage).into();
        }
    }
}

fn clear_closed(task: &mut Task) {
    task.closed_at = None;
    task.closed_day = None;
    task.close_reason = None;
    task.closed_by = None;
}

fn listed(task: &mut Task, item: &SyncItem, context: &Context, summary: &mut SyncSummary) {
    if item.closed {
        listed_closed(task, item, context, summary);
        return;
    }
    apply_fields(task, item);
    match task.status {
        Status::Open => summary.carried += 1,
        Status::NeedsCheck => {
            if task.check_reason == Some(CheckReason::Missing) {
                task.status = Status::Open;
                task.check_reason = None;
                // The placeholder from going missing is not a real status.
                if item.source_status.is_none() && task.source_status == UNKNOWN_STATUS {
                    task.source_status.clear();
                }
                summary.carried += 1;
            }
        }
        Status::Done => {
            if task.closed_by == Some(ClosedBy::User) {
                // The user answered an earlier check with Keep done: leave it be.
                if !task.kept_done {
                    task.status = Status::NeedsCheck;
                    task.check_reason = Some(CheckReason::DoneButSourceOpen);
                    summary.needs_check += 1;
                    clear_closed(task);
                }
            } else {
                task.status = Status::Open;
                task.kept_done = false;
                task.status_id = builtin_id(item_stage(item).unwrap_or(Stage::NotStarted)).into();
                summary.reopened += 1;
                clear_closed(task);
            }
        }
        Status::Snoozed => {}
        Status::Dismissed => {
            if item.reopened || task.source_closed_after_dismiss {
                task.status = Status::Open;
                task.source_closed_after_dismiss = false;
                // Never open with a finished status.
                if resolve(task, context.statuses).stage == Stage::Finished {
                    task.status_id =
                        builtin_id(item_stage(item).unwrap_or(Stage::NotStarted)).into();
                }
                summary.reopened += 1;
            }
        }
    }
    follow_stage(task, item, context);
}

fn listed_closed(task: &mut Task, item: &SyncItem, context: &Context, summary: &mut SyncSummary) {
    // Already closed by the source: nothing to change, not even the text.
    if task.status == Status::Done && task.closed_by != Some(ClosedBy::User) {
        return;
    }
    apply_fields(task, item);
    match task.status {
        Status::Dismissed => task.source_closed_after_dismiss = true,
        status => {
            // A done-by-user task keeps the moment the user closed it.
            if status != Status::Done {
                task.closed_at = Some(context.now);
                task.closed_day = Some(context.day.to_string());
            }
            if resolve(task, context.statuses).stage != Stage::Finished {
                task.status_id = statuses::DONE_ID.into();
            }
            task.status = Status::Done;
            task.closed_by = Some(ClosedBy::Source);
            task.close_reason = Some(
                item.close_reason
                    .clone()
                    .unwrap_or_else(|| DEFAULT_CLOSE_REASON.into()),
            );
            task.check_reason = None;
            task.snoozed_until = None;
            summary.closed += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: &str = "2026-07-14";

    fn statuses() -> &'static [WorkflowStatus] {
        static LIST: std::sync::OnceLock<Vec<WorkflowStatus>> = std::sync::OnceLock::new();
        LIST.get_or_init(|| {
            let mut list = statuses::default_statuses();
            list.insert(
                1,
                WorkflowStatus {
                    id: "custom-next".into(),
                    name: "Next".into(),
                    stage: Stage::NotStarted,
                    color: "#e5a23c".into(),
                    builtin: false,
                },
            );
            statuses::normalize(list)
        })
    }

    fn context(now: u64) -> Context<'static> {
        Context {
            source_id: "src",
            provider: "ClickUp",
            mono_id: "mono-1",
            day: DAY,
            now,
            statuses: statuses(),
        }
    }

    fn task(key: &str, status: Status) -> Task {
        Task {
            id: format!("id-{key}"),
            key: key.into(),
            title: format!("Title {key}"),
            source_id: Some("src".into()),
            source_name: "ClickUp".into(),
            mono_id: Some("mono-1".into()),
            status,
            first_seen: 1,
            first_day: "2026-07-10".into(),
            updated_at: 1,
            ..Task::default()
        }
    }

    fn item(key: &str) -> SyncItem {
        SyncItem {
            key: key.into(),
            title: format!("Title {key}"),
            ..SyncItem::default()
        }
    }

    fn closed(key: &str) -> SyncItem {
        SyncItem {
            closed: true,
            ..item(key)
        }
    }

    fn run(existing: &[Task], items: &[SyncItem]) -> Reconciled {
        reconcile(existing, &HashSet::new(), items, &context(1_000))
    }

    fn only(result: &Reconciled) -> &Task {
        assert_eq!(result.tasks.len(), 1, "{:?}", result.tasks);
        &result.tasks[0]
    }

    fn done_by(key: &str, by: ClosedBy) -> Task {
        Task {
            closed_at: Some(500),
            closed_day: Some("2026-07-12".into()),
            close_reason: Some("why".into()),
            closed_by: Some(by),
            ..task(key, Status::Done)
        }
    }

    #[test]
    fn listed_open_and_open_is_carried_with_new_source_status() {
        let existing = [task("a", Status::Open)];
        let mut next = item("a");
        next.source_status = Some("In Progress".into());
        let result = run(&existing, &[next]);
        assert_eq!(only(&result).source_status, "In Progress");
        assert_eq!(only(&result).status, Status::Open);
        assert_eq!(only(&result).first_day, "2026-07-10");
        assert_eq!(result.summary.carried, 1);
    }

    #[test]
    fn unchanged_listed_task_is_not_returned() {
        let existing = [task("a", Status::Open)];
        assert!(run(&existing, &[item("a")]).tasks.is_empty());
    }

    #[test]
    fn listed_open_missing_check_goes_back_to_open() {
        let existing = [Task {
            check_reason: Some(CheckReason::Missing),
            ..task("a", Status::NeedsCheck)
        }];
        let result = run(&existing, &[item("a")]);
        assert_eq!(only(&result).status, Status::Open);
        assert_eq!(only(&result).check_reason, None);
    }

    #[test]
    fn a_task_back_from_missing_loses_the_unknown_placeholder() {
        let existing = [Task {
            check_reason: Some(CheckReason::Missing),
            source_status: "Unknown".into(),
            ..task("a", Status::NeedsCheck)
        }];
        assert_eq!(only(&run(&existing, &[item("a")])).source_status, "");
        let mut with = item("a");
        with.source_status = Some("Open".into());
        assert_eq!(only(&run(&existing, &[with])).source_status, "Open");
    }

    #[test]
    fn listed_open_done_but_source_open_check_stays() {
        let existing = [Task {
            check_reason: Some(CheckReason::DoneButSourceOpen),
            ..task("a", Status::NeedsCheck)
        }];
        let result = run(&existing, &[item("a")]);
        assert!(result.tasks.is_empty());
        assert_eq!(result.summary.reopened, 0);
    }

    #[test]
    fn listed_open_done_by_user_needs_check_and_clears_closed_fields() {
        let existing = [done_by("a", ClosedBy::User)];
        let result = run(&existing, &[item("a")]);
        let got = only(&result);
        assert_eq!(got.status, Status::NeedsCheck);
        assert_eq!(got.check_reason, Some(CheckReason::DoneButSourceOpen));
        assert_eq!(
            (
                got.closed_at,
                &got.closed_day,
                &got.close_reason,
                got.closed_by
            ),
            (None, &None, &None, None)
        );
        assert_eq!(result.summary.needs_check, 1);
    }

    #[test]
    fn listed_open_done_by_source_opens_again_and_counts_reopened() {
        let existing = [done_by("a", ClosedBy::Source)];
        let result = run(&existing, &[item("a")]);
        let got = only(&result);
        assert_eq!(got.status, Status::Open);
        assert_eq!(got.closed_by, None);
        assert_eq!(got.closed_day, None);
        assert_eq!(result.summary.reopened, 1);
    }

    #[test]
    fn listed_open_snoozed_stays_snoozed_and_updates_fields() {
        let existing = [Task {
            snoozed_until: Some("2026-07-20".into()),
            ..task("a", Status::Snoozed)
        }];
        let mut next = item("a");
        next.source_status = Some("Blocked".into());
        let result = run(&existing, &[next]);
        assert_eq!(only(&result).status, Status::Snoozed);
        assert_eq!(only(&result).snoozed_until.as_deref(), Some("2026-07-20"));
        assert_eq!(only(&result).source_status, "Blocked");
    }

    #[test]
    fn listed_open_dismissed_stays_dismissed() {
        let existing = [task("a", Status::Dismissed)];
        let result = run(&existing, &[item("a")]);
        assert!(result.tasks.is_empty());
        assert_eq!(result.summary.reopened, 0);
    }

    #[test]
    fn listed_open_dismissed_returns_when_source_reopened_it() {
        let existing = [task("a", Status::Dismissed)];
        let mut next = item("a");
        next.reopened = true;
        let result = run(&existing, &[next]);
        assert_eq!(only(&result).status, Status::Open);
        assert_eq!(result.summary.reopened, 1);
    }

    #[test]
    fn listed_open_dismissed_returns_after_source_closed_it_earlier() {
        let existing = [Task {
            source_closed_after_dismiss: true,
            ..task("a", Status::Dismissed)
        }];
        let result = run(&existing, &[item("a")]);
        assert_eq!(only(&result).status, Status::Open);
        assert!(!only(&result).source_closed_after_dismiss);
        assert_eq!(result.summary.reopened, 1);
    }

    #[test]
    fn listed_closed_closes_open_needs_check_and_snoozed_tasks() {
        for status in [Status::Open, Status::NeedsCheck, Status::Snoozed] {
            let existing = [Task {
                check_reason: (status == Status::NeedsCheck).then_some(CheckReason::Missing),
                snoozed_until: (status == Status::Snoozed).then(|| "2026-07-20".into()),
                ..task("a", status)
            }];
            let mut next = closed("a");
            next.close_reason = Some("Merged".into());
            let result = run(&existing, &[next]);
            let got = only(&result);
            assert_eq!(got.status, Status::Done, "{status:?}");
            assert_eq!(got.closed_by, Some(ClosedBy::Source));
            assert_eq!(got.close_reason.as_deref(), Some("Merged"));
            assert_eq!(got.closed_at, Some(1_000));
            assert_eq!(got.closed_day.as_deref(), Some(DAY));
            assert_eq!(got.check_reason, None);
            assert_eq!(got.snoozed_until, None);
            assert_eq!(result.summary.closed, 1);
        }
    }

    #[test]
    fn listed_closed_without_reason_says_closed_by_source() {
        let result = run(&[task("a", Status::Open)], &[closed("a")]);
        assert_eq!(
            only(&result).close_reason.as_deref(),
            Some("Closed by source")
        );
    }

    #[test]
    fn listed_closed_done_by_user_keeps_closed_at_but_becomes_source() {
        let result = run(&[done_by("a", ClosedBy::User)], &[closed("a")]);
        let got = only(&result);
        assert_eq!(got.status, Status::Done);
        assert_eq!(got.closed_by, Some(ClosedBy::Source));
        assert_eq!(got.closed_at, Some(500));
        assert_eq!(got.closed_day.as_deref(), Some("2026-07-12"));
        assert_eq!(got.close_reason.as_deref(), Some("Closed by source"));
    }

    #[test]
    fn listed_closed_dismissed_stays_dismissed_and_flags_it() {
        let result = run(&[task("a", Status::Dismissed)], &[closed("a")]);
        let got = only(&result);
        assert_eq!(got.status, Status::Dismissed);
        assert!(got.source_closed_after_dismiss);
        assert_eq!(result.summary.closed, 0);
    }

    #[test]
    fn listed_closed_already_done_by_source_is_unchanged() {
        let mut edited = closed("a");
        edited.title = "A new title".into();
        edited.source_status = Some("Shipped".into());
        let result = run(&[done_by("a", ClosedBy::Source)], &[edited]);
        assert!(result.tasks.is_empty());
        assert_eq!(result.summary.closed, 0);
    }

    #[test]
    fn not_listed_open_needs_check_missing() {
        let result = run(&[task("a", Status::Open)], &[]);
        let got = only(&result);
        assert_eq!(got.status, Status::NeedsCheck);
        assert_eq!(got.check_reason, Some(CheckReason::Missing));
        assert_eq!(got.source_status, "Unknown");
        assert_eq!(result.summary.needs_check, 1);
    }

    #[test]
    fn not_listed_other_statuses_are_unchanged() {
        let existing = [
            Task {
                check_reason: Some(CheckReason::Missing),
                ..task("a", Status::NeedsCheck)
            },
            done_by("b", ClosedBy::User),
            task("c", Status::Dismissed),
            task("d", Status::Snoozed),
        ];
        let result = run(&existing, &[]);
        assert!(result.tasks.is_empty());
        assert_eq!(result.summary, SyncSummary::default());
    }

    #[test]
    fn new_key_open_is_added_today() {
        let mut next = item("a");
        next.kind = Some(Kind::PullRequest);
        next.reason = Some("Review requested".into());
        next.priority = Some(Priority::High);
        next.tags = Some(vec!["x".into()]);
        let result = run(&[], &[next]);
        let got = only(&result);
        assert_eq!(got.status, Status::Open);
        assert_eq!(got.first_day, DAY);
        assert_eq!(got.first_seen, 1_000);
        assert_eq!(got.kind, Kind::PullRequest);
        assert_eq!(got.source_name, "ClickUp");
        assert_eq!(got.source_id.as_deref(), Some("src"));
        assert_eq!(got.mono_id.as_deref(), Some("mono-1"));
        assert_eq!(got.priority, Priority::High);
        assert!(!got.id.is_empty());
        assert_eq!(result.summary.added, 1);
    }

    #[test]
    fn new_key_without_kind_is_other() {
        let result = run(&[], &[item("a")]);
        assert_eq!(only(&result).kind, Kind::Other);
    }

    #[test]
    fn new_key_listed_closed_is_ignored() {
        let result = run(&[], &[closed("a")]);
        assert!(result.tasks.is_empty());
        assert_eq!(result.summary.added, 0);
    }

    #[test]
    fn foreign_and_manual_keys_are_conflicts_and_never_touched() {
        let foreign: HashSet<String> = ["gh:1".to_string()].into();
        let items = [item("gh:1"), item("manual:x"), item("gh:1"), item("ok")];
        let result = reconcile(&[], &foreign, &items, &context(1_000));
        assert_eq!(result.summary.conflicts, ["gh:1", "manual:x"]);
        assert_eq!(only(&result).key, "ok");
    }

    #[test]
    fn a_duplicate_key_in_one_request_first_one_wins() {
        let mut second = item("a");
        second.title = "Second".into();
        let result = run(&[], &[item("a"), second]);
        assert_eq!(only(&result).title, "Title a");
        assert_eq!(result.summary.added, 1);
    }

    #[test]
    fn duplicate_listing_of_an_existing_key_applies_once() {
        let existing = [done_by("a", ClosedBy::Source)];
        let result = run(&existing, &[item("a"), item("a")]);
        assert_eq!(result.summary.reopened, 1);
    }

    #[test]
    fn pinned_project_and_priority_survive_a_sync() {
        let existing = [Task {
            project: Some("/mine".into()),
            priority: Priority::Urgent,
            project_pinned: true,
            priority_pinned: true,
            ..task("a", Status::Open)
        }];
        let mut next = item("a");
        next.project = Some("/other".into());
        next.priority = Some(Priority::Normal);
        let result = run(&existing, &[next]);
        assert!(result.tasks.is_empty());
    }

    #[test]
    fn unpinned_fields_update_only_when_supplied() {
        let existing = [Task {
            project: Some("/p".into()),
            priority: Priority::High,
            link: Some("https://old".into()),
            ..task("a", Status::Open)
        }];
        let mut next = item("a");
        next.link = Some("https://new".into());
        let result = run(&existing, &[next]);
        let got = only(&result);
        assert_eq!(got.link.as_deref(), Some("https://new"));
        assert_eq!(got.project.as_deref(), Some("/p"));
        assert_eq!(got.priority, Priority::High);
    }

    #[test]
    fn keys_never_duplicate() {
        let existing = [task("a", Status::Open), task("b", Status::Open)];
        let items = [item("a"), item("c"), item("c"), item("a")];
        let result = run(&existing, &items);
        let mut keys: Vec<_> = result.tasks.iter().map(|t| t.key.clone()).collect();
        keys.sort();
        let before = keys.len();
        keys.dedup();
        assert_eq!(before, keys.len());
    }

    fn staged(key: &str, stage: Stage) -> SyncItem {
        SyncItem {
            stage: Some(stage),
            ..item(key)
        }
    }

    fn with_status(mut t: Task, id: &str) -> Task {
        t.status_id = id.into();
        t
    }

    #[test]
    fn a_new_task_takes_the_explicit_stage() {
        let got = run(&[], &[staged("a", Stage::Started)]);
        assert_eq!(only(&got).status_id, "in_progress");
    }

    #[test]
    fn a_new_task_maps_the_stage_from_the_source_status() {
        let mut next = item("a");
        next.source_status = Some("In Review".into());
        assert_eq!(only(&run(&[], &[next])).status_id, "in_progress");
        let mut next = item("b");
        next.source_status = Some("Backlog".into());
        assert_eq!(only(&run(&[], &[next])).status_id, "todo");
    }

    #[test]
    fn an_explicit_stage_beats_the_source_status() {
        let mut next = staged("a", Stage::NotStarted);
        next.source_status = Some("In Progress".into());
        assert_eq!(only(&run(&[], &[next])).status_id, "todo");
    }

    #[test]
    fn a_new_task_with_no_stage_is_to_do() {
        assert_eq!(only(&run(&[], &[item("a")])).status_id, "todo");
        let mut odd = item("b");
        odd.source_status = Some("Wontfix".into());
        assert_eq!(only(&run(&[], &[odd])).status_id, "todo");
    }

    #[test]
    fn a_finished_stage_on_an_open_item_is_ignored() {
        let got = run(&[], &[staged("a", Stage::Finished)]);
        assert_eq!(only(&got).status_id, "todo");
        assert_eq!(only(&got).status, Status::Open);
        let mut done_text = item("b");
        done_text.source_status = Some("Done".into());
        assert_eq!(only(&run(&[], &[done_text])).status_id, "todo");
    }

    #[test]
    fn a_new_task_stores_the_summary() {
        let mut next = item("a");
        next.summary = Some("Line one\nLine two".into());
        assert_eq!(only(&run(&[], &[next])).summary, "Line one\nLine two");
    }

    #[test]
    fn an_absent_summary_keeps_the_old_one() {
        let existing = [Task {
            summary: "old".into(),
            ..with_status(task("a", Status::Open), "todo")
        }];
        assert!(run(&existing, &[item("a")]).tasks.is_empty());
        let mut next = item("a");
        next.summary = Some("new".into());
        assert_eq!(only(&run(&existing, &[next])).summary, "new");
    }

    #[test]
    fn an_unpinned_task_follows_a_different_stage() {
        let existing = [with_status(task("a", Status::Open), "todo")];
        let got = run(&existing, &[staged("a", Stage::Started)]);
        assert_eq!(only(&got).status_id, "in_progress");
        // Back to Not started lands on the built-in, not a custom status.
        let existing = [with_status(task("a", Status::Open), "in_progress")];
        let got = run(&existing, &[staged("a", Stage::NotStarted)]);
        assert_eq!(only(&got).status_id, "todo");
    }

    #[test]
    fn a_task_in_the_same_stage_keeps_its_custom_status() {
        let existing = [with_status(task("a", Status::Open), "custom-next")];
        assert!(run(&existing, &[staged("a", Stage::NotStarted)])
            .tasks
            .is_empty());
    }

    #[test]
    fn a_pinned_status_is_never_mapped() {
        let existing = [Task {
            status_pinned: true,
            ..with_status(task("a", Status::Open), "todo")
        }];
        assert!(run(&existing, &[staged("a", Stage::Started)])
            .tasks
            .is_empty());
    }

    #[test]
    fn a_finished_stage_on_an_open_item_does_not_move_an_existing_task() {
        let existing = [with_status(task("a", Status::Open), "in_progress")];
        assert!(run(&existing, &[staged("a", Stage::Finished)])
            .tasks
            .is_empty());
    }

    #[test]
    fn user_finished_task_listed_open_needs_check_and_keeps_its_finished_status() {
        let existing = [with_status(done_by("a", ClosedBy::User), "done")];
        let got = run(&existing, &[staged("a", Stage::Started)]);
        assert_eq!(only(&got).status, Status::NeedsCheck);
        assert_eq!(only(&got).status_id, "done");
    }

    #[test]
    fn a_kept_done_task_listed_open_stays_done() {
        let existing = [Task {
            kept_done: true,
            ..with_status(done_by("a", ClosedBy::User), "done")
        }];
        let result = run(&existing, &[item("a")]);
        assert!(result.tasks.is_empty());
        assert_eq!(result.summary.needs_check, 0);
    }

    #[test]
    fn a_kept_done_task_later_listed_closed_becomes_source_closed() {
        let existing = [Task {
            kept_done: true,
            ..with_status(done_by("a", ClosedBy::User), "done")
        }];
        let got = run(&existing, &[closed("a")]);
        assert_eq!(only(&got).closed_by, Some(ClosedBy::Source));
        assert_eq!(only(&got).status_id, "done");
    }

    #[test]
    fn source_reopening_a_kept_done_task_clears_the_flag() {
        let existing = [Task {
            kept_done: true,
            ..with_status(done_by("a", ClosedBy::Source), "done")
        }];
        let got = run(&existing, &[item("a")]);
        assert!(!only(&got).kept_done);
    }

    #[test]
    fn source_closing_moves_to_done_unless_already_finished() {
        let got = run(
            &[with_status(task("a", Status::Open), "in_progress")],
            &[closed("a")],
        );
        assert_eq!(only(&got).status_id, "done");
        let mut list = statuses().to_vec();
        list.push(WorkflowStatus {
            id: "shipped".into(),
            name: "Shipped".into(),
            stage: Stage::Finished,
            color: "#10b981".into(),
            builtin: false,
        });
        let list = statuses::normalize(list);
        let ctx = Context {
            statuses: &list,
            ..context(1_000)
        };
        let existing = [with_status(done_by("a", ClosedBy::User), "shipped")];
        let got = reconcile(&existing, &HashSet::new(), &[closed("a")], &ctx);
        assert_eq!(only(&got).status_id, "shipped");
    }

    #[test]
    fn source_closing_a_pinned_task_still_applies_the_lifecycle() {
        let existing = [Task {
            status_pinned: true,
            ..with_status(task("a", Status::Open), "in_progress")
        }];
        let got = run(&existing, &[closed("a")]);
        assert_eq!(only(&got).status, Status::Done);
        assert_eq!(only(&got).status_id, "done");
    }

    #[test]
    fn source_reopening_a_closed_task_takes_the_item_stage_or_to_do() {
        let existing = [with_status(done_by("a", ClosedBy::Source), "done")];
        let got = run(&existing, &[staged("a", Stage::Started)]);
        assert_eq!(only(&got).status, Status::Open);
        assert_eq!(only(&got).status_id, "in_progress");
        let got = run(&existing, &[item("a")]);
        assert_eq!(only(&got).status_id, "todo");
    }

    #[test]
    fn going_missing_keeps_the_status() {
        let existing = [with_status(task("a", Status::Open), "in_progress")];
        let got = run(&existing, &[]);
        assert_eq!(only(&got).status, Status::NeedsCheck);
        assert_eq!(only(&got).status_id, "in_progress");
    }

    #[test]
    fn a_task_saved_before_statuses_gets_its_id_only_when_it_changes() {
        let old = task("a", Status::Open);
        assert!(run(std::slice::from_ref(&old), &[item("a")])
            .tasks
            .is_empty());
        let mut next = item("a");
        next.title = "Renamed".into();
        assert_eq!(only(&run(&[old], &[next])).status_id, "todo");
    }

    #[test]
    fn a_dismissed_task_with_a_finished_status_reopens_with_an_unfinished_one() {
        let existing = [with_status(task("a", Status::Dismissed), "done")];
        let mut next = staged("a", Stage::Started);
        next.reopened = true;
        let got = run(&existing, &[next]);
        assert_eq!(only(&got).status, Status::Open);
        assert_eq!(only(&got).status_id, "in_progress");
        let mut plain = item("a");
        plain.reopened = true;
        assert_eq!(only(&run(&existing, &[plain])).status_id, "todo");
    }

    #[test]
    fn an_unknown_status_id_is_left_as_it_is() {
        let existing = [with_status(task("a", Status::Open), "gone")];
        let mut next = item("a");
        next.title = "Renamed".into();
        assert_eq!(only(&run(&existing, &[next])).status_id, "gone");
    }
}
