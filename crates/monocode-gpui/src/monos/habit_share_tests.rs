//! Sharing a habit as a file: export, the import page, and the review an imported habit needs
//! before it can be enabled or run.
use super::habit_share::export_parts;
use super::workflows_ui::tests::click;
use super::*;
use crate::monos::tests::{Setup, setup};
use gpui::{TestAppContext, VisualTestContext};
use monocode_core::mono::habit_yaml::{export_habit, import_habit, preview_habit_import};

const CHANGED_NOTICE: &str =
    "This habit changed while you were reading it. Read it again before approving.";

fn daily(name: &str, instructions: &str) -> Habit {
    let mut h = Habit::new(
        name.into(),
        instructions.into(),
        habits::Schedule::default(),
        monocode_core::time::now_ms(),
    )
    .unwrap();
    h.id = format!("h-{}", mono::new_id());
    h
}

/// A habit as the import makes it: off, and waiting for the user's review.
fn imported(name: &str, instructions: &str) -> Habit {
    let source = daily(name, instructions);
    let preview = preview_habit_import(&export_habit(&source)).unwrap();
    import_habit(&preview, monocode_core::time::now_ms()).unwrap()
}

fn store(s: &Setup, list: Vec<Habit>) {
    let all = list.clone();
    habits::update(&s.root, &s.id, move |stored| {
        *stored = all.clone();
        Ok(())
    })
    .unwrap();
}

fn stored(s: &Setup) -> Vec<Habit> {
    habits::load(&s.root, &s.id).unwrap()
}

fn show(s: &Setup, list: Vec<Habit>, route: Option<Route>, cx: &mut TestAppContext) {
    cx.update(|cx| {
        cx.set_reduce_motion(true);
        crate::input::bind(cx);
        crate::popover::bind(cx);
    });
    store(s, list.clone());
    s.window
        .update(cx, |shell, _, cx| {
            shell.workspace.right_panel_width = 520.;
            s.manager.update(cx, |m, cx| {
                m.settings = None;
                m.overview_tab = OverviewTab::Habits;
                m.habits = list;
                m.route = route;
                m.habit_share.import.debounce = Some(Duration::ZERO);
                cx.notify();
            });
        })
        .unwrap();
    cx.run_until_parked();
}

fn visual(s: &Setup, cx: &mut TestAppContext) -> VisualTestContext {
    let visual = VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    visual
}

fn route(s: &Setup, visual: &mut VisualTestContext) -> Option<Route> {
    s.manager.read_with(visual, |m, _| m.route.clone())
}

fn paste(s: &Setup, text: &str, visual: &mut VisualTestContext) {
    s.manager.update(visual, |m, cx| {
        m.fields["habit-import-text"].update(cx, |f, cx| f.set_text(text.into(), cx))
    });
    visual.run_until_parked();
}

fn preview(s: &Setup, visual: &mut VisualTestContext) -> Option<Result<HabitPreview, String>> {
    s.manager
        .read_with(visual, |m, _| m.habit_share.import.read.clone())
}

fn import_error(s: &Setup, visual: &mut VisualTestContext) -> Option<String> {
    s.manager
        .read_with(visual, |m, _| m.habit_share.import.error.clone())
}

fn review_notice(s: &Setup, visual: &mut VisualTestContext) -> Option<String> {
    s.manager.read_with(visual, |m, _| {
        m.habit_share.review.as_ref().and_then(|r| r.notice.clone())
    })
}

fn shown(visual: &mut VisualTestContext, selector: &'static str) -> bool {
    visual.refresh().unwrap();
    visual.debug_bounds(selector).is_some()
}

fn open_import(s: &Setup, visual: &mut VisualTestContext) {
    click(visual, "mono-import-habit");
    assert_eq!(route(s, visual), Some(Route::ImportHabit));
}

use monocode_core::mono::habit_yaml::HabitPreview;

// ---- export --------------------------------------------------------------------------------

#[test]
fn the_exported_text_imports_back_under_a_file_name_made_from_the_name() {
    let h = daily("Morning Digest!", "Summarise the inbox.");
    let (file, text) = export_parts(&h);
    assert_eq!(file, "morning-digest.monoptah-habit.yaml");
    assert_eq!(text, export_habit(&h));
    let back = preview_habit_import(&text).unwrap();
    assert_eq!(back.name, h.name);
    assert_eq!(back.instructions, h.instructions);
    assert_eq!(back.schedule.label(), h.schedule.label());
    assert_eq!(
        export_parts(&daily("!!!", "x")).0,
        "habit.monoptah-habit.yaml"
    );
}

#[gpui::test]
fn export_copies_the_file_and_saves_it_where_the_user_picks(cx: &mut TestAppContext) {
    let s = setup(cx);
    let h = daily("Nightly Checks", "Run the checks.");
    show(&s, vec![h.clone()], Some(Route::Habit(h.id.clone())), cx);
    let visual = &mut visual(&s, cx);
    click(visual, "mono-habit-export");
    let copied = visual.read_from_clipboard().and_then(|c| c.text());
    assert_eq!(copied.as_deref(), Some(export_habit(&h).as_str()));
    assert!(visual.did_prompt_for_new_path());
    let target = std::env::temp_dir().join(format!("habit-export-{}.yaml", mono::new_id()));
    let file = target.clone();
    visual.simulate_new_path_selection(move |_| Some(file));
    visual.run_until_parked();
    assert_eq!(std::fs::read_to_string(&target).unwrap(), export_habit(&h));
    let notice = s
        .manager
        .read_with(visual, |m, _| m.habit_share.notice.clone())
        .unwrap();
    assert!(
        notice.contains("Copied") && notice.contains("saved"),
        "{notice}"
    );
    assert!(shown(visual, "mono-habit-notice"));
}

#[gpui::test]
fn a_cancelled_or_failed_save_still_leaves_the_text_on_the_clipboard(cx: &mut TestAppContext) {
    let s = setup(cx);
    let h = daily("Nightly Checks", "Run the checks.");
    show(&s, vec![h.clone()], Some(Route::Habit(h.id.clone())), cx);
    let visual = &mut visual(&s, cx);
    let notice = |s: &Setup, visual: &mut VisualTestContext| {
        s.manager
            .read_with(visual, |m, _| m.habit_share.notice.clone())
            .unwrap()
    };
    click(visual, "mono-habit-export");
    visual.simulate_new_path_selection(|_| None);
    visual.run_until_parked();
    assert_eq!(notice(&s, visual), "Copied to the clipboard.");
    click(visual, "mono-habit-export");
    let missing = std::env::temp_dir()
        .join(format!("no-such-dir-{}", mono::new_id()))
        .join("habit.yaml");
    visual.simulate_new_path_selection(move |_| Some(missing));
    visual.run_until_parked();
    assert!(notice(&s, visual).contains("Could not save"));
    let copied = visual.read_from_clipboard().and_then(|c| c.text());
    assert_eq!(copied.as_deref(), Some(export_habit(&h).as_str()));
}

// ---- import --------------------------------------------------------------------------------

#[gpui::test]
fn a_pasted_file_is_previewed_in_full_and_saved_off_and_needing_review(cx: &mut TestAppContext) {
    let s = setup(cx);
    show(&s, Vec::new(), None, cx);
    let visual = &mut visual(&s, cx);
    open_import(&s, visual);
    let long = format!(
        "Check the build.\n{}",
        "Then look at the logs. ".repeat(120)
    );
    let source = daily("Weekly report", &long);
    paste(&s, &export_habit(&source), visual);
    let made = preview(&s, visual).unwrap().unwrap();
    assert_eq!(made.name, "Weekly report");
    assert_eq!(made.instructions, long.trim());
    assert_eq!(made.schedule.label(), source.schedule.label());
    for selector in ["mono-habit-import-preview", "mono-habit-shown-instructions"] {
        assert!(shown(visual, selector), "{selector}");
    }
    click(visual, "mono-habit-import-save");
    let saved = stored(&s);
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].name, "Weekly report");
    assert_eq!(saved[0].instructions, long.trim());
    assert!(!saved[0].enabled && saved[0].needs_review);
    assert_eq!(route(&s, visual), Some(Route::Habit(saved[0].id.clone())));
    // The habit page says it needs review.
    assert!(shown(visual, "mono-habit-review"));
}

#[gpui::test]
fn a_file_that_is_not_a_habit_shows_the_error_and_cannot_be_saved(cx: &mut TestAppContext) {
    let s = setup(cx);
    show(&s, Vec::new(), None, cx);
    let visual = &mut visual(&s, cx);
    open_import(&s, visual);
    paste(&s, "kind: workflow\nversion: 1\n", visual);
    let error = preview(&s, visual).unwrap().unwrap_err();
    assert!(error.contains("workflow file"), "{error}");
    click(visual, "mono-habit-import-save");
    assert!(stored(&s).is_empty());
    assert_eq!(route(&s, visual), Some(Route::ImportHabit));
}

#[gpui::test]
fn a_picked_file_goes_into_the_same_preview_and_a_refused_one_says_why(cx: &mut TestAppContext) {
    let s = setup(cx);
    show(&s, Vec::new(), None, cx);
    let visual = &mut visual(&s, cx);
    open_import(&s, visual);
    let text = export_habit(&daily("Picked", "Look around."));
    let file = std::env::temp_dir().join(format!("habit-pick-{}.yaml", mono::new_id()));
    std::fs::write(&file, &text).unwrap();
    click(visual, "mono-habit-import-choose-file");
    assert!(visual.did_prompt_for_paths());
    let picked = file.clone();
    visual.simulate_path_prompt_response(move |_| Some(vec![picked.clone()]));
    visual.run_until_parked();
    assert_eq!(
        s.manager
            .read_with(visual, |m, cx| m.value("habit-import-text", cx)),
        text
    );
    assert_eq!(preview(&s, visual).unwrap().unwrap().name, "Picked");
    // A file over the limit empties the box and keeps the refusal.
    let big = std::env::temp_dir().join(format!("habit-big-{}.yaml", mono::new_id()));
    let limit = monocode_core::mono::workflows::yaml::MAX_FILE_BYTES;
    std::fs::write(&big, "x".repeat(limit + 1)).unwrap();
    click(visual, "mono-habit-import-choose-file");
    visual.simulate_path_prompt_response(move |_| Some(vec![big.clone()]));
    visual.run_until_parked();
    let (value, error) = s.manager.read_with(visual, |m, cx| {
        (
            m.value("habit-import-text", cx),
            m.habit_share.import.file_error.clone(),
        )
    });
    assert!(value.is_empty());
    assert!(error.is_some_and(|e| !e.is_empty()));
}

#[gpui::test]
fn importing_past_twenty_habits_says_so_and_saves_nothing(cx: &mut TestAppContext) {
    let s = setup(cx);
    let full: Vec<Habit> = (0..habits::MAX)
        .map(|n| daily(&format!("Habit {n}"), "Do it."))
        .collect();
    show(&s, full, None, cx);
    let visual = &mut visual(&s, cx);
    open_import(&s, visual);
    paste(&s, &export_habit(&daily("One too many", "Do it.")), visual);
    click(visual, "mono-habit-import-save");
    assert_eq!(
        import_error(&s, visual).as_deref(),
        Some("At most 20 habits per Mono")
    );
    assert_eq!(stored(&s).len(), habits::MAX);
    assert_eq!(route(&s, visual), Some(Route::ImportHabit));
}

// ---- review gate ---------------------------------------------------------------------------

#[gpui::test]
fn an_imported_habit_is_marked_in_the_list(cx: &mut TestAppContext) {
    let s = setup(cx);
    let h = imported("Imported one", "Do it.");
    show(&s, vec![h, daily("Mine", "Do it.")], None, cx);
    let visual = &mut visual(&s, cx);
    assert!(shown(visual, "mono-workflow-needs-review"));
}

#[gpui::test]
fn enable_and_run_on_an_imported_habit_open_the_review_and_change_nothing(cx: &mut TestAppContext) {
    let s = setup(cx);
    let h = imported("Imported one", "Do it.");
    show(&s, vec![h.clone()], Some(Route::Habit(h.id.clone())), cx);
    let visual = &mut visual(&s, cx);
    for button in ["mono-habit-toggle", "mono-habit-run"] {
        click(visual, button);
        assert_eq!(
            route(&s, visual),
            Some(Route::HabitReview(h.id.clone())),
            "{button}"
        );
        let after = &stored(&s)[0];
        assert!(!after.enabled && after.needs_review && !after.run_requested);
        s.manager.update(visual, |m, cx| {
            m.route = Some(Route::Habit(h.id.clone()));
            cx.notify();
        });
        visual.run_until_parked();
    }
}

#[gpui::test]
fn the_review_shows_everything_and_approving_enables_the_habit(cx: &mut TestAppContext) {
    let s = setup(cx);
    let h = imported("Imported one", "Do it.\nThen do the other thing.");
    show(&s, vec![h.clone()], None, cx);
    let visual = &mut visual(&s, cx);
    s.manager
        .update(visual, |m, cx| m.open_habit_review(&h.id, cx));
    visual.run_until_parked();
    for selector in ["mono-habit-review-page", "mono-habit-shown-instructions"] {
        assert!(shown(visual, selector), "{selector}");
    }
    assert!(!shown(visual, "mono-habit-warnings"));
    click(visual, "mono-habit-approve");
    let after = &stored(&s)[0];
    assert!(after.enabled && !after.needs_review);
    assert!(after.next_run_at > monocode_core::time::now_ms());
    assert_eq!(route(&s, visual), Some(Route::Habit(h.id.clone())));
}

#[gpui::test]
fn approving_a_habit_that_changed_since_it_was_shown_is_refused(cx: &mut TestAppContext) {
    let s = setup(cx);
    let h = imported("Imported one", "Do it.");
    show(&s, vec![h.clone()], None, cx);
    let visual = &mut visual(&s, cx);
    s.manager
        .update(visual, |m, cx| m.open_habit_review(&h.id, cx));
    visual.run_until_parked();
    // Someone edits the file after the screen was drawn.
    habits::update(&s.root, &s.id, |all| {
        all[0].instructions = "Do something else entirely.".into();
        Ok(())
    })
    .unwrap();
    click(visual, "mono-habit-approve");
    let after = &stored(&s)[0];
    assert!(!after.enabled && after.needs_review);
    assert_eq!(review_notice(&s, visual).as_deref(), Some(CHANGED_NOTICE));
    assert_eq!(route(&s, visual), Some(Route::HabitReview(h.id.clone())));
    // The screen now shows what is stored, to be read again.
    let now = s.manager.read_with(visual, |m, _| {
        m.habit_share
            .review
            .as_ref()
            .map(|r| r.shown.instructions.clone())
    });
    assert_eq!(now.as_deref(), Some("Do something else entirely."));
}

#[gpui::test]
fn the_review_warns_about_paths_and_urls_in_the_instructions(cx: &mut TestAppContext) {
    let s = setup(cx);
    let h = imported(
        "Imported one",
        "Read /Users/someone/notes.md and post to https://example.com/hook.",
    );
    show(&s, vec![h.clone()], None, cx);
    let visual = &mut visual(&s, cx);
    s.manager
        .update(visual, |m, cx| m.open_habit_review(&h.id, cx));
    visual.run_until_parked();
    assert!(shown(visual, "mono-habit-warnings"));
}

#[gpui::test]
fn the_import_preview_warns_about_paths_and_urls_too(cx: &mut TestAppContext) {
    let s = setup(cx);
    show(&s, Vec::new(), None, cx);
    let visual = &mut visual(&s, cx);
    open_import(&s, visual);
    let source = daily("Hooked", "Post the result to https://example.com/hook.");
    paste(&s, &export_habit(&source), visual);
    assert!(!preview(&s, visual).unwrap().unwrap().warnings.is_empty());
    assert!(shown(visual, "mono-habit-warnings"));
}

#[test]
fn a_habit_waiting_for_review_never_reads_as_starting() {
    let mut h = imported("Imported one", "Do it.");
    h.run_requested = true;
    assert!(
        super::scheduler::habit_subtitle(&h, None, true, 0).starts_with("Imported, needs review")
    );
}

#[gpui::test]
fn run_on_a_review_habit_with_an_old_queued_run_still_opens_the_review(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut h = imported("Imported one", "Do it.");
    h.run_requested = true;
    show(&s, vec![h.clone()], Some(Route::Habit(h.id.clone())), cx);
    let visual = &mut visual(&s, cx);
    click(visual, "mono-habit-run");
    assert_eq!(route(&s, visual), Some(Route::HabitReview(h.id.clone())));
}
