//! Tests for `habit_yaml.rs` and the review gate on `Habit`.
use super::habit_yaml::*;
use super::habits::{Habit, Schedule};
use serde_json::json;

const NOW: u64 = 1_790_000_000_000;

fn schedule(value: serde_json::Value) -> Schedule {
    Schedule::from_cli(&value).unwrap()
}

fn habit(name: &str, instructions: &str, schedule: Schedule) -> Habit {
    Habit::new(name.into(), instructions.into(), schedule, NOW).unwrap()
}

fn file(body: &str) -> String {
    format!("kind: habit\nversion: 1\n{body}")
}

const VALID: &str =
    "name: Digest\nschedule:\n  kind: daily\n  time: \"09:00\"\ninstructions: Check the news\n";

fn refusal(text: &str) -> String {
    preview_habit_import(text).unwrap_err()
}

#[test]
fn every_schedule_kind_round_trips() {
    let all = [
        json!({"kind": "hourly", "minute": 15}),
        json!({"kind": "daily", "time": "07:30"}),
        json!({"kind": "weekdays", "time": "09:00"}),
        json!({"kind": "weekly", "dayOfWeek": 5, "time": "18:45"}),
        json!({"kind": "interval", "hours": 4, "time": "09:00"}),
    ];
    for spec in all {
        let original = habit("Digest", "Check the news", schedule(spec.clone()));
        let text = export_habit(&original);
        let preview = preview_habit_import(&text).unwrap_or_else(|e| panic!("{spec}: {e}\n{text}"));
        let imported = import_habit(&preview, NOW).unwrap();
        assert_eq!(imported.name, original.name, "{spec}");
        assert_eq!(imported.instructions, original.instructions, "{spec}");
        let mut expected = original.schedule.clone();
        expected.interval_from = imported.schedule.interval_from.clone();
        assert_eq!(imported.schedule, expected, "{spec}");
        assert_eq!(
            imported.schedule.interval_from.is_some(),
            spec["kind"] == "interval"
        );
    }
}

#[test]
fn multi_line_and_special_instructions_round_trip() {
    let instructions = "Line one: \"quoted\"\n  - indented # not a comment\n\nLast line with 'single' and {braces} and é ✓\n- dash";
    let original = habit(
        "Name: with #special \"chars\"",
        instructions,
        schedule(json!({"kind": "daily"})),
    );
    let preview = preview_habit_import(&export_habit(&original)).unwrap();
    assert_eq!(preview.name, original.name);
    assert_eq!(preview.instructions, original.instructions);
}

#[test]
fn the_file_has_nothing_machine_specific() {
    let mut h = habit(
        "Digest",
        "Check",
        schedule(json!({"kind": "interval", "hours": 6})),
    );
    h.from_card = Some("card-1".into());
    h.last_outcome = Some("posted".into());
    h.run_requested = true;
    let text = export_habit(&h);
    for key in [
        "id:",
        "runs",
        "enabled",
        "createdAt",
        "nextRunAt",
        "lastRunAt",
        "fromCard",
        "needsReview",
        "intervalFrom",
        "lastOutcome",
        "runRequested",
        &h.id,
    ] {
        assert!(!text.contains(key), "{key} leaked into\n{text}");
    }
    assert!(text.starts_with("kind: habit\nversion: 1\n"), "{text}");
}

#[test]
fn json_files_are_read_too() {
    let text = r#"{"kind":"habit","version":1,"name":"A","instructions":"B","schedule":{"kind":"hourly","minute":5}}"#;
    let preview = preview_habit_import(text).unwrap();
    assert_eq!(preview.schedule.minute, 5);
}

#[test]
fn files_that_are_not_habits_are_refused() {
    assert_eq!(refusal("- a\n- b\n"), "Not a Monoptah habit file");
    assert_eq!(refusal("name: x\n"), "Not a Monoptah habit file");
    assert!(refusal("").contains("empty"));
    assert!(refusal("a: [unclosed").starts_with("Could not read the habit file"));
    assert_eq!(
        refusal("kind: thing\nversion: 1\n"),
        "Unsupported kind: thing (expected habit)"
    );
    assert!(refusal("monoptahWorkflow: 1\nname: x\n").contains("Workflows"));
    assert!(refusal("kind: workflow\nversion: 1\n").contains("Workflows"));
    let big = format!("{}{}", file(VALID), "# x\n".repeat(140_000));
    assert!(refusal(&big).contains("too large"));
}

#[test]
fn versions_are_checked() {
    assert_eq!(
        refusal(&format!("kind: habit\nversion: 2\n{VALID}")),
        "Unsupported habit file version: 2 (this app reads version 1)"
    );
    assert_eq!(
        refusal(&format!("kind: habit\n{VALID}")),
        "Unsupported habit file version: missing (this app reads version 1)"
    );
}

#[test]
fn name_instructions_and_schedule_are_validated() {
    assert_eq!(
        refusal(&file("schedule:\n  kind: daily\ninstructions: x\n")),
        "The habit file needs a name"
    );
    assert_eq!(
        refusal(&file(
            "name: \"  \"\nschedule:\n  kind: daily\ninstructions: x\n"
        )),
        "The habit file needs a name"
    );
    let long = "n".repeat(121);
    assert!(refusal(&file(&format!(
        "name: {long}\nschedule:\n  kind: daily\ninstructions: x\n"
    )))
    .contains("too long"));
    assert_eq!(
        refusal(&file("name: A\nschedule:\n  kind: daily\n")),
        "The habit file needs instructions"
    );
    assert_eq!(
        refusal(&file(
            "name: A\nschedule:\n  kind: daily\ninstructions: \"  \"\n"
        )),
        "The habit file needs instructions"
    );
    let huge = "x".repeat(32001);
    assert!(refusal(&file(&format!(
        "name: A\nschedule:\n  kind: daily\ninstructions: {huge}\n"
    )))
    .contains("too long"));
    assert_eq!(
        refusal(&file("name: A\ninstructions: x\n")),
        "The habit file needs a schedule"
    );
    assert!(refusal(&file(
        "name: A\ninstructions: x\nschedule:\n  kind: yearly\n"
    ))
    .contains("schedule.kind"));
    assert!(refusal(&file(
        "name: A\ninstructions: x\nschedule:\n  kind: interval\n"
    ))
    .contains("schedule.hours"));
    assert!(refusal(&file(
        "name: A\ninstructions: x\nschedule:\n  kind: daily\n  time: \"25:00\"\n"
    ))
    .contains("schedule.time"));
    assert!(
        refusal(&file("name: A\ninstructions: x\nschedule: daily\n"))
            .contains("schedule must be an object")
    );
}

#[test]
fn unknown_top_level_keys_are_refused() {
    let text = file(&format!("{VALID}enabled: true\nid: abc\n"));
    assert_eq!(
        refusal(&text),
        "Unknown fields in the habit file: enabled, id"
    );
}

#[test]
fn paths_and_urls_are_warned_about_not_refused() {
    let instructions = "Read /Users/alice/notes/todo.md and ~/work/log.txt, then /Users/alice/notes/todo.md again. Post to https://example.com/hook or ssh://git@host/repo and visit /api/users.";
    let text = file(&format!(
        "name: A\nschedule:\n  kind: daily\ninstructions: \"{instructions}\"\n"
    ));
    let preview = preview_habit_import(&text).unwrap();
    assert_eq!(preview.warnings.len(), 2, "{:?}", preview.warnings);
    let paths = &preview.warnings[0];
    assert!(paths.contains("/Users/alice/notes/todo.md"), "{paths}");
    assert_eq!(paths.matches("/Users/alice/notes/todo.md").count(), 1);
    assert!(paths.contains("~/work/log.txt"));
    assert!(!paths.contains("/api/users"));
    let urls = &preview.warnings[1];
    assert!(urls.contains("https://example.com/hook") && urls.contains("ssh://git@host/repo"));
    assert!(preview_habit_import(&file(VALID))
        .unwrap()
        .warnings
        .is_empty());
}

#[test]
fn listed_warnings_are_capped() {
    let many: String = (0..9).map(|i| format!("https://e.com/{i} ")).collect();
    let text = file(&format!(
        "name: A\nschedule:\n  kind: daily\ninstructions: \"{many}\"\n"
    ));
    let warning = &preview_habit_import(&text).unwrap().warnings[0];
    assert!(warning.contains("and 4 more"), "{warning}");
    assert!(!warning.contains("https://e.com/8"));
}

#[test]
fn an_imported_habit_is_off_and_waits_for_review() {
    let preview = preview_habit_import(&file(VALID)).unwrap();
    let h = import_habit(&preview, NOW).unwrap();
    assert!(!h.enabled && h.needs_review);
    assert!(!h.due(u64::MAX / 2));
}

#[test]
fn a_habit_waiting_for_review_is_never_due() {
    let mut h = habit("A", "B", schedule(json!({"kind": "hourly"})));
    assert!(h.due(h.next_run_at));
    h.needs_review = true;
    assert!(!h.due(h.next_run_at));
    h.run_requested = true;
    assert!(!h.due(h.next_run_at));
}

#[test]
fn enabling_and_running_are_refused_until_approved() {
    let preview = preview_habit_import(&file(VALID)).unwrap();
    let mut h = import_habit(&preview, NOW).unwrap();
    assert!(h
        .set_enabled(true, NOW)
        .unwrap_err()
        .contains("needs review"));
    assert!(h.request_run().unwrap_err().contains("needs review"));
    assert!(!h.enabled && !h.run_requested);
    h.set_enabled(false, NOW).unwrap();

    let fingerprint = h.review_fingerprint();
    h.approve(&fingerprint, NOW).unwrap();
    assert!(h.enabled && !h.needs_review);
    assert!(h.next_run_at > NOW);
    h.request_run().unwrap();
    assert!(h.run_requested);
}

#[test]
fn approval_needs_the_fingerprint_that_was_shown() {
    let preview = preview_habit_import(&file(VALID)).unwrap();
    let mut h = import_habit(&preview, NOW).unwrap();
    let shown = h.review_fingerprint();
    assert_eq!(
        h.approve("not-it", NOW).unwrap_err(),
        "This habit changed since it was shown; review it again"
    );
    assert!(h.needs_review && !h.enabled);
    h.instructions = "Something else".into();
    assert!(h.approve(&shown, NOW).is_err());
    h.instructions = "Check the news".into();
    h.schedule.time = "10:00".into();
    assert_ne!(h.review_fingerprint(), shown);
    assert!(h.approve(&shown, NOW).is_err());
    assert!(h.needs_review);
}

#[test]
fn approving_needs_a_pending_review_and_drops_an_earlier_queued_run() {
    let preview = preview_habit_import(&file(VALID)).unwrap();
    let mut h = import_habit(&preview, NOW).unwrap();
    h.run_requested = true;
    let fingerprint = h.review_fingerprint();
    h.approve(&fingerprint, NOW).unwrap();
    assert!(h.enabled && !h.run_requested);
    // Nothing left to approve: refused, and nothing changes.
    h.enabled = false;
    let fingerprint = h.review_fingerprint();
    assert_eq!(
        h.approve(&fingerprint, NOW).unwrap_err(),
        "This habit does not need review"
    );
    assert!(!h.enabled);
}

#[test]
fn the_review_mark_is_saved_and_absent_when_false() {
    let normal = habit("A", "B", schedule(json!({"kind": "daily"})));
    assert!(!serde_json::to_string(&normal)
        .unwrap()
        .contains("needsReview"));
    let mut marked = normal.clone();
    marked.needs_review = true;
    let text = serde_json::to_string(&marked).unwrap();
    assert!(text.contains("\"needsReview\":true"));
    let back: Habit = serde_json::from_str(&text).unwrap();
    assert!(back.needs_review);
    let old: Habit = serde_json::from_str(&serde_json::to_string(&normal).unwrap()).unwrap();
    assert!(!old.needs_review);
}

#[test]
fn the_review_mark_survives_habits_json() {
    let dir = std::env::temp_dir().join(format!("habit-yaml-{}", super::new_id()));
    std::fs::create_dir_all(&dir).unwrap();
    let preview = preview_habit_import(&file(VALID)).unwrap();
    let h = import_habit(&preview, NOW).unwrap();
    super::habits::update(&dir, "mono", |all| {
        all.push(h.clone());
        Ok(())
    })
    .unwrap();
    let loaded = super::habits::load(&dir, "mono").unwrap();
    assert!(loaded[0].needs_review && !loaded[0].enabled);
    let _ = std::fs::remove_dir_all(&dir);
}
