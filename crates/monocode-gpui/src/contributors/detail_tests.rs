//! The member detail page: what its cards, trend, areas and PR list say (pure
//! view functions) and how it opens, closes and keeps the list's state (in a
//! real window over the fake backend).
use super::{
    GithubEvent, chart,
    detail::{
        COMMITS_ID, ListRow, REVIEWS_ID, area_rows, detail_texts, has_lead_column, pr_rows,
        survival_text, trend_data,
    },
    page_tests::{click, settle, shown},
    stats::{Tone, versus},
    stats_tests::{
        activity_views, day_commits, fixture, many_authors_raw, model_of, now, right_click,
        stats_window, stats_window_of,
    },
};
use chrono::{DateTime, Local, TimeZone};
use gpui::{TestAppContext, VisualTestContext};
use monocode_core::contributors::{
    GitIdentity, RawReview,
    model::{
        member::{MemberDetail, member_detail},
        period::PeriodKind,
        pipeline::ContributorsModel,
        snapshot::SnapshotScope,
    },
};
use std::{cell::RefCell, rc::Rc};

fn local(d: u32, h: u32) -> DateTime<Local> {
    Local
        .with_ymd_and_hms(2026, 10, d, h, 0, 0)
        .earliest()
        .unwrap()
}

fn who(name: &str, email: &str) -> GitIdentity {
    GitIdentity {
        name: name.into(),
        email: email.into(),
        login: None,
    }
}

/// The fixture plus Bob's reviews of Ana's PR: two reviews of one PR.
fn reviewed() -> monocode_core::contributors::RawData {
    let mut raw = fixture();
    raw.reviews = [local(13, 9), local(16, 9)]
        .iter()
        .map(|at| RawReview {
            pr_id: "1".into(),
            reviewer: who("Bob", "bob@example.com"),
            state: "APPROVED".into(),
            submitted_at: at.to_rfc3339(),
            comments: 0,
        })
        .collect();
    raw
}

fn detail_for(model: &ContributorsModel, id: &str) -> MemberDetail {
    member_detail(model, id, &now()).expect("a detail")
}

// ---- the pure view functions ---------------------------------------------------

#[test]
fn survival_reads_as_a_percentage_or_not_available() {
    assert_eq!(survival_text(Some(0.874)), "87%");
    assert_eq!(survival_text(Some(1.0)), "100%");
    assert_eq!(survival_text(None), "n/a");
}

#[test]
fn the_cards_say_the_counts_and_how_they_moved() {
    let model = model_of(reviewed(), PeriodKind::Month);
    let ana = detail_for(&model, "ana");
    let texts = detail_texts(&ana, &versus(&model));
    // Ana: 3 commits in October against 1 in September; one merged PR.
    assert_eq!(texts.commits.value, "3");
    let change = texts.commits.change.expect("a change");
    assert_eq!(change.tone, Tone::Up);
    assert!(change.text.starts_with("▲ +"), "{}", change.text);
    assert_eq!(texts.prs.value, "1");
    assert_eq!(texts.prs.change.unwrap().text, "▲ +1");
    assert_eq!(texts.survival.value, "n/a");
    assert!(texts.survival.change.is_none());
    assert_eq!(texts.lines.added, "+12k");
}

#[test]
fn the_review_card_counts_prs_as_the_scores_and_the_calendar_do() {
    let model = model_of(reviewed(), PeriodKind::Month);
    let bob = detail_for(&model, "bob@example.com");
    let texts = detail_texts(&bob, &versus(&model));
    // Two reviews of one PR are one PR reviewed.
    assert_eq!(texts.reviews.value, "1");
    let score = model
        .current
        .people
        .iter()
        .find(|p| p.person_id == "bob@example.com")
        .unwrap();
    assert_eq!(texts.reviews.value, score.reviewed_prs.to_string());
    assert_eq!(bob.reviews_by_bucket.iter().sum::<u64>(), 1);
}

#[test]
fn the_trend_has_two_plain_series_on_the_lists_buckets() {
    let model = model_of(reviewed(), PeriodKind::Month);
    let bob = detail_for(&model, "bob@example.com");
    let data = trend_data(&bob, &model.stats);
    let ids: Vec<&str> = data.series.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, [COMMITS_ID, REVIEWS_ID]);
    assert_eq!(data.buckets.len(), model.stats.buckets.len());
    assert!(
        data.series
            .iter()
            .all(|s| s.values.len() == data.buckets.len())
    );
    assert_eq!(data.series[0].values.iter().sum::<f64>(), 2.0);
    assert_eq!(data.series[1].values.iter().sum::<f64>(), 1.0);
    assert!(
        data.summary.starts_with("Commits and PRs reviewed per day"),
        "{}",
        data.summary
    );
}

#[test]
fn areas_show_their_share_and_prs_read_newest_first() {
    let model = model_of(reviewed(), PeriodKind::Month);
    let ana = detail_for(&model, "ana");
    let areas = area_rows(&ana);
    assert_eq!(areas[0].label, "src");
    assert_eq!(areas[0].fraction, 1.0);
    assert!(areas[0].value.starts_with("100% - "), "{}", areas[0].value);
    let prs = pr_rows(&ana);
    assert_eq!(prs.len(), 1);
    // Git-sourced PRs have no number: no "-" to fill the column with.
    assert_eq!(prs[0].lead, "");
    assert!(!has_lead_column(&prs));
    let numbered = ListRow {
        lead: "#12".into(),
        ..prs[0].clone()
    };
    assert!(has_lead_column(&[prs[0].clone(), numbered]));
    assert_eq!(prs[0].tag.as_deref(), Some("merged"));
    assert_eq!(prs[0].trail, "Oct 12, 2026");
}

// ---- the page in a window --------------------------------------------------------

fn person_at(ui: &super::page_tests::Ui, v: &mut VisualTestContext, row: usize) -> String {
    let view = ui.view(v);
    v.read(|cx| {
        view.read(cx).model.clone().unwrap().stats.rows[row]
            .person_id
            .clone()
    })
}

fn detail_of(ui: &super::page_tests::Ui, v: &mut VisualTestContext) -> Option<String> {
    let view = ui.view(v);
    v.read(|cx| view.read(cx).detail.clone())
}

#[gpui::test]
fn the_page_has_a_header_cards_a_trend_the_activity_and_two_lists(cx: &mut TestAppContext) {
    let (_, mut v) = stats_window(cx);
    click(&mut v, "contributors-stats-row-0");
    for selector in [
        "contributors-detail",
        "contributors-detail-back",
        "contributors-detail-header",
        "contributors-detail-ask",
        "contributors-detail-kpi-commits",
        "contributors-detail-kpi-prs",
        "contributors-detail-kpi-reviews",
        "contributors-detail-kpi-lines",
        "contributors-detail-kpi-survival",
        "contributors-detail-chart",
        "contributors-detail-legend-0",
        "contributors-detail-legend-1",
        "contributors-activity-calendar",
        "contributors-activity-rhythm",
        "contributors-detail-areas",
        "contributors-detail-prs",
    ] {
        assert!(shown(&mut v, selector), "{selector} is drawn");
    }
    // The member is fixed: no pills. The list and its section tabs are gone.
    for selector in [
        "contributors-activity-pills",
        "contributors-stats-table",
        "contributors-sections",
    ] {
        assert!(!shown(&mut v, selector), "{selector} is not drawn");
    }
}

#[gpui::test]
fn the_calendar_follows_the_member_and_goes_back_to_the_team(cx: &mut TestAppContext) {
    let (ui, mut v) = stats_window(cx);
    let (calendar, _) = activity_views(&ui, &mut v);
    assert_eq!(day_commits(&calendar, &mut v, 10, 13), 2);
    // Row 1 is Bob (Ana has more commits): his day, not Ana's.
    click(&mut v, "contributors-stats-row-1");
    assert_eq!(day_commits(&calendar, &mut v, 10, 13), 2);
    assert_eq!(day_commits(&calendar, &mut v, 10, 14), 0);
    click(&mut v, "contributors-detail-back");
    assert_eq!(day_commits(&calendar, &mut v, 10, 14), 1);
}

#[gpui::test]
fn back_restores_the_section_period_focus_selection_and_scroll(cx: &mut TestAppContext) {
    let (ui, mut v) = stats_window_of(cx, many_authors_raw(30));
    let view = ui.view(&mut v);
    // Something of every kind to remember.
    click(&mut v, "contributors-activity-pill-2");
    click(&mut v, "contributors-stats-legend-1");
    let offset = gpui::point(gpui::px(0.), gpui::px(-100.));
    view.update(&mut v, |this, _| this.scroll.set_offset(offset));
    settle(&mut v);
    let snapshot = |v: &mut VisualTestContext| {
        v.read(|cx| {
            let this = view.read(cx);
            (
                this.section,
                this.pick.clone(),
                this.activity.selected.clone(),
                this.stats_ui.chart.clone().unwrap().read(cx).focus.clone(),
                this.scroll.offset(),
            )
        })
    };
    let before = snapshot(&mut v);
    assert_eq!(before.4, offset, "the scroll took");
    click(&mut v, "contributors-stats-row-3");
    // The page opens at the top.
    assert_eq!(
        v.read(|cx| view.read(cx).scroll.offset()),
        gpui::point(gpui::px(0.), gpui::px(0.))
    );
    assert!(detail_of(&ui, &mut v).is_some());
    click(&mut v, "contributors-detail-back");
    assert_eq!(detail_of(&ui, &mut v), None);
    assert!(shown(&mut v, "contributors-stats-table"));
    assert_eq!(snapshot(&mut v), before);
}

#[gpui::test]
fn escape_closes_the_detail_first_and_the_page_next(cx: &mut TestAppContext) {
    let (ui, mut v) = stats_window(cx);
    click(&mut v, "contributors-stats-row-0");
    v.simulate_keystrokes("escape");
    settle(&mut v);
    assert_eq!(detail_of(&ui, &mut v), None);
    assert!(shown(&mut v, "contributors-stats-table"));
    assert_eq!(ui.closed_count(), 0);
    v.simulate_keystrokes("escape");
    settle(&mut v);
    assert_eq!(ui.closed_count(), 1);
}

#[gpui::test]
fn escape_closes_the_palette_first_and_opening_a_page_closes_it(cx: &mut TestAppContext) {
    let (ui, mut v) = stats_window(cx);
    right_click(&mut v, "contributors-stats-legend-0");
    assert!(shown(&mut v, "contributors-stats-palette-0"));
    v.simulate_keystrokes("escape");
    settle(&mut v);
    assert!(!shown(&mut v, "contributors-stats-palette-0"));
    assert_eq!(ui.closed_count(), 0);
    // Opening a page takes an open palette away with it.
    let view = ui.view(&mut v);
    right_click(&mut v, "contributors-stats-legend-0");
    let first = person_at(&ui, &mut v, 0);
    view.update(&mut v, |this, cx| this.open_detail(first, cx));
    settle(&mut v);
    let list_chart = v.read(|cx| view.read(cx).stats_ui.chart.clone().unwrap());
    assert!(v.read(|cx| list_chart.read(cx).palette.is_none()));
}

#[gpui::test]
fn a_person_without_activity_gets_a_line_and_the_way_back(cx: &mut TestAppContext) {
    let (ui, mut v) = stats_window(cx);
    let view = ui.view(&mut v);
    view.update(&mut v, |this, cx| this.open_detail("nobody".into(), cx));
    settle(&mut v);
    assert!(shown(&mut v, "contributors-detail-empty"));
    assert!(shown(&mut v, "contributors-detail-back"));
    click(&mut v, "contributors-detail-back");
    assert_eq!(detail_of(&ui, &mut v), None);
    assert!(shown(&mut v, "contributors-stats-table"));
}

#[gpui::test]
fn a_period_change_applies_to_the_open_page(cx: &mut TestAppContext) {
    let (ui, mut v) = stats_window(cx);
    let view = ui.view(&mut v);
    view.update(&mut v, |this, cx| this.set_kind(PeriodKind::Month, cx));
    settle(&mut v);
    // Bob committed in October only: September is empty for him.
    let bob = (0..2)
        .map(|i| person_at(&ui, &mut v, i))
        .find(|id| id.starts_with("bob"))
        .unwrap();
    view.update(&mut v, |this, cx| this.open_detail(bob.clone(), cx));
    settle(&mut v);
    assert!(shown(&mut v, "contributors-detail-kpi-commits"));
    click(&mut v, "contributors-previous");
    assert_eq!(detail_of(&ui, &mut v), Some(bob));
    assert!(shown(&mut v, "contributors-detail-empty"));
    assert!(!shown(&mut v, "contributors-detail-kpi-commits"));
}

#[gpui::test]
fn the_trend_shares_the_chart_type_and_focuses_by_legend_without_a_palette(
    cx: &mut TestAppContext,
) {
    let (ui, mut v) = stats_window(cx);
    click(&mut v, "contributors-stats-row-0");
    let view = ui.view(&mut v);
    let detail_chart = v.read(|cx| view.read(cx).detail_ui.chart.clone().expect("a chart"));
    click(&mut v, "contributors-detail-legend-1");
    assert!(v.read(|cx| detail_chart.read(cx).focus.contains(REVIEWS_ID)));
    // No palette on series that are not people.
    right_click(&mut v, "contributors-detail-legend-0");
    assert!(v.read(|cx| detail_chart.read(cx).palette.is_none()));
    assert!(!shown(&mut v, "contributors-detail-legend-0-color"));
    // The type is the one shared by every chart.
    click(&mut v, "contributors-chart-kind-bars");
    assert_eq!(
        v.read(|cx| detail_chart.read(cx).kind),
        chart::ChartKind::Bars
    );
    click(&mut v, "contributors-detail-back");
    let list_chart = v.read(|cx| view.read(cx).stats_ui.chart.clone().unwrap());
    assert_eq!(
        v.read(|cx| list_chart.read(cx).kind),
        chart::ChartKind::Bars
    );
}

#[gpui::test]
fn each_visit_starts_with_a_fresh_legend_focus(cx: &mut TestAppContext) {
    let (ui, mut v) = stats_window(cx);
    click(&mut v, "contributors-stats-row-0");
    click(&mut v, "contributors-detail-legend-1");
    click(&mut v, "contributors-detail-back");
    click(&mut v, "contributors-stats-row-0");
    let view = ui.view(&mut v);
    let chart = v.read(|cx| view.read(cx).detail_ui.chart.clone().unwrap());
    assert!(v.read(|cx| chart.read(cx).focus.is_empty()));
}

#[gpui::test]
fn ask_on_the_page_reports_the_person(cx: &mut TestAppContext) {
    let (ui, mut v) = stats_window(cx);
    let asked = Rc::new(RefCell::new(Vec::new()));
    let log = asked.clone();
    let view = ui.view(&mut v);
    let _watch = v.update(|_, cx| {
        cx.subscribe(&view, move |_, event: &GithubEvent, _| {
            let GithubEvent::Ask(scope) = event;
            log.borrow_mut().push(scope.clone());
        })
    });
    let first = person_at(&ui, &mut v, 0);
    click(&mut v, "contributors-stats-row-0");
    click(&mut v, "contributors-detail-ask");
    assert_eq!(
        *asked.borrow(),
        [SnapshotScope::Person { person_id: first }]
    );
    // Ask does not leave the page.
    assert!(detail_of(&ui, &mut v).is_some());
}

#[gpui::test]
fn escape_closes_the_detail_even_when_the_focus_is_on_the_page_root(cx: &mut TestAppContext) {
    let (ui, mut v) = stats_window(cx);
    click(&mut v, "contributors-stats-row-0");
    // As after a cancelled Ask dialog, a project switch or a reopened page.
    ui.window
        .update(&mut v, |page, window, cx| window.focus(&page.focus, cx))
        .unwrap();
    settle(&mut v);
    v.simulate_keystrokes("escape");
    settle(&mut v);
    assert_eq!(detail_of(&ui, &mut v), None);
    assert_eq!(ui.closed_count(), 0, "the page stayed open");
    v.simulate_keystrokes("escape");
    settle(&mut v);
    assert_eq!(ui.closed_count(), 1);
}

#[gpui::test]
fn a_member_with_no_commits_in_the_period_gets_their_own_calendar(cx: &mut TestAppContext) {
    let (ui, mut v) = stats_window(cx);
    let (calendar, _) = activity_views(&ui, &mut v);
    let view = ui.view(&mut v);
    // September: Bob has no commits there, so no row.
    click(&mut v, "contributors-previous");
    view.update(&mut v, |this, cx| {
        this.open_detail("bob@example.com".into(), cx)
    });
    settle(&mut v);
    // His own October days, not the team's (Ana committed on the 14th).
    assert_eq!(day_commits(&calendar, &mut v, 10, 13), 2);
    assert_eq!(day_commits(&calendar, &mut v, 10, 14), 0);
}
