//! StatsTab.test.ts: the rules behind the Stats section (what the cards, the
//! chart, the legend and the table say) as tests of the pure view functions,
//! and the interactions (expand a row, Ask, hover tooltips) in a real window
//! over the fake backend.
use super::{
    GithubEvent,
    data_tests::{Fake, start_of_tests},
    page_tests::{click, github, settle, shown},
    stats::{
        self, Tone, axis_labels, bucket_lines, change, chart_peak, chart_summary, identities_chip,
        kpi_texts, legend_parts, row_label, segment_heights, share_label, versus,
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
fn now() -> DateTime<Local> {
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
fn fixture() -> RawData {
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

fn model_of(raw: RawData, kind: PeriodKind) -> ContributorsModel {
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
    })
}

fn month() -> ContributorsModel {
    model_of(fixture(), PeriodKind::Month)
}

/// `count` authors; author N has `count - N` commits spread over October.
fn many_authors_raw(count: usize) -> RawData {
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
fn a_bar_is_split_in_legend_order_and_the_tallest_fills_the_chart() {
    let m = month();
    let stats = &m.stats;
    let peak = 2.0;
    let heights: Vec<f32> = stats
        .buckets
        .iter()
        .flat_map(|b| segment_heights(stats, b, peak))
        .map(|(_, h)| h)
        .collect();
    assert!(!heights.is_empty());
    assert!(heights.iter().copied().fold(0., f32::max) <= 164.0);
    assert!(heights.iter().all(|h| *h > 0.0));
    // Oct 13 is Bob's alone: one segment, the whole 164px.
    let oct13 = stats
        .buckets
        .iter()
        .find(|b| b.start == "2026-10-13")
        .unwrap();
    let segments = segment_heights(stats, oct13, peak);
    assert_eq!(segments.len(), 1);
    assert_eq!(segments[0].0, "bob@example.com");
    assert!((segments[0].1 - 164.0).abs() < 0.001);
    // A bucket with several people stacks them in legend order.
    let order: Vec<&str> = stats.legend.iter().map(|l| l.person_id.as_str()).collect();
    for bucket in &stats.buckets {
        let ids: Vec<usize> = segment_heights(stats, bucket, peak)
            .iter()
            .map(|(id, _)| order.iter().position(|o| o == id).unwrap())
            .collect();
        assert!(ids.windows(2).all(|w| w[0] < w[1]), "{ids:?}");
    }
}

#[test]
fn the_x_axis_labels_the_first_and_last_bucket_and_few_between() {
    let m = month();
    let labels = axis_labels(&m.stats);
    let buckets = &m.stats.buckets;
    assert_eq!(labels[0].1, buckets[0].short);
    assert_eq!(labels[labels.len() - 1].1, buckets[buckets.len() - 1].short);
    assert_eq!(labels[0].0, 0);
    assert_eq!(labels[labels.len() - 1].0, buckets.len() - 1);
    assert!((2..=10).contains(&labels.len()));
    let week = model_of(fixture(), PeriodKind::Days7);
    let first = &axis_labels(&week.stats)[0].1;
    assert!(
        first.len() >= 5 && first.len() <= 6 && first.chars().nth(3) == Some(' '),
        "{first}"
    );
}

#[test]
fn the_y_axis_maximum_is_the_busiest_bucket() {
    let m = month();
    assert_eq!(chart_peak(&m.stats), 2);
    assert_eq!(
        chart_peak(&model_of(empty_raw(), PeriodKind::Month).stats),
        0
    );
}

#[test]
fn a_bucket_tooltip_names_the_day_and_every_person_with_a_count() {
    let m = month();
    let i = m
        .stats
        .buckets
        .iter()
        .position(|b| b.start == "2026-10-13")
        .unwrap();
    assert_eq!(
        bucket_lines(&m.stats, i),
        ["Oct 13, 2026: 2 commits", "Bob: 2"]
    );
    let j = m
        .stats
        .buckets
        .iter()
        .position(|b| b.start == "2026-10-14")
        .unwrap();
    assert_eq!(
        bucket_lines(&m.stats, j),
        ["Oct 14, 2026: 1 commit", "Ana Sol: 1"]
    );
    let empty = m
        .stats
        .buckets
        .iter()
        .position(|b| b.start == "2026-10-02")
        .unwrap();
    assert_eq!(bucket_lines(&m.stats, empty), ["Oct 2, 2026: 0 commits"]);
}

// ---- the legend ---------------------------------------------------------------

#[test]
fn the_legend_lists_everyone_when_there_are_eight_or_fewer() {
    let m = many_authors(8);
    let (shown, rest) = legend_parts(&m.stats);
    assert_eq!(shown.len(), 8);
    assert!(rest.is_empty());
}

#[test]
fn the_legend_shows_the_top_eight_by_commits_then_folds_the_rest() {
    let m = many_authors(12);
    let (shown, rest) = legend_parts(&m.stats);
    let names: Vec<&str> = shown.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(
        names,
        (1..=8).map(|i| format!("Person {i}")).collect::<Vec<_>>()
    );
    let rest: Vec<&str> = rest.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(rest, ["Person 9", "Person 10", "Person 11", "Person 12"]);
    assert_eq!(
        stats::more_chip(&legend_parts(&m.stats).1),
        (
            "+4 more".into(),
            "Person 9, Person 10, Person 11, Person 12".into()
        )
    );
}

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
    assert_eq!(row_label(ana), "Ana Sol, 2 identities");
    let bob = m.stats.rows.iter().find(|r| r.name == "Bob").unwrap();
    assert_eq!(identities_chip(bob), None);
    assert_eq!(row_label(bob), "Bob, 1 identity");
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

fn stats_window(cx: &mut TestAppContext) -> (super::page_tests::Ui, VisualTestContext) {
    stats_window_of(cx, fixture())
}

fn stats_window_of(
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

fn hover(v: &mut VisualTestContext, selector: &'static str) {
    let bounds = v
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("nothing drawn as {selector}"));
    v.simulate_mouse_move(bounds.center(), None, Default::default());
    settle(v);
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

#[gpui::test]
fn the_chart_has_a_column_per_bucket_and_a_segment_per_person_in_it(cx: &mut TestAppContext) {
    let (ui, mut v) = stats_window(cx);
    let view = ui.view(&mut v);
    let (buckets, ids) = v.read(|cx| {
        let model = view.read(cx).snapshot(cx).model.expect("a model");
        (
            model
                .stats
                .buckets
                .iter()
                .map(|b| (b.start.clone(), b.by_person.clone()))
                .collect::<Vec<_>>(),
            model
                .stats
                .legend
                .iter()
                .map(|l| l.person_id.clone())
                .collect::<Vec<_>>(),
        )
    });
    // Oct 1 to Oct 15: the days that have happened.
    assert_eq!(buckets.len(), 15);
    let leak = |text: String| -> &'static str { Box::leak(text.into_boxed_str()) };
    for (i, (_, people)) in buckets.iter().enumerate() {
        assert!(
            shown(&mut v, leak(format!("contributors-stats-bucket-{i}"))),
            "bucket {i}"
        );
        for id in &ids {
            let present = shown(&mut v, leak(format!("contributors-stats-seg-{i}-{id}")));
            assert_eq!(
                present,
                people.get(id).copied().unwrap_or(0) > 0,
                "bucket {i} {id}"
            );
        }
    }
    assert!(!shown(
        &mut v,
        leak(format!("contributors-stats-bucket-{}", buckets.len()))
    ));
}

#[gpui::test]
fn the_chart_labels_the_y_maximum_and_only_the_chosen_x_ticks(cx: &mut TestAppContext) {
    let (ui, mut v) = stats_window(cx);
    assert!(shown(&mut v, "contributors-stats-ymax"));
    let view = ui.view(&mut v);
    let (count, ticks) = v.read(|cx| {
        let model = view.read(cx).snapshot(cx).model.expect("a model");
        (
            model.stats.buckets.len(),
            axis_labels(&model.stats)
                .into_iter()
                .map(|(i, _)| i)
                .collect::<Vec<_>>(),
        )
    });
    let leak = |text: String| -> &'static str { Box::leak(text.into_boxed_str()) };
    for i in 0..count {
        assert_eq!(
            shown(&mut v, leak(format!("contributors-stats-axis-{i}"))),
            ticks.contains(&i),
            "tick {i}"
        );
    }
}

#[gpui::test]
fn a_row_opens_to_its_identities_on_click_and_closes_on_the_next(cx: &mut TestAppContext) {
    let (_, mut v) = stats_window(cx);
    assert!(!shown(&mut v, "contributors-stats-identities-0"));
    click(&mut v, "contributors-stats-row-0");
    assert!(shown(&mut v, "contributors-stats-identities-0"));
    assert!(!shown(&mut v, "contributors-stats-identities-1"));
    click(&mut v, "contributors-stats-row-0");
    assert!(!shown(&mut v, "contributors-stats-identities-0"));
}

#[gpui::test]
fn the_name_button_opens_the_row_once_and_one_row_is_open_at_a_time(cx: &mut TestAppContext) {
    let (_, mut v) = stats_window(cx);
    click(&mut v, "contributors-stats-name-0");
    assert!(shown(&mut v, "contributors-stats-identities-0"));
    click(&mut v, "contributors-stats-row-1");
    assert!(!shown(&mut v, "contributors-stats-identities-0"));
    assert!(shown(&mut v, "contributors-stats-identities-1"));
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
    assert!(!shown(&mut v, "contributors-stats-identities-0"));
}

#[gpui::test]
fn hovering_a_bar_shows_the_bucket_tooltip_and_a_segment_its_own(cx: &mut TestAppContext) {
    let (ui, mut v) = stats_window(cx);
    let view = ui.view(&mut v);
    let (oct13, bob) = v.read(|cx| {
        let model = view.read(cx).snapshot(cx).model.expect("a model");
        (
            model
                .stats
                .buckets
                .iter()
                .position(|b| b.start == "2026-10-13")
                .unwrap(),
            "bob@example.com".to_string(),
        )
    });
    let leak = |text: String| -> &'static str { Box::leak(text.into_boxed_str()) };
    assert!(!shown(&mut v, "contributors-stats-tip-bucket"));
    // The gap above a bar is still its bucket (Oct 14 is half as tall as Oct 13).
    let oct14 = oct13 + 1;
    let column = v
        .debug_bounds(leak(format!("contributors-stats-bucket-{oct14}")))
        .unwrap();
    v.simulate_mouse_move(
        column.origin + gpui::point(column.size.width / 2., gpui::px(10.)),
        None,
        Default::default(),
    );
    settle(&mut v);
    assert!(shown(&mut v, "contributors-stats-tip-bucket"));
    assert!(!shown(&mut v, "contributors-stats-tip-segment"));
    hover(
        &mut v,
        leak(format!("contributors-stats-seg-{oct13}-{bob}")),
    );
    assert!(shown(&mut v, "contributors-stats-tip-segment"));
    assert!(!shown(&mut v, "contributors-stats-tip-bucket"));
    move_away(&mut v);
    assert!(!shown(&mut v, "contributors-stats-tip-bucket"));
    assert!(!shown(&mut v, "contributors-stats-tip-segment"));
}

#[gpui::test]
fn the_overflow_chip_appears_past_eight_people_and_lists_the_hidden_names_on_hover(
    cx: &mut TestAppContext,
) {
    let (_, mut v) = stats_window(cx);
    assert!(!shown(&mut v, "contributors-stats-legend-more"));
    let (_, mut v) = stats_window_of(cx, many_authors_raw(12));
    assert!(shown(&mut v, "contributors-stats-legend-more"));
    assert!(!shown(&mut v, "contributors-stats-tip-more"));
    hover(&mut v, "contributors-stats-legend-more");
    assert!(shown(&mut v, "contributors-stats-tip-more"));
    move_away(&mut v);
    assert!(!shown(&mut v, "contributors-stats-tip-more"));
}
