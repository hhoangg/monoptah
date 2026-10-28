//! FormulaSidebar.test.ts: the copy, the viewer's breakdown and the weights
//! editor's rules as tests of the pure functions, and the open/close, Escape,
//! Ask and save behaviour in a real window over the fake backend.
use super::{
    GithubEvent,
    data_tests::{Call, Fake},
    formula::{
        Block, CALLOUT, FormulaSidebar, INTRO, TITLE, WeightKey, breakdown, clamp_weight,
        custom_weights_text, editor_hint, period_word, quiet_text, saved_in, score_title, sections,
        typed_weights, viewer, weight_texts,
    },
    leaderboard_tests::{Over, board, leaderboard_window, model_in, model_with, now, view_model},
    page_tests::{Ui, click, settle, shown},
};
use gpui::{Entity, Focusable as _, TestAppContext, VisualTestContext};
use monocode_core::contributors::{
    RawData,
    model::{
        config::{default_config, serialize_config},
        format::format_score,
        period::{PeriodKind, current_period, previous_period},
        scoring::SCORING,
        snapshot::SnapshotScope,
        types::{ContributorsConfig, Weights},
    },
};
use std::{cell::RefCell, rc::Rc};

fn config_with(patch: impl FnOnce(&mut ContributorsConfig)) -> ContributorsConfig {
    let mut config = default_config();
    patch(&mut config);
    config
}

fn all_text(model: &monocode_core::contributors::model::pipeline::ContributorsModel) -> String {
    let mut out = String::new();
    for section in sections(model) {
        out.push_str(&section.title);
        out.push('\n');
        for block in section.blocks {
            match block {
                Block::Text(rich) => out.push_str(&rich.text),
                Block::Formula(text) | Block::Warning(text) => out.push_str(&text),
                Block::Bullets(items) => {
                    for item in items {
                        out.push_str(&item.text);
                        out.push('\n');
                    }
                }
                Block::Globs(globs) => out.push_str(&globs.join(" ")),
            }
            out.push('\n');
        }
    }
    out
}

fn titles(model: &monocode_core::contributors::model::pipeline::ContributorsModel) -> Vec<String> {
    sections(model).into_iter().map(|s| s.title).collect()
}

fn model_without_survival() -> monocode_core::contributors::model::pipeline::ContributorsModel {
    let mut raw = super::leaderboard_tests::board_raw();
    raw.survival.clear();
    model_with(Over {
        raw,
        ..Over::default()
    })
}

// ---- breakdown ----------------------------------------------------------------------

#[test]
fn the_title_matches_the_link_that_opens_it() {
    assert_eq!(TITLE, "How the score is calculated");
}

#[test]
fn shows_the_viewers_score_and_the_five_weighted_parts() {
    let model = board();
    let me = viewer(&model).expect("Ana is the git user");
    assert_eq!(
        score_title(&model),
        format!("Your score in Oct 2026: {}", format_score(me.score))
    );
    let data = breakdown(me, &model.config.weights);
    assert_eq!(
        data.bars.iter().map(|b| b.key).collect::<Vec<_>>(),
        WeightKey::ALL
    );
    for bar in &data.bars {
        assert_eq!(bar.value, format_score(bar.key.part(&me.parts)));
        assert_eq!(bar.weight, None, "all weights are 1");
    }
    assert!(me.parts.delivery > 0.);
    assert_eq!(me.parts.review, 0.);
}

#[test]
fn scales_the_bars_to_the_largest_part() {
    let model = board();
    let me = viewer(&model).unwrap();
    let data = breakdown(me, &model.config.weights);
    let largest = WeightKey::ALL
        .into_iter()
        .map(|k| k.part(&me.parts))
        .fold(1., f64::max);
    for bar in &data.bars {
        assert!((bar.fraction - bar.key.part(&me.parts) / largest).abs() < 1e-9);
    }
    let max = data.bars.iter().map(|b| b.fraction).fold(0., f64::max);
    assert!((max - 1.).abs() < 1e-9);
}

#[test]
fn shows_quality_as_a_separate_chip_with_the_survival_share() {
    let model = board();
    let me = viewer(&model).unwrap();
    assert!((me.quality - 1.14).abs() < 0.005, "{}", me.quality);
    let data = breakdown(me, &model.config.weights);
    assert_eq!(data.quality_chip, format!("×{:.2}", me.quality));
    assert_eq!(data.quality_note, "(survival 85% at 30 days)");
}

#[test]
fn says_so_when_there_is_no_survival_sample() {
    let model = model_without_survival();
    let me = viewer(&model).unwrap();
    let data = breakdown(me, &model.config.weights);
    assert_eq!(data.quality_chip, "×1.00");
    assert_eq!(data.quality_note, "(no 30-day data yet)");
}

#[test]
fn reads_the_weighted_parts_and_marks_a_custom_weight() {
    let plain = board();
    let plain_me = viewer(&plain).unwrap().parts.consistency;
    let model = model_with(Over {
        config: config_with(|c| c.weights.consistency = 2.),
        ..Over::default()
    });
    let me = viewer(&model).unwrap();
    assert!((me.parts.consistency - plain_me * 2.).abs() < 1e-5);
    let data = breakdown(me, &model.config.weights);
    let bar = data
        .bars
        .iter()
        .find(|b| b.key == WeightKey::Consistency)
        .unwrap();
    assert_eq!(bar.weight.as_deref(), Some("×2"));
    assert_eq!(bar.value, format_score(me.parts.consistency));
    assert_eq!(
        custom_weights_text(&model.config.weights).as_deref(),
        Some("Custom weights are active: Consistency ×2.")
    );
    assert_eq!(custom_weights_text(&plain.config.weights), None);
    let mut two = model.config.weights.clone();
    two.care = 0.5;
    assert_eq!(
        custom_weights_text(&two).as_deref(),
        Some("Custom weights are active: Care ×0.5, Consistency ×2.")
    );
}

#[test]
fn explains_when_the_viewer_has_no_activity_in_the_period() {
    let model = model_with(Over {
        user: false,
        ..Over::default()
    });
    assert!(viewer(&model).is_none());
    assert_eq!(score_title(&model), "Your score in Oct 2026");
    assert_eq!(quiet_text(&model), "No activity for you in Oct 2026 yet");
}

#[test]
fn names_an_older_month_by_its_label() {
    let october = current_period(PeriodKind::Month, &now(), None);
    let september = previous_period(&october).unwrap();
    let model = model_in(Over::default(), september);
    assert!(score_title(&model).starts_with("Your score in Sep 2026:"));
}

// ---- period wording -------------------------------------------------------------------

#[test]
fn names_the_models_period_whatever_the_clock_says() {
    assert!(score_title(&board()).starts_with("Your score in Oct 2026:"));
}

#[test]
fn says_the_last_7_days_for_the_7d_period() {
    let model = model_with(Over {
        kind: PeriodKind::Days7,
        ..Over::default()
    });
    assert_eq!(period_word(&model.period), "in the last 7 days");
    assert!(score_title(&model).starts_with("Your score in the last 7 days"));
}

#[test]
fn names_all_time_by_its_label() {
    let model = model_with(Over {
        kind: PeriodKind::All,
        ..Over::default()
    });
    assert!(score_title(&model).starts_with("Your score of all time:"));
    let quiet = model_with(Over {
        kind: PeriodKind::All,
        user: false,
        ..Over::default()
    });
    assert_eq!(quiet_text(&quiet), "No activity for you in all time yet");
}

#[test]
fn names_a_quarter_and_a_year_by_their_labels() {
    let quarter = model_with(Over {
        kind: PeriodKind::Quarter,
        ..Over::default()
    });
    assert!(score_title(&quarter).starts_with("Your score in Q4 2026"));
    let year = model_with(Over {
        kind: PeriodKind::Year,
        ..Over::default()
    });
    assert!(score_title(&year).starts_with("Your score in 2026"));
    let missing = model_with(Over {
        kind: PeriodKind::Quarter,
        user: false,
        ..Over::default()
    });
    assert_eq!(quiet_text(&missing), "No activity for you in Q4 2026 yet");
}

// ---- copy ------------------------------------------------------------------------------

#[test]
fn says_that_consistency_grows_with_the_period_and_breadth_is_capped() {
    assert!(all_text(&board()).contains(
        "Consistency grows with the length of the period; Breadth is capped, so in long periods it weighs less."
    ));
}

#[test]
fn renders_the_numbers_from_the_scoring_constants() {
    let t = all_text(&board());
    let s = SCORING;
    assert_eq!(s.iter_k, 3.);
    for line in [
        "iteration = 3 × log2(1 + commits in PR)  (max 12)",
        "PR points = 10 + 4 × size + iteration     (max 72)",
        "Quality: ×0.8 to ×1.2",
        "quality = 0.8 + 0.4 × survival",
        "+20% if it adds or updates tests",
        "+10% if it updates docs",
        "+15% if it deletes more than it adds",
        "+15% if it fixes a bug",
        "8 points per PR you review for someone else, +1 per meaningful comment (max 5 per PR)",
        "5 points per active day, plus 2 per day of streak (max 14)",
        "15 points per module where you changed at least 30 lines (max 6 modules)",
    ] {
        assert!(t.contains(line), "missing {line:?} in\n{t}");
    }
}

#[test]
fn the_copy_follows_the_constants_not_literals() {
    // The sentences are built from SCORING, so each constant appears as the text of itself.
    let s = SCORING;
    let t = all_text(&board());
    for value in [
        s.iter_k,
        s.iter_cap,
        s.pr_base,
        s.size_k,
        s.pr_cap,
        s.review_pr,
    ] {
        assert!(t.contains(&monocode_core::contributors::model::js_number_text(value)));
    }
}

#[test]
fn has_the_formula_every_dimension_section_the_ai_note_and_the_callout() {
    let model = board();
    let t = all_text(&model);
    assert!(
        t.contains("Score = Delivery × Quality + Care\n      + Review + Consistency + Breadth")
    );
    let titles = titles(&model);
    for heading in [
        "The formula",
        "Delivery: merged pull requests",
        "Care: bonus on a PR",
        "Review",
        "Consistency",
        "Breadth",
        "AI agents",
        "What is ignored",
    ] {
        assert!(titles.iter().any(|t| t == heading), "{heading}");
    }
    assert!(t.contains("There is no AI bonus or penalty"));
    assert_eq!(
        CALLOUT,
        "This board is for motivation, not performance review. Roles differ: someone who reviews and mentors a lot may ship less code."
    );
    assert!(INTRO.starts_with("Scores reward impact, not busyness."));
}

#[test]
fn bold_runs_cover_the_squash_note_and_the_hundred_percent() {
    let model = board();
    let bold: Vec<String> = sections(&model)
        .into_iter()
        .flat_map(|s| s.blocks)
        .flat_map(|b| match b {
            Block::Text(r) => vec![r],
            Block::Bullets(items) => items,
            _ => vec![],
        })
        .flat_map(|r| {
            r.bold
                .iter()
                .map(|range| r.text[range.clone()].to_string())
                .collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(bold, ["Squash-merged PRs keep their effort.", "100%"]);
}

#[test]
fn lists_the_active_ignore_globs_of_the_project() {
    let model = model_with(Over {
        config: config_with(|c| c.ignore = vec!["vendor/**".into(), "*.snap".into()]),
        ..Over::default()
    });
    let globs: Vec<Vec<String>> = sections(&model)
        .into_iter()
        .flat_map(|s| s.blocks)
        .filter_map(|b| match b {
            Block::Globs(g) => Some(g),
            _ => None,
        })
        .collect();
    assert_eq!(globs, [vec!["vendor/**".to_string(), "*.snap".to_string()]]);
}

#[test]
fn says_when_nothing_is_ignored() {
    let model = model_with(Over {
        config: config_with(|c| c.ignore = vec![]),
        ..Over::default()
    });
    let t = all_text(&model);
    assert!(
        !sections(&model)
            .iter()
            .flat_map(|s| &s.blocks)
            .any(|b| matches!(b, Block::Globs(_)))
    );
    assert!(t.contains("No file patterns are ignored for this project."));
}

#[test]
fn explains_in_the_review_section_when_github_data_is_unavailable() {
    let mut raw: RawData = super::leaderboard_tests::board_raw();
    raw.gh_available = false;
    raw.gh_note = Some("GitHub CLI (`gh`) is not installed.".into());
    let model = model_with(Over {
        raw,
        ..Over::default()
    });
    assert!(all_text(&model).contains(
        "Review points are not counted because GitHub data is unavailable: GitHub CLI (`gh`) is not installed."
    ));
    let mut raw = super::leaderboard_tests::board_raw();
    raw.gh_available = false;
    let model = model_with(Over {
        raw,
        ..Over::default()
    });
    assert!(all_text(&model).contains("because GitHub data is unavailable."));
}

#[test]
fn has_no_github_notice_when_review_data_exists() {
    assert!(!all_text(&board()).contains("GitHub data is unavailable"));
}

// ---- the weights editor's rules ---------------------------------------------------------

#[test]
fn clamps_a_typed_weight_to_zero_to_five_and_two_decimals() {
    assert_eq!(clamp_weight("9", 1.), 5.);
    assert_eq!(clamp_weight("-2", 1.), 0.);
    assert_eq!(clamp_weight("0.35", 1.), 0.35);
    assert_eq!(clamp_weight("1.239", 1.), 1.24);
    assert_eq!(clamp_weight(" 2 ", 1.), 2.);
}

#[test]
fn an_empty_or_invalid_entry_keeps_the_current_weight() {
    for text in ["", "  ", "abc", "NaN", "inf", "-inf", "1,5"] {
        assert_eq!(clamp_weight(text, 1.5), 1.5, "{text:?}");
    }
}

#[test]
fn typed_weights_apply_per_field_and_keep_the_rest() {
    let current = default_config().weights;
    let texts: Vec<String> = ["9", "-2", "0.35", "", "1"]
        .into_iter()
        .map(String::from)
        .collect();
    let next = typed_weights(&texts, &current);
    assert_eq!(
        next,
        Weights {
            delivery: 5.,
            care: 0.,
            review: 0.35,
            consistency: 1.,
            breadth: 1.,
        }
    );
}

#[test]
fn only_saves_when_something_changed() {
    let current = default_config().weights;
    let same = weight_texts(&current);
    assert_eq!(typed_weights(&same, &current), current);
    let mut changed = same.clone();
    changed[4] = "2".into();
    assert_ne!(typed_weights(&changed, &current), current);
}

#[test]
fn the_fields_start_from_the_saved_weights() {
    let mut weights = default_config().weights;
    weights.care = 1.5;
    assert_eq!(weight_texts(&weights), ["1", "1.5", "1", "1", "1"]);
    assert_eq!(
        WeightKey::ALL.map(|k| format!("{} weight", k.label())),
        [
            "Delivery weight",
            "Care weight",
            "Review weight",
            "Consistency weight",
            "Breadth weight"
        ]
    );
}

#[test]
fn states_the_range_and_where_it_is_saved() {
    assert_eq!(
        editor_hint(),
        "Each dimension is multiplied by its weight, from 0 to 5. The default is 1."
    );
    assert_eq!(saved_in(), "Saved in .monoptah/contributors.json");
}

// ---- in a window --------------------------------------------------------------------------

use super::leaderboard_tests::board_raw;

fn fake_with_board() -> std::sync::Arc<Fake> {
    let fake = Fake::new();
    let october = current_period(
        PeriodKind::Month,
        &super::data_tests::start_of_tests(),
        None,
    );
    let mut raw = board_raw();
    raw.since = monocode_core::contributors::model::period::sync_window(
        &october,
        &super::data_tests::start_of_tests(),
    )
    .from
    .to_rfc3339();
    fake.set_load(monocode_core::contributors::jobs::LoadResponse {
        raw,
        covered: true,
        has_gaps: false,
        missing_commits: 0,
        synced_at: Some(super::data_tests::start_of_tests().timestamp() - 300),
        coverage_head: Some("h1".into()),
        head: "h1".into(),
        gh_state: Some("complete".into()),
    });
    fake.with(|s| {
        s.config.default = Ok(Some(serialize_config(&default_config())));
        s.saved_range = Some(monocode_core::contributors::model::prefs::serialize_pick(
            &monocode_core::contributors::model::period::PeriodPick {
                kind: PeriodKind::Month,
                id: Some("2026-10".into()),
            },
        ));
    });
    fake
}

fn open_sidebar(cx: &mut TestAppContext) -> (Ui, VisualTestContext) {
    let (ui, mut v) = leaderboard_window(cx, fake_with_board());
    click(&mut v, "contributors-lb-how");
    assert!(shown(&mut v, "contributors-formula"));
    (ui, v)
}

/// The sidebar is taller than the window: scroll its panel to the end.
fn scroll_to_end(v: &mut VisualTestContext) {
    let panel = v.debug_bounds("contributors-formula").expect("the sidebar");
    v.simulate_event(gpui::ScrollWheelEvent {
        position: panel.center(),
        delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-20000.))),
        modifiers: Default::default(),
        touch_phase: gpui::TouchPhase::Moved,
    });
    settle(v);
}

/// Clicks a control at the foot of the sidebar.
fn click_low(v: &mut VisualTestContext, selector: &'static str) {
    scroll_to_end(v);
    click(v, selector);
}

fn sidebar(ui: &Ui, v: &mut VisualTestContext) -> Entity<FormulaSidebar> {
    let view = ui.view(v);
    v.read(|cx| {
        view.read(cx)
            .sidebar
            .clone()
            .expect("the sidebar is open")
            .downcast::<FormulaSidebar>()
            .expect("a FormulaSidebar")
    })
}

fn closed(ui: &Ui) -> u32 {
    ui.closed_count()
}

fn watch_asks(ui: &Ui, v: &mut VisualTestContext) -> Rc<RefCell<Vec<SnapshotScope>>> {
    let asked = Rc::new(RefCell::new(Vec::new()));
    let log = asked.clone();
    let view = ui.view(v);
    let subscription = v.update(|_, cx| {
        cx.subscribe(&view, move |_, event: &GithubEvent, _| {
            let GithubEvent::Ask(scope) = event;
            log.borrow_mut().push(scope.clone());
        })
    });
    std::mem::forget(subscription);
    asked
}

fn focused(ui: &Ui, v: &mut VisualTestContext, which: &str) -> bool {
    let view = ui.view(v);
    let sidebar_focus = sidebar_focus(ui, v);
    let how = v.read(|cx| view.read(cx).leaderboard_ui.how_focus.clone());
    ui.window
        .update(v, |_, window, _| match which {
            "panel" => sidebar_focus.is_some_and(|f| f.is_focused(window)),
            "link" => how.is_focused(window),
            _ => false,
        })
        .unwrap()
}

fn sidebar_focus(ui: &Ui, v: &mut VisualTestContext) -> Option<gpui::FocusHandle> {
    let view = ui.view(v);
    v.read(|cx| {
        view.read(cx)
            .sidebar
            .clone()
            .and_then(|s| s.downcast::<FormulaSidebar>().ok())
            .map(|s| s.read(cx).panel_focus().clone())
    })
}

#[gpui::test]
fn opens_from_the_link_over_the_right_400px_and_takes_the_focus(cx: &mut TestAppContext) {
    let (ui, mut v) = leaderboard_window(cx, fake_with_board());
    assert!(!shown(&mut v, "contributors-formula"));
    click(&mut v, "contributors-lb-how");
    assert!(shown(&mut v, "contributors-formula"));
    let host = v.debug_bounds("contributors-sidebar-host").unwrap();
    assert_eq!(host.size.width, gpui::px(400.));
    let page = v.debug_bounds("contributors-github").unwrap();
    assert_eq!(host.right(), page.right());
    assert!(focused(&ui, &mut v, "panel"));
}

#[gpui::test]
fn escape_closes_the_sidebar_only_and_returns_the_focus_to_the_link(cx: &mut TestAppContext) {
    let (ui, mut v) = open_sidebar(cx);
    v.simulate_keystrokes("escape");
    settle(&mut v);
    assert!(!shown(&mut v, "contributors-formula"));
    assert_eq!(closed(&ui), 0);
    assert!(focused(&ui, &mut v, "link"));
    // The next Escape belongs to the page.
    v.simulate_keystrokes("escape");
    settle(&mut v);
    assert_eq!(closed(&ui), 1);
}

#[gpui::test]
fn the_x_button_closes_it_and_returns_the_focus_to_the_link(cx: &mut TestAppContext) {
    let (ui, mut v) = open_sidebar(cx);
    click(&mut v, "contributors-formula-close");
    assert!(!shown(&mut v, "contributors-formula"));
    assert!(focused(&ui, &mut v, "link"));
    assert_eq!(closed(&ui), 0);
}

#[gpui::test]
fn leaving_the_section_closes_the_sidebar(cx: &mut TestAppContext) {
    let (_, mut v) = open_sidebar(cx);
    click(&mut v, "contributors-section-stats");
    assert!(!shown(&mut v, "contributors-formula"));
    click(&mut v, "contributors-section-leaderboard");
    assert!(!shown(&mut v, "contributors-formula"));
}

#[gpui::test]
fn shows_the_breakdown_of_the_viewer_and_a_bar_per_part(cx: &mut TestAppContext) {
    let (_, mut v) = open_sidebar(cx);
    for key in WeightKey::ALL {
        let part: &'static str =
            Box::leak(format!("contributors-formula-part-{}", key.id()).into_boxed_str());
        assert!(shown(&mut v, part), "{part}");
    }
    assert!(shown(&mut v, "contributors-formula-part-quality"));
    for id in [
        "formula",
        "delivery",
        "quality",
        "care",
        "review",
        "consistency",
        "breadth",
        "ai",
        "ignored",
    ] {
        let section: &'static str =
            Box::leak(format!("contributors-formula-section-{id}").into_boxed_str());
        assert!(shown(&mut v, section), "{section}");
    }
}

#[gpui::test]
fn without_a_viewer_there_is_no_breakdown_and_ask_is_disabled(cx: &mut TestAppContext) {
    let fake = fake_with_board();
    fake.with(|s| s.user.default = Err("no git user".into()));
    let (ui, mut v) = leaderboard_window(cx, fake);
    click(&mut v, "contributors-lb-how");
    assert!(!shown(&mut v, "contributors-formula-part-delivery"));
    let asked = watch_asks(&ui, &mut v);
    click_low(&mut v, "contributors-formula-ask");
    assert!(asked.borrow().is_empty());
}

#[gpui::test]
fn asks_about_the_viewers_score(cx: &mut TestAppContext) {
    let (ui, mut v) = open_sidebar(cx);
    let asked = watch_asks(&ui, &mut v);
    let me = view_model(&ui, &mut v).me_id.clone().expect("a viewer");
    click_low(&mut v, "contributors-formula-ask");
    assert_eq!(*asked.borrow(), [SnapshotScope::Me { person_id: me }]);
}

#[gpui::test]
fn the_editor_expands_to_five_fields_with_the_current_weights(cx: &mut TestAppContext) {
    let (ui, mut v) = open_sidebar(cx);
    assert!(!shown(&mut v, "contributors-formula-editor"));
    assert!(shown(&mut v, "contributors-formula-customize"));
    click_low(&mut v, "contributors-formula-customize");
    assert!(shown(&mut v, "contributors-formula-editor"));
    let side = sidebar(&ui, &mut v);
    let texts: Vec<String> = v.read(|cx| {
        WeightKey::ALL
            .into_iter()
            .map(|k| side.read(cx).field(k).unwrap().read(cx).content.to_string())
            .collect()
    });
    assert_eq!(texts, ["1", "1", "1", "1", "1"]);
    for key in WeightKey::ALL {
        let field: &'static str =
            Box::leak(format!("contributors-formula-weight-{}", key.id()).into_boxed_str());
        assert!(shown(&mut v, field), "{field}");
    }
    click_low(&mut v, "contributors-formula-customize");
    assert!(!shown(&mut v, "contributors-formula-editor"));
}

fn type_into(ui: &Ui, v: &mut VisualTestContext, key: WeightKey, text: &str) {
    let side = sidebar(ui, v);
    let field = v.read(|cx| side.read(cx).field(key).unwrap());
    field.update(v, |input, cx| input.set_text(text.to_string(), cx));
    settle(v);
}

fn writes(fake: &Fake) -> Vec<String> {
    fake.calls()
        .into_iter()
        .filter_map(|c| match c {
            Call::Write(text) => Some(text),
            _ => None,
        })
        .collect()
}

#[gpui::test]
fn saves_the_typed_weights_clamped_through_the_save_queue_and_keeps_the_rest(
    cx: &mut TestAppContext,
) {
    let (ui, mut v) = open_sidebar(cx);
    click_low(&mut v, "contributors-formula-customize");
    type_into(&ui, &mut v, WeightKey::Delivery, "9");
    type_into(&ui, &mut v, WeightKey::Care, "-2");
    type_into(&ui, &mut v, WeightKey::Review, "0.35");
    type_into(&ui, &mut v, WeightKey::Consistency, "");
    assert!(writes(&ui.fake).is_empty());
    click_low(&mut v, "contributors-formula-save");
    let mut expected = default_config();
    expected.weights = Weights {
        delivery: 5.,
        care: 0.,
        review: 0.35,
        consistency: 1.,
        breadth: 1.,
    };
    assert_eq!(writes(&ui.fake), [serialize_config(&expected)]);
    // The fields show what was saved, and the model follows the saved weights.
    let side = sidebar(&ui, &mut v);
    let texts: Vec<String> = v.read(|cx| {
        WeightKey::ALL
            .into_iter()
            .map(|k| side.read(cx).field(k).unwrap().read(cx).content.to_string())
            .collect()
    });
    assert_eq!(texts, ["5", "0", "0.35", "1", "1"]);
    let model = view_model(&ui, &mut v);
    assert_eq!(model.config.weights, expected.weights);
}

#[gpui::test]
fn saving_needs_a_change_and_does_nothing_otherwise(cx: &mut TestAppContext) {
    let (ui, mut v) = open_sidebar(cx);
    click_low(&mut v, "contributors-formula-customize");
    click_low(&mut v, "contributors-formula-save");
    assert!(writes(&ui.fake).is_empty(), "nothing changed");
    type_into(&ui, &mut v, WeightKey::Breadth, "2");
    click_low(&mut v, "contributors-formula-save");
    assert_eq!(writes(&ui.fake).len(), 1);
    click_low(&mut v, "contributors-formula-save");
    assert_eq!(writes(&ui.fake).len(), 1, "saved weights are no change");
}

#[gpui::test]
fn reset_puts_the_defaults_in_the_fields_and_saves_only_on_request(cx: &mut TestAppContext) {
    let fake = fake_with_board();
    let mut config = default_config();
    config.weights = Weights {
        delivery: 2.,
        care: 0.,
        review: 3.,
        consistency: 1.,
        breadth: 4.,
    };
    fake.with(|s| s.config.default = Ok(Some(serialize_config(&config))));
    let (ui, mut v) = leaderboard_window(cx, fake);
    click(&mut v, "contributors-lb-how");
    click_low(&mut v, "contributors-formula-customize");
    click_low(&mut v, "contributors-formula-reset");
    assert!(writes(&ui.fake).is_empty());
    let side = sidebar(&ui, &mut v);
    let texts: Vec<String> = v.read(|cx| {
        WeightKey::ALL
            .into_iter()
            .map(|k| side.read(cx).field(k).unwrap().read(cx).content.to_string())
            .collect()
    });
    assert_eq!(texts, ["1", "1", "1", "1", "1"]);
    click_low(&mut v, "contributors-formula-save");
    assert_eq!(writes(&ui.fake), [serialize_config(&default_config())]);
}

#[gpui::test]
fn the_editor_is_disabled_while_the_config_file_is_unreadable(cx: &mut TestAppContext) {
    let fake = fake_with_board();
    fake.with(|s| s.config.default = Ok(Some("{ nope".into())));
    let (ui, mut v) = leaderboard_window(cx, fake);
    click(&mut v, "contributors-lb-how");
    click_low(&mut v, "contributors-formula-customize");
    assert!(!shown(&mut v, "contributors-formula-editor"));
    let side = sidebar(&ui, &mut v);
    assert!(!v.read(|cx| side.read(cx).is_editing()));
    assert!(writes(&ui.fake).is_empty());
}

#[gpui::test]
fn locks_an_editor_that_was_already_open_when_the_file_became_unreadable(cx: &mut TestAppContext) {
    let (ui, mut v) = open_sidebar(cx);
    click_low(&mut v, "contributors-formula-customize");
    type_into(&ui, &mut v, WeightKey::Breadth, "2");
    // The file turns invalid and the page reloads it.
    ui.fake
        .with(|s| s.config.default = Ok(Some("{ nope".into())));
    click(&mut v, "contributors-refresh");
    assert!(shown(&mut v, "contributors-locked"));
    assert!(
        shown(&mut v, "contributors-formula-editor"),
        "stays visible"
    );
    let side = sidebar(&ui, &mut v);
    let field = v.read(|cx| side.read(cx).field(WeightKey::Breadth).unwrap());
    assert!(v.read(|cx| field.read(cx).is_read_only()));
    click_low(&mut v, "contributors-formula-save");
    click_low(&mut v, "contributors-formula-reset");
    assert!(writes(&ui.fake).is_empty());
}

#[gpui::test]
fn escape_in_a_field_only_leaves_it_and_the_next_one_closes_the_sidebar(cx: &mut TestAppContext) {
    let (ui, mut v) = open_sidebar(cx);
    click_low(&mut v, "contributors-formula-customize");
    click_low(&mut v, "contributors-formula-weight-care");
    let side = sidebar(&ui, &mut v);
    let field_focus = v.read(|cx| {
        side.read(cx)
            .field(WeightKey::Care)
            .unwrap()
            .read(cx)
            .focus_handle(cx)
    });
    assert!(
        ui.window
            .update(&mut v, |_, window, _| field_focus.is_focused(window))
            .unwrap()
    );
    v.simulate_keystrokes("escape");
    settle(&mut v);
    assert!(shown(&mut v, "contributors-formula"), "the sidebar stays");
    assert_eq!(closed(&ui), 0, "the page stays");
    assert!(focused(&ui, &mut v, "panel"), "the field was left");
    v.simulate_keystrokes("escape");
    settle(&mut v);
    assert!(!shown(&mut v, "contributors-formula"));
    assert_eq!(closed(&ui), 0);
}

#[gpui::test]
fn a_saved_weight_shows_in_the_breakdown_at_once(cx: &mut TestAppContext) {
    let (ui, mut v) = open_sidebar(cx);
    click_low(&mut v, "contributors-formula-customize");
    type_into(&ui, &mut v, WeightKey::Consistency, "2");
    click_low(&mut v, "contributors-formula-save");
    let model = view_model(&ui, &mut v);
    assert_eq!(
        custom_weights_text(&model.config.weights).as_deref(),
        Some("Custom weights are active: Consistency ×2.")
    );
}

#[gpui::test]
fn enter_in_a_field_saves_like_the_button(cx: &mut TestAppContext) {
    let (ui, mut v) = open_sidebar(cx);
    click_low(&mut v, "contributors-formula-customize");
    type_into(&ui, &mut v, WeightKey::Review, "3");
    let side = sidebar(&ui, &mut v);
    let field = v.read(|cx| side.read(cx).field(WeightKey::Review).unwrap());
    field.update(&mut v, |_, cx| cx.emit(crate::input::Submitted));
    settle(&mut v);
    let mut expected = default_config();
    expected.weights.review = 3.;
    assert_eq!(writes(&ui.fake), [serialize_config(&expected)]);
}

#[gpui::test]
fn save_refuses_to_write_over_an_unreadable_file_even_when_called_directly(
    cx: &mut TestAppContext,
) {
    let (ui, mut v) = open_sidebar(cx);
    click_low(&mut v, "contributors-formula-customize");
    type_into(&ui, &mut v, WeightKey::Breadth, "2");
    ui.fake
        .with(|s| s.config.default = Ok(Some("{ nope".into())));
    click(&mut v, "contributors-refresh");
    assert!(shown(&mut v, "contributors-locked"));
    let side = sidebar(&ui, &mut v);
    side.update(&mut v, |side, cx| side.save(cx));
    settle(&mut v);
    assert!(writes(&ui.fake).is_empty());
}

#[gpui::test]
fn escape_closes_only_the_sidebar_wherever_the_focus_is(cx: &mut TestAppContext) {
    let (ui, mut v) = open_sidebar(cx);
    // A click in the table moves the focus to the view, outside the sidebar.
    click(&mut v, "contributors-lb-head-1");
    assert!(!focused(&ui, &mut v, "panel"));
    v.simulate_keystrokes("escape");
    settle(&mut v);
    assert!(!shown(&mut v, "contributors-formula"));
    assert_eq!(closed(&ui), 0, "the page stays open");
    assert!(focused(&ui, &mut v, "link"));
    v.simulate_keystrokes("escape");
    settle(&mut v);
    assert_eq!(closed(&ui), 1, "the next Escape belongs to the page");
}

#[gpui::test]
fn the_callout_has_an_accent_strip(cx: &mut TestAppContext) {
    let (_, mut v) = open_sidebar(cx);
    scroll_to_end(&mut v);
    let strip = v
        .debug_bounds("contributors-formula-callout-strip")
        .expect("the strip is drawn");
    assert_eq!(strip.size.width, gpui::px(3.));
}
