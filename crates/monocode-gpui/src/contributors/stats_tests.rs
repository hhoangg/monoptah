//! StatsTab.test.ts: the rules behind the Stats section (what the cards, the
//! chart, the legend and the table say) as tests of the pure view functions,
//! and the interactions (expand a row, Ask, hover tooltips) in a real window
//! over the fake backend.
use super::{
    GithubEvent, chart,
    data_tests::{Fake, start_of_tests},
    page_tests::{click, github, settle, shown},
    stats::{
        Tone, change, chart_data, chart_summary, identities_chip, kpi_texts, row_label,
        share_label, table_height, versus,
    },
};
use chrono::{DateTime, Local, TimeZone};
use gpui::{TestAppContext, VisualTestContext};
use monocode_core::contributors::{
    GitIdentity, RawCommit, RawData, RawFile, RawPr,
    jobs::LoadResponse,
    model::{
        config::{default_config, serialize_config},
        period::{PeriodKind, PeriodPick, current_period, sync_window},
        pipeline::{ContributorsModel, ModelInput, build_contributors_model},
        prefs::serialize_pick,
        snapshot::SnapshotScope,
        types::Person,
    },
};
use std::{cell::RefCell, rc::Rc, sync::Arc};

fn local(y: i32, m: u32, d: u32, h: u32) -> DateTime<Local> {
    Local.with_ymd_and_hms(y, m, d, h, 0, 0).earliest().unwrap()
}

/// The pure tests build their model at this moment, the window tests use the
/// clock of `data_tests` (Oct 15): every fixture commit is before both.
pub(super) fn now() -> DateTime<Local> {
    local(2026, 10, 20, 10)
}

fn who(name: &str, email: &str) -> GitIdentity {
    GitIdentity {
        name: name.into(),
        email: email.into(),
        login: None,
    }
}

fn commit(
    hash: &str,
    author: &GitIdentity,
    at: DateTime<Local>,
    files: &[(&str, u64, u64)],
) -> RawCommit {
    let at = at.to_rfc3339();
    RawCommit {
        hash: hash.into(),
        subject: format!("commit {hash}"),
        author: author.clone(),
        committer: author.clone(),
        authored_at: at.clone(),
        committed_at: at,
        co_authors: vec![],
        files: files
            .iter()
            .map(|&(path, added, deleted)| RawFile {
                path: path.into(),
                added,
                deleted,
            })
            .collect(),
        pr_id: None,
    }
}

fn empty_raw() -> RawData {
    RawData {
        head: "h1".into(),
        branch: "main".into(),
        since: String::new(),
        until: String::new(),
        gh_available: true,
        gh_note: None,
        commits: vec![],
        prs: vec![],
        reviews: vec![],
        survival: vec![],
        reverted_pr_ids: vec![],
        truncated: false,
    }
}

fn pr(id: &str, author: &GitIdentity, merged: DateTime<Local>) -> RawPr {
    RawPr {
        id: id.into(),
        number: None,
        title: format!("PR {id}"),
        state: "merged".into(),
        source: "gh".into(),
        author: author.clone(),
        committer: None,
        merged_at: Some(merged.to_rfc3339()),
        closed_at: None,
        merge_commit: None,
        commit_count: 1,
        branch_commits: vec![],
        landing_co_authors: vec![],
        files: vec![],
        closes_bug: false,
    }
}

fn ana() -> GitIdentity {
    who("Ana", "ana@example.com")
}
fn bob() -> GitIdentity {
    who("Bob", "bob@example.com")
}

/// Ana (two identities, one big commit on Oct 1), Bob (two commits on Oct 13)
/// and one September commit, so October has a previous period to compare with.
pub(super) fn fixture() -> RawData {
    let laptop = who("Ana", "ana@laptop.local");
    RawData {
        commits: vec![
            commit(
                "p1",
                &ana(),
                local(2026, 10, 1, 9),
                &[("src/a.ts", 12000, 1000)],
            ),
            commit("early", &ana(), local(2026, 9, 5, 9), &[("src/a.ts", 4, 0)]),
            commit("a1", &ana(), local(2026, 10, 14, 9), &[("src/a.ts", 20, 5)]),
            commit(
                "a2",
                &laptop,
                local(2026, 10, 15, 9),
                &[("src/b.ts", 10, 0)],
            ),
            commit("b1", &bob(), local(2026, 10, 13, 9), &[("src/c.ts", 7, 0)]),
            commit("b2", &bob(), local(2026, 10, 13, 14), &[("src/c.ts", 3, 0)]),
        ],
        prs: vec![pr("1", &ana(), local(2026, 10, 12, 9))],
        ..empty_raw()
    }
}

fn config() -> monocode_core::contributors::model::types::ContributorsConfig {
    let mut config = default_config();
    config.people = vec![Person {
        id: "ana".into(),
        name: "Ana Sol".into(),
        identities: vec!["ana@example.com".into(), "ana@laptop.local".into()],
    }];
    config
}

pub(super) fn model_of(raw: RawData, kind: PeriodKind) -> ContributorsModel {
    let now = now();
    model_in(raw, current_period(kind, &now, None))
}

fn model_in(
    mut raw: RawData,
    period: monocode_core::contributors::model::period::Period,
) -> ContributorsModel {
    // The data starts where the sync window of the period does, so the
    // previous period counts as read.
    raw.since = sync_window(&period, &now()).from.to_rfc3339();
    build_contributors_model(ModelInput {
        raw: Arc::new(raw),
        config: Arc::new(config()),
        mailmap: vec![],
        merge: true,
        period,
        now: now(),
        user: None,
        color_overrides: Default::default(),
    })
}

pub(super) fn month() -> ContributorsModel {
    model_of(fixture(), PeriodKind::Month)
}

/// `count` authors; author N has `count - N` commits spread over October.
pub(super) fn many_authors_raw(count: usize) -> RawData {
    let mut raw = empty_raw();
    for i in 0..count {
        let author = who(
            &format!("Person {}", i + 1),
            &format!("p{}@example.com", i + 1),
        );
        for n in 0..(count - i) {
            raw.commits.push(commit(
                &format!("c{i}-{n}"),
                &author,
                local(2026, 10, 10 + (n % 5) as u32, 9),
                &[(&format!("src/f{i}.ts"), 3, 0)],
            ));
        }
    }
    raw
}

fn many_authors(count: usize) -> ContributorsModel {
    let now = now();
    build_contributors_model(ModelInput {
        raw: Arc::new(many_authors_raw(count)),
        config: Arc::new(default_config()),
        mailmap: vec![],
        merge: true,
        period: current_period(PeriodKind::Month, &now, None),
        now,
        user: None,
        color_overrides: Default::default(),
    })
}

// ---- the cards ---------------------------------------------------------------

#[test]
fn the_four_cards_show_the_numbers_of_the_model() {
    let m = month();
    let kpis = kpi_texts(&m);
    assert_eq!(kpis.commits.value, m.stats.kpis.commits.value.to_string());
    assert_eq!(kpis.contributors.value, "2");
    assert_eq!(kpis.contributors.from, "from 3 identities");
    assert_eq!(kpis.lines.added, "+12k");
    assert!(kpis.lines.deleted.starts_with('-'));
    assert_eq!(kpis.prs.value, m.stats.kpis.prs_merged.value.to_string());
}

#[test]
fn one_identity_is_singular() {
    let mut raw = empty_raw();
    raw.commits = vec![commit(
        "a",
        &ana(),
        local(2026, 10, 14, 9),
        &[("a.ts", 1, 0)],
    )];
    let m = model_of(raw, PeriodKind::Month);
    assert_eq!(kpi_texts(&m).contributors.from, "from 1 identity");
}

#[test]
fn the_change_against_the_previous_period_has_an_arrow_a_sign_and_a_tone() {
    let m = month();
    let delta = m.stats.kpis.commits.delta.expect("September has a commit");
    assert!(delta > 0.0);
    let commits = kpi_texts(&m).commits.change.expect("a change");
    assert_eq!(commits.tone, Tone::Up);
    assert!(commits.text.starts_with("▲ +"), "{}", commits.text);
    assert!(commits.text.ends_with('%'));
    assert_eq!(commits.suffix.as_deref(), Some("vs Sep 2026"));
}

#[test]
fn a_change_is_up_down_or_flat_and_never_colour_alone() {
    let up = change(Some(0.18), "+18%".into(), Some("vs Sep 2026".into())).unwrap();
    assert_eq!((up.tone, up.text.as_str()), (Tone::Up, "▲ +18%"));
    let down = change(Some(-0.2), "-20%".into(), None).unwrap();
    assert_eq!((down.tone, down.text.as_str()), (Tone::Down, "▼ -20%"));
    let flat = change(Some(0.0), "0%".into(), None).unwrap();
    assert_eq!((flat.tone, flat.text.as_str()), (Tone::Flat, "0%"));
    assert!(change(None, String::new(), Some("vs Sep 2026".into())).is_none());
}

#[test]
fn newcomers_are_an_up_change_only_when_there_are_some() {
    let m = month();
    assert_eq!(m.stats.kpis.contributors.newcomers, 1);
    let change = kpi_texts(&m).contributors.change.expect("Bob is new");
    assert_eq!(change.text, "▲ 1 new");
    assert_eq!(change.suffix, None);
    // Nobody new: no line at all.
    let mut raw = empty_raw();
    raw.commits = vec![
        commit("old", &ana(), local(2026, 9, 5, 9), &[("a.ts", 1, 0)]),
        commit("now", &ana(), local(2026, 10, 14, 9), &[("a.ts", 1, 0)]),
    ];
    let m = model_of(raw, PeriodKind::Month);
    assert_eq!(m.stats.kpis.contributors.newcomers, 0);
    assert_eq!(kpi_texts(&m).contributors.change, None);
}

#[test]
fn the_pr_change_is_a_signed_count() {
    let m = month();
    let prs = kpi_texts(&m).prs.change.expect("one PR against none");
    assert_eq!(prs.text, "▲ +1");
    assert_eq!(prs.tone, Tone::Up);
}

#[test]
fn the_comparison_names_the_previous_period_of_each_kind() {
    let named = |kind| versus(&model_of(fixture(), kind));
    assert_eq!(named(PeriodKind::Days7).as_deref(), Some("vs prev 7d"));
    assert_eq!(named(PeriodKind::Month).as_deref(), Some("vs Sep 2026"));
    assert_eq!(named(PeriodKind::Quarter).as_deref(), Some("vs Q3 2026"));
    assert_eq!(named(PeriodKind::Year).as_deref(), Some("vs 2025"));
}

#[test]
fn all_time_hides_every_comparison() {
    let now = now();
    let all = current_period(PeriodKind::All, &now, Some(&local(2026, 9, 1, 0)));
    let m = model_in(fixture(), all);
    assert!(m.previous_period.is_none());
    assert_eq!(versus(&m), None);
    let kpis = kpi_texts(&m);
    assert!(kpis.commits.change.is_none());
    assert!(kpis.lines.change.is_none());
    assert!(kpis.prs.change.is_none());
    assert!(kpis.contributors.change.is_none());
}

// ---- the chart -----------------------------------------------------------------

#[test]
fn the_chart_is_summarised_for_screen_readers() {
    let m = month();
    let label = chart_summary(&m.stats);
    assert!(label.starts_with("Commit activity per day: "), "{label}");
    assert!(label.contains(&format!("{} days", m.stats.buckets.len())));
    assert!(label.contains("busiest day Oct 13, 2026 with 2"), "{label}");
    // The year view counts months, the quarter view weeks.
    assert!(chart_summary(&model_of(fixture(), PeriodKind::Year).stats).contains("per month"));
    assert!(chart_summary(&model_of(fixture(), PeriodKind::Quarter).stats).contains("weeks"));
}

#[test]
fn an_empty_period_says_there_were_no_commits() {
    let m = model_of(empty_raw(), PeriodKind::Month);
    let label = chart_summary(&m.stats);
    assert!(label.contains("no commits"), "{label}");
    assert!(label.contains(&format!("{} days", m.stats.buckets.len())));
}

#[test]
fn the_chart_data_has_a_series_per_person_and_a_value_per_bucket() {
    let m = month();
    let data = chart_data(&m.stats);
    assert_eq!(data.buckets.len(), m.stats.buckets.len());
    assert_eq!(data.buckets[0].label, m.stats.buckets[0].label);
    assert_eq!(data.buckets[0].short, m.stats.buckets[0].short);
    assert_eq!(data.series.len(), 2);
    for series in &data.series {
        assert_eq!(series.values.len(), data.buckets.len());
    }
    let oct13 = m
        .stats
        .buckets
        .iter()
        .position(|b| b.start == "2026-10-13")
        .unwrap();
    let bob = data
        .series
        .iter()
        .find(|s| s.id == "bob@example.com")
        .unwrap();
    assert_eq!(bob.name, "Bob");
    assert_eq!(bob.values[oct13], 2.);
    assert_eq!(data.summary, chart_summary(&m.stats));
}

#[test]
fn the_chart_keeps_the_five_busiest_people_and_sums_the_rest_into_others() {
    let m = many_authors(12);
    let data = chart_data(&m.stats);
    let names: Vec<&str> = data.series.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "Person 1", "Person 2", "Person 3", "Person 4", "Person 5", "Others"
        ]
    );
    let others = data.series.last().unwrap();
    assert_eq!(others.id, chart::OTHERS_ID);
    // Person 6 to 12 made 7+6+5+4+3+2+1 commits.
    assert_eq!(others.values.iter().sum::<f64>(), 28.);
    // The colors of the five are the palette ones the rows use.
    for series in &data.series[..5] {
        let row = m
            .stats
            .rows
            .iter()
            .find(|r| r.person_id == series.id)
            .unwrap();
        assert_eq!(series.color, ui_hex(&row.color));
    }
    // Five people or fewer: no Others at all.
    assert!(
        chart_data(&many_authors(5).stats)
            .series
            .iter()
            .all(|s| s.id != chart::OTHERS_ID)
    );
}

fn ui_hex(hex: &str) -> gpui::Hsla {
    super::ui::hex_color(hex)
}

#[test]
fn the_column_of_a_bucket_names_the_day_and_every_person_in_the_tooltip() {
    let m = month();
    let data = chart_data(&m.stats);
    let at = |start: &str| {
        m.stats
            .buckets
            .iter()
            .position(|b| b.start == start)
            .unwrap()
    };
    let focus = std::collections::BTreeSet::new();
    let tip = chart::tooltip(chart::ChartKind::Bars, &data, &focus, at("2026-10-13")).unwrap();
    assert_eq!(tip.title, "Oct 13, 2026");
    let rows: Vec<(&str, &str)> = tip
        .rows
        .iter()
        .map(|(_, n, v)| (n.as_str(), v.as_str()))
        .collect();
    assert_eq!(rows, [("Bob", "2")]);
    let tip = chart::tooltip(chart::ChartKind::Bars, &data, &focus, at("2026-10-02")).unwrap();
    assert!(tip.rows.is_empty());
}

#[test]
fn the_x_axis_labels_the_first_and_last_bucket_and_few_between() {
    let m = month();
    let data = chart_data(&m.stats);
    let labels = chart::axis_labels(&data.buckets);
    let buckets = &m.stats.buckets;
    assert_eq!(labels[0].1, buckets[0].short);
    assert_eq!(labels[labels.len() - 1].1, buckets[buckets.len() - 1].short);
    assert_eq!(labels[0].0, 0);
    assert_eq!(labels[labels.len() - 1].0, buckets.len() - 1);
    assert!((2..=10).contains(&labels.len()));
    let week = chart_data(&model_of(fixture(), PeriodKind::Days7).stats);
    let first = &chart::axis_labels(&week.buckets)[0].1;
    assert!(
        first.len() >= 5 && first.len() <= 6 && first.chars().nth(3) == Some(' '),
        "{first}"
    );
}

#[test]
fn the_y_axis_maximum_covers_the_busiest_bucket() {
    let m = month();
    let data = chart_data(&m.stats);
    // Bob made two commits on Oct 13, the most of any day.
    assert_eq!(chart::y_scale(chart::ChartKind::Bars, &data.series).max, 2.);
    let none = chart_data(&model_of(empty_raw(), PeriodKind::Month).stats);
    assert!(chart::y_scale(chart::ChartKind::Bars, &none.series).max >= 1.);
}

#[test]
fn the_table_is_as_tall_as_its_head_and_its_rows() {
    assert_eq!(table_height(0), 34.);
    assert_eq!(table_height(3), 34. + 3. * 41.);
}

// ---- the legend ---------------------------------------------------------------

#[test]
fn a_person_has_the_same_colour_in_the_table_the_legend_and_the_chart() {
    let m = month();
    let row = &m.stats.rows[0];
    let hex = m.colors[&row.person_id];
    assert_eq!(row.color, hex);
    let entry = m
        .stats
        .legend
        .iter()
        .find(|l| l.person_id == row.person_id)
        .unwrap();
    assert_eq!(entry.color, hex);
}

// ---- the table ------------------------------------------------------------------

#[test]
fn a_row_says_how_many_identities_are_behind_a_person() {
    let m = month();
    let ana = m.stats.rows.iter().find(|r| r.name == "Ana Sol").unwrap();
    assert_eq!(ana.identities.len(), 2);
    assert_eq!(identities_chip(ana).as_deref(), Some("2 identities"));
    assert_eq!(row_label(ana), "Open details for Ana Sol");
    let bob = m.stats.rows.iter().find(|r| r.name == "Bob").unwrap();
    assert_eq!(identities_chip(bob), None);
    assert_eq!(row_label(bob), "Open details for Bob");
}

#[test]
fn the_share_is_a_rounded_percentage() {
    let m = month();
    for row in &m.stats.rows {
        assert_eq!(
            share_label(row),
            format!("{}%", row.share_pct.round() as i64)
        );
    }
}

// ---- in a window ------------------------------------------------------------------

fn window_fake(mut raw: RawData) -> Arc<Fake> {
    let fake = Fake::new();
    let october = current_period(PeriodKind::Month, &start_of_tests(), None);
    raw.since = sync_window(&october, &start_of_tests()).from.to_rfc3339();
    fake.set_load(LoadResponse {
        raw,
        covered: true,
        has_gaps: false,
        missing: vec![],
        missing_commits: 0,
        synced_at: Some(start_of_tests().timestamp() - 300),
        coverage_head: Some("h1".into()),
        head: "h1".into(),
        gh_state: Some("complete".into()),
    });
    fake.with(|s| {
        s.config.default = Ok(Some(serialize_config(&config())));
        s.saved_range = Some(serialize_pick(&PeriodPick {
            kind: PeriodKind::Month,
            id: Some("2026-10".into()),
        }));
    });
    fake
}

pub(super) fn stats_window(cx: &mut TestAppContext) -> (super::page_tests::Ui, VisualTestContext) {
    stats_window_of(cx, fixture())
}

pub(super) fn stats_window_of(
    cx: &mut TestAppContext,
    raw: RawData,
) -> (super::page_tests::Ui, VisualTestContext) {
    let (ui, mut v) = github(cx, window_fake(raw));
    settle(&mut v);
    assert!(
        shown(&mut v, "contributors-body-stats"),
        "the Stats section is shown"
    );
    (ui, v)
}

fn move_away(v: &mut VisualTestContext) {
    v.simulate_mouse_move(
        gpui::point(gpui::px(1.), gpui::px(1.)),
        None,
        Default::default(),
    );
    settle(v);
}

#[gpui::test]
fn the_section_shows_the_cards_the_chart_the_legend_and_the_table(cx: &mut TestAppContext) {
    let (_, mut v) = stats_window(cx);
    for selector in [
        "contributors-stats-kpi-commits",
        "contributors-stats-kpi-contributors",
        "contributors-stats-kpi-lines",
        "contributors-stats-kpi-prs",
        "contributors-stats-chart",
        "contributors-stats-legend",
        "contributors-stats-table",
        "contributors-stats-row-0",
        "contributors-stats-row-1",
    ] {
        assert!(shown(&mut v, selector), "{selector}");
    }
    assert!(!shown(&mut v, "contributors-stats-row-2"));
}

fn chart_of(
    ui: &super::page_tests::Ui,
    v: &mut VisualTestContext,
) -> gpui::Entity<chart::ChartView> {
    let view = ui.view(v);
    v.read(|cx| view.read(cx).stats_ui.chart.clone().expect("the chart"))
}

pub(super) fn right_click(v: &mut VisualTestContext, selector: &'static str) {
    let at = v
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("nothing drawn as {selector}"))
        .center();
    v.simulate_event(gpui::MouseDownEvent {
        position: at,
        button: gpui::MouseButton::Right,
        modifiers: Default::default(),
        click_count: 1,
        first_mouse: false,
    });
    v.simulate_event(gpui::MouseUpEvent {
        position: at,
        button: gpui::MouseButton::Right,
        modifiers: Default::default(),
        click_count: 1,
    });
    settle(v);
}

fn leak(text: String) -> &'static str {
    Box::leak(text.into_boxed_str())
}

#[gpui::test]
fn the_chart_labels_the_y_maximum_and_only_the_chosen_x_ticks(cx: &mut TestAppContext) {
    let (ui, mut v) = stats_window(cx);
    assert!(shown(&mut v, "contributors-stats-ymax"));
    let view = ui.view(&mut v);
    let (count, ticks) = v.read(|cx| {
        let model = view.read(cx).snapshot(cx).model.expect("a model");
        let data = chart_data(&model.stats);
        (
            data.buckets.len(),
            chart::axis_labels(&data.buckets)
                .into_iter()
                .map(|(i, _)| i)
                .collect::<Vec<_>>(),
        )
    });
    // Oct 1 to Oct 15: the days that have happened.
    assert_eq!(count, 15);
    for i in 0..count {
        assert_eq!(
            shown(&mut v, leak(format!("contributors-stats-axis-{i}"))),
            ticks.contains(&i),
            "tick {i}"
        );
    }
}

#[gpui::test]
fn the_chart_is_one_labelled_image_and_not_an_element_per_segment(cx: &mut TestAppContext) {
    let (ui, mut v) = stats_window(cx);
    assert!(shown(&mut v, "contributors-stats-chart"));
    for i in 0..15 {
        assert!(!shown(
            &mut v,
            leak(format!("contributors-stats-bucket-{i}"))
        ));
    }
    let chart = chart_of(&ui, &mut v);
    let summary = v.read(|cx| chart.read(cx).data.summary.clone());
    assert!(
        summary.starts_with("Commit activity per day: "),
        "{summary}"
    );
}

#[gpui::test]
fn the_legend_sits_below_the_chart_with_a_button_per_series(cx: &mut TestAppContext) {
    let (_, mut v) = stats_window(cx);
    let chart = v.debug_bounds("contributors-stats-chart").unwrap();
    let legend = v.debug_bounds("contributors-stats-legend").unwrap();
    assert!(
        legend.top() >= chart.bottom(),
        "the legend is under the chart"
    );
    assert!(shown(&mut v, "contributors-stats-legend-0"));
    assert!(shown(&mut v, "contributors-stats-legend-1"));
    assert!(!shown(&mut v, "contributors-stats-legend-2"));
}

#[gpui::test]
fn past_five_people_the_rest_are_one_others_entry(cx: &mut TestAppContext) {
    let (ui, mut v) = stats_window_of(cx, many_authors_raw(12));
    for i in 0..6 {
        assert!(
            shown(&mut v, leak(format!("contributors-stats-legend-{i}"))),
            "{i}"
        );
    }
    assert!(!shown(&mut v, "contributors-stats-legend-6"));
    assert!(!shown(&mut v, "contributors-stats-legend-more"));
    let chart = chart_of(&ui, &mut v);
    let last = v.read(|cx| chart.read(cx).data.series.last().cloned().unwrap());
    assert_eq!(
        (last.id.as_str(), last.name.as_str()),
        (chart::OTHERS_ID, "Others")
    );
    // Others has no color button, a person has.
    assert!(shown(&mut v, "contributors-stats-legend-0-color"));
    assert!(!shown(&mut v, "contributors-stats-legend-5-color"));
}

#[gpui::test]
fn the_chart_is_a_line_at_first_and_the_chosen_type_survives_a_restart(cx: &mut TestAppContext) {
    let fake = window_fake(fixture());
    let (ui, mut v) = github(cx, fake.clone());
    settle(&mut v);
    let chart = chart_of(&ui, &mut v);
    assert_eq!(v.read(|cx| chart.read(cx).kind), chart::ChartKind::Line);
    for kind in ["line", "cumulative", "bars"] {
        assert!(shown(
            &mut v,
            leak(format!("contributors-chart-kind-{kind}"))
        ));
    }
    click(&mut v, "contributors-chart-kind-bars");
    assert_eq!(v.read(|cx| chart.read(cx).kind), chart::ChartKind::Bars);
    assert_eq!(
        fake.with(|s| s.saved_chart_kind.clone()).as_deref(),
        Some("bars")
    );
    let view = ui.view(&mut v);
    assert_eq!(
        v.read(|cx| view.read(cx).chart_kind(cx)),
        chart::ChartKind::Bars
    );
    // A second window over the same preferences opens on it.
    let (again, mut w) = github(cx, fake);
    settle(&mut w);
    let chart = chart_of(&again, &mut w);
    assert_eq!(w.read(|cx| chart.read(cx).kind), chart::ChartKind::Bars);
}

#[gpui::test]
fn clicking_legend_entries_focuses_them_together_and_never_saves(cx: &mut TestAppContext) {
    let fake = window_fake(fixture());
    let (ui, mut v) = github(cx, fake.clone());
    settle(&mut v);
    let chart = chart_of(&ui, &mut v);
    let ids = v.read(|cx| {
        chart
            .read(cx)
            .data
            .series
            .iter()
            .map(|s| s.id.clone())
            .collect::<Vec<_>>()
    });
    let saved = fake.with(|s| (s.saved_chart_kind.clone(), s.saved_colors.clone()));
    click(&mut v, "contributors-stats-legend-0");
    assert_eq!(
        v.read(|cx| chart.read(cx).focus.iter().cloned().collect::<Vec<_>>()),
        [ids[0].clone()]
    );
    click(&mut v, "contributors-stats-legend-1");
    assert_eq!(v.read(|cx| chart.read(cx).focus.len()), 2);
    click(&mut v, "contributors-stats-legend-0");
    assert_eq!(
        v.read(|cx| chart.read(cx).focus.iter().cloned().collect::<Vec<_>>()),
        [ids[1].clone()]
    );
    click(&mut v, "contributors-stats-legend-1");
    assert!(v.read(|cx| chart.read(cx).focus.is_empty()));
    assert_eq!(
        fake.with(|s| (s.saved_chart_kind.clone(), s.saved_colors.clone())),
        saved
    );
}

#[gpui::test]
fn the_pointer_moves_the_chart_hover_without_redrawing_the_page(cx: &mut TestAppContext) {
    let (ui, mut v) = stats_window(cx);
    let view = ui.view(&mut v);
    let chart = chart_of(&ui, &mut v);
    let notified = Rc::new(std::cell::Cell::new(0));
    let count = notified.clone();
    let _watch = v.update(|_, cx| cx.observe(&view, move |_, _| count.set(count.get() + 1)));
    assert_eq!(v.read(|cx| chart.read(cx).hover), None);
    let bounds = v.debug_bounds("contributors-stats-chart").unwrap();
    // The first bucket, then the last.
    for (x, expected) in [(0.01, 0), (0.99, 14)] {
        let at = gpui::point(
            bounds.origin.x + bounds.size.width * x,
            bounds.origin.y + bounds.size.height * 0.5,
        );
        v.simulate_mouse_move(at, None, Default::default());
        settle(&mut v);
        assert_eq!(v.read(|cx| chart.read(cx).hover), Some(expected));
    }
    move_away(&mut v);
    assert_eq!(v.read(|cx| chart.read(cx).hover), None);
    assert_eq!(
        notified.get(),
        0,
        "hovering the chart did not notify the page"
    );
}

#[gpui::test]
fn the_palette_opens_on_right_click_and_picking_a_color_recolors_the_page(cx: &mut TestAppContext) {
    use monocode_core::contributors::model::palette::PERSON_COLORS;
    let fake = window_fake(fixture());
    let (ui, mut v) = github(cx, fake.clone());
    settle(&mut v);
    let view = ui.view(&mut v);
    let chart = chart_of(&ui, &mut v);
    assert!(!shown(&mut v, "contributors-stats-palette-0"));
    right_click(&mut v, "contributors-stats-legend-0");
    for n in 0..PERSON_COLORS.len() {
        assert!(
            shown(&mut v, leak(format!("contributors-stats-palette-{n}"))),
            "swatch {n}"
        );
    }
    // Escape closes it, and so does a press outside.
    v.simulate_keystrokes("escape");
    settle(&mut v);
    assert!(!shown(&mut v, "contributors-stats-palette-0"));
    assert!(v.read(|cx| chart.read(cx).palette.is_none()));
    right_click(&mut v, "contributors-stats-legend-0");
    assert!(shown(&mut v, "contributors-stats-palette-0"));
    let outside = v
        .debug_bounds("contributors-stats-kpi-commits")
        .unwrap()
        .center();
    v.simulate_click(outside, Default::default());
    settle(&mut v);
    assert!(!shown(&mut v, "contributors-stats-palette-0"));
    // The color button opens it too.
    click(&mut v, "contributors-stats-legend-0-color");
    assert!(shown(&mut v, "contributors-stats-palette-0"));
    let (person, project, before) = v.read(|cx| {
        let this = view.read(cx);
        let model = this.model.clone().expect("a model");
        let person = chart.read(cx).data.series[0].id.clone();
        let before = model.colors[&person];
        (person, this.project().to_string(), before)
    });
    let picked = PERSON_COLORS
        .iter()
        .position(|c| *c != before)
        .expect("another color");
    click(&mut v, leak(format!("contributors-stats-palette-{picked}")));
    assert!(
        !shown(&mut v, "contributors-stats-palette-0"),
        "picking closes it"
    );
    // Saved for this project only.
    let saved = fake
        .with(|s| s.saved_colors.get(&project).cloned())
        .unwrap();
    assert_eq!(saved.get(&person), Some(&PERSON_COLORS[picked]));
    // The model was rebuilt with it: the table, the legend and the chart agree.
    settle(&mut v);
    let (model_color, row_color, series_color) = v.read(|cx| {
        let model = view.read(cx).model.clone().unwrap();
        let row = model
            .stats
            .rows
            .iter()
            .find(|r| r.person_id == person)
            .unwrap();
        (
            model.colors[&person],
            row.color.clone(),
            chart
                .read(cx)
                .data
                .series
                .iter()
                .find(|s| s.id == person)
                .unwrap()
                .color,
        )
    });
    assert_eq!(model_color, PERSON_COLORS[picked]);
    assert_eq!(row_color, PERSON_COLORS[picked]);
    assert_eq!(series_color, ui_hex(PERSON_COLORS[picked]));
    assert!(
        !ui.snap(&mut v).model_pending,
        "a recolor does not dim the page"
    );
}

#[gpui::test]
fn saved_colors_are_read_back_for_the_project_and_foreign_values_are_dropped(
    cx: &mut TestAppContext,
) {
    use monocode_core::contributors::model::palette::PERSON_COLORS;
    let fake = window_fake(fixture());
    fake.with(|s| {
        s.saved_colors.insert(
            "/work/alpha".into(),
            [("bob@example.com".to_string(), PERSON_COLORS[6])].into(),
        );
        s.saved_colors.insert(
            "/work/beta".into(),
            [("bob@example.com".to_string(), PERSON_COLORS[1])].into(),
        );
    });
    let (ui, mut v) = github(cx, fake);
    settle(&mut v);
    let view = ui.view(&mut v);
    let color = v.read(|cx| view.read(cx).model.clone().unwrap().colors["bob@example.com"]);
    assert_eq!(color, PERSON_COLORS[6]);
}

#[gpui::test]
fn the_table_is_laid_out_at_the_height_the_cache_was_given(cx: &mut TestAppContext) {
    let (ui, mut v) = stats_window(cx);
    let view = ui.view(&mut v);
    let rows = v.read(|cx| view.read(cx).model.clone().unwrap().stats.rows.len());
    let height = v
        .debug_bounds("contributors-stats-table")
        .unwrap()
        .size
        .height;
    assert_eq!(height, gpui::px(table_height(rows)));
}

fn detail_of(ui: &super::page_tests::Ui, v: &mut VisualTestContext) -> Option<String> {
    let view = ui.view(v);
    v.read(|cx| view.read(cx).detail.clone())
}

#[gpui::test]
fn a_row_opens_the_detail_page_of_its_person(cx: &mut TestAppContext) {
    let (ui, mut v) = stats_window(cx);
    let view = ui.view(&mut v);
    let first = v.read(|cx| {
        view.read(cx).model.clone().unwrap().stats.rows[0]
            .person_id
            .clone()
    });
    assert!(!shown(&mut v, "contributors-detail"));
    click(&mut v, "contributors-stats-row-0");
    assert_eq!(detail_of(&ui, &mut v), Some(first));
    assert!(shown(&mut v, "contributors-detail"));
    // The page stands in place of the list.
    assert!(!shown(&mut v, "contributors-stats-table"));
    assert!(!shown(&mut v, "contributors-stats-row-0"));
}

#[gpui::test]
fn the_name_button_opens_the_detail_page_once(cx: &mut TestAppContext) {
    let (ui, mut v) = stats_window(cx);
    click(&mut v, "contributors-stats-name-1");
    let view = ui.view(&mut v);
    let second = v.read(|cx| {
        view.read(cx).model.clone().unwrap().stats.rows[1]
            .person_id
            .clone()
    });
    assert_eq!(detail_of(&ui, &mut v), Some(second));
    assert!(shown(&mut v, "contributors-detail"));
}

#[gpui::test]
fn ask_reports_the_person_without_opening_the_row(cx: &mut TestAppContext) {
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
    let first = v.read(|cx| {
        view.read(cx)
            .snapshot(cx)
            .model
            .expect("a model")
            .stats
            .rows[0]
            .person_id
            .clone()
    });
    click(&mut v, "contributors-stats-ask-0");
    assert_eq!(
        *asked.borrow(),
        [SnapshotScope::Person { person_id: first }]
    );
    assert!(!shown(&mut v, "contributors-detail"));
}

#[gpui::test]
fn the_chart_type_reaches_every_open_page_at_once(cx: &mut TestAppContext) {
    let fake = window_fake(fixture());
    let (first, mut a) = github(cx, fake.clone());
    let (second, mut b) = github(cx, fake);
    settle(&mut a);
    settle(&mut b);
    let other = chart_of(&second, &mut b);
    assert_eq!(b.read(|cx| other.read(cx).kind), chart::ChartKind::Line);
    click(&mut a, "contributors-chart-kind-bars");
    settle(&mut b);
    assert_eq!(b.read(|cx| other.read(cx).kind), chart::ChartKind::Bars);
    let mine = chart_of(&first, &mut a);
    assert_eq!(a.read(|cx| mine.read(cx).kind), chart::ChartKind::Bars);
}

#[gpui::test]
fn closing_the_palette_gives_the_keys_back_to_the_page(cx: &mut TestAppContext) {
    let (ui, mut v) = stats_window(cx);
    right_click(&mut v, "contributors-stats-legend-0");
    v.simulate_keystrokes("escape");
    settle(&mut v);
    assert!(!shown(&mut v, "contributors-stats-palette-0"));
    assert_eq!(ui.closed_count(), 0);
    // The next Escape is the page's again.
    v.simulate_keystrokes("escape");
    settle(&mut v);
    assert_eq!(ui.closed_count(), 1);
    // Picking a color also hands the focus back.
    let (ui, mut v) = stats_window(cx);
    right_click(&mut v, "contributors-stats-legend-0");
    click(&mut v, "contributors-stats-palette-2");
    v.simulate_keystrokes("escape");
    settle(&mut v);
    assert_eq!(ui.closed_count(), 1);
}

#[gpui::test]
fn the_table_follows_the_dim_of_a_stale_page(cx: &mut TestAppContext) {
    let (ui, mut v) = stats_window(cx);
    let view = ui.view(&mut v);
    let table = v.read(|cx| view.read(cx).stats_ui.table.clone().unwrap());
    assert!(!v.read(|cx| table.read(cx).dim));
    // A period that is not loaded yet: the old numbers stay on screen, dimmed.
    view.update(&mut v, |this, cx| {
        this.hold_backend_calls();
        this.set_kind(PeriodKind::Quarter, cx);
    });
    settle(&mut v);
    assert!(ui.snap(&mut v).stale);
    assert!(v.read(|cx| table.read(cx).dim));
}

#[gpui::test]
fn the_chart_opens_on_the_saved_type_in_a_fresh_app(cx: &mut TestAppContext) {
    let fake = window_fake(fixture());
    fake.with(|s| s.saved_chart_kind = Some("bars".into()));
    let (ui, mut v) = github(cx, fake);
    settle(&mut v);
    let chart = chart_of(&ui, &mut v);
    assert_eq!(v.read(|cx| chart.read(cx).kind), chart::ChartKind::Bars);
}

#[gpui::test]
fn an_unknown_saved_type_opens_as_the_default_line(cx: &mut TestAppContext) {
    let fake = window_fake(fixture());
    fake.with(|s| s.saved_chart_kind = Some("pie".into()));
    let (ui, mut v) = github(cx, fake);
    settle(&mut v);
    let chart = chart_of(&ui, &mut v);
    assert_eq!(v.read(|cx| chart.read(cx).kind), chart::ChartKind::Line);
}

#[gpui::test]
fn new_data_that_drops_the_open_palette_gives_the_keys_back(cx: &mut TestAppContext) {
    let (ui, mut v) = stats_window(cx);
    let chart = chart_of(&ui, &mut v);
    right_click(&mut v, "contributors-stats-legend-0");
    assert!(shown(&mut v, "contributors-stats-palette-0"));
    let mut data = (*v.read(|cx| chart.read(cx).data.clone())).clone();
    data.series.remove(0);
    chart.update(&mut v, |chart, cx| {
        chart.set_data(std::sync::Arc::new(data), cx)
    });
    settle(&mut v);
    assert!(!shown(&mut v, "contributors-stats-palette-0"));
    v.simulate_keystrokes("escape");
    settle(&mut v);
    assert_eq!(ui.closed_count(), 1);
}

/// Opens the palette of the first legend entry; returns the chart.
fn open_palette_of(
    ui: &super::page_tests::Ui,
    v: &mut VisualTestContext,
) -> gpui::Entity<chart::ChartView> {
    let chart = chart_of(ui, v);
    right_click(v, "contributors-stats-legend-0");
    assert!(v.read(|cx| chart.read(cx).palette.is_some()));
    chart
}

fn palette_closed_and_escape_reaches_the_page(
    ui: &super::page_tests::Ui,
    v: &mut VisualTestContext,
    chart: &gpui::Entity<chart::ChartView>,
) {
    settle(v);
    assert!(v.read(|cx| chart.read(cx).palette.is_none()));
    assert_eq!(ui.closed_count(), 0);
    v.simulate_keystrokes("escape");
    settle(v);
    assert_eq!(ui.closed_count(), 1, "Escape reached the page");
}

#[gpui::test]
fn changing_section_by_code_closes_the_palette_and_keeps_the_keys(cx: &mut TestAppContext) {
    let (ui, mut v) = stats_window(cx);
    let view = ui.view(&mut v);
    let chart = open_palette_of(&ui, &mut v);
    view.update(&mut v, |this, cx| {
        this.set_section(super::github::Section::Leaderboard, cx)
    });
    palette_closed_and_escape_reaches_the_page(&ui, &mut v, &chart);
}

#[gpui::test]
fn another_project_closes_the_palette_of_the_one_left(cx: &mut TestAppContext) {
    let (ui, mut v) = stats_window(cx);
    let chart = open_palette_of(&ui, &mut v);
    ui.window
        .update(&mut v, |page, _, cx| {
            page.set_context(
                vec!["/work/alpha".into(), "/work/beta".into()],
                "/work/beta".into(),
                cx,
            )
        })
        .unwrap();
    settle(&mut v);
    // The focus handle of the view left is not on screen any more, so only the
    // palette is checked here (Escape is covered by the section test).
    assert!(v.read(|cx| chart.read(cx).palette.is_none()));
}

#[gpui::test]
fn another_tab_closes_the_palette(cx: &mut TestAppContext) {
    let (ui, mut v) = stats_window(cx);
    let chart = open_palette_of(&ui, &mut v);
    ui.window
        .update(&mut v, |page, _, cx| page.set_tab(super::Tab::Ticket, cx))
        .unwrap();
    settle(&mut v);
    assert!(v.read(|cx| chart.read(cx).palette.is_none()));
}

#[gpui::test]
fn hiding_the_page_closes_the_palette(cx: &mut TestAppContext) {
    let (ui, mut v) = stats_window(cx);
    let chart = open_palette_of(&ui, &mut v);
    ui.window
        .update(&mut v, |page, _, cx| page.hide(cx))
        .unwrap();
    settle(&mut v);
    assert!(v.read(|cx| chart.read(cx).palette.is_none()));
}

#[gpui::test]
fn a_long_name_is_cut_inside_its_legend_entry(cx: &mut TestAppContext) {
    let (ui, mut v) = stats_window(cx);
    let chart = chart_of(&ui, &mut v);
    let mut data = (*v.read(|cx| chart.read(cx).data.clone())).clone();
    data.series[0].name = "A very long contributor name ".repeat(8);
    chart.update(&mut v, |chart, cx| {
        chart.set_data(std::sync::Arc::new(data), cx)
    });
    settle(&mut v);
    let item = v.debug_bounds("contributors-stats-legend-0").unwrap();
    let button = v.debug_bounds("contributors-stats-legend-0-color").unwrap();
    assert!(item.size.width <= gpui::px(200.5), "{:?}", item.size);
    assert!(
        button.right() <= item.right(),
        "the color button stays inside"
    );
}

// ---- the Activity card -------------------------------------------------------------

pub(super) fn activity_views(
    ui: &super::page_tests::Ui,
    v: &mut VisualTestContext,
) -> (
    gpui::Entity<super::activity::CalendarView>,
    gpui::Entity<super::activity::RhythmView>,
) {
    let view = ui.view(v);
    v.read(|cx| {
        let this = view.read(cx);
        (
            this.activity.calendar.clone().expect("the calendar"),
            this.activity.rhythm.clone().expect("the rhythm"),
        )
    })
}

pub(super) fn day_commits(
    calendar: &gpui::Entity<super::activity::CalendarView>,
    v: &mut VisualTestContext,
    month: u32,
    day: u32,
) -> u32 {
    let date = chrono::NaiveDate::from_ymd_opt(2026, month, day).unwrap();
    v.read(|cx| {
        calendar
            .read(cx)
            .days
            .iter()
            .find(|c| c.date == date)
            .map_or(0, |c| c.counts.commits)
    })
}

#[test]
fn the_year_window_covers_53_whole_weeks_and_stays_the_same_all_day() {
    let morning = local(2026, 10, 15, 9);
    let evening = local(2026, 10, 15, 23);
    let (start, since, until) = super::activity::window(&morning);
    assert_eq!(chrono::Datelike::weekday(&start), chrono::Weekday::Sun);
    assert_eq!((morning.date_naive() - start).num_days(), 52 * 7 + 4);
    assert_eq!(
        super::activity::window(&evening),
        (start, since, until.clone())
    );
    let (_, _, next_until) = super::activity::window(&local(2026, 10, 16, 9));
    assert_ne!(next_until, until);
}

#[test]
fn a_selection_that_left_the_list_falls_back_to_the_team() {
    use super::activity::selection;
    assert_eq!(selection(Some("ana"), &["ana", "bob"]), Some("ana"));
    assert_eq!(selection(Some("zed"), &["ana", "bob"]), None);
    assert_eq!(selection(None, &["ana"]), None);
}

#[test]
fn cells_fill_every_day_and_zero_the_quiet_ones() {
    use monocode_core::contributors::model::activity::{DayCounts, PersonActivity};
    let day = |d| chrono::NaiveDate::from_ymd_opt(2026, 10, d).unwrap();
    let mut person = PersonActivity::default();
    person.days.insert(
        day(3),
        DayCounts {
            commits: 2,
            ..Default::default()
        },
    );
    let cells = super::activity::cells(Some(&person), day(1), day(5));
    assert_eq!(cells.len(), 5);
    assert_eq!(cells[2].counts.commits, 2);
    assert_eq!(cells[0].counts, DayCounts::default());
    let nobody = super::activity::cells(None, day(1), day(2));
    assert!(nobody.iter().all(|c| c.counts == DayCounts::default()));
}

#[test]
fn a_hovered_day_lists_what_happened_and_an_empty_one_nothing() {
    use crate::visuals::calendar::Active;
    use monocode_core::contributors::model::activity::DayCounts;
    let cell = super::activity::Cell {
        date: chrono::NaiveDate::from_ymd_opt(2026, 10, 14).unwrap(),
        counts: DayCounts {
            commits: 2,
            reviews: 1,
            merged: 0,
            additions: 30,
            deletions: 4,
        },
    };
    let rows = cell.details(gpui::black());
    let texts: Vec<_> = rows.iter().map(|(_, n, v)| format!("{n} {v}")).collect();
    assert_eq!(texts, ["Commits 2", "PRs reviewed 1", "Lines +30 -4"]);
    assert_eq!(cell.amount(), 3);
}

#[gpui::test]
fn the_activity_card_shows_the_pills_the_calendar_and_the_rhythm(cx: &mut TestAppContext) {
    let (ui, mut v) = stats_window(cx);
    for selector in [
        "contributors-activity",
        "contributors-activity-pills",
        "contributors-activity-pill-0",
        "contributors-activity-pill-1",
        "contributors-activity-pill-2",
        "contributors-activity-calendar",
        "contributors-activity-rhythm",
    ] {
        assert!(shown(&mut v, selector), "{selector} is drawn");
    }
    // The card sits below the chart and above the table.
    let top = |v: &mut VisualTestContext, s| v.debug_bounds(s).unwrap().origin.y;
    assert!(top(&mut v, "contributors-stats-chart") < top(&mut v, "contributors-activity"));
    assert!(top(&mut v, "contributors-activity") < top(&mut v, "contributors-stats-table"));
    let (calendar, _) = activity_views(&ui, &mut v);
    // Team: Ana on the 14th and 15th, Bob on the 13th.
    assert_eq!(day_commits(&calendar, &mut v, 10, 13), 2);
    assert_eq!(day_commits(&calendar, &mut v, 10, 14), 1);
    assert_eq!(day_commits(&calendar, &mut v, 10, 15), 1);
}

#[gpui::test]
fn the_year_is_one_read_only_load_that_leaves_the_page_state_alone(cx: &mut TestAppContext) {
    let fake = window_fake(fixture());
    let (ui, mut v) = github(cx, fake.clone());
    settle(&mut v);
    let view = ui.view(&mut v);
    let (since, _, _) = {
        let (_, since, until) = super::activity::window(&start_of_tests());
        (since, until, ())
    };
    let year: Vec<_> = fake
        .loads()
        .into_iter()
        .filter(|c| matches!(c, super::data_tests::Call::Load { since: s, .. } if *s == since))
        .collect();
    assert_eq!(year.len(), 1, "the year is read once");
    // Redraws and selections do not read it again, nor start any sync work.
    click(&mut v, "contributors-activity-pill-1");
    click(&mut v, "contributors-activity-pill-0");
    settle(&mut v);
    let again = fake
        .loads()
        .into_iter()
        .filter(|c| matches!(c, super::data_tests::Call::Load { since: s, .. } if *s == since))
        .count();
    assert_eq!(again, 1);
    assert!(fake.starts().is_empty());
    let snap = ui.snap(&mut v);
    assert!(!snap.refreshing);
    assert!(snap.model.is_some());
    assert_eq!(v.read(|cx| view.read(cx).running), 0);
    assert_eq!(
        v.read(|cx| view.read(cx).activity_note(cx)),
        "last 12 months"
    );
}

#[gpui::test]
fn a_pill_shows_one_member_and_team_shows_everyone(cx: &mut TestAppContext) {
    let (ui, mut v) = stats_window(cx);
    let (calendar, rhythm) = activity_views(&ui, &mut v);
    let total = |v: &mut VisualTestContext| -> u32 {
        v.read(|cx| rhythm.read(cx).counts.iter().flatten().sum())
    };
    assert_eq!(total(&mut v), 6);
    // Rows are Ana (4 commits in the fixture), then Bob; pill 0 is the team.
    click(&mut v, "contributors-activity-pill-2");
    assert_eq!(day_commits(&calendar, &mut v, 10, 13), 2);
    assert_eq!(day_commits(&calendar, &mut v, 10, 14), 0);
    assert_eq!(total(&mut v), 2);
    click(&mut v, "contributors-activity-pill-1");
    assert_eq!(day_commits(&calendar, &mut v, 10, 13), 0);
    assert_eq!(day_commits(&calendar, &mut v, 10, 14), 1);
    click(&mut v, "contributors-activity-pill-0");
    assert_eq!(day_commits(&calendar, &mut v, 10, 13), 2);
}

#[gpui::test]
fn the_member_tint_is_their_color_and_the_team_tint_is_the_accent(cx: &mut TestAppContext) {
    let (ui, mut v) = stats_window(cx);
    let (calendar, _) = activity_views(&ui, &mut v);
    let team = v.read(|cx| calendar.read(cx).tint);
    assert_eq!(team, crate::theme::accent_color());
    click(&mut v, "contributors-activity-pill-1");
    let view = ui.view(&mut v);
    let (member, expected) = v.read(|cx| {
        let model = view.read(cx).model.clone().unwrap();
        let row = &model.stats.rows[0];
        (calendar.read(cx).tint, super::ui::hex_color(&row.color))
    });
    assert_eq!(member, expected);
}

#[gpui::test]
fn pressing_the_chosen_pill_again_keeps_the_selection(cx: &mut TestAppContext) {
    let (ui, mut v) = stats_window(cx);
    click(&mut v, "contributors-activity-pill-2");
    let view = ui.view(&mut v);
    let selected = v.read(|cx| view.read(cx).activity.selected.clone());
    assert!(selected.is_some());
    click(&mut v, "contributors-activity-pill-2");
    // Pressing the chosen pill again keeps it: there is always a selection.
    assert_eq!(
        v.read(|cx| view.read(cx).activity.selected.clone()),
        selected
    );
}

/// Table, calendar, rhythm and pills builds so far, on this test's thread.
fn builds() -> [usize; 4] {
    let [calendar, rhythm, pills] = super::activity::BUILDS.with(|n| n.get());
    [
        super::stats::TABLE_RENDERS.with(|n| n.get()),
        calendar,
        rhythm,
        pills,
    ]
}

/// Moves the pointer off without the forced redraw `settle` does, which
/// would bypass the view caches these tests are about.
fn away(v: &mut VisualTestContext) {
    v.simulate_mouse_move(
        gpui::point(gpui::px(1.), gpui::px(1.)),
        None,
        Default::default(),
    );
    v.run_until_parked();
}

fn glide(v: &mut VisualTestContext, selector: &'static str, xs: &[f32]) {
    let bounds = v.debug_bounds(selector).unwrap();
    for x in xs {
        let at = gpui::point(
            bounds.origin.x + bounds.size.width * *x,
            bounds.origin.y + bounds.size.height * 0.5,
        );
        v.simulate_mouse_move(at, None, Default::default());
        v.run_until_parked();
    }
}

#[gpui::test]
fn a_chart_hover_keeps_the_table_the_calendar_the_rhythm_and_the_pills(cx: &mut TestAppContext) {
    let (_, mut v) = stats_window(cx);
    let before = builds();
    glide(&mut v, "contributors-stats-chart", &[0.05, 0.3, 0.6, 0.95]);
    away(&mut v);
    assert_eq!(builds(), before, "nothing but the chart was built");
}

#[gpui::test]
fn the_calendar_and_the_rhythm_repaint_only_when_the_hovered_cell_changes(cx: &mut TestAppContext) {
    let (ui, mut v) = stats_window(cx);
    let view = ui.view(&mut v);
    let (calendar, rhythm) = activity_views(&ui, &mut v);
    let page = Rc::new(std::cell::Cell::new(0));
    let count = page.clone();
    let _watch = v.update(|_, cx| cx.observe(&view, move |_, _| count.set(count.get() + 1)));
    let before = builds();
    // The calendar's right edge holds the latest days: a few pixels apart in
    // one cell is one repaint, a move to the next cell another.
    let area = v.debug_bounds("contributors-activity-calendar").unwrap();
    let at = |dx: f32, dy: f32| {
        gpui::point(
            area.origin.x + gpui::px(28. + 6. * 15. + dx),
            area.origin.y + gpui::px(18. + dy),
        )
    };
    for point in [at(1., 1.), at(2., 2.), at(3., 3.)] {
        v.simulate_mouse_move(point, None, Default::default());
        v.run_until_parked();
    }
    let inside = builds()[1] - before[1];
    assert_eq!(inside, 1, "one cell, one repaint of the calendar");
    assert!(v.read(|cx| calendar.read(cx).hover).is_some());
    v.simulate_mouse_move(at(16., 1.), None, Default::default());
    v.run_until_parked();
    assert_eq!(builds()[1] - before[1], 2, "the next cell repaints again");
    // The rhythm, the same way.
    let area = v.debug_bounds("contributors-activity-rhythm").unwrap();
    let at = |dx: f32| {
        gpui::point(
            area.origin.x + gpui::px(28. + dx),
            area.origin.y + gpui::px(18. + 2.),
        )
    };
    let mid = builds();
    for dx in [1., 2., 3.] {
        v.simulate_mouse_move(at(dx), None, Default::default());
        v.run_until_parked();
    }
    assert_eq!(builds()[2] - mid[2], 1);
    assert!(v.read(|cx| rhythm.read(cx).hover).is_some());
    away(&mut v);
    assert!(v.read(|cx| calendar.read(cx).hover.is_none()));
    assert_eq!(builds()[0], before[0], "the table was kept");
    assert_eq!(builds()[3], before[3], "the pills were kept");
    assert_eq!(page.get(), 0, "the page was not asked to redraw");
}

#[gpui::test]
fn a_settled_calendar_and_rhythm_draw_their_ink_as_one_image(cx: &mut TestAppContext) {
    let (ui, mut v) = stats_window(cx);
    let (calendar, rhythm) = activity_views(&ui, &mut v);
    let (a, b) = v.read(|cx| {
        (
            calendar.read(cx).sheet.borrow().is_some(),
            rhythm.read(cx).sheet.borrow().is_some(),
        )
    });
    assert!(a && b, "the image was made ({a}, {b})");
    // Another person is another image, not a stale one.
    let before = v.read(|cx| calendar.read(cx).sheet.borrow().as_ref().map(|s| s.key));
    click(&mut v, "contributors-activity-pill-1");
    let after = v.read(|cx| calendar.read(cx).sheet.borrow().as_ref().map(|s| s.key));
    assert_ne!(before, after);
}

fn epoch_of(date: chrono::NaiveDate) -> i64 {
    monocode_core::contributors::model::period::midnight(date).timestamp()
}

#[test]
fn the_coverage_note_says_what_the_store_lacks() {
    use super::activity::coverage_note;
    let start_day = chrono::NaiveDate::from_ymd_opt(2025, 10, 12).unwrap();
    let at = |days: i64| epoch_of(start_day + chrono::Duration::days(days));
    let (start, now) = (at(0), at(368));
    // A quiet start with everything synced is not a note.
    assert_eq!(coverage_note(&[], start, now), None);
    // Synced only from day 200: history starts there.
    assert_eq!(
        coverage_note(&[(start, at(200) - 1)], start, now).as_deref(),
        Some("History starts Apr 30, 2026")
    );
    // A hole in the middle.
    assert_eq!(
        coverage_note(&[(at(30), at(60) - 1)], start, now).as_deref(),
        Some("Not synced Nov 11, 2025 - Dec 10, 2025")
    );
    // Both, and more than one hole.
    assert_eq!(
        coverage_note(
            &[
                (start, at(100) - 1),
                (at(150), at(160) - 1),
                (at(200), at(210) - 1)
            ],
            start,
            now
        )
        .as_deref(),
        Some("History starts Jan 20, 2026 (+2 more)")
    );
    // Nothing synced at all.
    assert_eq!(
        coverage_note(&[(start, now)], start, now).as_deref(),
        Some("nothing synced in this range")
    );
    // A hole under a day is not worth a line.
    assert_eq!(coverage_note(&[(at(5), at(5) + 3600)], start, now), None);
}

#[gpui::test]
fn a_synced_year_with_a_hole_shows_the_coverage_note_in_the_card(cx: &mut TestAppContext) {
    let fake = window_fake(fixture());
    let (start, _, _) = super::activity::window(&start_of_tests());
    let mut hole = fake.with(|s| s.load.default.clone()).unwrap();
    hole.missing = vec![(
        epoch_of(start),
        epoch_of(start + chrono::Duration::days(200)) - 1,
    )];
    fake.with(|s| s.year_load = Some(Ok(hole)));
    let (ui, mut v) = github(cx, fake);
    settle(&mut v);
    let view = ui.view(&mut v);
    assert_eq!(
        v.read(|cx| view.read(cx).activity_note(cx)),
        format!(
            "History starts {}",
            (start + chrono::Duration::days(200)).format("%b %-d, %Y")
        )
    );
}

fn year_reads(fake: &Fake) -> usize {
    let (_, since, _) = super::activity::window(&start_of_tests());
    fake.loads()
        .into_iter()
        .filter(|c| matches!(c, super::data_tests::Call::Load { since: s, .. } if *s == since))
        .count()
}

fn calendar_total(ui: &super::page_tests::Ui, v: &mut VisualTestContext) -> u32 {
    let (calendar, _) = activity_views(ui, v);
    v.read(|cx| {
        calendar
            .read(cx)
            .days
            .iter()
            .map(|c| c.counts.count())
            .sum()
    })
}

#[gpui::test]
fn another_branch_never_shows_the_year_of_the_first_and_a_failed_read_shows_none(
    cx: &mut TestAppContext,
) {
    let fake = window_fake(fixture());
    let (ui, mut v) = github(cx, fake.clone());
    settle(&mut v);
    let view = ui.view(&mut v);
    assert!(calendar_total(&ui, &mut v) > 0);
    assert_eq!(year_reads(&fake), 1);
    // The new branch's year cannot be read: the old branch's must not stay.
    fake.with(|s| s.year_load = Some(Err("boom".into())));
    v.update(|_, cx| view.update(cx, |this, cx| this.set_branch(Some("dev".into()), cx)));
    settle(&mut v);
    for _ in 0..3 {
        v.update(|_, cx| view.update(cx, |_, cx| cx.notify()));
        settle(&mut v);
    }
    assert_eq!(calendar_total(&ui, &mut v), 0);
    assert_eq!(
        v.read(|cx| view.read(cx).activity_note(cx)),
        "could not be read"
    );
    assert_eq!(
        year_reads(&fake),
        2,
        "the failing read is not repeated per draw"
    );
}

#[gpui::test]
fn while_another_branch_loads_the_card_is_empty_and_says_so(cx: &mut TestAppContext) {
    let fake = window_fake(fixture());
    let (ui, mut v) = github(cx, fake);
    settle(&mut v);
    let view = ui.view(&mut v);
    assert!(calendar_total(&ui, &mut v) > 0);
    v.update(|_, cx| {
        view.update(cx, |this, cx| {
            this.manual = true;
            this.set_branch(Some("dev".into()), cx)
        })
    });
    settle(&mut v);
    assert_eq!(calendar_total(&ui, &mut v), 0);
    assert_eq!(v.read(|cx| view.read(cx).activity_note(cx)), "loading");
}

#[gpui::test]
fn a_job_ending_reads_the_year_again_and_keeps_the_old_one_until_it_lands(cx: &mut TestAppContext) {
    use super::{data_tests::event, jobs::JobsEvent};
    use monocode_core::contributors::jobs::JobState;
    let fake = window_fake(fixture());
    let (ui, mut v) = github(cx, fake.clone());
    settle(&mut v);
    let view = ui.view(&mut v);
    let before = calendar_total(&ui, &mut v);
    assert!(before > 0);
    assert_eq!(year_reads(&fake), 1);
    // The sync stored more: the next read of the year is empty on purpose.
    let mut empty = fake.with(|s| s.load.default.clone()).unwrap();
    empty.raw.commits.clear();
    empty.raw.prs.clear();
    fake.with(|s| s.year_load = Some(Ok(empty)));
    let ended = JobsEvent::Ended(event("/work/alpha", (0, 1), JobState::Done));
    v.update(|_, cx| {
        view.update(cx, |this, cx| {
            this.job_ended(&ended, cx);
            this.push_activity(cx);
        })
    });
    // Nothing has landed yet: the card keeps the year it had, it does not blank.
    assert_eq!(calendar_total(&ui, &mut v), before);
    assert_ne!(
        v.read(|cx| view.read(cx).activity_note(cx)),
        "loading",
        "the note does not flash while the new year is read"
    );
    settle(&mut v);
    assert_eq!(year_reads(&fake), 2, "the year is read again after a job");
    assert_eq!(calendar_total(&ui, &mut v), 0, "and the new read is shown");
}

#[gpui::test]
fn a_dim_change_redraws_the_cached_calendar_rhythm_and_pills(cx: &mut TestAppContext) {
    let (ui, mut v) = stats_window(cx);
    let (calendar, rhythm) = activity_views(&ui, &mut v);
    let view = ui.view(&mut v);
    let pills = v.read(|cx| view.read(cx).activity.pills.clone().expect("the pills"));
    let before = builds();
    // The page dims (a model on its way) and settles again.
    v.update(|_, cx| {
        calendar.update(cx, |c, cx| c.set_dim(true, cx));
        rhythm.update(cx, |r, cx| r.set_dim(true, cx));
        pills.update(cx, |p, cx| p.set_dim(true, cx));
    });
    v.run_until_parked();
    v.update(|window, cx| window.draw(cx).clear(cx));
    let dimmed = builds();
    assert!(dimmed[1] > before[1] && dimmed[2] > before[2] && dimmed[3] > before[3]);
    v.update(|_, cx| {
        calendar.update(cx, |c, cx| c.set_dim(true, cx));
    });
    v.update(|window, cx| window.draw(cx).clear(cx));
    assert_eq!(builds(), dimmed, "the same dim is not a redraw");
    v.update(|_, cx| {
        calendar.update(cx, |c, cx| c.set_dim(false, cx));
    });
    v.update(|window, cx| window.draw(cx).clear(cx));
    assert!(builds()[1] > dimmed[1], "and the end of the dim is");
}
