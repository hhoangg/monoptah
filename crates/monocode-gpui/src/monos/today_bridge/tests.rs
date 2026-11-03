use super::*;
use crate::monos::tests::{Setup, WORKFLOW_ACTIONS, call_from, setup};
use gpui::TestAppContext;
use monocode_core::mono::habits::Schedule;
use serde_json::{Value, json};

/// A habit run of the Mono, driven through its chat view (nothing is sent).
fn fake_run(s: &Setup, habit: &str, cx: &mut TestAppContext) -> i64 {
    let run = locked(&s.store)
        .unwrap()
        .create("codex", &s.project)
        .unwrap()
        .id;
    cx.update(|cx| {
        s.manager.update(cx, |m, _| {
            let view = m.chats.get(&s.id).unwrap().clone();
            m.running.insert(
                run,
                RunningHabit {
                    mono: s.id.clone(),
                    habit: habit.into(),
                    view,
                    started: now_ms(),
                    launched: now_ms(),
                    failure: None,
                    waiting_since: None,
                    cards: Vec::new(),
                    sessions: Vec::new(),
                    created: Vec::new(),
                    stop: None,
                },
            );
        })
    });
    run
}
fn call(
    s: &Setup,
    session: i64,
    action: &str,
    input: Value,
    cx: &mut TestAppContext,
) -> Result<Value, String> {
    // A repeated request ID replays the first answer, so every call gets its own.
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let request = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    call_from(s, session, &format!("r{request}"), action, input, cx)
        .try_recv()
        .unwrap()
}
fn sync(source: &str, key: &str) -> Value {
    json!({"source": source, "status": "ok", "items": [{"key": key, "title": "T", "status": "open"}]})
}
fn add_habit(s: &Setup, next_run_at: u64) -> String {
    let schedule = Schedule::from_cli(&json!({"kind": "daily", "time": "09:00"})).unwrap();
    let mut habit = Habit::new("Fetch".into(), "Fetch".into(), schedule, now_ms()).unwrap();
    habit.next_run_at = next_run_at;
    let id = habit.id.clone();
    habits::update(&s.root, &s.id, |all| {
        all.push(habit.clone());
        Ok(())
    })
    .unwrap();
    id
}
fn habit_of(s: &Setup, id: &str) -> Habit {
    habits::load(&s.root, &s.id)
        .unwrap()
        .into_iter()
        .find(|h| h.id == id)
        .unwrap()
}
/// Stops a started run before it reaches a provider.
fn no_provider(s: &Setup, cx: &mut TestAppContext) {
    s.manager.update(cx, |m, cx| {
        m.chats[&s.id].update(cx, |chat, _| {
            chat.settings.usage_limit = Some(Default::default());
        })
    });
}

#[gpui::test]
fn the_caller_comes_from_the_session_and_a_heartbeat_is_not_a_habit(cx: &mut TestAppContext) {
    let s = setup(cx);
    let caller = |session: i64, cx: &mut TestAppContext| {
        cx.update(|cx| {
            let m = s.manager.read(cx);
            let mono = m.roster.get(&s.id).unwrap().clone();
            let (reply, _rx) = std::sync::mpsc::channel();
            let call = Call {
                session,
                request_id: "x".into(),
                action: "today.list".into(),
                input: json!({}),
                reply,
            };
            m.today_caller(&call, &mono)
        })
    };
    let chat = caller(s.session, cx);
    assert_eq!(
        (chat.id.as_str(), chat.habit.clone(), chat.in_run),
        (s.id.as_str(), None, false)
    );
    assert_eq!(chat.projects, vec![s.project.clone()]);
    let run = caller(fake_run(&s, "h1", cx), cx);
    assert_eq!((run.habit.as_deref(), run.in_run), (Some("h1"), true));
    let beat = caller(fake_run(&s, heartbeat::RUN_ID, cx), cx);
    assert_eq!((beat.habit, beat.in_run), (None, true));
}

#[gpui::test]
fn a_run_may_list_and_sync_but_not_hide_tasks_and_workflows_stay_refused(cx: &mut TestAppContext) {
    let s = setup(cx);
    let run = fake_run(&s, "h1", cx);
    let listed = call(&s, run, "today.list", json!({}), cx).unwrap();
    assert_eq!(listed["count"], 0);
    let synced = call(&s, run, "today.sync", sync("gh", "gh:1"), cx).unwrap();
    assert_eq!(synced["added"], 1);
    let refused = call(&s, run, "today.dismiss", json!({"id": "gh:1"}), cx).unwrap_err();
    assert!(
        refused.starts_with("A run cannot use today.dismiss"),
        "{refused}"
    );
    assert!(call(&s, run, "today.complete", json!({"id": "gh:1"}), cx).is_err());
    assert!(
        call(
            &s,
            run,
            "today.snooze",
            json!({"id": "gh:1", "until": "2099-01-01"}),
            cx
        )
        .is_err()
    );
    // It may still note on it and read it back.
    assert!(
        call(
            &s,
            run,
            "today.note",
            json!({"id": "gh:1", "body": "seen"}),
            cx
        )
        .is_ok()
    );
    let listed = call(&s, run, "today.list", json!({}), cx).unwrap();
    assert_eq!(listed["tasks"][0]["notes"][0]["body"], "seen");
    assert_eq!(
        call(&s, run, "workflows.add", json!({}), cx),
        Err("A habit run can only read and run workflows".into())
    );
    assert_eq!(
        call(&s, run, "habits.run", json!({"id": "h1"}), cx),
        Err("A habit run cannot schedule more runs".into())
    );
    // The Mono's own conversation is the user's side: it may dismiss.
    assert!(call(&s, s.session, "today.dismiss", json!({"id": "gh:1"}), cx).is_ok());
}

#[gpui::test]
fn a_run_sync_records_its_habit_and_a_chat_sync_leaves_it(cx: &mut TestAppContext) {
    let s = setup(cx);
    let run = fake_run(&s, "h1", cx);
    call(&s, run, "today.sync", sync("gh", "gh:1"), cx).unwrap();
    let sources = || locked(&s.store).unwrap().today_sources().unwrap();
    assert_eq!(sources()[0].mono_id, s.id);
    assert_eq!(sources()[0].habit_id.as_deref(), Some("h1"));
    call(&s, s.session, "today.sync", sync("gh", "gh:2"), cx).unwrap();
    assert_eq!(sources()[0].habit_id.as_deref(), Some("h1"));
}

#[gpui::test]
fn a_session_cannot_claim_another_monos_identity_or_source(cx: &mut TestAppContext) {
    let s = setup(cx);
    call(&s, s.session, "today.sync", sync("gh", "gh:1"), cx).unwrap();
    let handle: gpui::AnyWindowHandle = s.window.into();
    handle
        .update(cx, |_, w, cx| {
            s.manager
                .update(cx, |m, cx| m.create(vec![s.project.clone()], w, cx))
        })
        .unwrap();
    cx.run_until_parked();
    let other = cx.update(|cx| s.manager.read(cx).current().unwrap().session_id.unwrap());
    assert_ne!(other, s.session);
    let refused = call(&s, other, "today.sync", sync("gh", "gh:2"), cx).unwrap_err();
    assert!(refused.contains("another Mono"), "{refused}");
    // No field names a Mono: the identity is the session's alone.
    for key in ["monoId", "mono", "caller", "habit", "inRun"] {
        let mut input = sync("mine", "m:1");
        input[key] = json!(s.id);
        let error = call(&s, other, "today.sync", input, cx).unwrap_err();
        assert_eq!(error, format!("today.sync does not take {key}"));
    }
    let sources = locked(&s.store).unwrap().today_sources().unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].mono_id, s.id);
    assert_eq!(sources[0].item_count, 1);
}

#[gpui::test]
fn a_today_action_bumps_the_revision_only_when_it_succeeds(cx: &mut TestAppContext) {
    let s = setup(cx);
    let revision = |cx: &mut TestAppContext| cx.update(|cx| s.manager.read(cx).today_revision());
    let before = revision(cx);
    assert!(call(&s, s.session, "today.sync", json!({"source": "gh"}), cx).is_err());
    assert_eq!(revision(cx), before);
    call(&s, s.session, "today.sync", sync("gh", "gh:1"), cx).unwrap();
    assert!(revision(cx) > before);
}

#[gpui::test]
fn the_roster_lists_each_mono_with_its_habits_from_a_cache(cx: &mut TestAppContext) {
    let s = setup(cx);
    let habit = add_habit(&s, now_ms() + 3_600_000);
    // The first read never waits for the file: it starts the load.
    let first = cx.update(|cx| s.manager.read(cx).today_roster(cx).unwrap());
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].id, s.id);
    assert_eq!(first[0].projects, vec![s.project.clone()]);
    assert!(first[0].habits.is_empty());
    assert!(!first[0].habits_known, "not read yet is not 'no habits'");
    let before = cx.update(|cx| s.manager.read(cx).today_revision());
    cx.run_until_parked();
    assert!(cx.update(|cx| s.manager.read(cx).today_revision()) > before);
    let second = cx.update(|cx| s.manager.read(cx).today_roster(cx).unwrap());
    assert!(second[0].habits_known);
    assert_eq!(second[0].habits.len(), 1);
    assert_eq!(second[0].habits[0].id, habit);
    assert_eq!(second[0].habits[0].schedule, "Daily at 09:00");
    assert!(second[0].habits[0].enabled && !second[0].habits[0].busy);
}

#[gpui::test]
fn an_unreadable_habits_file_leaves_the_habits_unknown(cx: &mut TestAppContext) {
    let s = setup(cx);
    monocode_core::mono::files::save(&s.root, &s.id, None, "habits.json", "{ not json", None)
        .unwrap();
    cx.update(|cx| s.manager.update(cx, |m, cx| m.reload_today_habits(cx)));
    cx.run_until_parked();
    let roster = cx.update(|cx| s.manager.read(cx).today_roster(cx).unwrap());
    assert!(roster[0].habits.is_empty());
    assert!(!roster[0].habits_known);
}

#[gpui::test]
fn a_roster_that_failed_to_load_is_not_an_empty_roster(cx: &mut TestAppContext) {
    let s = setup(cx);
    cx.update(|cx| s.manager.update(cx, |m, _| m.roster_failed = true));
    assert!(
        cx.update(|cx| s.manager.read(cx).today_roster(cx))
            .is_none()
    );
}

#[gpui::test]
fn the_first_habit_load_wakes_the_page(cx: &mut TestAppContext) {
    let s = setup(cx);
    let woke = std::rc::Rc::new(std::cell::Cell::new(0));
    let seen = woke.clone();
    let _watch = cx.update(|cx| cx.observe(&s.manager, move |_, _| seen.set(seen.get() + 1)));
    cx.update(|cx| s.manager.update(cx, |m, cx| m.reload_today_habits(cx)));
    cx.run_until_parked();
    assert!(woke.get() > 0, "an empty habit list still counts as read");
    let roster = cx.update(|cx| s.manager.read(cx).today_roster(cx).unwrap());
    assert!(roster[0].habits_known);
}

#[gpui::test]
fn a_requested_run_goes_through_the_habit_path_and_a_busy_habit_is_skipped(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    no_provider(&s, cx);
    let habit = add_habit(&s, now_ms() + 3_600_000);
    let ask = |cx: &mut TestAppContext| {
        let target = vec![(s.id.clone(), habit.clone())];
        s.manager
            .update(cx, |m, cx| m.request_habit_runs(target, cx));
        cx.run_until_parked();
    };
    ask(cx);
    cx.update(|cx| {
        let m = s.manager.read(cx);
        assert!(m.error.is_none(), "{:?}", m.error);
        assert!(
            m.running
                .values()
                .any(|r| r.habit == habit && r.mono == s.id)
        );
    });
    let after = habit_of(&s, &habit);
    assert!(after.running_since.is_some() && !after.run_requested);
    // Running now: asking again changes nothing and starts nothing.
    let runs = cx.update(|cx| s.manager.read(cx).running.len());
    let revision = cx.update(|cx| s.manager.read(cx).today_revision());
    ask(cx);
    assert_eq!(cx.update(|cx| s.manager.read(cx).running.len()), runs);
    assert_eq!(
        cx.update(|cx| s.manager.read(cx).today_revision()),
        revision
    );
}

// --- catch-up: a habit only runs within GRACE_MS of its time ----------------------------------

#[gpui::test]
fn a_habit_missed_by_more_than_the_grace_skips_to_its_next_slot(cx: &mut TestAppContext) {
    let s = setup(cx);
    no_provider(&s, cx);
    let late = now_ms() - habits::GRACE_MS - 60_000;
    let habit = add_habit(&s, late);
    let tick = |cx: &mut TestAppContext| {
        let handle: gpui::AnyWindowHandle = s.window.into();
        handle
            .update(cx, |_, w, cx| s.manager.update(cx, |m, cx| m.tick(w, cx)))
            .unwrap();
        cx.run_until_parked();
    };
    tick(cx);
    let after = habit_of(&s, &habit);
    assert!(
        after.running_since.is_none() && after.last_run_at.is_none(),
        "must not run"
    );
    assert!(after.next_run_at > now_ms(), "moved to the next slot");
    cx.update(|cx| assert!(s.manager.read(cx).running.is_empty()));
}

#[gpui::test]
fn a_habit_missed_within_the_grace_still_runs(cx: &mut TestAppContext) {
    let s = setup(cx);
    no_provider(&s, cx);
    let habit = add_habit(&s, now_ms() - habits::GRACE_MS / 2);
    let handle: gpui::AnyWindowHandle = s.window.into();
    handle
        .update(cx, |_, w, cx| s.manager.update(cx, |m, cx| m.tick(w, cx)))
        .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        assert!(
            s.manager
                .read(cx)
                .running
                .values()
                .any(|r| r.habit == habit)
        );
    });
}

#[gpui::test]
fn a_workflow_question_session_is_a_run_without_a_habit(cx: &mut TestAppContext) {
    let s = setup(cx);
    let host = cx.update(|cx| s.manager.update(cx, |m, cx| m.workflow_host(cx)));
    let mono = s.id.clone();
    cx.spawn(async move |_| {
        let _ = host.ask_mono(&mono, "Ship it?".into()).await;
    })
    .detach();
    cx.run_until_parked();
    let ask = cx.update(|cx| {
        let ids: Vec<_> = s
            .manager
            .read(cx)
            .workflow_live
            .asks
            .keys()
            .copied()
            .collect();
        assert_eq!(ids.len(), 1);
        ids[0]
    });
    let refused = call(&s, ask, "today.dismiss", json!({"id": "gh:1"}), cx).unwrap_err();
    assert!(
        refused.starts_with("A run cannot use today.dismiss"),
        "{refused}"
    );
    assert!(call(&s, ask, "today.list", json!({}), cx).is_ok());
}

#[gpui::test]
fn a_read_leaves_the_revision_alone_and_a_habit_change_wakes_the_roster(cx: &mut TestAppContext) {
    let s = setup(cx);
    call(&s, s.session, "today.sync", sync("gh", "gh:1"), cx).unwrap();
    let revision = |cx: &mut TestAppContext| cx.update(|cx| s.manager.read(cx).today_revision());
    let before = revision(cx);
    call(&s, s.session, "today.list", json!({}), cx).unwrap();
    call(&s, s.session, "today.sources", json!({}), cx).unwrap();
    call(&s, s.session, "today.statuses", json!({}), cx).unwrap();
    assert_eq!(revision(cx), before);
    // Reads leave no receipt behind; the sync above left one.
    assert_eq!(cx.update(|cx| s.manager.read(cx).app_results.len()), 1);
    let schedule = json!({"kind": "daily", "time": "09:00"});
    let add = json!({"name": "Fetch", "instructions": "x", "schedule": schedule});
    call(&s, s.session, "habits.add", add, cx).unwrap();
    cx.run_until_parked();
    let roster = cx.update(|cx| s.manager.read(cx).today_roster(cx).unwrap());
    assert_eq!(roster[0].habits.len(), 1);
}

#[gpui::test]
fn a_heartbeat_cannot_use_any_workflows_action(cx: &mut TestAppContext) {
    let s = setup(cx);
    let beat = fake_run(&s, heartbeat::RUN_ID, cx);
    for action in WORKFLOW_ACTIONS {
        assert_eq!(
            call(&s, beat, action, json!({}), cx),
            Err("A heartbeat cannot use workflows".into()),
            "{action}"
        );
    }
}
