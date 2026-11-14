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
    let context = context(&mono, &files, std::path::Path::new("/bin/app"), false, None);
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
    for needle in [
        "accounts.list {provider?}",
        "target?,account?}",
        "sessions.start account:",
        "NEW codespace or sandbox only",
        "the account belongs to the instance",
    ] {
        assert!(bridge::HELP.contains(needle), "help lacks {needle}");
    }
    for needle in [
        "model and account",
        "@provider/label",
        "local sessions only",
    ] {
        assert!(context.contains(needle), "context lacks {needle}");
    }
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
    let context = context(&mono, &files, std::path::Path::new("/bin/app"), false, None);
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

#[test]
fn context_shows_how_to_delegate_without_running_help() {
    let mono = Mono {
        projects: vec!["/repo".into()],
        ..Default::default()
    };
    let files = files::AgentFiles {
        id: mono.id.clone(),
        dir: String::new(),
        soul: Some(DEFAULT_SOUL.into()),
        soul_hash: String::new(),
        memory: String::new(),
        memory_hash: String::new(),
        memory_path: String::new(),
        topics: Vec::new(),
    };
    let executable = std::path::Path::new("/Applications/Monoptah.app/monocode");
    let start = r#"/Applications/Monoptah.app/monocode app sessions.start --json '{"project":"<assigned path>","title":"<short title>","prompt":"<complete instructions>","notifyOnComplete":true}'"#;
    let rules = instructions(&mono, executable);
    assert!(rules.contains(start));
    assert!(rules.contains("## Be quick"));
    assert!(rules.contains("- /repo"));
    // Without system instructions, the rules ride along in the message.
    let inline = context(&mono, &files, executable, false, None);
    assert!(inline.contains(&rules));
    assert!(inline.contains("<soul>"));
    // With them, the message carries only what changes between turns.
    let system = context(&mono, &files, executable, true, None);
    assert!(!system.contains(start));
    assert!(system.contains("<soul>") && system.contains("<memory>"));
    assert!(system_instructions_supported(crate::provider::Kind::Claude));
    assert!(system_instructions_supported(crate::provider::Kind::Codex));
    assert!(!system_instructions_supported(crate::provider::Kind::Devin));
}

#[test]
fn a_resumed_codex_thread_gets_rules_it_has_not_seen_in_the_message() {
    use crate::provider::Kind;
    // A fresh thread takes them as developer instructions.
    assert!(rules_in_system(Kind::Codex, false, None, "new"));
    // Codex keeps a thread's first instructions, so a moved app CLI or
    // rules from before they lived there must come with the message.
    assert!(!rules_in_system(Kind::Codex, true, None, "new"));
    assert!(!rules_in_system(Kind::Codex, true, Some("old"), "new"));
    assert!(rules_in_system(Kind::Codex, true, Some("new"), "new"));
    // Claude's system prompt is sent with every process.
    assert!(rules_in_system(Kind::Claude, true, None, "new"));
    assert!(!rules_in_system(Kind::Devin, false, None, "new"));
}

#[test]
fn subsessions_fill_only_a_start_that_names_no_model() {
    let set = Subsessions {
        harness: Some("claude".into()),
        model: Some("opus".into()),
        effort: Some("high".into()),
        account: None,
    };
    let mut bare = serde_json::json!({"prompt": "x"});
    set.fill(&mut bare, "codex");
    assert_eq!(
        bare,
        serde_json::json!({"prompt": "x", "harness": "claude", "model": "opus", "effort": "high"})
    );
    for chosen in ["harness", "model", "modelSettings", "effort"] {
        let mut named = serde_json::json!({"prompt": "x", chosen: "codex"});
        let before = named.clone();
        set.fill(&mut named, "codex");
        assert_eq!(named, before, "{chosen} is the agent's own choice");
    }
    let effort_only = Subsessions {
        effort: Some("low".into()),
        ..Default::default()
    };
    let mut input = serde_json::json!({"prompt": "x"});
    effort_only.fill(&mut input, "codex");
    assert_eq!(input, serde_json::json!({"prompt": "x", "effort": "low"}));
}

#[test]
fn old_subsessions_data_reads_back_unchanged_and_writes_no_account() {
    // Saved before the account existed: it deserializes, and serializes the same.
    let old = serde_json::json!({"harness": "claude", "model": "opus", "effort": null});
    let set: Subsessions = serde_json::from_value(old.clone()).unwrap();
    assert_eq!(set.account, None);
    assert_eq!(serde_json::to_value(&set).unwrap()["harness"], "claude");
    assert!(serde_json::to_value(&set).unwrap().get("account").is_none());
    let empty: Subsessions = serde_json::from_value(serde_json::json!({})).unwrap();
    assert_eq!(empty, Subsessions::default());
    // A whole Mono saved without `subsessions` keeps its defaults.
    let mono: Mono = serde_json::from_value(serde_json::json!({"id": "m1"})).unwrap();
    assert_eq!(mono.subsessions, None);
    // The account round trips by id.
    let with = Subsessions {
        account: Some("acct-1".into()),
        ..Default::default()
    };
    let back: Subsessions = serde_json::from_str(&serde_json::to_string(&with).unwrap()).unwrap();
    assert_eq!(back.account.as_deref(), Some("acct-1"));
    // An account alone (the Mono's own provider) is a real setting, not "inherit everything".
    assert_eq!(with.clone().normalized(), Some(with));
    let blank = Subsessions {
        account: Some("  ".into()),
        ..Default::default()
    };
    assert_eq!(blank.normalized(), None);
}

#[test]
fn subsessions_default_account_fills_only_for_the_provider_it_was_set_for() {
    let set = Subsessions {
        harness: Some("claude".into()),
        model: Some("opus".into()),
        effort: None,
        account: Some("work".into()),
    };
    // Mono on codex, sessions on claude: a bare start takes the other provider and its account.
    let mut bare = serde_json::json!({"prompt": "x"});
    set.fill(&mut bare, "codex");
    assert_eq!(
        bare,
        serde_json::json!({"prompt": "x", "harness": "claude", "model": "opus", "account": "work"})
    );
    // An explicit account wins.
    let mut named = serde_json::json!({"prompt": "x", "account": "home"});
    set.fill(&mut named, "codex");
    assert_eq!(named["account"], "home");
    assert_eq!(named["harness"], "claude");
    // An explicit harness of another provider never gets this provider's account.
    let mut other = serde_json::json!({"prompt": "x", "harness": "codex"});
    set.fill(&mut other, "codex");
    assert_eq!(
        other,
        serde_json::json!({"prompt": "x", "harness": "codex"})
    );
    // The same provider with only a model named keeps its account.
    let mut same = serde_json::json!({"prompt": "x", "harness": "claude", "model": "sonnet"});
    set.fill(&mut same, "codex");
    assert_eq!(same["account"], "work");
    assert_eq!(same["model"], "sonnet");
    // A cloud target keeps its own account rules.
    let mut cloud = serde_json::json!({"prompt": "x", "target": {"kind": "codespace"}});
    set.fill(&mut cloud, "codex");
    assert!(cloud.get("account").is_none());
    // Provider inherited: an account alone applies to the Mono's own provider only.
    let own = Subsessions {
        account: Some("work".into()),
        ..Default::default()
    };
    let mut bare = serde_json::json!({"prompt": "x"});
    own.fill(&mut bare, "claude");
    assert_eq!(bare, serde_json::json!({"prompt": "x", "account": "work"}));
    let mut elsewhere = serde_json::json!({"prompt": "x", "harness": "codex"});
    own.fill(&mut elsewhere, "claude");
    assert!(elsewhere.get("account").is_none());
    let mut model_only = serde_json::json!({"prompt": "x", "model": "opus"});
    own.fill(&mut model_only, "claude");
    assert_eq!(model_only["account"], "work");
}

#[test]
fn subsessions_account_resets_when_the_provider_changes_and_effort_follows_the_model() {
    let mut set = Subsessions {
        harness: Some("claude".into()),
        model: Some("opus".into()),
        effort: Some("high".into()),
        account: Some("work".into()),
    };
    // Same provider and model: nothing resets.
    set.pick_model("codex", "claude", Some("opus".into()));
    assert_eq!(set.account.as_deref(), Some("work"));
    assert_eq!(set.effort.as_deref(), Some("high"));
    // Same provider, another model: the effort may not fit, the account does.
    set.pick_model("codex", "claude", Some("sonnet".into()));
    assert_eq!(set.account.as_deref(), Some("work"));
    assert_eq!(set.effort, None);
    // Another provider: the account goes back to that provider's default.
    set.effort = Some("low".into());
    set.pick_model("codex", "codex", Some("gpt".into()));
    assert_eq!(set.account, None);
    assert_eq!(set.effort, None);
    assert_eq!(set.harness.as_deref(), Some("codex"));
    // An account picked for codex survives a model change within codex.
    set.account = Some("pro".into());
    set.pick_model("claude", "codex", None);
    assert_eq!(set.account.as_deref(), Some("pro"));
    // Back to inheriting: a codex account does not apply to a claude Mono.
    set.inherit_model("claude");
    assert_eq!(set, Subsessions::default());
    // Inheriting keeps an account of the Mono's own provider.
    let mut own = Subsessions {
        harness: Some("claude".into()),
        model: Some("opus".into()),
        effort: None,
        account: Some("work".into()),
    };
    own.inherit_model("claude");
    assert_eq!(own.account.as_deref(), Some("work"));
    assert_eq!(own.harness, None);
    // The Mono moving to another provider drops an account that followed the old one,
    // but not one of an explicitly chosen provider.
    own.follow_own_provider("claude", "codex");
    assert_eq!(own.account, None);
    let mut chosen = Subsessions {
        harness: Some("claude".into()),
        account: Some("work".into()),
        ..Default::default()
    };
    chosen.follow_own_provider("claude", "codex");
    assert_eq!(chosen.account.as_deref(), Some("work"));
}

#[test]
fn subsessions_normalize_away_when_everything_is_inherited() {
    assert_eq!(Subsessions::default().normalized(), None);
    let blank = Subsessions {
        harness: Some(" ".into()),
        ..Default::default()
    };
    assert_eq!(blank.normalized(), None);
    // A model without its provider can't be named.
    let orphan = Subsessions {
        model: Some("opus".into()),
        ..Default::default()
    };
    assert_eq!(orphan.normalized(), None);
}

#[test]
fn rules_live_in_instructions_and_the_context_only_repeats_them_when_not_in_system() {
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
    let path = std::path::Path::new("/bin/app");
    let rules = instructions(&mono, path);
    for needle in [
        "completionDelivery",
        "parentSessionId",
        "folderName",
        "Workflows are graphs of steps",
        "workflows.guide",
        "Monoptah",
    ] {
        assert!(rules.contains(needle), "instructions lack {needle}");
    }
    // Rules held by the system instructions are not repeated in the message.
    let slim = context(&mono, &files, path, true, None);
    for absent in [
        "Workflows are graphs of steps",
        "workflows.guide",
        "completionDelivery",
    ] {
        assert!(!slim.contains(absent), "context repeats {absent}");
    }
    for needle in ["<soul>", "<memory>", "Monoptah adds this context"] {
        assert!(slim.contains(needle), "context lacks {needle}");
    }
    // Without system instructions the same rules travel in the message.
    let full = context(&mono, &files, path, false, None);
    assert!(full.contains("Workflows are graphs of steps"));
    assert!(full.contains("completionDelivery"));
}
