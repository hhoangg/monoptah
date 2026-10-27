use super::*;
use habits::{Habit, Schedule};
use std::sync::atomic::{AtomicBool, Ordering};

#[test]
fn identity_projects_order_and_hidden_sessions_survive_restart() {
    let temp = tempfile::tempdir().unwrap();
    let db = Store::open(&temp.path().join("native.sqlite")).unwrap();
    let mut roster = Roster::default();
    let a = roster
        .create(vec!["/one/app".into(), "/two/app".into()])
        .id
        .clone();
    let other = roster.create(vec!["/else".into()]).id.clone();
    let b = roster.create(vec!["/one/app".into()]).id.clone();
    assert!(roster.get(&a).unwrap().project(Some("app")).is_err());
    assert!(roster.get(&a).unwrap().project(Some("/outside")).is_err());
    assert!(roster.get(&a).unwrap().project(None).is_err());
    assert_eq!(roster.get(&b).unwrap().project(None).unwrap(), "/one/app");
    roster.reorder(&[b.clone(), a.clone()]);
    roster.hidden_sessions.push(17);
    roster.save(&db).unwrap();
    let saved = Roster::load(&db).unwrap();
    assert_eq!(
        saved.monos.iter().map(|m| &m.id).collect::<Vec<_>>(),
        vec![&b, &other, &a]
    );
    assert_eq!(saved.hidden_sessions, vec![17]);
    assert_ne!(saved.monos[0].mascot, saved.monos[1].mascot);
}
#[test]
fn memory_supersedes_uniquely_redacts_and_archives_before_prompt_budget() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let id = "memory-test";
    let secret = "sk-1234567890abcdefghijklmnop";
    let entry = memory::entry(&format!("Use Rust. token={secret}"), "2026-10-19", None).unwrap();
    assert!(!entry.contains(secret));
    let first = memory::add("# User notes\n", &entry);
    assert_eq!(memory::add(&first, &entry), first);
    let new = memory::entry("Use GPUI", "2026-10-19", None).unwrap();
    let replaced = memory::replace(&first, "Use Rust", Some(&new), "2026-10-19").unwrap();
    assert!(replaced.contains("~~"));
    assert!(memory::replace(&replaced, "not present", None, "2026-10-19").is_err());
    let mut many = "# User notes\nDo not drop this user-written line.\n".to_string();
    for n in 0..205 {
        many.push_str(&memory::entry(&format!("fact number {n}"), "2026-10-18", None).unwrap());
        many.push('\n');
    }
    memory::edit(root, id, None, |_| Ok(many.clone())).unwrap();
    let file = memory::read(root, id, None).unwrap().text.unwrap();
    assert!(file.lines().count() <= 200);
    assert!(file.contains("Do not drop"));
    assert!(file.contains("fact number 204"));
    assert!(files::read(root, id, None, "memory/archive.md")
        .unwrap()
        .text
        .unwrap()
        .contains("fact number 0"));
    assert_eq!(
        memory::search(root, id, "fact number 204", None)
            .unwrap()
            .len(),
        1
    );
    assert!(memory::topic("../secret").is_err());
}
#[test]
fn habit_updates_retry_conflicts_without_losing_other_edits() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let id = "habit-test";
    let now = 1_792_411_200_000;
    let a = Habit::new(
        "A".into(),
        "Report changes".into(),
        Schedule::default(),
        now,
    )
    .unwrap();
    let b = Habit::new("B".into(), "Check builds".into(), Schedule::default(), now).unwrap();
    let inject = AtomicBool::new(true);
    habits::update(root, id, |all| {
        if inject.swap(false, Ordering::SeqCst) {
            habits::update(root, id, |other| {
                other.push(b.clone());
                Ok(())
            })?;
        }
        all.push(a.clone());
        Ok(())
    })
    .unwrap();
    let all = habits::load(root, id).unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].id, b.id);
    assert_eq!(all[1].id, a.id);
    let mut h = a;
    h.enabled = false;
    h.next_run_at = now;
    assert!(!h.due(now));
    h.run_requested = true;
    assert!(h.due(now));
    h.running_since = Some(now);
    assert!(!h.due(now));
    assert!(habits::report("`NOTHING_TO_REPORT`").is_none());
    assert!(habits::report("The build failed.\nNOTHING_TO_REPORT")
        .unwrap()
        .contains("build failed"));
}
#[test]
fn schedules_are_strictly_future_and_reject_invalid_fields() {
    let now = 1_792_411_200_000;
    for kind in ["hourly", "daily", "weekdays", "weekly"] {
        let s = Schedule {
            schedule_kind: kind.into(),
            ..Default::default()
        };
        let next = s.next(now).unwrap();
        assert!(next > now);
        assert!(next - now <= 8 * 86400000);
        assert!(s.next(next).unwrap() > next);
    }
    assert!(Schedule {
        time: "25:00".into(),
        ..Default::default()
    }
    .validate()
    .is_err());
    assert!(Schedule {
        minute: 60,
        ..Default::default()
    }
    .validate()
    .is_err());
    let s: Schedule = serde_json::from_str(r#"{"kind":"hourly","minute":15}"#).unwrap();
    assert_eq!(s.minute, 15);
}
#[test]
fn delivery_receipt_is_saved_with_the_user_message() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("native.sqlite");
    let mut db = Store::open(&path).unwrap();
    let session = db.create("codex", "/project").unwrap();
    let row = Row {
        role: "user".into(),
        text: "hello".into(),
        first: true,
        sent_at: None,
    };
    db.write_rows_with_message(session.id, 0, std::slice::from_ref(&row), Some("message-1"))
        .unwrap();
    drop(db);
    let db = Store::open(&path).unwrap();
    assert_eq!(db.load(session.id).unwrap().rows, vec![row]);
    assert_eq!(
        db.setting(&format!("mono-delivered:{}:message-1", session.id))
            .unwrap()
            .as_deref(),
        Some("1")
    );
}
#[test]
fn a_launch_saved_before_completion_delivery_loads_as_group() {
    let old = r#"{"sessionId":7,"cwd":"/p","title":"t","harness":"codex","model":null,"originTurn":4294967297,"notifyOnComplete":true}"#;
    let launch: Launch = serde_json::from_str(old).unwrap();
    assert!(!launch.each);
    let saved = serde_json::to_value(Launch {
        each: true,
        ..launch
    })
    .unwrap();
    assert_eq!(saved["each"], true);
}
#[test]
fn a_launch_is_shown_only_when_a_workflow_run_made_it() {
    let old = r#"{"sessionId":7,"cwd":"/p","title":"t","harness":"codex","model":null,"originTurn":4294967297,"notifyOnComplete":true}"#;
    let launch: Launch = serde_json::from_str(old).unwrap();
    assert!(!launch.visible);
    // The flag is only written when set, so saved Monos keep their shape.
    assert!(serde_json::to_value(&launch)
        .unwrap()
        .get("visible")
        .is_none());
    let shown = serde_json::to_value(Launch {
        visible: true,
        ..launch
    })
    .unwrap();
    assert_eq!(shown["visible"], true);
    assert!(serde_json::from_value::<Launch>(shown).unwrap().visible);
}
#[test]
fn the_prompt_and_help_describe_each_delivery_and_habit_reporting() {
    let mut roster = Roster::default();
    let mono = roster.create(vec!["/p".into()]).clone();
    let files = files::AgentFiles {
        id: mono.id.clone(),
        dir: String::new(),
        soul: None,
        soul_hash: String::new(),
        memory: String::new(),
        memory_hash: String::new(),
        memory_path: String::new(),
        topics: Vec::new(),
    };
    let context = context(&mono, &files, std::path::Path::new("/bin/app"));
    for needle in [
        "completionDelivery",
        "\"each\"",
        "habit run",
        "not to the run",
        "parentSessionId",
        "folderName",
    ] {
        assert!(context.contains(needle), "context lacks {needle}");
    }
    assert!(!context.contains("app-<"), "Tauri id scheme leaked");
    for needle in [
        "\"each\" delivers",
        "habit run",
        "report to the Mono's chat",
        "draft:true with notifyOnComplete:true is refused",
        "sessions.start notifyOnComplete defaults to true",
        "sessions.send notifyOnComplete defaults to false",
    ] {
        assert!(bridge::HELP.contains(needle), "help lacks {needle}");
    }
    assert!(!bridge::HELP.contains("Ignored for drafts"));
}
#[test]
fn the_prompt_and_help_describe_workflows() {
    let mut roster = Roster::default();
    let mono = roster.create(vec!["/p".into()]).clone();
    let files = files::AgentFiles {
        id: mono.id.clone(),
        dir: String::new(),
        soul: None,
        soul_hash: String::new(),
        memory: String::new(),
        memory_hash: String::new(),
        memory_path: String::new(),
        topics: Vec::new(),
    };
    let context = context(&mono, &files, std::path::Path::new("/bin/app"));
    for needle in [
        "workflows.guide",
        "workflows.validate",
        "workflows.resumeRun",
        "{{nodeId.field}}",
        "{{run.round}}",
        "Projects are always inputs, never paths.",
        "show them the steps first",
        "only then save it with workflows.add or workflows.update",
        "A workflow you add is always saved paused, however clean it is: the user enables it in the Workflows panel",
        "workflows.run needs the workflow to be resumed (not paused)",
        "its model is written `<harness>:<id from models.list>`, for Claude without the `claude-` prefix (for example claude:sonnet-5, as in workflows.guide)",
        "workflows.validate flags one it does not",
    ] {
        assert!(context.contains(needle), "context lacks {needle}");
    }
    // The paragraph follows the habits and keeps the native CLI's words (no React `app ` prefix).
    assert!(
        context.find("Habits are scheduled work").unwrap()
            < context.find("Workflows are graphs of steps").unwrap()
    );
    assert!(!context.contains("`app workflows"));
    assert!(!context.contains("workflows_rules"));
    for action in [
        "list",
        "get",
        "add",
        "validate",
        "guide",
        "update",
        "setInputs",
        "run",
        "stop",
        "pause",
        "resume",
        "resumeRun",
        "remove",
        "export",
        "import",
        "runs",
    ] {
        let name = format!("workflows.{action}");
        assert!(
            bridge::HELP.contains(&format!("{name} {{")),
            "help lacks {name}"
        );
        assert!(
            api::validate(&name, &serde_json::json!({})).is_ok(),
            "{name} is not an app action"
        );
    }
    for needle in [
        "always saved paused, however clean it is",
        "needing review",
        "returns at once",
        "<harness>:<id from models.list>\", for Claude without the \"claude-\" prefix, for example \"claude:sonnet-5\" as in workflows.guide",
        "workflows.validate flags an effort the model does not offer",
        "limit 1 to 20, default 5",
    ] {
        assert!(bridge::HELP.contains(needle), "help lacks {needle}");
    }
}
