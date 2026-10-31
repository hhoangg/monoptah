//! The Stats section of the GitHub tab (`StatsTab.tsx`): four KPI cards with
//! their change against the previous period, the commit chart (see
//! `chart.rs`) and the contributors table, whose rows open a member's detail
//! page (`detail.rs`), with a per-row Ask button. What the cards and the chart
//! say is computed by the pure functions below (unit tested); the `GithubView` methods only lay it
//! out. The chart and the table are entities of their own, so hovering the
//! chart repaints the chart alone and the table is drawn from a cache.
use super::{
    chart::{self, ChartBucket, ChartData, Series},
    github::GithubView,
    ui::{self, AvatarSize},
};
use crate::{
    right_panel::{DIFF_ADD, DIFF_DEL, tabular},
    theme::*,
};
use chrono::{DateTime, Local};
use gpui::{
    AnyElement, Context, Div, ElementId, Entity, Render, Role, SharedString, Stateful,
    StyleRefinement, Styled, Subscription, WeakEntity, Window, div, prelude::*, px, relative, rgb,
};
use monocode_core::contributors::model::{
    compact::format_compact,
    format::{format_delta, relative_time},
    period::PeriodKind,
    pipeline::ContributorsModel,
    snapshot::SnapshotScope,
    stats::{Bucket, BucketGranularity, Stats, StatsRow},
};
use std::sync::Arc;

// Table columns (the contributor column takes the rest).
const W_RANK: f32 = 36.;
const W_COMMITS: f32 = 76.;
const W_LINES: f32 = 116.;
const W_FILES: f32 = 64.;
const W_AI: f32 = 104.;
const W_SHARE: f32 = 160.;
const W_LAST: f32 = 104.;
const W_ASK: f32 = 64.;
const TABLE_MIN: f32 = 880.;
// The table is drawn from a cache, which needs its height up front: every row
// has a fixed height.
const HEAD_H: f32 = 34.;
const ROW_H: f32 = 41.;

// ---- the pure view functions ---------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Tone {
    Up,
    Down,
    Flat,
}

/// A signed change line: arrow, text and tone together so colour is never the
/// only cue.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct ChangeView {
    pub tone: Tone,
    /// The arrow and the text: "▲ +18%".
    pub text: String,
    /// "vs Sep 2026".
    pub suffix: Option<String>,
}

pub(super) fn change(
    value: Option<f64>,
    text: String,
    suffix: Option<String>,
) -> Option<ChangeView> {
    let value = value?;
    let (tone, arrow) = if value > 0.0 {
        (Tone::Up, "▲ ")
    } else if value < 0.0 {
        (Tone::Down, "▼ ")
    } else {
        (Tone::Flat, "")
    };
    Some(ChangeView {
        tone,
        text: format!("{arrow}{text}"),
        suffix,
    })
}

/// The change of a card that moved by a fraction: "▲ +18%".
pub(super) fn fraction_change(delta: Option<f64>, versus: &Option<String>) -> Option<ChangeView> {
    delta.and_then(|d| change(Some(d), format_delta(d), versus.clone()))
}

/// The change of a card that moved by a count: "▲ +3".
pub(super) fn count_change(delta: Option<i64>, versus: &Option<String>) -> Option<ChangeView> {
    delta.and_then(|d| {
        let text = if d > 0 {
            format!("+{d}")
        } else {
            d.to_string()
        };
        change(Some(d as f64), text, versus.clone())
    })
}

/// What the deltas compare with; `None` without a previous period, so no card
/// shows a comparison.
pub(super) fn versus(model: &ContributorsModel) -> Option<String> {
    let previous = model.previous_period.as_ref()?;
    Some(if model.period.kind == PeriodKind::Days7 {
        "vs prev 7d".into()
    } else {
        format!("vs {}", previous.label)
    })
}

pub(super) struct CountCard {
    pub value: String,
    pub change: Option<ChangeView>,
}
pub(super) struct ContributorsCard {
    pub value: String,
    /// "from 3 identities".
    pub from: String,
    pub change: Option<ChangeView>,
}
pub(super) struct LinesCard {
    pub added: String,
    pub deleted: String,
    pub change: Option<ChangeView>,
}
pub(super) struct KpiTexts {
    pub commits: CountCard,
    pub contributors: ContributorsCard,
    pub lines: LinesCard,
    pub prs: CountCard,
}

pub(super) fn kpi_texts(model: &ContributorsModel) -> KpiTexts {
    let kpis = &model.stats.kpis;
    let versus = versus(model);
    let identities = kpis.contributors.identities;
    KpiTexts {
        commits: CountCard {
            value: kpis.commits.value.to_string(),
            change: fraction_change(kpis.commits.delta, &versus),
        },
        contributors: ContributorsCard {
            value: kpis.contributors.value.to_string(),
            from: format!(
                "from {identities} {}",
                if identities == 1 {
                    "identity"
                } else {
                    "identities"
                }
            ),
            change: change(
                (kpis.contributors.newcomers > 0).then_some(1.0),
                format!("{} new", kpis.contributors.newcomers),
                None,
            ),
        },
        lines: LinesCard {
            added: format!("+{}", format_compact(kpis.lines.added as f64)),
            deleted: format!("-{}", format_compact(kpis.lines.deleted as f64)),
            change: fraction_change(kpis.lines.delta, &versus),
        },
        prs: CountCard {
            value: kpis.prs_merged.value.to_string(),
            change: count_change(kpis.prs_merged.delta, &versus),
        },
    }
}

pub(super) fn unit(granularity: BucketGranularity) -> &'static str {
    match granularity {
        BucketGranularity::Day => "day",
        BucketGranularity::Week => "week",
        BucketGranularity::Month => "month",
        BucketGranularity::Year => "year",
    }
}

fn bucket_total(bucket: &Bucket) -> u64 {
    bucket.by_person.values().sum()
}

/// The text a screen reader gets for the chart.
pub(super) fn chart_summary(stats: &Stats) -> String {
    let buckets = chart_buckets(stats);
    let totals: Vec<f64> = stats
        .buckets
        .iter()
        .map(|b| bucket_total(b) as f64)
        .collect();
    chart::summary(
        "Commit activity",
        unit(stats.granularity),
        "commits",
        &buckets,
        &totals,
    )
}

pub(super) fn chart_buckets(stats: &Stats) -> Vec<ChartBucket> {
    stats
        .buckets
        .iter()
        .map(|b| ChartBucket {
            label: b.label.clone(),
            short: b.short.clone(),
        })
        .collect()
}

/// The chart's input: one series of commits per bucket for each person in the
/// legend, the busiest five on their own and the rest as "Others".
pub(super) fn chart_data(stats: &Stats) -> ChartData {
    let series = stats
        .legend
        .iter()
        .map(|entry| Series {
            id: entry.person_id.clone(),
            name: entry.name.clone(),
            color: ui::hex_color(&entry.color),
            values: stats
                .buckets
                .iter()
                .map(|b| b.by_person.get(&entry.person_id).copied().unwrap_or(0) as f64)
                .collect(),
        })
        .collect();
    ChartData {
        buckets: chart_buckets(stats),
        series: chart::top_series(series),
        summary: chart_summary(stats),
    }
}

fn identity_count(n: usize) -> String {
    format!("{n} {}", if n == 1 { "identity" } else { "identities" })
}

/// "2 identities", only for a person merged from several.
pub(super) fn identities_chip(row: &StatsRow) -> Option<String> {
    (row.identities.len() > 1).then(|| identity_count(row.identities.len()))
}

/// The accessible name of a row's button, the keyboard route to its detail page.
pub(super) fn row_label(row: &StatsRow) -> String {
    format!("Open details for {}", row.name)
}

pub(super) fn share_label(row: &StatsRow) -> String {
    // `Math.round`: halves go up.
    format!("{}%", (row.share_pct + 0.5).floor() as i64)
}

/// The height of the whole table: the head and a fixed height per row.
pub(super) fn table_height(rows: usize) -> f32 {
    HEAD_H + rows as f32 * ROW_H
}

// ---- state -------------------------------------------------------------------------

/// The entities of the section, made when it is first drawn and kept: the
/// chart repaints itself on hover and the table is drawn from a cache.
#[derive(Default)]
pub(super) struct StatsUi {
    pub(super) chart: Option<Entity<chart::ChartView>>,
    pub(super) table: Option<Entity<StatsTable>>,
    /// The model the chart was last given (its address).
    pushed: Option<usize>,
    subscriptions: Vec<Subscription>,
}

// ---- drawing -----------------------------------------------------------------------

fn tone_color(tone: Tone) -> gpui::Hsla {
    match tone {
        Tone::Up => rgb(DIFF_ADD).into(),
        Tone::Down => rgb(DIFF_DEL).into(),
        Tone::Flat => content(0.45),
    }
}

pub(super) fn card() -> Div {
    div()
        .overflow_hidden()
        .rounded_xl()
        .border_1()
        .border_color(content(0.1))
        .bg(content(0.03))
}

pub(super) fn card_title(title: &'static str, note: String) -> Div {
    div()
        .px(px(12.))
        .py(px(10.))
        .border_b_1()
        .border_color(content(0.1))
        .text_size(px(13.))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .child(title)
        .child(
            div()
                .ml(px(6.))
                .font_weight(gpui::FontWeight::NORMAL)
                .text_color(content(0.5))
                .child(note),
        )
        .flex()
        .items_baseline()
}

fn change_line(change: Option<&ChangeView>) -> Div {
    let line = div().mt(px(2.)).h(px(16.)).text_size(px(11.));
    match change {
        None => line,
        Some(change) => line
            .flex()
            .text_color(tone_color(change.tone))
            .child(change.text.clone())
            .when_some(change.suffix.clone(), |d, suffix| {
                d.child(div().ml(px(4.)).text_color(content(0.45)).child(suffix))
            }),
    }
}

pub(super) fn kpi(
    id: &'static str,
    label: &'static str,
    value: Div,
    change: Option<&ChangeView>,
) -> Div {
    div()
        .debug_selector(move || id.to_string())
        .flex_1()
        .min_w(px(170.))
        .rounded_xl()
        .border_1()
        .border_color(content(0.1))
        .bg(content(0.03))
        .px(px(14.))
        .py(px(12.))
        .child(
            div()
                .text_size(px(12.))
                .text_color(content(0.5))
                .child(label),
        )
        .child(
            value
                .mt(px(2.))
                .text_size(px(20.))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .font_features(tabular()),
        )
        .child(change_line(change))
}

impl GithubView {
    pub(super) fn stats_body(
        &mut self,
        model: &Arc<ContributorsModel>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let stats = &model.stats;
        let (chart, table) = self.stats_views(model, cx);
        let table = if stats.rows.is_empty() {
            div()
                .px(px(12.))
                .py(px(24.))
                .flex()
                .justify_center()
                .text_size(px(12.))
                .text_color(content(0.5))
                .child("No contributors in this period.")
                .into_any_element()
        } else {
            if self.a11y {
                // Replayed frames carry no accessibility nodes: draw it afresh.
                table.into_any_element()
            } else {
                let height = table.read(cx).height();
                table
                    .cached(StyleRefinement::default().w_full().h(px(height)))
                    .into_any_element()
            }
        };
        div()
            .debug_selector(|| "contributors-body-stats".into())
            .flex()
            .flex_col()
            .gap(px(14.))
            .child(self.kpi_cards(model))
            .child(
                card()
                    .child(card_title(
                        "Commit activity",
                        format!("per {}", unit(stats.granularity)),
                    ))
                    .child(chart),
            )
            .child(self.activity_card(model, true, cx))
            .child(
                card()
                    .child(card_title("Contributors", "click a row for details".into()))
                    .child(table),
            )
            .into_any_element()
    }

    /// The chart and the table, made on first use. They learn about a new
    /// model (or minute, or chart type) from `push_stats`, which runs when this
    /// view notifies, never while it draws.
    fn stats_views(
        &mut self,
        model: &Arc<ContributorsModel>,
        cx: &mut Context<Self>,
    ) -> (Entity<chart::ChartView>, Entity<StatsTable>) {
        if let (Some(chart), Some(table)) = (&self.stats_ui.chart, &self.stats_ui.table) {
            return (chart.clone(), table.clone());
        }
        let data = Arc::new(chart_data(&model.stats));
        let kind = self.chart_kind(cx);
        let chart = cx.new(|cx| chart::ChartView::new("contributors-stats", data, kind, cx));
        let github = cx.weak_entity();
        let (shown, now) = (model.clone(), (self.clock)());
        let dim = self.page_dim(cx);
        let table = cx.new(|_| StatsTable::new(github, shown, now, dim));
        let me = cx.entity();
        self.stats_ui.subscriptions = vec![
            cx.subscribe(&chart, |this, _, event: &chart::ChartEvent, cx| {
                this.chart_event(event, cx)
            }),
            cx.observe(&me, |this, _, cx| this.push_stats(cx)),
        ];
        self.stats_ui.pushed = Some(Arc::as_ptr(model) as usize);
        self.stats_ui.chart = Some(chart.clone());
        self.stats_ui.table = Some(table.clone());
        (chart, table)
    }

    /// Hands the chart and the table what changed. Each notifies itself only
    /// when its input differs, so most calls cost nothing.
    pub(super) fn push_stats(&mut self, cx: &mut Context<Self>) {
        let (Some(chart), Some(table)) = (self.stats_ui.chart.clone(), self.stats_ui.table.clone())
        else {
            return;
        };
        let Some(model) = self.model.clone() else {
            return;
        };
        let at = Arc::as_ptr(&model) as usize;
        if self.stats_ui.pushed != Some(at) {
            self.stats_ui.pushed = Some(at);
            let data = Arc::new(chart_data(&model.stats));
            chart.update(cx, |chart, cx| chart.set_data(data, cx));
        }
        let kind = self.chart_kind(cx);
        chart.update(cx, |chart, cx| chart.set_kind(kind, cx));
        let now = (self.clock)();
        let dim = self.page_dim(cx);
        table.update(cx, |table, cx| table.set_input(model, now, dim, cx));
        if self.section == super::github::Section::Stats {
            self.ensure_activity(cx);
        }
        self.push_activity(cx);
        self.push_detail(cx);
    }

    /// Whether the body is drawn dimmed: stale data or a model on its way.
    pub(super) fn page_dim(&self, cx: &gpui::App) -> bool {
        let snap = self.snapshot(cx);
        snap.stale || snap.model_pending
    }

    pub(super) fn chart_event(&mut self, event: &chart::ChartEvent, cx: &mut Context<Self>) {
        match event {
            chart::ChartEvent::Kind(kind) => self.set_chart_kind(*kind, cx),
            chart::ChartEvent::Color { id, hex } => self.set_person_color(id, hex, cx),
        }
    }

    /// Two pairs that sit side by side when the window is wide and stack as a
    /// 2x2 grid when it is not.
    fn kpi_cards(&self, model: &ContributorsModel) -> Div {
        let kpis = kpi_texts(model);
        let pair = |first: Div, second: Div| {
            div()
                .flex_1()
                .min_w(px(350.))
                .flex()
                .gap(px(10.))
                .child(first)
                .child(second)
        };
        div()
            .flex()
            .flex_wrap()
            .gap(px(10.))
            .child(pair(
                kpi(
                    "contributors-stats-kpi-commits",
                    "Commits",
                    div().child(kpis.commits.value.clone()),
                    kpis.commits.change.as_ref(),
                ),
                kpi(
                    "contributors-stats-kpi-contributors",
                    "Contributors",
                    div()
                        .flex()
                        .items_baseline()
                        .child(kpis.contributors.value.clone())
                        .child(
                            div()
                                .ml(px(4.))
                                .text_size(px(12.))
                                .font_weight(gpui::FontWeight::NORMAL)
                                .text_color(content(0.5))
                                .child(kpis.contributors.from.clone()),
                        ),
                    kpis.contributors.change.as_ref(),
                ),
            ))
            .child(pair(
                kpi(
                    "contributors-stats-kpi-lines",
                    "Lines changed",
                    div()
                        .flex()
                        .gap(px(6.))
                        .child(
                            div()
                                .text_color(rgb(DIFF_ADD))
                                .child(kpis.lines.added.clone()),
                        )
                        .child(
                            div()
                                .text_color(rgb(DIFF_DEL))
                                .child(kpis.lines.deleted.clone()),
                        ),
                    kpis.lines.change.as_ref(),
                ),
                kpi(
                    "contributors-stats-kpi-prs",
                    "PRs merged",
                    div().child(kpis.prs.value.clone()),
                    kpis.prs.change.as_ref(),
                ),
            ))
    }
}

// ---- the table ---------------------------------------------------------------------

/// The contributors table: an entity so that it is drawn from a cache while
/// the chart above it is hovered.
pub(super) struct StatsTable {
    github: WeakEntity<GithubView>,
    model: Arc<ContributorsModel>,
    now: DateTime<Local>,
    /// The page is dimmed (stale data or a model on its way). The cached rows
    /// bake that in when they paint, so a change must redraw them.
    pub(super) dim: bool,
}

fn minute(at: &DateTime<Local>) -> i64 {
    at.timestamp() / 60
}

impl StatsTable {
    fn new(
        github: WeakEntity<GithubView>,
        model: Arc<ContributorsModel>,
        now: DateTime<Local>,
        dim: bool,
    ) -> Self {
        Self {
            github,
            model,
            now,
            dim,
        }
    }

    /// A new model, or a new minute for "Last active".
    fn set_input(
        &mut self,
        model: Arc<ContributorsModel>,
        now: DateTime<Local>,
        dim: bool,
        cx: &mut Context<Self>,
    ) {
        if Arc::ptr_eq(&self.model, &model) && minute(&self.now) == minute(&now) && self.dim == dim
        {
            return;
        }
        self.dim = dim;
        self.model = model;
        self.now = now;
        cx.notify();
    }

    /// The height the cache lays the table out at.
    fn height(&self) -> f32 {
        table_height(self.model.stats.rows.len())
    }

    /// Opens the detail page of a person and hands the keys to the page, so
    /// Escape closes the detail even though the clicked row is gone.
    fn open(&mut self, person_id: String, window: &mut Window, cx: &mut Context<Self>) {
        let _ = self.github.update(cx, |github, cx| {
            github.open_detail(person_id, cx);
            window.focus(&github.focus, cx);
        });
    }

    fn ask(&mut self, person_id: String, cx: &mut Context<Self>) {
        let _ = self.github.update(cx, |github, cx| {
            github.open_ask(SnapshotScope::Person { person_id }, cx);
        });
    }

    fn contributor_row(&self, index: usize, row: &StatsRow, cx: &mut Context<Self>) -> AnyElement {
        let group = SharedString::from(format!("contributors-stats-group-{index}"));
        let (on_row, on_name, person) = (
            row.person_id.clone(),
            row.person_id.clone(),
            row.person_id.clone(),
        );
        row_div(("contributors-stats-row", index))
            .debug_selector(move || format!("contributors-stats-row-{index}"))
            .group(group.clone())
            .role(Role::Row)
            .h(px(ROW_H))
            .flex()
            .border_t_1()
            .border_color(content(0.1))
            .cursor_pointer()
            .hover(|s| s.bg(content(0.03)))
            .on_click(cx.listener(move |this, _, window, cx| this.open(on_row.clone(), window, cx)))
            .child(
                cell(Some(index))
                    .w(px(W_RANK))
                    .text_color(content(0.5))
                    .child((index + 1).to_string()),
            )
            .child(
                cell(None).flex_1().min_w(px(200.)).justify_start().child(
                    div()
                        .id(("contributors-stats-name", index))
                        .debug_selector(move || format!("contributors-stats-name-{index}"))
                        .role(Role::Button)
                        .aria_label(row_label(row))
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open(on_name.clone(), window, cx);
                            cx.stop_propagation();
                        }))
                        .child(ui::avatar(&row.name, &row.color, AvatarSize::Sm))
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .child(row.name.clone()),
                        )
                        .children(identities_chip(row).map(|chip| {
                            div()
                                .rounded_full()
                                .border_1()
                                .border_dashed()
                                .border_color(content(0.2))
                                .px(px(6.))
                                .text_size(px(11.))
                                .text_color(content(0.5))
                                .child(chip)
                        })),
                ),
            )
            .child(
                cell(Some(index))
                    .w(px(W_COMMITS))
                    .justify_end()
                    .child(row.commits.to_string()),
            )
            .child(
                cell(None)
                    .w(px(W_LINES))
                    .justify_end()
                    .gap(px(6.))
                    .font_features(tabular())
                    .child(
                        div()
                            .text_color(rgb(DIFF_ADD))
                            .child(format!("+{}", format_compact(row.added as f64))),
                    )
                    .child(
                        div()
                            .text_color(rgb(DIFF_DEL))
                            .child(format!("-{}", format_compact(row.deleted as f64))),
                    ),
            )
            .child(
                cell(Some(index))
                    .w(px(W_FILES))
                    .justify_end()
                    .child(row.files.to_string()),
            )
            .child(
                cell(Some(index))
                    .w(px(W_AI))
                    .justify_end()
                    .child(format!("{}%", row.ai_assisted_pct)),
            )
            .child(
                cell(None)
                    .w(px(W_SHARE))
                    .justify_start()
                    .gap(px(8.))
                    .child(
                        div()
                            .w(px(96.))
                            .h(px(6.))
                            .flex_shrink_0()
                            .overflow_hidden()
                            .rounded_full()
                            .bg(content(0.1))
                            .child(
                                div()
                                    .h_full()
                                    .w(relative((row.share_pct.min(100.) / 100.) as f32))
                                    .rounded_full()
                                    .bg(ui::hex_color(&row.color)),
                            ),
                    )
                    .child(
                        div()
                            .w(px(32.))
                            .text_size(px(11.))
                            .text_color(content(0.5))
                            .font_features(tabular())
                            .child(share_label(row)),
                    ),
            )
            .child(
                cell(None)
                    .w(px(W_LAST))
                    .justify_end()
                    .whitespace_nowrap()
                    .text_color(content(0.5))
                    .child(
                        row.last_active
                            .as_deref()
                            .map(|at| relative_time(at, &self.now))
                            .unwrap_or_default(),
                    ),
            )
            .child(
                cell(None).w(px(W_ASK)).justify_end().child(
                    div()
                        .id(("contributors-stats-ask", index))
                        .debug_selector(move || format!("contributors-stats-ask-{index}"))
                        .role(Role::Button)
                        .aria_label(format!("Ask Agent about {}", row.name))
                        .rounded_full()
                        .border_1()
                        .border_color(content(0.1))
                        .px(px(8.))
                        .py(px(2.))
                        .text_size(px(11.))
                        .text_color(accent_color())
                        .opacity(0.)
                        .group_hover(group, |s| s.opacity(1.))
                        .cursor_pointer()
                        .hover(|s| s.bg(content(0.05)))
                        .child("Ask")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.ask(person.clone(), cx);
                            cx.stop_propagation();
                        })),
                ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
thread_local! {
    /// How many times the table was built, for the tests that prove a cache is kept.
    pub(super) static TABLE_RENDERS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

impl Render for StatsTable {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        TABLE_RENDERS.with(|n| n.set(n.get() + 1));
        let model = self.model.clone();
        let head = |text: &'static str| cell(None).child(text).text_color(content(0.5));
        div()
            .id("contributors-stats-table-scroll")
            .role(Role::Region)
            .aria_label("Contributors table")
            .w_full()
            .h(px(self.height()))
            .overflow_x_scroll()
            .restrict_scroll_to_axis()
            .child(
                div()
                    .id("contributors-stats-table")
                    .debug_selector(|| "contributors-stats-table".into())
                    .role(Role::Table)
                    .min_w(px(TABLE_MIN))
                    .text_size(px(13.))
                    .child(
                        div()
                            .id("contributors-stats-head")
                            .role(Role::Row)
                            .h(px(HEAD_H))
                            .flex()
                            .text_size(px(12.))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child(head("#").w(px(W_RANK)))
                            .child(head("Contributor").flex_1().min_w(px(200.)).justify_start())
                            .child(head("Commits").w(px(W_COMMITS)).justify_end())
                            .child(head("+ / -").w(px(W_LINES)).justify_end())
                            .child(head("Files").w(px(W_FILES)).justify_end())
                            .child(head("AI-assisted").w(px(W_AI)).justify_end())
                            .child(head("Share").w(px(W_SHARE)).justify_start())
                            .child(head("Last active").w(px(W_LAST)).justify_end())
                            .child(cell(None).w(px(W_ASK))),
                    )
                    .children(
                        model
                            .stats
                            .rows
                            .iter()
                            .enumerate()
                            .map(|(i, row)| self.contributor_row(i, row, cx)),
                    ),
            )
    }
}

fn row_div(id: impl Into<ElementId>) -> Stateful<Div> {
    div().id(id)
}

/// One table cell: padded, vertically centred, tabular numbers for the number columns.
fn cell(number: Option<usize>) -> Div {
    div()
        .flex_shrink_0()
        .px(px(12.))
        .py(px(8.))
        .flex()
        .items_center()
        .when(number.is_some(), |d| d.font_features(tabular()))
}
