//! The Stats section of the GitHub tab (`StatsTab.tsx`): four KPI cards with
//! their change against the previous period, the stacked commit chart with its
//! legend, and the contributors table with expandable identities and a per-row
//! Ask button. What the cards and the chart say is computed by the pure
//! functions below (unit tested); the `GithubView` methods only draw it.
use super::{
    github::GithubView,
    ui::{self, AvatarSize},
};
use crate::{
    popover::{Align, Anchor, AnchorSlot, Side, popover},
    right_panel::{DIFF_ADD, DIFF_DEL, tabular},
    theme::*,
};
use gpui::{
    AnyElement, Context, Div, ElementId, Role, SharedString, Stateful, div, prelude::*, px,
    relative, rgb,
};
use monocode_core::contributors::model::{
    axis::axis_ticks,
    compact::format_compact,
    format::{format_delta, relative_time},
    period::PeriodKind,
    pipeline::ContributorsModel,
    snapshot::SnapshotScope,
    stats::{Bucket, BucketGranularity, LegendEntry, Stats, StatsRow},
};
use std::sync::Arc;

const CHART_H: f32 = 180.;
const CHART_PAD: f32 = 8.;
/// The bars fill this much of their slot, so neighbours never touch.
const BAR_SHARE: f32 = 0.7;
const LEGEND_LIMIT: usize = 8;

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
            change: kpis
                .commits
                .delta
                .and_then(|d| change(Some(d), format_delta(d), versus.clone())),
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
            change: kpis
                .lines
                .delta
                .and_then(|d| change(Some(d), format_delta(d), versus.clone())),
        },
        prs: CountCard {
            value: kpis.prs_merged.value.to_string(),
            change: kpis.prs_merged.delta.and_then(|d| {
                let text = if d > 0 {
                    format!("+{d}")
                } else {
                    d.to_string()
                };
                change(Some(d as f64), text, versus.clone())
            }),
        },
    }
}

fn unit(granularity: BucketGranularity) -> &'static str {
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

fn commits_text(n: u64) -> String {
    format!("{n} {}", if n == 1 { "commit" } else { "commits" })
}

/// The commits of the busiest bucket: the y-axis maximum.
pub(super) fn chart_peak(stats: &Stats) -> u64 {
    stats.buckets.iter().map(bucket_total).max().unwrap_or(0)
}

/// The text a screen reader gets for the chart.
pub(super) fn chart_summary(stats: &Stats) -> String {
    let unit = unit(stats.granularity);
    let count = stats.buckets.len();
    let all: u64 = stats.buckets.iter().map(bucket_total).sum();
    let mut busiest: Option<&Bucket> = None;
    for bucket in &stats.buckets {
        if busiest.is_none_or(|best| bucket_total(bucket) > bucket_total(best)) {
            busiest = Some(bucket);
        }
    }
    match busiest {
        Some(best) if all > 0 => format!(
            "Commit activity per {unit}: {all} commits over {count} {unit}s, busiest {unit} {} with {}.",
            best.label,
            bucket_total(best)
        ),
        _ => format!("Commit activity per {unit}: no commits in {count} {unit}s."),
    }
}

fn name_of<'a>(stats: &'a Stats, person_id: &'a str) -> &'a str {
    stats
        .legend
        .iter()
        .find(|entry| entry.person_id == person_id)
        .map_or(person_id, |entry| entry.name.as_str())
}

/// The tooltip of a bar: the bucket and its total, then one line per person
/// in legend order.
pub(super) fn bucket_lines(stats: &Stats, index: usize) -> Vec<String> {
    let bucket = &stats.buckets[index];
    let mut lines = vec![format!(
        "{}: {}",
        bucket.label,
        commits_text(bucket_total(bucket))
    )];
    lines.extend(stats.legend.iter().filter_map(|entry| {
        let count = *bucket.by_person.get(&entry.person_id)?;
        (count > 0).then(|| format!("{}: {count}", entry.name))
    }));
    lines
}

/// One person's segment: "Ana Sol: 3".
fn segment_line(stats: &Stats, person_id: &str, count: u64) -> String {
    format!("{}: {count}", name_of(stats, person_id))
}

/// The segments of one bar, bottom to top: legend order, only people with
/// commits, each as tall as its share of the busiest bucket (`peak`).
pub(super) fn segment_heights(stats: &Stats, bucket: &Bucket, peak: f64) -> Vec<(String, f32)> {
    let usable = f64::from(CHART_H - CHART_PAD * 2.);
    let max = peak.max(1.);
    stats
        .legend
        .iter()
        .filter_map(|entry| {
            let count = *bucket.by_person.get(&entry.person_id)?;
            (count > 0).then(|| {
                (
                    entry.person_id.clone(),
                    (count as f64 / max * usable) as f32,
                )
            })
        })
        .collect()
}

/// Which buckets get an x label, and the label.
pub(super) fn axis_labels(stats: &Stats) -> Vec<(usize, &str)> {
    axis_ticks(stats.buckets.len())
        .into_iter()
        .map(|i| (i, stats.buckets[i].short.as_str()))
        .collect()
}

/// The busiest contributors, then the rest: a big team must not push the chart
/// down.
pub(super) fn legend_parts(stats: &Stats) -> (Vec<&LegendEntry>, Vec<&LegendEntry>) {
    let commits = |entry: &LegendEntry| {
        stats
            .rows
            .iter()
            .find(|row| row.person_id == entry.person_id)
            .map_or(0, |row| row.commits)
    };
    let mut ranked: Vec<&LegendEntry> = stats.legend.iter().collect();
    // A stable sort keeps the legend order among equals.
    ranked.sort_by_key(|entry| std::cmp::Reverse(commits(entry)));
    let rest = ranked.split_off(ranked.len().min(LEGEND_LIMIT));
    (ranked, rest)
}

/// The "+4 more" chip and the names it stands for.
pub(super) fn more_chip(rest: &[&LegendEntry]) -> (String, String) {
    (
        format!("+{} more", rest.len()),
        rest.iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>()
            .join(", "),
    )
}

fn identity_count(n: usize) -> String {
    format!("{n} {}", if n == 1 { "identity" } else { "identities" })
}

/// "2 identities", only for a person merged from several.
pub(super) fn identities_chip(row: &StatsRow) -> Option<String> {
    (row.identities.len() > 1).then(|| identity_count(row.identities.len()))
}

/// The accessible name of a row's expand button.
pub(super) fn row_label(row: &StatsRow) -> String {
    format!("{}, {}", row.name, identity_count(row.identities.len()))
}

pub(super) fn share_label(row: &StatsRow) -> String {
    // `Math.round`: halves go up.
    format!("{}%", (row.share_pct + 0.5).floor() as i64)
}

// ---- state -------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
enum ChartHover {
    /// A bar slot (the gap above a bar counts).
    Bucket(usize),
    /// One segment of a bar.
    Segment(usize, String),
}

/// What the section remembers between frames.
#[derive(Default)]
pub(super) struct StatsUi {
    /// The person whose identities are shown.
    open: Option<String>,
    hover: Option<ChartHover>,
    more_hover: bool,
    /// Where the hovered bar (or the chip) is, for its tooltip.
    anchor: AnchorSlot,
}

// ---- drawing -----------------------------------------------------------------------

fn tone_color(tone: Tone) -> gpui::Hsla {
    match tone {
        Tone::Up => rgb(DIFF_ADD).into(),
        Tone::Down => rgb(DIFF_DEL).into(),
        Tone::Flat => content(0.45),
    }
}

fn card() -> Div {
    div()
        .overflow_hidden()
        .rounded_xl()
        .border_1()
        .border_color(content(0.1))
        .bg(content(0.03))
}

fn card_title(title: &'static str, note: String) -> Div {
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

fn kpi(id: &'static str, label: &'static str, value: Div, change: Option<&ChangeView>) -> Div {
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
        let now = (self.clock)();
        let stats = &model.stats;
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
                        format!("per {}, stacked by contributor", unit(stats.granularity)),
                    ))
                    .child(self.legend(stats, cx))
                    .child(self.chart(stats, cx)),
            )
            .child(
                card()
                    .child(card_title(
                        "Contributors",
                        "click a row to see its identities".into(),
                    ))
                    .child(self.table(stats, &now, cx)),
            )
            .into_any_element()
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

    fn legend(&mut self, stats: &Stats, cx: &mut Context<Self>) -> Div {
        let (shown, rest) = legend_parts(stats);
        let chip = (!rest.is_empty()).then(|| {
            let (text, names) = more_chip(&rest);
            let anchor = self.stats_ui.anchor.clone();
            let hovered = self.stats_ui.more_hover;
            div()
                .id("contributors-stats-legend-more")
                .debug_selector(|| "contributors-stats-legend-more".into())
                .relative()
                .rounded_full()
                .bg(content(0.08))
                .px(px(6.))
                .text_color(content(0.6))
                .aria_label(format!("{text}: {names}"))
                .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                    this.stats_ui.more_hover = *hovered;
                    cx.notify();
                }))
                .child(text)
                .when(hovered, |d| {
                    d.child(anchor.probe()).child(
                        popover("contributors-stats-more-tip", Anchor::Slot(anchor.clone()))
                            .side(Side::Bottom)
                            .align(Align::Start)
                            .gap(6.)
                            .padding(8.)
                            .max_width(280.)
                            .without_animation()
                            .allow_outside_press()
                            .child(
                                div()
                                    .id("contributors-stats-tip-more")
                                    .debug_selector(|| "contributors-stats-tip-more".into())
                                    .role(Role::Tooltip)
                                    .text_size(px(12.))
                                    .line_height(px(16.))
                                    .child(names),
                            ),
                    )
                })
        });
        div()
            .debug_selector(|| "contributors-stats-legend".into())
            .px(px(12.))
            .pt(px(8.))
            .flex()
            .flex_wrap()
            .gap_x(px(12.))
            .gap_y(px(4.))
            .text_size(px(12.))
            .text_color(content(0.5))
            .children(shown.into_iter().map(|entry| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        div()
                            .size(px(8.))
                            .rounded(px(2.))
                            .bg(ui::hex_color(&entry.color)),
                    )
                    .child(entry.name.clone())
            }))
            .children(chip)
    }

    fn chart(&mut self, stats: &Stats, cx: &mut Context<Self>) -> Div {
        let count = stats.buckets.len();
        let peak = chart_peak(stats);
        let hover = self.stats_ui.hover.clone();
        let anchor = self.stats_ui.anchor.clone();
        let columns = stats.buckets.iter().enumerate().map(|(i, bucket)| {
            let bucket_hovered = matches!(&hover, Some(ChartHover::Bucket(at) | ChartHover::Segment(at, _)) if *at == i);
            let segments = segment_heights(stats, bucket, peak as f64);
            let tip = match &hover {
                Some(ChartHover::Bucket(at)) if *at == i => {
                    Some(("contributors-stats-tip-bucket", bucket_lines(stats, i)))
                }
                Some(ChartHover::Segment(at, id)) if *at == i => bucket
                    .by_person
                    .get(id)
                    .map(|n| ("contributors-stats-tip-segment", vec![segment_line(stats, id, *n)])),
                _ => None,
            };
            let bar = div()
                .w(relative(BAR_SHARE))
                .flex()
                .flex_col_reverse()
                .children(segments.into_iter().map(|(id, height)| {
                    let color = stats
                        .legend
                        .iter()
                        .find(|entry| entry.person_id == id)
                        .map_or("", |entry| entry.color.as_str());
                    let seg_id = id.clone();
                    let selector = format!("contributors-stats-seg-{i}-{id}");
                    div()
                        .id(SharedString::from(selector.clone()))
                        .debug_selector(move || selector.clone())
                        .w_full()
                        .h(px(height))
                        .rounded(px(1.))
                        .bg(ui::hex_color(color))
                        .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                            let now = Some(ChartHover::Segment(i, seg_id.clone()));
                            if *hovered {
                                this.stats_ui.hover = now;
                            } else if this.stats_ui.hover == now {
                                // Still inside the bar: back to the bucket.
                                this.stats_ui.hover = Some(ChartHover::Bucket(i));
                            }
                            cx.notify();
                        }))
                }));
            div()
                .id(("contributors-stats-bucket", i))
                .debug_selector(move || format!("contributors-stats-bucket-{i}"))
                .relative()
                .flex_1()
                .h_full()
                .pb(px(CHART_PAD))
                .flex()
                .justify_center()
                .items_end()
                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                    let hover = &mut this.stats_ui.hover;
                    if *hovered {
                        if !matches!(hover, Some(ChartHover::Segment(at, _)) if *at == i) {
                            *hover = Some(ChartHover::Bucket(i));
                        }
                    } else if matches!(hover, Some(ChartHover::Bucket(at) | ChartHover::Segment(at, _)) if *at == i) {
                        *hover = None;
                    }
                    cx.notify();
                }))
                .child(bar)
                .when(bucket_hovered, |d| d.child(anchor.probe()))
                .when_some(tip, |d, (selector, lines)| {
                    d.child(
                        popover("contributors-stats-tip", Anchor::Slot(anchor.clone()))
                            .side(Side::Top)
                            .align(Align::Center)
                            .gap(6.)
                            .padding(8.)
                            .max_width(280.)
                            .without_animation()
                            .allow_outside_press()
                            .child(
                                div()
                                    .id(selector)
                                    .debug_selector(move || selector.into())
                                    .role(Role::Tooltip)
                                    .flex()
                                    .flex_col()
                                    .text_size(px(12.))
                                    .line_height(px(16.))
                                    .children(lines.into_iter().enumerate().map(|(n, line)| {
                                        div()
                                            .when(n == 0, |d| {
                                                d.font_weight(gpui::FontWeight::MEDIUM)
                                            })
                                            .when(n > 0, |d| d.text_color(content(0.7)))
                                            .child(line)
                                    })),
                            ),
                    )
                })
        });
        let last = count.saturating_sub(1);
        let axis = axis_labels(stats)
            .into_iter()
            .map(|(i, text)| {
                // The first label starts at the left edge, the last ends at the
                // right one, the others are centred on their slot.
                let slot = div()
                    .debug_selector(move || format!("contributors-stats-axis-{i}"))
                    .absolute()
                    .top_0()
                    .whitespace_nowrap()
                    .child(text.to_string());
                if i == 0 {
                    slot.left_0()
                } else if i == last {
                    slot.right_0()
                } else {
                    slot.left(relative(i as f32 / count as f32))
                        .w(relative(1. / count as f32))
                        .flex()
                        .justify_center()
                }
            })
            .collect::<Vec<_>>();
        div()
            .px(px(12.))
            .py(px(8.))
            .child(
                div()
                    .id("contributors-stats-chart")
                    .debug_selector(|| "contributors-stats-chart".into())
                    .role(Role::Image)
                    .aria_label(chart_summary(stats))
                    .relative()
                    .h(px(CHART_H))
                    .w_full()
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .right_0()
                            .bottom(px(CHART_PAD))
                            .h(px(1.))
                            .bg(content(0.15)),
                    )
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .right_0()
                            .bottom_0()
                            .flex()
                            .children(columns),
                    )
                    .child(
                        div()
                            .debug_selector(|| "contributors-stats-ymax".into())
                            .absolute()
                            .top_0()
                            .left_0()
                            .text_size(px(10.))
                            .text_color(content(0.4))
                            .font_features(tabular())
                            .child(peak.to_string()),
                    ),
            )
            .child(
                div()
                    .relative()
                    .mt(px(4.))
                    .h(px(16.))
                    .text_size(px(10.))
                    .text_color(content(0.45))
                    .children(axis),
            )
    }

    fn table(
        &mut self,
        stats: &Stats,
        now: &chrono::DateTime<chrono::Local>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if stats.rows.is_empty() {
            return div()
                .px(px(12.))
                .py(px(24.))
                .flex()
                .justify_center()
                .text_size(px(12.))
                .text_color(content(0.5))
                .child("No contributors in this period.")
                .into_any_element();
        }
        let head = |text: &'static str| cell(None).child(text).text_color(content(0.5));
        div()
            .id("contributors-stats-table-scroll")
            .role(Role::Region)
            .aria_label("Contributors table")
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
                        stats
                            .rows
                            .iter()
                            .enumerate()
                            .map(|(i, row)| self.contributor_row(i, row, now, cx)),
                    ),
            )
            .into_any_element()
    }

    fn contributor_row(
        &self,
        index: usize,
        row: &StatsRow,
        now: &chrono::DateTime<chrono::Local>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let expanded = self.stats_ui.open.as_deref() == Some(row.person_id.as_str());
        let group = SharedString::from(format!("contributors-stats-group-{index}"));
        let toggle = |id: String| {
            move |this: &mut Self, cx: &mut Context<Self>| {
                this.stats_ui.open = if this.stats_ui.open.as_deref() == Some(id.as_str()) {
                    None
                } else {
                    Some(id.clone())
                };
                cx.notify();
            }
        };
        let on_row = toggle(row.person_id.clone());
        let on_name = toggle(row.person_id.clone());
        let person = row.person_id.clone();
        let identities = expanded.then(|| {
            div()
                .debug_selector(move || format!("contributors-stats-identities-{index}"))
                .flex()
                .bg(content(0.03))
                .child(div().w(px(W_RANK)).flex_shrink_0())
                .child(
                    div()
                        .flex_1()
                        .px(px(12.))
                        .py(px(8.))
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .text_size(px(12.))
                        .text_color(content(0.5))
                        .font_family(crate::fonts::mono())
                        .when(row.identities.is_empty(), |d| {
                            d.child("No commit identities")
                        })
                        .children(row.identities.iter().map(|id| div().child(id.clone()))),
                )
        });
        div()
            .child(
                row_div(("contributors-stats-row", index))
                    .debug_selector(move || format!("contributors-stats-row-{index}"))
                    .group(group.clone())
                    .role(Role::Row)
                    .flex()
                    .border_t_1()
                    .border_color(content(0.1))
                    .cursor_pointer()
                    .hover(|s| s.bg(content(0.03)))
                    .on_click(cx.listener(move |this, _, _, cx| on_row(this, cx)))
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
                                .aria_expanded(expanded)
                                .min_w_0()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    on_name(this, cx);
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
                                    .map(|at| relative_time(at, now))
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
                                    this.open_ask(
                                        SnapshotScope::Person {
                                            person_id: person.clone(),
                                        },
                                        cx,
                                    );
                                    cx.stop_propagation();
                                })),
                        ),
                    ),
            )
            .children(identities)
            .into_any_element()
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
