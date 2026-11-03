use super::*;
use crate::today::{not_synced, visible_to, ClosedBy, SyncItem};
use chrono::{Local, TimeZone};

/// A local time in July 2026, away from any DST change.
fn t(day: u32, hour: u32, minute: u32) -> u64 {
    Local
        .with_ymd_and_hms(2026, 7, day, hour, minute, 0)
        .earliest()
        .unwrap()
        .timestamp_millis() as u64
}

fn store() -> Store {
    Store::open(std::path::Path::new(":memory:")).unwrap()
}

fn caller(id: &str, projects: &[&str]) -> MonoCaller {
    MonoCaller {
        id: id.into(),
        projects: projects.iter().map(|p| p.to_string()).collect(),
        habit: None,
        in_run: false,
    }
}

fn item(key: &str) -> SyncItem {
    SyncItem {
        key: key.into(),
        title: format!("Title {key}"),
        ..SyncItem::default()
    }
}

fn request(source: &str, items: Vec<SyncItem>) -> SyncRequest {
    SyncRequest {
        source: source.into(),
        name: Some("ClickUp".into()),
        provider: Some("ClickUp".into()),
        items: Some(items),
        ..SyncRequest::default()
    }
}

fn failed(source: &str) -> SyncRequest {
    SyncRequest {
        source: source.into(),
        failed: true,
        error: Some("timeout".into()),
        ..SyncRequest::default()
    }
}

fn sync(store: &Store, who: &MonoCaller, source: &str, keys: &[&str], now: u64) -> SyncSummary {
    let items = keys.iter().map(|k| item(k)).collect();
    store.today_sync(&request(source, items), who, now).unwrap()
}

fn by_key(store: &Store, key: &str) -> Task {
    store.today_task(key).unwrap().unwrap()
}

fn titles(view: &DayView) -> Vec<(String, Group)> {
    view.rows
        .iter()
        .map(|r| (r.task.key.clone(), r.group))
        .collect()
}

#[test]
fn sync_adds_tasks_and_a_view_groups_them() {
    let store = store();
    let me = caller("m1", &[]);
    let summary = sync(&store, &me, "cu", &["a", "b"], t(14, 10, 0));
    assert_eq!(summary.added, 2);
    let view = store.today_view("2026-07-14", t(14, 10, 0)).unwrap();
    assert_eq!(view.mode, DayMode::Current);
    assert_eq!(view.summary.open, 2);
    assert_eq!(view.summary.carried_over, 0);
    assert!(view.rows.iter().all(|r| r.group == Group::Today));
    assert!(view.recorded);
    assert_eq!(store.today_open_count(t(14, 10, 0)).unwrap(), 2);
    let source = &store.today_sources().unwrap()[0];
    assert_eq!(source.item_count, 2);
    assert_eq!(source.last_sync_day.as_deref(), Some("2026-07-14"));
    assert_eq!(source.mono_id, "m1");
}

#[test]
fn a_failed_sync_changes_nothing_and_keeps_old_data() {
    let store = store();
    let me = caller("m1", &[]);
    sync(&store, &me, "cu", &["a", "b"], t(14, 10, 0));
    let before = store.today_tasks(t(14, 11, 0)).unwrap();
    let summary = store.today_sync(&failed("cu"), &me, t(14, 12, 0)).unwrap();
    assert_eq!(summary, SyncSummary::default());
    assert_eq!(store.today_tasks(t(14, 12, 0)).unwrap(), before);
    let source = &store.today_sources().unwrap()[0];
    assert_eq!(source.item_count, 2);
    assert_eq!(source.last_sync_at, Some(t(14, 10, 0)));
    assert_eq!(source.last_attempt_at, Some(t(14, 12, 0)));
    assert_eq!(source.last_status, SyncStatus::Failed);
    assert_eq!(source.last_error.as_deref(), Some("timeout"));
}

#[test]
fn a_failed_sync_ignores_items_it_carries() {
    let store = store();
    let me = caller("m1", &[]);
    sync(&store, &me, "cu", &["a"], t(14, 10, 0));
    let mut bad = failed("cu");
    bad.items = Some(vec![]);
    store.today_sync(&bad, &me, t(14, 11, 0)).unwrap();
    assert_eq!(by_key(&store, "a").status, Status::Open);
}

#[test]
fn an_empty_ok_list_flags_open_tasks_but_a_missing_list_is_an_error() {
    let store = store();
    let me = caller("m1", &[]);
    sync(&store, &me, "cu", &["a", "b"], t(14, 10, 0));
    let summary = sync(&store, &me, "cu", &[], t(14, 12, 0));
    assert_eq!(summary.needs_check, 2);
    assert_eq!(by_key(&store, "a").status, Status::NeedsCheck);
    assert_eq!(store.today_sources().unwrap()[0].item_count, 0);
    let mut none = request("cu", vec![]);
    none.items = None;
    assert!(store.today_sync(&none, &me, t(14, 13, 0)).is_err());
    assert_eq!(
        store.today_sources().unwrap()[0].last_sync_at,
        Some(t(14, 12, 0))
    );
}

#[test]
fn another_mono_cannot_sync_an_owned_source_and_nothing_changes() {
    let store = store();
    let owner = caller("m1", &[]);
    let other = caller("m2", &[]);
    sync(&store, &owner, "cu", &["a"], t(14, 10, 0));
    let err = store
        .today_sync(&request("cu", vec![]), &other, t(14, 11, 0))
        .unwrap_err();
    assert!(err.contains("another Mono"));
    assert_eq!(by_key(&store, "a").status, Status::Open);
}

#[test]
fn a_key_another_source_owns_is_a_conflict_and_manual_keys_are_refused() {
    let store = store();
    let m1 = caller("m1", &[]);
    let m2 = caller("m2", &[]);
    sync(&store, &m1, "cu", &["a"], t(14, 10, 0));
    let summary = sync(&store, &m2, "gh", &["a", "manual:x", "b"], t(14, 11, 0));
    assert_eq!(summary.conflicts, ["a", "manual:x"]);
    assert_eq!(summary.added, 1);
    assert_eq!(by_key(&store, "a").source_id.as_deref(), Some("cu"));
    assert_eq!(store.today_sources().unwrap().len(), 2);
    assert_eq!(store.today_tasks(t(14, 12, 0)).unwrap().len(), 2);
}

#[test]
fn habit_id_is_set_by_a_run_and_kept_by_chat() {
    let store = store();
    let mut run = caller("m1", &[]);
    run.habit = Some("h1".into());
    sync(&store, &run, "cu", &["a"], t(14, 10, 0));
    assert_eq!(
        store.today_sources().unwrap()[0].habit_id.as_deref(),
        Some("h1")
    );
    sync(&store, &caller("m1", &[]), "cu", &["a"], t(14, 11, 0));
    assert_eq!(
        store.today_sources().unwrap()[0].habit_id.as_deref(),
        Some("h1")
    );
}

#[test]
fn the_store_leaves_item_projects_as_actions_set_them() {
    let store = store();
    let mut with = item("a");
    with.project = Some("/p1".into());
    store
        .today_sync(
            &request("cu", vec![with, item("b")]),
            &caller("m1", &["/p1"]),
            t(14, 10, 0),
        )
        .unwrap();
    assert_eq!(by_key(&store, "a").project.as_deref(), Some("/p1"));
    assert_eq!(by_key(&store, "b").project, None);
}

#[test]
fn a_user_pinned_project_and_priority_survive_a_sync() {
    let store = store();
    let me = caller("m1", &["/p1"]);
    sync(&store, &me, "cu", &["a"], t(14, 10, 0));
    let id = by_key(&store, "a").id;
    let patch = TaskPatch {
        priority: Some(Priority::Urgent),
        project: Some(Some("/mine".into())),
        ..TaskPatch::default()
    };
    store
        .today_update(&id, &patch, &Actor::User, t(14, 11, 0))
        .unwrap();
    let mut changed = item("a");
    changed.priority = Some(Priority::Normal);
    changed.project = Some("/p1".into());
    store
        .today_sync(&request("cu", vec![changed]), &me, t(14, 12, 0))
        .unwrap();
    let task = by_key(&store, "a");
    assert_eq!(task.priority, Priority::Urgent);
    assert_eq!(task.project.as_deref(), Some("/mine"));
}

#[test]
fn a_day_rolls_at_four_not_midnight() {
    let store = store();
    assert_eq!(store.today_roll(t(14, 3, 59)).unwrap(), "2026-07-13");
    assert_eq!(store.today_roll(t(14, 4, 0)).unwrap(), "2026-07-14");
}

#[test]
fn a_custom_rollover_moves_the_boundary() {
    let store = store();
    assert_eq!(store.today_rollover_minutes().unwrap(), 240);
    store.save_today_rollover_minutes(390).unwrap();
    assert_eq!(store.today_rollover_minutes().unwrap(), 390);
    assert_eq!(store.today_roll(t(14, 6, 29)).unwrap(), "2026-07-13");
    assert_eq!(store.today_roll(t(14, 6, 30)).unwrap(), "2026-07-14");
}

#[test]
fn the_rollover_setting_is_validated() {
    let store = store();
    assert!(store.save_today_rollover_minutes(721).is_err());
    store.save_today_rollover_minutes(0).unwrap();
    store.save_today_rollover_minutes(720).unwrap();
    store.save_setting("today.rolloverMinutes", "9999").unwrap();
    assert_eq!(store.today_rollover_minutes().unwrap(), 240);
    store.save_setting("today.rolloverMinutes", "soon").unwrap();
    assert_eq!(store.today_rollover_minutes().unwrap(), 240);
}

#[test]
fn a_task_stays_open_into_the_next_day_as_carried_over_with_age() {
    let store = store();
    sync(&store, &caller("m1", &[]), "cu", &["a"], t(14, 10, 0));
    let view = store.today_view("2026-07-16", t(16, 9, 0)).unwrap();
    assert_eq!(titles(&view), [("a".to_string(), Group::CarriedOver)]);
    assert_eq!(view.rows[0].age_days, 2);
    assert_eq!(view.summary.carried_over, 1);
    assert_eq!(view.summary.open, 1);
}

#[test]
fn rollover_after_several_days_closed_returns_a_snooze_from_the_gap() {
    let store = store();
    let me = caller("m1", &[]);
    sync(&store, &me, "cu", &["a", "b"], t(10, 10, 0));
    let a = by_key(&store, "a").id;
    store
        .today_snooze(&a, "2026-07-12", &Actor::User, t(10, 11, 0))
        .unwrap();
    // The app is closed 11th to 13th and opens on the 14th.
    let view = store.today_view("2026-07-14", t(14, 9, 0)).unwrap();
    let task = by_key(&store, "a");
    assert_eq!(task.status, Status::Open);
    assert_eq!(task.snoozed_until, None);
    assert_eq!(task.first_day, "2026-07-10");
    let row = view.rows.iter().find(|r| r.task.key == "a").unwrap();
    assert_eq!(row.group, Group::CarriedOver);
    assert_eq!(row.age_days, 4);
    // Days the app never saw get no records and read as unrecorded.
    for gap in ["2026-07-11", "2026-07-12", "2026-07-13"] {
        let past = store.today_view(gap, t(14, 9, 0)).unwrap();
        assert_eq!(past.mode, DayMode::Past);
        assert!(!past.recorded, "{gap}");
        assert!(past.rows.is_empty());
    }
    // The last day the app ran keeps its record.
    let tenth = store.today_view("2026-07-10", t(14, 9, 0)).unwrap();
    assert!(tenth.recorded);
    assert_eq!(tenth.rows.len(), 2);
    assert_eq!(
        tenth.rows.iter().find(|r| r.task.key == "a").unwrap().group,
        Group::Snoozed
    );
}

#[test]
fn a_snooze_returns_on_its_day_not_before() {
    let store = store();
    sync(&store, &caller("m1", &[]), "cu", &["a"], t(10, 10, 0));
    let a = by_key(&store, "a").id;
    store
        .today_snooze(&a, "2026-07-12", &Actor::User, t(10, 11, 0))
        .unwrap();
    store.today_roll(t(11, 9, 0)).unwrap();
    assert_eq!(by_key(&store, "a").status, Status::Snoozed);
    let future = store.today_view("2026-07-12", t(11, 9, 0)).unwrap();
    assert_eq!(future.mode, DayMode::Future);
    assert_eq!(titles(&future), [("a".to_string(), Group::Snoozed)]);
    assert_eq!(future.summary, Summary::default());
    store.today_roll(t(12, 9, 0)).unwrap();
    assert_eq!(by_key(&store, "a").status, Status::Open);
}

#[test]
fn snooze_needs_a_day_after_the_current_one() {
    let store = store();
    sync(&store, &caller("m1", &[]), "cu", &["a"], t(14, 10, 0));
    let a = by_key(&store, "a").id;
    for until in ["2026-07-14", "2026-07-13"] {
        assert!(store
            .today_snooze(&a, until, &Actor::User, t(14, 11, 0))
            .is_err());
    }
    assert!(store
        .today_snooze(&a, "tomorrow", &Actor::User, t(14, 11, 0))
        .is_err());
    assert!(store
        .today_snooze(&a, "2026-07-15", &Actor::User, t(14, 11, 0))
        .is_ok());
}

#[test]
fn snooze_across_the_rollover_boundary_counts_from_the_work_day() {
    let store = store();
    // 03:30 on the 14th is still the 13th.
    sync(&store, &caller("m1", &[]), "cu", &["a"], t(14, 3, 30));
    let a = by_key(&store, "a").id;
    assert!(store
        .today_snooze(&a, "2026-07-13", &Actor::User, t(14, 3, 40))
        .is_err());
    assert!(store
        .today_snooze(&a, "2026-07-14", &Actor::User, t(14, 3, 40))
        .is_ok());
    store.today_roll(t(14, 4, 0)).unwrap();
    assert_eq!(by_key(&store, "a").status, Status::Open);
}

#[test]
fn past_days_are_never_rewritten() {
    let store = store();
    let me = caller("m1", &[]);
    sync(&store, &me, "cu", &["a", "b"], t(14, 10, 0));
    let a = by_key(&store, "a").id;
    store
        .today_note(&a, "first", &Actor::User, t(14, 11, 0))
        .unwrap();
    let yesterday = store.today_view("2026-07-14", t(14, 12, 0)).unwrap();

    // The next day: a note, a completion, a sync that closes and adds.
    store
        .today_note(&a, "second", &Actor::User, t(15, 9, 0))
        .unwrap();
    store.today_complete(&a, &Actor::User, t(15, 9, 5)).unwrap();
    let mut closed = item("b");
    closed.closed = true;
    store
        .today_sync(&request("cu", vec![closed, item("c")]), &me, t(15, 9, 10))
        .unwrap();
    store
        .today_update(
            &by_key(&store, "c").id,
            &TaskPatch {
                title: Some("Renamed".into()),
                ..TaskPatch::default()
            },
            &Actor::User,
            t(15, 9, 20),
        )
        .unwrap();

    let frozen = store.today_view("2026-07-14", t(15, 10, 0)).unwrap();
    assert_eq!(frozen.mode, DayMode::Past);
    assert_eq!(frozen.rows.len(), yesterday.rows.len());
    for (before, after) in yesterday.rows.iter().zip(&frozen.rows) {
        assert_eq!(before.task, after.task);
        assert_eq!(before.group, after.group);
        assert_eq!(before.age_days, after.age_days);
    }
    // Only a note written on or before that day shows on that row.
    let row = frozen.rows.iter().find(|r| r.task.key == "a").unwrap();
    assert_eq!(row.latest_note.as_ref().unwrap().body, "first");
    assert_eq!(frozen.summary, Summary::default());
    // The live view shows the newer note.
    let today = store.today_view("2026-07-15", t(15, 10, 0)).unwrap();
    let row = today.rows.iter().find(|r| r.task.key == "a").unwrap();
    assert_eq!(row.latest_note.as_ref().unwrap().body, "second");
    assert_eq!(row.group, Group::Done);
}

#[test]
fn a_done_task_shows_only_on_the_day_it_closed() {
    let store = store();
    sync(&store, &caller("m1", &[]), "cu", &["a"], t(14, 10, 0));
    let a = by_key(&store, "a").id;
    let done = store
        .today_complete(&a, &Actor::User, t(14, 11, 0))
        .unwrap();
    assert_eq!(done.closed_by, Some(ClosedBy::User));
    assert_eq!(done.closed_day.as_deref(), Some("2026-07-14"));
    let same = store.today_view("2026-07-14", t(14, 12, 0)).unwrap();
    assert_eq!(titles(&same), [("a".to_string(), Group::Done)]);
    let next = store.today_view("2026-07-15", t(15, 12, 0)).unwrap();
    assert!(next.rows.is_empty());
    let past = store.today_view("2026-07-14", t(15, 12, 0)).unwrap();
    assert_eq!(titles(&past), [("a".to_string(), Group::Done)]);
}

#[test]
fn records_follow_changes_through_the_day() {
    let store = store();
    sync(
        &store,
        &caller("m1", &[]),
        "cu",
        &["a", "b", "c"],
        t(14, 10, 0),
    );
    let id = |k: &str| by_key(&store, k).id;
    store
        .today_dismiss(&id("a"), &Actor::User, t(14, 11, 0))
        .unwrap();
    store
        .today_snooze(&id("b"), "2026-07-20", &Actor::User, t(14, 11, 0))
        .unwrap();
    store.today_roll(t(15, 9, 0)).unwrap();
    let past = store.today_view("2026-07-14", t(15, 9, 0)).unwrap();
    let mut groups = titles(&past);
    groups.sort_by_key(|(key, _)| key.clone());
    assert_eq!(
        groups,
        [
            ("a".to_string(), Group::Dismissed),
            ("b".to_string(), Group::Snoozed),
            ("c".to_string(), Group::Today),
        ]
    );
    let now = store.today_view("2026-07-15", t(15, 9, 0)).unwrap();
    assert_eq!(titles(&now), [("c".to_string(), Group::CarriedOver)]);
}

#[test]
fn a_task_missing_from_the_list_needs_check_and_a_sync_that_lists_it_again_clears_it() {
    let store = store();
    let me = caller("m1", &[]);
    sync(&store, &me, "cu", &["a"], t(14, 10, 0));
    sync(&store, &me, "cu", &[], t(14, 11, 0));
    let task = by_key(&store, "a");
    assert_eq!(task.status, Status::NeedsCheck);
    assert_eq!(task.check_reason, Some(today::CheckReason::Missing));
    let view = store.today_view("2026-07-14", t(14, 11, 0)).unwrap();
    assert_eq!(view.summary.to_check, 1);
    assert_eq!(store.today_open_count(t(14, 11, 0)).unwrap(), 1);
    sync(&store, &me, "cu", &["a"], t(14, 12, 0));
    assert_eq!(by_key(&store, "a").status, Status::Open);
}

#[test]
fn a_user_completed_task_still_open_at_the_source_needs_check() {
    let store = store();
    let me = caller("m1", &[]);
    sync(&store, &me, "cu", &["a"], t(14, 10, 0));
    let a = by_key(&store, "a").id;
    store
        .today_complete(&a, &Actor::User, t(14, 11, 0))
        .unwrap();
    sync(&store, &me, "cu", &["a"], t(14, 12, 0));
    let task = by_key(&store, "a");
    assert_eq!(task.status, Status::NeedsCheck);
    assert_eq!(
        task.check_reason,
        Some(today::CheckReason::DoneButSourceOpen)
    );
    assert_eq!(task.closed_by, None);
}

#[test]
fn add_stores_manual_tasks_and_future_days_snoozed() {
    let store = store();
    let task = store
        .today_add(
            &NewTask {
                title: " Write plan ".into(),
                ..NewTask::default()
            },
            &Actor::User,
            t(14, 10, 0),
        )
        .unwrap();
    assert_eq!(task.title, "Write plan");
    assert_eq!(task.source_id, None);
    assert_eq!(task.mono_id, None);
    assert!(task.key.starts_with("manual:"));
    assert_eq!(task.first_day, "2026-07-14");
    assert_eq!(task.status, Status::Open);

    let future = store
        .today_add(
            &NewTask {
                title: "Later".into(),
                day: Some("2026-07-17".into()),
                ..NewTask::default()
            },
            &Actor::User,
            t(14, 10, 0),
        )
        .unwrap();
    assert_eq!(future.status, Status::Snoozed);
    assert_eq!(future.snoozed_until.as_deref(), Some("2026-07-17"));
    assert_eq!(future.first_day, "2026-07-17");
    let view = store.today_view("2026-07-17", t(14, 10, 0)).unwrap();
    assert_eq!(titles(&view).len(), 1);
    store.today_roll(t(17, 9, 0)).unwrap();
    let woke = by_key(&store, &future.key);
    assert_eq!(woke.status, Status::Open);
    let live = store.today_view("2026-07-17", t(17, 9, 0)).unwrap();
    let row = live.rows.iter().find(|r| r.task.key == future.key).unwrap();
    assert_eq!(row.group, Group::Today);
    assert_eq!(row.age_days, 0);

    let err = store
        .today_add(
            &NewTask {
                title: "Old".into(),
                day: Some("2026-07-10".into()),
                ..NewTask::default()
            },
            &Actor::User,
            t(17, 9, 0),
        )
        .unwrap_err();
    assert!(err.contains("past"));
    assert!(store
        .today_add(&NewTask::default(), &Actor::User, t(17, 9, 0))
        .is_err());
}

#[test]
fn a_mono_can_only_add_to_its_own_projects() {
    let store = store();
    let mono = Actor::Mono(caller("m1", &["/p1", "/p2"]));
    let bad = NewTask {
        title: "x".into(),
        project: Some("/other".into()),
        ..NewTask::default()
    };
    assert!(store.today_add(&bad, &mono, t(14, 10, 0)).is_err());
    let ok = NewTask {
        project: Some("/p2".into()),
        ..bad
    };
    let task = store.today_add(&ok, &mono, t(14, 10, 0)).unwrap();
    assert_eq!(task.mono_id.as_deref(), Some("m1"));
    assert_eq!(task.project.as_deref(), Some("/p2"));
    let single = Actor::Mono(caller("m2", &["/only"]));
    let filled = store
        .today_add(
            &NewTask {
                title: "y".into(),
                ..NewTask::default()
            },
            &single,
            t(14, 10, 0),
        )
        .unwrap();
    assert_eq!(filled.project.as_deref(), Some("/only"));
    let none = store
        .today_add(
            &NewTask {
                title: "z".into(),
                ..NewTask::default()
            },
            &mono,
            t(14, 10, 0),
        )
        .unwrap();
    assert_eq!(none.project, None);
    assert!(store
        .today_update(
            &task.id,
            &TaskPatch {
                project: Some(Some("/other".into())),
                ..TaskPatch::default()
            },
            &mono,
            t(14, 11, 0),
        )
        .is_err());
}

#[test]
fn ownership_scope_hides_other_tasks_as_not_found() {
    let store = store();
    sync(&store, &caller("m1", &["/p1"]), "cu", &["a"], t(14, 10, 0));
    let a = by_key(&store, "a").id;
    let stranger = Actor::Mono(caller("m9", &["/zzz"]));
    for result in [
        store
            .today_complete(&a, &stranger, t(14, 11, 0))
            .map(|_| ()),
        store.today_dismiss(&a, &stranger, t(14, 11, 0)).map(|_| ()),
        store
            .today_snooze(&a, "2026-07-20", &stranger, t(14, 11, 0))
            .map(|_| ()),
        store
            .today_note(&a, "hi", &stranger, t(14, 11, 0))
            .map(|_| ()),
        store
            .today_update(&a, &TaskPatch::default(), &stranger, t(14, 11, 0))
            .map(|_| ()),
    ] {
        assert_eq!(result.unwrap_err(), "Task not found");
    }
    assert_eq!(by_key(&store, "a").status, Status::Open);
    assert!(store.today_notes(&a).unwrap().is_empty());
    // Project paths compare exactly on unix; the owner and the user always see it.
    #[cfg(not(windows))]
    {
        let neighbour = Actor::Mono(caller("m8", &["/P1"]));
        assert!(store
            .today_note(&a, "hi", &neighbour, t(14, 11, 0))
            .is_err());
    }
    let owner = Actor::Mono(caller("m1", &[]));
    store.today_note(&a, "mine", &owner, t(14, 11, 0)).unwrap();
    store
        .today_complete(&a, &Actor::User, t(14, 12, 0))
        .unwrap();
}

#[test]
fn visible_to_covers_owner_project_and_manual_without_project() {
    let mono = caller("m1", &["/p1/"]);
    let task = |mono_id: Option<&str>, project: Option<&str>, source: Option<&str>| Task {
        mono_id: mono_id.map(str::to_string),
        project: project.map(str::to_string),
        source_id: source.map(str::to_string),
        ..Task::default()
    };
    assert!(visible_to(&task(Some("m1"), None, Some("s")), &mono));
    assert!(visible_to(&task(Some("m2"), Some("/p1"), Some("s")), &mono));
    assert!(visible_to(&task(None, None, None), &mono));
    assert!(!visible_to(&task(None, Some("/p2"), None), &mono));
    assert!(!visible_to(&task(Some("m2"), None, Some("s")), &mono));
}

#[test]
fn a_run_cannot_complete_snooze_or_dismiss_but_can_update_note_and_sync() {
    let store = store();
    let mut run = caller("m1", &[]);
    run.in_run = true;
    sync(&store, &run, "cu", &["a"], t(14, 10, 0));
    let a = by_key(&store, "a").id;
    let actor = Actor::Mono(run.clone());
    let refused =
        |r: Result<Task, String>| assert!(r.unwrap_err().contains("not allowed in a run"));
    refused(store.today_complete(&a, &actor, t(14, 11, 0)));
    refused(store.today_dismiss(&a, &actor, t(14, 11, 0)));
    refused(store.today_snooze(&a, "2026-07-20", &actor, t(14, 11, 0)));
    assert_eq!(by_key(&store, "a").status, Status::Open);
    store
        .today_update(
            &a,
            &TaskPatch {
                title: Some("New".into()),
                ..TaskPatch::default()
            },
            &actor,
            t(14, 11, 0),
        )
        .unwrap();
    store.today_note(&a, "seen", &actor, t(14, 11, 0)).unwrap();
    sync(&store, &run, "cu", &["a"], t(14, 12, 0));
    // The same Mono in chat may complete it.
    let chat = Actor::Mono(caller("m1", &[]));
    let done = store.today_complete(&a, &chat, t(14, 13, 0)).unwrap();
    assert_eq!(done.status, Status::Done);
}

#[test]
fn update_never_changes_status_and_only_the_user_pins() {
    let store = store();
    sync(&store, &caller("m1", &["/p1"]), "cu", &["a"], t(14, 10, 0));
    let a = by_key(&store, "a").id;
    let patch = TaskPatch {
        priority: Some(Priority::High),
        ..TaskPatch::default()
    };
    let by_mono = store
        .today_update(
            &a,
            &patch,
            &Actor::Mono(caller("m1", &["/p1"])),
            t(14, 11, 0),
        )
        .unwrap();
    assert_eq!(by_mono.priority, Priority::High);
    assert!(!by_mono.priority_pinned);
    assert_eq!(by_mono.status, Status::Open);
    let by_user = store
        .today_update(&a, &patch, &Actor::User, t(14, 12, 0))
        .unwrap();
    assert!(by_user.priority_pinned);
    assert!(store
        .today_update(
            &a,
            &TaskPatch {
                due_time: Some(Some("25:00".into())),
                due: Some(Some("2026-07-20".into())),
                ..TaskPatch::default()
            },
            &Actor::User,
            t(14, 13, 0)
        )
        .is_err());
    assert!(store
        .today_update(
            &a,
            &TaskPatch {
                title: Some("  ".into()),
                ..TaskPatch::default()
            },
            &Actor::User,
            t(14, 13, 0)
        )
        .is_err());
}

#[test]
fn notes_are_append_only_and_listed_oldest_first() {
    let store = store();
    sync(&store, &caller("m1", &[]), "cu", &["a"], t(14, 10, 0));
    let a = by_key(&store, "a").id;
    store
        .today_note(&a, "one", &Actor::User, t(14, 11, 0))
        .unwrap();
    let second = store
        .today_note(&a, "two", &Actor::Mono(caller("m1", &[])), t(14, 12, 0))
        .unwrap();
    assert_eq!(second.mono_id.as_deref(), Some("m1"));
    let notes = store.today_notes(&a).unwrap();
    assert_eq!(
        notes.iter().map(|n| n.body.as_str()).collect::<Vec<_>>(),
        ["one", "two"]
    );
    assert!(notes.iter().all(|n| n.day == "2026-07-14"));
    assert!(store
        .today_note(&a, "   ", &Actor::User, t(14, 13, 0))
        .is_err());
    assert!(store
        .today_note("missing", "x", &Actor::User, t(14, 13, 0))
        .is_err());
}

#[test]
fn reopen_takes_back_done_snoozed_and_dismissed() {
    let store = store();
    sync(
        &store,
        &caller("m1", &[]),
        "cu",
        &["a", "b", "c"],
        t(14, 10, 0),
    );
    let id = |k: &str| by_key(&store, k).id;
    store
        .today_complete(&id("a"), &Actor::User, t(14, 11, 0))
        .unwrap();
    store
        .today_snooze(&id("b"), "2026-07-20", &Actor::User, t(14, 11, 0))
        .unwrap();
    store
        .today_dismiss(&id("c"), &Actor::User, t(14, 11, 0))
        .unwrap();
    for key in ["a", "b", "c"] {
        let task = store.today_reopen(&id(key), t(14, 12, 0)).unwrap();
        assert_eq!(task.status, Status::Open, "{key}");
        assert_eq!(task.closed_day, None);
        assert_eq!(task.snoozed_until, None);
    }
    assert!(store
        .today_dismiss(&id("a"), &Actor::User, t(14, 13, 0))
        .is_ok());
    // A dismissed task cannot be completed until it is reopened.
    assert!(store
        .today_complete(&id("a"), &Actor::User, t(14, 13, 0))
        .unwrap_err()
        .contains("dismissed task"));
    store.today_reopen(&id("a"), t(14, 13, 30)).unwrap();
    assert!(store
        .today_complete(&id("a"), &Actor::User, t(14, 13, 40))
        .is_ok());
    assert!(store
        .today_dismiss(&id("a"), &Actor::User, t(14, 14, 0))
        .is_err());
}

#[test]
fn a_dismissed_task_returns_only_when_the_source_says_so() {
    let store = store();
    let me = caller("m1", &[]);
    sync(&store, &me, "cu", &["a"], t(14, 10, 0));
    let a = by_key(&store, "a").id;
    store.today_dismiss(&a, &Actor::User, t(14, 11, 0)).unwrap();
    sync(&store, &me, "cu", &["a"], t(14, 12, 0));
    assert_eq!(by_key(&store, "a").status, Status::Dismissed);
    let mut reopened = item("a");
    reopened.reopened = true;
    store
        .today_sync(&request("cu", vec![reopened]), &me, t(14, 13, 0))
        .unwrap();
    assert_eq!(by_key(&store, "a").status, Status::Open);
}

#[test]
fn not_synced_needs_a_source_and_no_sync_today() {
    let store = store();
    assert!(!not_synced(&store.today_sources().unwrap(), "2026-07-14"));
    sync(&store, &caller("m1", &[]), "cu", &["a"], t(14, 10, 0));
    let sources = store.today_sources().unwrap();
    assert!(!not_synced(&sources, "2026-07-14"));
    assert!(not_synced(&sources, "2026-07-15"));
    let mut other = sources[0].clone();
    other.last_sync_day = Some("2026-07-15".into());
    assert!(!not_synced(&[sources[0].clone(), other], "2026-07-15"));
}

#[test]
fn a_clock_set_back_does_not_reopen_a_passed_day() {
    let store = store();
    sync(&store, &caller("m1", &[]), "cu", &["a"], t(15, 10, 0));
    assert_eq!(store.today_roll(t(14, 10, 0)).unwrap(), "2026-07-15");
    let view = store.today_view("2026-07-14", t(14, 10, 0)).unwrap();
    assert_eq!(view.mode, DayMode::Past);
}

#[test]
fn a_corrupt_task_row_is_skipped_everywhere() {
    let store = store();
    let me = caller("m1", &[]);
    sync(&store, &me, "cu", &["a", "b"], t(14, 10, 0));
    store
        .conn
        .execute(
            "INSERT INTO today_tasks (id, key, source_id, status, first_day, json)
             VALUES ('bad', 'bad-key', 'cu', 'open', '2026-07-14', '{not json')",
            [],
        )
        .unwrap();
    store
        .conn
        .execute(
            "INSERT INTO today_sources (id, json) VALUES ('bad-src', 'nope')",
            [],
        )
        .unwrap();
    assert_eq!(store.today_tasks(t(14, 11, 0)).unwrap().len(), 2);
    assert_eq!(
        store
            .today_view("2026-07-14", t(14, 11, 0))
            .unwrap()
            .rows
            .len(),
        2
    );
    assert_eq!(store.today_sources().unwrap().len(), 1);
    assert_eq!(store.today_task("bad").unwrap(), None);
    // A sync that names the corrupt row's key cannot duplicate it.
    let summary = sync(&store, &me, "cu", &["a", "bad-key"], t(14, 12, 0));
    assert_eq!(summary.conflicts, ["bad-key"]);
    assert_eq!(summary.added, 0);
    assert_eq!(store.today_open_count(t(14, 12, 0)).unwrap(), 2);
    // A row from an older build (no tags) and one from a newer build (extra field) parse.
    store
        .conn
        .execute(
            "UPDATE today_tasks SET json = json_set(json, '$.futureField', 7) WHERE key = 'b'",
            [],
        )
        .unwrap();
    assert!(store.today_task("b").unwrap().is_some());
    store
        .conn
        .execute(
            "UPDATE today_tasks SET json = json_remove(json, '$.tags') WHERE key = 'a'",
            [],
        )
        .unwrap();
    assert!(store.today_task("a").unwrap().is_some());
}

fn tables(path: &std::path::Path) -> Vec<String> {
    let conn = rusqlite::Connection::open(path).unwrap();
    let mut stmt = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name LIKE 'today_%' ORDER BY name")
        .unwrap();
    stmt.query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn count(path: &std::path::Path, table: &str) -> i64 {
    rusqlite::Connection::open(path)
        .unwrap()
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

const TODAY_TABLES: [&str; 5] = [
    "today_days",
    "today_events",
    "today_notes",
    "today_sources",
    "today_tasks",
];

#[test]
fn the_legacy_store_gains_today_tables_in_place_and_reopens_idempotently() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("native.db");
    {
        let store = Store::open(&path).unwrap();
        store.create("codex", "/tmp").unwrap();
        store.save_setting("keep", "me").unwrap();
    }
    assert!(tables(&path).is_empty());
    {
        let store = Store::open(&path).unwrap();
        sync(&store, &caller("m1", &[]), "cu", &["a"], t(14, 10, 0));
    }
    assert_eq!(tables(&path), TODAY_TABLES);
    let store = Store::open(&path).unwrap();
    assert_eq!(store.setting("keep").unwrap().as_deref(), Some("me"));
    assert_eq!(count(&path, "native_sessions"), 1);
    assert_eq!(store.today_tasks(t(14, 11, 0)).unwrap().len(), 1);
    assert_eq!(store.today_sources().unwrap().len(), 1);
    drop(store);
    assert_eq!(tables(&path), TODAY_TABLES);
}

#[test]
fn the_shared_desktop_database_gains_today_tables_and_keeps_its_data() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("monocode.db");
    let session = {
        let store = Store::open_monocode(&path).unwrap();
        store.create("codex", "/tmp").unwrap().id
    };
    assert!(tables(&path).is_empty());
    {
        let store = Store::open_monocode(&path).unwrap();
        sync(&store, &caller("m1", &[]), "cu", &["a"], t(14, 10, 0));
        assert_eq!(store.today_rollover_minutes().unwrap(), 240);
        store.save_today_rollover_minutes(300).unwrap();
    }
    assert_eq!(tables(&path), TODAY_TABLES);
    let store = Store::open_monocode(&path).unwrap();
    assert_eq!(store.today_tasks(t(14, 11, 0)).unwrap().len(), 1);
    assert_eq!(store.today_rollover_minutes().unwrap(), 300);
    assert_eq!(count(&path, "sessions"), 1);
    let _ = session;
    sync(&store, &caller("m1", &[]), "cu", &["a", "b"], t(14, 12, 0));
    assert_eq!(store.today_tasks(t(14, 12, 0)).unwrap().len(), 2);
    drop(store);
    assert_eq!(tables(&path), TODAY_TABLES);
}

#[test]
fn a_read_only_shared_store_reads_empty_and_refuses_writes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("monocode.db");
    Store::open_monocode(&path).unwrap();
    let store = Store::read_monocode(&path).unwrap();
    assert!(store.today_tasks(t(14, 10, 0)).unwrap().is_empty());
    assert!(store.today_sources().unwrap().is_empty());
    assert!(store
        .today_add(
            &NewTask {
                title: "x".into(),
                ..NewTask::default()
            },
            &Actor::User,
            t(14, 10, 0)
        )
        .is_err());
    assert!(tables(&path).is_empty());
}

#[test]
fn a_done_task_gets_no_record_on_a_day_it_did_not_close() {
    let store = store();
    let me = caller("m1", &[]);
    sync(&store, &me, "cu", &["a"], t(13, 10, 0));
    let a = by_key(&store, "a").id;
    store
        .today_complete(&a, &Actor::User, t(13, 11, 0))
        .unwrap();
    // Next day the source closes it too, and the user edits it.
    let mut closed = item("a");
    closed.closed = true;
    store
        .today_sync(&request("cu", vec![closed]), &me, t(14, 10, 0))
        .unwrap();
    store
        .today_update(
            &a,
            &TaskPatch {
                title: Some("Edited".into()),
                ..TaskPatch::default()
            },
            &Actor::User,
            t(14, 10, 5),
        )
        .unwrap();
    assert_eq!(by_key(&store, "a").closed_by, Some(ClosedBy::Source));
    assert!(store
        .today_view("2026-07-14", t(14, 11, 0))
        .unwrap()
        .rows
        .is_empty());
    let later = store.today_view("2026-07-14", t(15, 11, 0)).unwrap();
    assert!(later.rows.is_empty(), "{:?}", titles(&later));
    let first = store.today_view("2026-07-13", t(15, 11, 0)).unwrap();
    assert_eq!(titles(&first), [("a".to_string(), Group::Done)]);
}

#[test]
fn a_task_already_closed_by_the_source_is_not_rewritten_by_a_later_sync() {
    let store = store();
    let me = caller("m1", &[]);
    sync(&store, &me, "cu", &["a"], t(13, 10, 0));
    let mut closed = item("a");
    closed.closed = true;
    store
        .today_sync(&request("cu", vec![closed.clone()]), &me, t(13, 11, 0))
        .unwrap();
    let before = by_key(&store, "a");
    closed.title = "Renamed upstream".into();
    closed.source_status = Some("Shipped".into());
    store
        .today_sync(&request("cu", vec![closed]), &me, t(15, 11, 0))
        .unwrap();
    assert_eq!(by_key(&store, "a"), before);
    assert!(store
        .today_view("2026-07-15", t(15, 12, 0))
        .unwrap()
        .rows
        .is_empty());
}

#[test]
fn snoozing_a_future_task_moves_its_first_day() {
    let store = store();
    let task = store
        .today_add(
            &NewTask {
                title: "Later".into(),
                day: Some("2026-07-17".into()),
                ..NewTask::default()
            },
            &Actor::User,
            t(14, 10, 0),
        )
        .unwrap();
    let moved = store
        .today_snooze(&task.id, "2026-07-20", &Actor::User, t(14, 11, 0))
        .unwrap();
    assert_eq!(moved.first_day, "2026-07-20");
    store.today_roll(t(20, 9, 0)).unwrap();
    let view = store.today_view("2026-07-20", t(20, 9, 0)).unwrap();
    assert_eq!(view.rows[0].group, Group::Today);
    assert_eq!(view.rows[0].age_days, 0);
}

#[test]
fn an_unreadable_source_row_is_never_adopted() {
    let store = store();
    sync(&store, &caller("m1", &[]), "cu", &["a"], t(14, 10, 0));
    store
        .conn
        .execute(
            "UPDATE today_sources SET json = 'garbage' WHERE id = 'cu'",
            [],
        )
        .unwrap();
    let err = store
        .today_sync(&request("cu", vec![]), &caller("m2", &[]), t(14, 11, 0))
        .unwrap_err();
    assert!(err.contains("unreadable"));
    assert_eq!(by_key(&store, "a").status, Status::Open);
}

#[test]
fn an_id_wins_over_a_key_that_equals_it() {
    let store = store();
    let me = caller("m1", &[]);
    sync(&store, &me, "cu", &["a"], t(14, 10, 0));
    let a = by_key(&store, "a");
    // Another source's task whose key is the first task's id.
    sync(
        &store,
        &caller("m2", &[]),
        "gh",
        &[a.id.as_str()],
        t(14, 10, 5),
    );
    assert_eq!(store.today_task(&a.id).unwrap().unwrap().key, "a");
}

// --- workflow statuses, stage-aware reconcile, events (Today v2) ------------------------------

use crate::today::statuses::{self, default_statuses};
use crate::today::{Activity, EventKind, Stage};

fn at(day: u32, hour: u32) -> u64 {
    t(day, hour, 0)
}

fn sync_items(store: &Store, who: &MonoCaller, items: Vec<SyncItem>, now: u64) -> SyncSummary {
    store.today_sync(&request("cu", items), who, now).unwrap()
}

fn staged(key: &str, stage: Stage) -> SyncItem {
    SyncItem {
        stage: Some(stage),
        ..item(key)
    }
}

fn closed_item(key: &str) -> SyncItem {
    SyncItem {
        closed: true,
        ..item(key)
    }
}

fn manual(store: &Store, title: &str, now: u64) -> Task {
    let new = NewTask {
        title: title.into(),
        ..NewTask::default()
    };
    store.today_add(&new, &Actor::User, now).unwrap()
}

fn add_status(store: &Store, name: &str, stage: Stage) -> WorkflowStatus {
    store.today_add_status(name, stage, "#112233").unwrap()
}

fn ids(list: &[WorkflowStatus]) -> Vec<String> {
    list.iter().map(|s| s.id.clone()).collect()
}

fn kinds(store: &Store, key: &str) -> Vec<EventKind> {
    store
        .today_activity(key)
        .unwrap()
        .into_iter()
        .filter_map(|a| match a {
            Activity::Event(e) => Some(e.kind),
            Activity::Note(_) => None,
        })
        .collect()
}

fn event_of(store: &Store, key: &str, kind: EventKind) -> TaskEvent {
    store
        .today_activity(key)
        .unwrap()
        .into_iter()
        .find_map(|a| match a {
            Activity::Event(e) if e.kind == kind => Some(e),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no {kind:?} event"))
}

#[test]
fn statuses_default_when_absent_and_unreadable_is_not_overwritten() {
    let store = store();
    assert_eq!(store.today_statuses().unwrap(), default_statuses());
    store.save_setting("today.statuses", "{not json").unwrap();
    assert_eq!(store.today_statuses().unwrap(), default_statuses());
    manual(&store, "a", at(14, 10));
    store.today_view("2026-07-14", at(14, 11)).unwrap();
    assert_eq!(
        store.setting("today.statuses").unwrap().as_deref(),
        Some("{not json")
    );
    store
        .today_add_status("Review", Stage::Started, "#aabbcc")
        .unwrap();
    assert_ne!(
        store.setting("today.statuses").unwrap().as_deref(),
        Some("{not json")
    );
}

#[test]
fn a_stored_list_missing_a_builtin_is_repaired_on_read() {
    let store = store();
    store.save_setting("today.statuses", "[]").unwrap();
    assert_eq!(store.today_statuses().unwrap(), default_statuses());
}

#[test]
fn adding_a_status_goes_to_the_end_of_its_stage() {
    let store = store();
    let first = add_status(&store, "Next", Stage::NotStarted);
    let second = add_status(&store, "Review", Stage::Started);
    assert!(first.id.starts_with("status-"));
    assert!(!first.builtin);
    assert_eq!(
        ids(&store.today_statuses().unwrap()),
        [
            "todo",
            first.id.as_str(),
            "in_progress",
            second.id.as_str(),
            "done"
        ]
    );
}

#[test]
fn status_names_are_trimmed_checked_and_unique_ignoring_case() {
    let store = store();
    assert_eq!(
        add_status(&store, "  Next  ", Stage::NotStarted).name,
        "Next"
    );
    for bad in ["", "   "] {
        assert!(store
            .today_add_status(bad, Stage::Started, "#112233")
            .is_err());
    }
    let long = "x".repeat(statuses::MAX_NAME + 1);
    assert!(store
        .today_add_status(&long, Stage::Started, "#112233")
        .unwrap_err()
        .contains("40"));
    assert!(store
        .today_add_status(&"x".repeat(statuses::MAX_NAME), Stage::Started, "#112233")
        .is_ok());
    for clash in ["next", "NEXT", "to do", "Done"] {
        assert_eq!(
            store
                .today_add_status(clash, Stage::Started, "#112233")
                .unwrap_err(),
            "Another status already has that name"
        );
    }
    // Renaming to its own name (other case) is fine.
    let next = store.today_statuses().unwrap()[1].clone();
    let patch = StatusPatch {
        name: Some("NEXT".into()),
        color: None,
    };
    assert_eq!(
        store
            .today_update_status(&next.id, &patch, at(14, 10))
            .unwrap()
            .name,
        "NEXT"
    );
}

#[test]
fn status_colors_must_be_rrggbb_and_are_stored_lowercase() {
    let store = store();
    for bad in ["red", "#12345", "#1234567", "112233", "#12345g"] {
        assert!(
            store.today_add_status("A", Stage::Started, bad).is_err(),
            "{bad}"
        );
    }
    assert_eq!(
        store
            .today_add_status("A", Stage::Started, "#AABBCC")
            .unwrap()
            .color,
        "#aabbcc"
    );
    let patch = StatusPatch {
        name: None,
        color: Some("nope".into()),
    };
    assert!(store
        .today_update_status("todo", &patch, at(14, 10))
        .is_err());
}

#[test]
fn builtins_can_be_renamed_and_recolored_but_not_deleted() {
    let store = store();
    let patch = StatusPatch {
        name: Some("Backlog".into()),
        color: Some("#ABCDEF".into()),
    };
    let got = store
        .today_update_status("todo", &patch, at(14, 10))
        .unwrap();
    assert_eq!(
        (got.name.as_str(), got.color.as_str()),
        ("Backlog", "#abcdef")
    );
    assert!(got.builtin);
    assert_eq!(store.today_statuses().unwrap()[0], got);
    assert!(store.today_delete_status("todo", at(14, 10)).is_err());
    assert!(store
        .today_update_status("missing", &StatusPatch::default(), at(14, 10))
        .is_err());
}

#[test]
fn moving_a_status_clamps_the_index_and_protects_builtins() {
    let store = store();
    let a = add_status(&store, "A", Stage::Started);
    let b = add_status(&store, "B", Stage::Started);
    let moved = store
        .today_move_status(&b.id, Stage::Started, 0, at(14, 10))
        .unwrap();
    assert_eq!(
        ids(&moved),
        ["todo", b.id.as_str(), "in_progress", a.id.as_str(), "done"]
    );
    let moved = store
        .today_move_status(&b.id, Stage::Started, 99, at(14, 10))
        .unwrap();
    assert_eq!(
        ids(&moved),
        ["todo", "in_progress", a.id.as_str(), b.id.as_str(), "done"]
    );
    // A built-in moves inside its stage only.
    let moved = store
        .today_move_status("in_progress", Stage::Started, 1, at(14, 10))
        .unwrap();
    assert_eq!(
        ids(&moved),
        ["todo", a.id.as_str(), "in_progress", b.id.as_str(), "done"]
    );
    assert!(store
        .today_move_status("in_progress", Stage::Finished, 0, at(14, 10))
        .is_err());
    assert_eq!(store.today_statuses().unwrap(), moved);
}

#[test]
fn a_status_can_move_to_a_stage_with_only_its_builtin() {
    let store = store();
    let a = add_status(&store, "A", Stage::Started);
    let moved = store
        .today_move_status(&a.id, Stage::NotStarted, 0, at(14, 10))
        .unwrap();
    assert_eq!(ids(&moved), [a.id.as_str(), "todo", "in_progress", "done"]);
    assert_eq!(moved[0].stage, Stage::NotStarted);
}

#[test]
fn deleting_a_status_moves_its_tasks_to_the_builtin_of_the_stage_with_events() {
    let store = store();
    let review = add_status(&store, "Review", Stage::Started);
    let task = manual(&store, "a", at(14, 10));
    store
        .today_set_status(&task.id, &review.id, &Actor::User, at(14, 11))
        .unwrap();
    let list = store.today_delete_status(&review.id, at(14, 12)).unwrap();
    assert_eq!(ids(&list), ["todo", "in_progress", "done"]);
    let got = by_key(&store, &task.key);
    assert_eq!(got.status_id, "in_progress");
    assert_eq!(got.status, Status::Open);
    let last = store
        .today_activity(&task.id)
        .unwrap()
        .into_iter()
        .rev()
        .find_map(|a| match a {
            Activity::Event(e) if e.kind == EventKind::StatusChanged => Some(e),
            _ => None,
        })
        .unwrap();
    assert_eq!(last.from.unwrap().id, review.id);
    assert_eq!(last.to.unwrap().id, "in_progress");
    assert!(store.today_delete_status("nope", at(14, 12)).is_err());
}

#[test]
fn deleting_a_finished_status_keeps_done_tasks_done() {
    let store = store();
    let shipped = add_status(&store, "Shipped", Stage::Finished);
    let task = manual(&store, "a", at(14, 10));
    store
        .today_set_status(&task.id, &shipped.id, &Actor::User, at(14, 11))
        .unwrap();
    store.today_delete_status(&shipped.id, at(14, 12)).unwrap();
    let got = by_key(&store, &task.key);
    assert_eq!((got.status, got.status_id.as_str()), (Status::Done, "done"));
}

#[test]
fn moving_a_custom_status_into_finished_completes_its_tasks_and_back_reopens() {
    let store = store();
    let review = add_status(&store, "Review", Stage::Started);
    let task = manual(&store, "a", at(14, 10));
    let other = manual(&store, "other", at(14, 10));
    store
        .today_set_status(&task.id, &review.id, &Actor::User, at(14, 11))
        .unwrap();
    store
        .today_move_status(&review.id, Stage::Finished, 0, at(14, 12))
        .unwrap();
    let got = by_key(&store, &task.key);
    assert_eq!(got.status, Status::Done);
    assert_eq!(got.status_id, review.id);
    assert_eq!(got.closed_by, Some(ClosedBy::User));
    assert_eq!(got.closed_day.as_deref(), Some("2026-07-14"));
    assert_eq!(by_key(&store, &other.key).status, Status::Open);
    assert!(kinds(&store, &task.key).contains(&EventKind::StatusChanged));
    let view = store.today_view("2026-07-14", at(14, 13)).unwrap();
    let row = view.rows.iter().find(|r| r.task.id == task.id).unwrap();
    assert_eq!(row.status.stage, Stage::Finished);
    store
        .today_move_status(&review.id, Stage::Started, 0, at(14, 14))
        .unwrap();
    let got = by_key(&store, &task.key);
    assert_eq!(
        (got.status, got.status_id.as_str()),
        (Status::Open, review.id.as_str())
    );
    assert_eq!(got.closed_day, None);
}

#[test]
fn new_manual_tasks_start_in_to_do_and_complete_reopen_keep_the_status_in_step() {
    let store = store();
    let task = manual(&store, "a", at(14, 10));
    assert_eq!(task.status_id, "todo");
    let done = store
        .today_complete(&task.id, &Actor::User, at(14, 11))
        .unwrap();
    assert_eq!(
        (done.status, done.status_id.as_str()),
        (Status::Done, "done")
    );
    let open = store.today_reopen(&task.id, at(14, 12)).unwrap();
    assert_eq!(
        (open.status, open.status_id.as_str()),
        (Status::Open, "todo")
    );
    assert_eq!(
        kinds(&store, &task.key),
        [
            EventKind::Added,
            EventKind::StatusChanged,
            EventKind::StatusChanged
        ]
    );
}

#[test]
fn completing_a_task_already_in_a_finished_status_keeps_it() {
    let store = store();
    let shipped = add_status(&store, "Shipped", Stage::Finished);
    let task = manual(&store, "a", at(14, 10));
    store
        .today_set_status(&task.id, &shipped.id, &Actor::User, at(14, 11))
        .unwrap();
    store.today_reopen(&task.id, at(14, 12)).unwrap();
    assert_eq!(by_key(&store, &task.key).status_id, "todo");
    let started = add_status(&store, "Review", Stage::Started);
    store
        .today_set_status(&task.id, &started.id, &Actor::User, at(14, 13))
        .unwrap();
    store.today_reopen(&task.id, at(14, 14)).unwrap();
    assert_eq!(by_key(&store, &task.key).status_id, started.id);
}

#[test]
fn set_status_inside_a_stage_changes_only_the_id_and_a_user_pins_it() {
    let store = store();
    let review = add_status(&store, "Review", Stage::Started);
    let task = manual(&store, "a", at(14, 10));
    let got = store
        .today_set_status(&task.id, &review.id, &Actor::User, at(14, 11))
        .unwrap();
    assert_eq!(
        (got.status, got.status_id.as_str()),
        (Status::Open, review.id.as_str())
    );
    assert!(got.status_pinned);
    assert!(store
        .today_set_status(&task.id, "nope", &Actor::User, at(14, 12))
        .is_err());
}

#[test]
fn set_status_out_of_finished_reopens_even_from_needs_check() {
    let store = store();
    let me = caller("m1", &[]);
    sync_items(&store, &me, vec![item("a")], at(14, 10));
    store.today_complete("a", &Actor::User, at(14, 11)).unwrap();
    sync_items(&store, &me, vec![item("a")], at(14, 12));
    let checked = by_key(&store, "a");
    assert_eq!(checked.status, Status::NeedsCheck);
    assert_eq!(checked.check_reason, Some(CheckReason::DoneButSourceOpen));
    assert_eq!(checked.status_id, "done");
    let got = store
        .today_set_status("a", "todo", &Actor::User, at(14, 13))
        .unwrap();
    assert_eq!((got.status, got.check_reason), (Status::Open, None));
    assert_eq!(got.status_id, "todo");
    assert!(kinds(&store, "a").contains(&EventKind::CheckCleared));
}

#[test]
fn set_status_into_finished_completes_a_task() {
    let store = store();
    let task = manual(&store, "a", at(14, 10));
    let got = store
        .today_set_status(&task.id, "done", &Actor::User, at(14, 11))
        .unwrap();
    assert_eq!(got.status, Status::Done);
    assert_eq!(got.closed_by, Some(ClosedBy::User));
    assert_eq!(got.closed_day.as_deref(), Some("2026-07-14"));
}

#[test]
fn a_mono_sets_status_without_pinning_and_never_finishes_in_a_run() {
    let store = store();
    let me = caller("m1", &[]);
    let other = caller("m2", &["/elsewhere"]);
    sync_items(&store, &me, vec![item("a")], at(14, 10));
    let got = store
        .today_set_status("a", "in_progress", &Actor::Mono(me.clone()), at(14, 11))
        .unwrap();
    assert!(!got.status_pinned);
    assert_eq!(got.status_id, "in_progress");
    let in_run = MonoCaller {
        in_run: true,
        habit: Some("h".into()),
        ..me.clone()
    };
    assert!(store
        .today_set_status("a", "done", &Actor::Mono(in_run.clone()), at(14, 12))
        .is_err());
    assert!(store
        .today_set_status("a", "todo", &Actor::Mono(in_run), at(14, 12))
        .is_ok());
    // A Mono that cannot see the task gets not found.
    assert_eq!(
        store
            .today_set_status("a", "todo", &Actor::Mono(other), at(14, 12))
            .unwrap_err(),
        "Task not found"
    );
}

#[test]
fn keep_done_answers_the_check_and_a_sync_does_not_raise_it_again() {
    let store = store();
    let me = caller("m1", &[]);
    sync_items(&store, &me, vec![item("a")], at(14, 10));
    assert!(store.today_keep_done("a", at(14, 10)).is_err());
    store.today_complete("a", &Actor::User, at(14, 11)).unwrap();
    assert!(store.today_keep_done("a", at(14, 11)).is_err());
    sync_items(&store, &me, vec![item("a")], at(14, 12));
    let kept = store.today_keep_done("a", at(14, 13)).unwrap();
    assert_eq!(kept.status, Status::Done);
    assert!(kept.kept_done);
    assert_eq!(kept.check_reason, None);
    assert_eq!(kept.status_id, "done");
    assert_eq!(kept.closed_day.as_deref(), Some("2026-07-14"));
    sync_items(&store, &me, vec![item("a")], at(14, 14));
    assert_eq!(by_key(&store, "a").status, Status::Done);
    let kinds = kinds(&store, "a");
    assert!(kinds.contains(&EventKind::KeptDone));
    assert!(!kinds[kinds.len() - 2..].contains(&EventKind::CheckCleared));
}

#[test]
fn a_kept_done_task_closed_by_the_source_later_is_source_closed_and_reopens_clear_the_flag() {
    let store = store();
    let me = caller("m1", &[]);
    sync_items(&store, &me, vec![item("a")], at(14, 10));
    store.today_complete("a", &Actor::User, at(14, 11)).unwrap();
    sync_items(&store, &me, vec![item("a")], at(14, 12));
    store.today_keep_done("a", at(14, 13)).unwrap();
    sync_items(&store, &me, vec![closed_item("a")], at(14, 14));
    let got = by_key(&store, "a");
    assert_eq!(got.closed_by, Some(ClosedBy::Source));
    sync_items(&store, &me, vec![item("a")], at(14, 15));
    let got = by_key(&store, "a");
    assert_eq!(got.status, Status::Open);
    assert!(!got.kept_done);
    assert_eq!(got.status_id, "todo");
}

#[test]
fn a_user_reopen_clears_kept_done() {
    let store = store();
    let me = caller("m1", &[]);
    sync_items(&store, &me, vec![item("a")], at(14, 10));
    store.today_complete("a", &Actor::User, at(14, 11)).unwrap();
    sync_items(&store, &me, vec![item("a")], at(14, 12));
    store.today_keep_done("a", at(14, 13)).unwrap();
    store.today_reopen("a", at(14, 14)).unwrap();
    assert!(!by_key(&store, "a").kept_done);
}

#[test]
fn sync_places_new_tasks_by_stage_explicit_then_source_status() {
    let store = store();
    let me = caller("m1", &[]);
    let mut reviewing = item("c");
    reviewing.source_status = Some("In Review".into());
    let mut explicit = staged("d", Stage::NotStarted);
    explicit.source_status = Some("In Progress".into());
    sync_items(
        &store,
        &me,
        vec![staged("a", Stage::Started), item("b"), reviewing, explicit],
        at(14, 10),
    );
    let got = |key: &str| by_key(&store, key).status_id;
    assert_eq!(
        (got("a").as_str(), got("b").as_str()),
        ("in_progress", "todo")
    );
    assert_eq!(
        (got("c").as_str(), got("d").as_str()),
        ("in_progress", "todo")
    );
}

#[test]
fn a_synced_stage_change_moves_the_task_with_an_event_by_the_source() {
    let store = store();
    let me = caller("m1", &[]);
    sync_items(&store, &me, vec![item("a")], at(14, 10));
    sync_items(&store, &me, vec![staged("a", Stage::Started)], at(14, 11));
    assert_eq!(by_key(&store, "a").status_id, "in_progress");
    let moved = event_of(&store, "a", EventKind::StatusChanged);
    assert_eq!(moved.source_id.as_deref(), Some("cu"));
    assert_eq!(moved.mono_id.as_deref(), Some("m1"));
    assert_eq!(moved.from.unwrap().id, "todo");
    assert_eq!(moved.to.unwrap().id, "in_progress");
}

#[test]
fn a_users_status_change_is_never_overwritten_by_a_sync() {
    let store = store();
    let me = caller("m1", &[]);
    sync_items(&store, &me, vec![item("a")], at(14, 10));
    store
        .today_set_status("a", "in_progress", &Actor::User, at(14, 11))
        .unwrap();
    sync_items(&store, &me, vec![item("a")], at(14, 12));
    sync_items(
        &store,
        &me,
        vec![staged("a", Stage::NotStarted)],
        at(14, 13),
    );
    assert_eq!(by_key(&store, "a").status_id, "in_progress");
    // The lifecycle still applies: the source closing it closes it.
    sync_items(&store, &me, vec![closed_item("a")], at(14, 14));
    let got = by_key(&store, "a");
    assert_eq!((got.status, got.status_id.as_str()), (Status::Done, "done"));
}

#[test]
fn a_finished_stage_on_an_open_item_is_ignored_and_the_sync_summary_counts_stay_v1() {
    let store = store();
    let me = caller("m1", &[]);
    let summary = sync_items(&store, &me, vec![staged("a", Stage::Finished)], at(14, 10));
    assert_eq!(summary.added, 1);
    let got = by_key(&store, "a");
    assert_eq!((got.status, got.status_id.as_str()), (Status::Open, "todo"));
}

#[test]
fn needs_check_both_causes_keep_the_status_and_record_events() {
    let store = store();
    let me = caller("m1", &[]);
    sync_items(
        &store,
        &me,
        vec![staged("a", Stage::Started), item("b")],
        at(14, 10),
    );
    store.today_complete("b", &Actor::User, at(14, 11)).unwrap();
    let summary = sync_items(&store, &me, vec![item("b")], at(14, 12));
    // "a" went missing, "b" was done by the user and is still listed.
    assert_eq!(summary.needs_check, 2);
    let a = by_key(&store, "a");
    assert_eq!(
        (a.status, a.check_reason),
        (Status::NeedsCheck, Some(CheckReason::Missing))
    );
    assert_eq!(a.status_id, "in_progress");
    let b = by_key(&store, "b");
    assert_eq!(b.check_reason, Some(CheckReason::DoneButSourceOpen));
    assert_eq!(b.status_id, "done");
    assert_eq!(
        event_of(&store, "a", EventKind::NeedsCheck).check_reason,
        Some(CheckReason::Missing)
    );
    assert_eq!(
        event_of(&store, "b", EventKind::NeedsCheck).check_reason,
        Some(CheckReason::DoneButSourceOpen)
    );
    // Listed again, the missing one comes back and the check clears.
    sync_items(&store, &me, vec![item("a"), item("b")], at(14, 13));
    assert_eq!(by_key(&store, "a").status, Status::Open);
    assert!(kinds(&store, "a").contains(&EventKind::CheckCleared));
}

#[test]
fn a_failed_sync_changes_no_status_and_writes_no_event() {
    let store = store();
    let me = caller("m1", &[]);
    sync_items(&store, &me, vec![item("a")], at(14, 10));
    let before = (store.today_tasks(at(14, 11)).unwrap(), kinds(&store, "a"));
    store.today_sync(&failed("cu"), &me, at(14, 12)).unwrap();
    assert_eq!(
        (store.today_tasks(at(14, 12)).unwrap(), kinds(&store, "a")),
        before
    );
}

#[test]
fn source_close_and_reopen_record_events_and_set_the_status() {
    let store = store();
    let me = caller("m1", &[]);
    sync_items(&store, &me, vec![staged("a", Stage::Started)], at(14, 10));
    let mut done = closed_item("a");
    done.close_reason = Some("Merged".into());
    sync_items(&store, &me, vec![done], at(14, 11));
    let got = by_key(&store, "a");
    assert_eq!((got.status, got.status_id.as_str()), (Status::Done, "done"));
    assert_eq!(
        event_of(&store, "a", EventKind::ClosedBySource)
            .detail
            .as_deref(),
        Some("Merged")
    );
    sync_items(&store, &me, vec![staged("a", Stage::Started)], at(14, 12));
    let got = by_key(&store, "a");
    assert_eq!(
        (got.status, got.status_id.as_str()),
        (Status::Open, "in_progress")
    );
    let reopened = event_of(&store, "a", EventKind::ReopenedBySource);
    assert_eq!(reopened.source_id.as_deref(), Some("cu"));
}

#[test]
fn snooze_and_dismiss_record_events() {
    let store = store();
    let me = caller("m1", &[]);
    sync_items(&store, &me, vec![item("a"), item("b")], at(14, 10));
    store
        .today_snooze("a", "2026-07-20", &Actor::User, at(14, 11))
        .unwrap();
    store.today_dismiss("b", &Actor::User, at(14, 11)).unwrap();
    assert_eq!(
        event_of(&store, "a", EventKind::Snoozed).detail.as_deref(),
        Some("2026-07-20")
    );
    assert!(kinds(&store, "b").contains(&EventKind::Dismissed));
}

#[test]
fn snoozing_a_done_but_source_open_check_leaves_the_finished_status() {
    let store = store();
    let me = caller("m1", &[]);
    sync_items(&store, &me, vec![item("a")], at(14, 10));
    store.today_complete("a", &Actor::User, at(14, 11)).unwrap();
    sync_items(&store, &me, vec![item("a")], at(14, 12));
    store
        .today_snooze("a", "2026-07-20", &Actor::User, at(14, 13))
        .unwrap();
    let got = by_key(&store, "a");
    assert_eq!(
        (got.status, got.status_id.as_str()),
        (Status::Snoozed, "todo")
    );
}

#[test]
fn rows_carry_the_status_and_sort_by_status_order_first() {
    let store = store();
    let review = add_status(&store, "Review", Stage::Started);
    let a = manual(&store, "A first", at(14, 10));
    let b = manual(&store, "B second", at(14, 10));
    let c = manual(&store, "C third", at(14, 10));
    store
        .today_set_status(&c.id, &review.id, &Actor::User, at(14, 11))
        .unwrap();
    store
        .today_set_status(&b.id, "in_progress", &Actor::User, at(14, 11))
        .unwrap();
    let view = store.today_view("2026-07-14", at(14, 12)).unwrap();
    let order: Vec<_> = view
        .rows
        .iter()
        .map(|r| (r.task.id.clone(), r.status.id.clone()))
        .collect();
    assert_eq!(
        order,
        [
            (a.id.clone(), "todo".to_string()),
            (b.id.clone(), "in_progress".to_string()),
            (c.id.clone(), review.id.clone()),
        ]
    );
    // Reordering the statuses reorders the rows.
    store
        .today_move_status(&review.id, Stage::Started, 0, at(14, 13))
        .unwrap();
    let view = store.today_view("2026-07-14", at(14, 13)).unwrap();
    assert_eq!(view.rows[1].task.id, c.id);
    assert_eq!(view.rows[2].task.id, b.id);
}

#[test]
fn a_past_day_keeps_the_status_as_it_stood_at_the_end_of_that_day() {
    let store = store();
    let review = store
        .today_add_status("Review", Stage::Started, "#aa00aa")
        .unwrap();
    let task = manual(&store, "a", at(14, 10));
    store
        .today_set_status(&task.id, &review.id, &Actor::User, at(14, 11))
        .unwrap();
    // A rename on the 14th shows on the 14th: its record is rewritten.
    let first = StatusPatch {
        name: Some("Code review".into()),
        color: Some("#00ff00".into()),
    };
    store
        .today_update_status(&review.id, &first, at(14, 12))
        .unwrap();
    let past = store.today_view("2026-07-14", at(15, 10)).unwrap();
    assert_eq!(past.mode, DayMode::Past);
    let row = &past.rows[0];
    assert_eq!(
        (row.status.name.as_str(), row.status.color.as_str()),
        ("Code review", "#00ff00")
    );
    // A rename on the 15th leaves the 14th alone.
    let second = StatusPatch {
        name: Some("QA".into()),
        color: None,
    };
    store.today_view("2026-07-15", at(15, 10)).unwrap();
    store
        .today_update_status(&review.id, &second, at(15, 10))
        .unwrap();
    let past = store.today_view("2026-07-14", at(15, 11)).unwrap();
    assert_eq!(past.rows[0].status.name, "Code review");
    let now = store.today_view("2026-07-15", at(15, 11)).unwrap();
    assert_eq!(now.rows[0].status.name, "QA");
    // Deleting the status later leaves the past view alone.
    store.today_delete_status(&review.id, at(15, 12)).unwrap();
    let past = store.today_view("2026-07-14", at(15, 13)).unwrap();
    assert_eq!(past.rows[0].status.name, "Code review");
    let now = store.today_view("2026-07-15", at(15, 13)).unwrap();
    assert_eq!(now.rows[0].status.id, "in_progress");
}

#[test]
fn moving_a_status_into_finished_does_not_finish_a_dismissed_task() {
    let store = store();
    let review = add_status(&store, "Review", Stage::Started);
    let task = manual(&store, "a", at(14, 10));
    store
        .today_set_status(&task.id, &review.id, &Actor::User, at(14, 11))
        .unwrap();
    store
        .today_dismiss(&task.id, &Actor::User, at(14, 12))
        .unwrap();
    store
        .today_move_status(&review.id, Stage::Finished, 0, at(14, 13))
        .unwrap();
    let got = by_key(&store, &task.key);
    assert_eq!(
        (got.status, got.status_id.as_str()),
        (Status::Dismissed, "in_progress")
    );
}

#[test]
fn an_unparseable_status_entry_is_dropped_alone() {
    let store = store();
    let good =
        serde_json::json!({"id": "c1", "name": "Next", "stage": "not_started", "color": "#112233"});
    let json = serde_json::json!([good, {"id": "bad", "stage": "nowhere"}, 5]);
    store
        .save_setting("today.statuses", &json.to_string())
        .unwrap();
    assert_eq!(
        ids(&store.today_statuses().unwrap()),
        ["todo", "c1", "in_progress", "done"]
    );
    store.save_setting("today.statuses", "{\"a\":1}").unwrap();
    assert_eq!(store.today_statuses().unwrap(), default_statuses());
}

#[test]
fn an_unknown_status_id_is_not_rewritten_by_a_save() {
    let store = store();
    let task = manual(&store, "a", at(14, 10));
    let mut value = serde_json::to_value(by_key(&store, &task.key)).unwrap();
    value["statusId"] = serde_json::json!("from-a-newer-build");
    store
        .conn
        .execute(
            "UPDATE today_tasks SET json = ?1 WHERE id = ?2",
            params![value.to_string(), task.id],
        )
        .unwrap();
    store
        .today_update(
            &task.id,
            &TaskPatch {
                title: Some("b".into()),
                ..TaskPatch::default()
            },
            &Actor::User,
            at(14, 11),
        )
        .unwrap();
    assert_eq!(by_key(&store, &task.key).status_id, "from-a-newer-build");
}

#[test]
fn past_rows_sort_a_deleted_status_after_the_rest_of_its_stage() {
    let store = store();
    let review = add_status(&store, "Review", Stage::Started);
    let a = manual(&store, "A", at(14, 10));
    let b = manual(&store, "B", at(14, 10));
    store
        .today_set_status(&a.id, &review.id, &Actor::User, at(14, 11))
        .unwrap();
    store
        .today_set_status(&b.id, "in_progress", &Actor::User, at(14, 11))
        .unwrap();
    store.today_delete_status(&review.id, at(15, 10)).unwrap();
    let past = store.today_view("2026-07-14", at(15, 11)).unwrap();
    let names: Vec<_> = past.rows.iter().map(|r| r.status.name.clone()).collect();
    assert_eq!(names, ["In progress", "Review"]);
}

#[test]
fn a_task_saved_before_statuses_resolves_on_read_and_persists_on_save() {
    let store = store();
    let me = caller("m1", &[]);
    sync_items(&store, &me, vec![item("a"), item("b")], at(14, 10));
    // Simulate pre-v2 JSON: no status fields at all.
    for key in ["a", "b"] {
        let mut value: serde_json::Value = serde_json::to_value(by_key(&store, key)).unwrap();
        for field in ["statusId", "statusPinned", "keptDone", "summary"] {
            value.as_object_mut().unwrap().remove(field);
        }
        store
            .conn
            .execute(
                "UPDATE today_tasks SET json = ?1 WHERE key = ?2",
                params![value.to_string(), key],
            )
            .unwrap();
    }
    assert_eq!(by_key(&store, "a").status_id, "");
    let view = store.today_view("2026-07-14", at(14, 11)).unwrap();
    assert!(view.rows.iter().all(|r| r.status.id == "todo"));
    // Reading changed nothing; the next real save persists the resolved id.
    assert_eq!(by_key(&store, "a").status_id, "");
    store
        .today_note("a", "hi", &Actor::User, at(14, 11))
        .unwrap();
    assert_eq!(by_key(&store, "a").status_id, "");
    store.today_complete("a", &Actor::User, at(14, 12)).unwrap();
    assert_eq!(by_key(&store, "a").status_id, "done");
    store
        .today_snooze("b", "2026-07-20", &Actor::User, at(14, 12))
        .unwrap();
    assert_eq!(by_key(&store, "b").status_id, "todo");
}

#[test]
fn old_lifecycle_states_resolve_to_sensible_statuses() {
    let store = store();
    let me = caller("m1", &[]);
    sync_items(
        &store,
        &me,
        vec![
            item("a"),
            item("b"),
            item("c"),
            item("d"),
            item("e"),
            item("f"),
        ],
        at(14, 10),
    );
    store.today_complete("a", &Actor::User, at(14, 11)).unwrap();
    store.today_dismiss("c", &Actor::User, at(14, 11)).unwrap();
    store
        .today_snooze("d", "2026-07-20", &Actor::User, at(14, 11))
        .unwrap();
    for key in ["a", "b", "c", "d", "e", "f"] {
        let mut value = serde_json::to_value(by_key(&store, key)).unwrap();
        let object = value.as_object_mut().unwrap();
        object.remove("statusId");
        // Two tasks saved in Needs check, one per reason.
        if key == "e" || key == "f" {
            object.insert("status".into(), "needs_check".into());
            let reason = if key == "e" {
                "done_but_source_open"
            } else {
                "missing"
            };
            object.insert("checkReason".into(), reason.into());
        }
        store
            .conn
            .execute(
                "UPDATE today_tasks SET json = ?1 WHERE key = ?2",
                params![value.to_string(), key],
            )
            .unwrap();
    }
    let list = store.today_statuses().unwrap();
    let resolved = |key: &str| statuses::resolve(&by_key(&store, key), &list).id.clone();
    assert_eq!(resolved("a"), "done");
    assert_eq!(resolved("b"), "todo");
    assert_eq!(resolved("c"), "todo");
    assert_eq!(resolved("d"), "todo");
    assert_eq!(resolved("e"), "done", "done but the source is open");
    assert_eq!(resolved("f"), "todo", "missing from the source");
    // Dismissed and snoozed stay hidden, done shows on its day.
    let view = store.today_view("2026-07-14", at(14, 12)).unwrap();
    let keys: Vec<_> = view.rows.iter().map(|r| r.task.key.as_str()).collect();
    assert_eq!(keys, ["f", "b", "e", "a"]);
}

#[test]
fn a_day_record_without_status_json_reads_through_the_current_statuses() {
    let store = store();
    let task = manual(&store, "a", at(14, 10));
    store
        .conn
        .execute("UPDATE today_days SET status_json = NULL", [])
        .unwrap();
    store
        .today_update_status(
            "todo",
            &StatusPatch {
                name: Some("Backlog".into()),
                color: None,
            },
            at(15, 10),
        )
        .unwrap();
    let past = store.today_view("2026-07-14", at(15, 10)).unwrap();
    assert_eq!(past.rows[0].task.id, task.id);
    assert_eq!(past.rows[0].status.name, "Backlog");
}

#[test]
fn a_legacy_days_table_gains_the_status_column_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("native.db");
    {
        let store = Store::open(&path).unwrap();
        store.conn.execute_batch(
            "CREATE TABLE today_days (day TEXT NOT NULL, task_id TEXT NOT NULL, grp TEXT NOT NULL,
               age_days INTEGER NOT NULL, json TEXT NOT NULL, PRIMARY KEY (day, task_id));
             CREATE TABLE today_tasks (id TEXT PRIMARY KEY, key TEXT NOT NULL UNIQUE, source_id TEXT,
               status TEXT NOT NULL, first_day TEXT NOT NULL, snoozed_until TEXT, closed_day TEXT,
               json TEXT NOT NULL);",
        ).unwrap();
        let old = serde_json::json!({
            "id": "t1", "key": "manual:t1", "title": "Old", "status": "open",
            "firstSeen": 1, "firstDay": "2026-07-10", "updatedAt": 1
        });
        store.conn.execute(
            "INSERT INTO today_tasks (id,key,source_id,status,first_day,json) VALUES ('t1','manual:t1',NULL,'open','2026-07-10',?1)",
            [old.to_string()],
        ).unwrap();
        store.conn.execute(
            "INSERT INTO today_days (day,task_id,grp,age_days,json) VALUES ('2026-07-10','t1','today',0,?1)",
            [old.to_string()],
        ).unwrap();
    }
    let store = Store::open(&path).unwrap();
    let past = store.today_view("2026-07-10", at(14, 10)).unwrap();
    assert_eq!(past.rows.len(), 1);
    assert_eq!(past.rows[0].status.id, "todo");
    let current = store.today_view("2026-07-14", at(14, 10)).unwrap();
    assert_eq!(current.rows.len(), 1);
    manual(&store, "new", at(14, 11));
    assert!(super::super::shared::has_table(&store.conn, "today_events").unwrap());
}

#[test]
fn activity_is_oldest_first_with_notes_merged_in() {
    let store = store();
    let me = caller("m1", &[]);
    sync_items(&store, &me, vec![item("a")], at(14, 10));
    store
        .today_note("a", "first", &Actor::User, at(14, 11))
        .unwrap();
    store
        .today_set_status("a", "in_progress", &Actor::User, at(14, 12))
        .unwrap();
    store
        .today_note("a", "second", &Actor::User, at(14, 13))
        .unwrap();
    let line: Vec<String> = store
        .today_activity("a")
        .unwrap()
        .iter()
        .map(|a| match a {
            Activity::Event(e) => format!("{:?}", e.kind),
            Activity::Note(n) => n.body.clone(),
        })
        .collect();
    assert_eq!(line, ["Added", "first", "StatusChanged", "second"]);
    let times: Vec<u64> = store
        .today_activity("a")
        .unwrap()
        .iter()
        .map(Activity::at)
        .collect();
    assert!(times.windows(2).all(|w| w[0] <= w[1]));
}

#[test]
fn a_task_with_no_added_event_gets_one_synthesized_first() {
    let store = store();
    let me = caller("m1", &[]);
    sync_items(&store, &me, vec![item("a")], at(14, 10));
    store.conn.execute("DELETE FROM today_events", []).unwrap();
    store
        .today_note("a", "n", &Actor::User, at(14, 11))
        .unwrap();
    let activity = store.today_activity("a").unwrap();
    assert_eq!(activity.len(), 2);
    let Activity::Event(first) = &activity[0] else {
        panic!("first is an event")
    };
    assert_eq!(first.kind, EventKind::Added);
    assert_eq!(first.at, at(14, 10));
    assert_eq!(first.source_id.as_deref(), Some("cu"));
    assert!(store.today_activity("missing").is_err());
}

#[test]
fn added_events_say_who_added_the_task() {
    let store = store();
    let me = caller("m1", &[]);
    sync_items(&store, &me, vec![item("a")], at(14, 10));
    let by_sync = event_of(&store, "a", EventKind::Added);
    assert_eq!(
        (by_sync.source_id.as_deref(), by_sync.mono_id.as_deref()),
        (Some("cu"), Some("m1"))
    );
    let mine = manual(&store, "mine", at(14, 10));
    let by_user = event_of(&store, &mine.key, EventKind::Added);
    assert_eq!((by_user.source_id, by_user.mono_id), (None, None));
    let new = NewTask {
        title: "mono".into(),
        ..NewTask::default()
    };
    let task = store.today_add(&new, &Actor::Mono(me), at(14, 10)).unwrap();
    assert_eq!(
        event_of(&store, &task.key, EventKind::Added)
            .mono_id
            .as_deref(),
        Some("m1")
    );
}

#[test]
fn mention_keys_skip_dismissed_tasks() {
    let store = store();
    let me = caller("m1", &[]);
    assert!(store.today_mention_keys().unwrap().is_empty());
    sync_items(&store, &me, vec![item("a"), item("b")], at(14, 10));
    store.today_dismiss("b", &Actor::User, at(14, 11)).unwrap();
    assert_eq!(store.today_mention_keys().unwrap(), ["a"]);
}

#[test]
fn a_status_edit_that_fails_changes_no_task() {
    let store = store();
    let task = manual(&store, "a", at(14, 10));
    let before = by_key(&store, &task.key);
    assert!(store.today_delete_status("todo", at(14, 11)).is_err());
    assert!(store
        .today_move_status("todo", Stage::Finished, 0, at(14, 11))
        .is_err());
    assert_eq!(by_key(&store, &task.key), before);
}

#[test]
fn a_rename_right_after_the_rollover_does_not_touch_yesterdays_record() {
    let store = store();
    let task = manual(&store, "a", at(14, 10));
    // 04:30 on the 15th, after the rollover, and nothing has rolled the day yet.
    let rename = StatusPatch {
        name: Some("Backlog".into()),
        color: None,
    };
    store
        .today_update_status("todo", &rename, t(15, 4, 30))
        .unwrap();
    let past = store.today_view("2026-07-14", t(15, 5, 0)).unwrap();
    assert_eq!(past.rows[0].task.id, task.id);
    assert_eq!(past.rows[0].status.name, "To do");
    let now = store.today_view("2026-07-15", t(15, 5, 0)).unwrap();
    assert_eq!(now.rows[0].status.name, "Backlog");
}
