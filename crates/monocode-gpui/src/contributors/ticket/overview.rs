//! What the Ticket tab shows above the leaderboard: the stat cards with their
//! change against the previous period, the points trend chart (the shared
//! `chart.rs`, with the chart type every other chart uses) and the member by
//! level heatmap. What they say is computed by the pure functions at the top
//! (unit tested); `TicketView` methods lay it out.
use super::{
    fmt, plural,
    state::{Comparison, TicketView},
};
use crate::{
    contributors::{
        backend::ColorOverrides,
        chart::{self, ChartBucket, ChartData, ChartView, Series},
        detail::section,
        stats::{ChangeView, change, kpi},
        ui::{self, Tip},
    },
    right_panel::tabular,
    theme::*,
    visuals::dither::{CELL, Grid, inked},
};
use gpui::{
    AnyElement, Context, Div, Entity, IntoElement, Role, SharedString, Stateful, Subscription,
    canvas, div, prelude::*, px,
};
use monocode_core::contributors::{
    model::palette::person_color_map,
    tickets::{
        board::Board,
        report::{MatrixCell, MatrixRow, MemberTrend, TileStats, percent_change},
        tickets::{LEVELS, Level},
    },
};
use std::sync::Arc;

/// Selector prefix of the trend chart.
pub(super) const TREND: &str = "ticket-trend";

// ---- the pure view functions ---------------------------------------------------

/// One stat card: the value, a line under it, and how it moved.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct StatCard {
    pub id: &'static str,
    /// The selector of the card.
    pub selector: &'static str,
    pub label: &'static str,
    pub value: String,
    /// "3 of 4".
    pub sub: Option<String>,
    /// A warning that stays visible: "1 not scored".
    pub flag: Option<String>,
    pub change: Option<ChangeView>,
}

fn percent_text(value: f64) -> String {
    format!("{}{}%", if value > 0. { "+" } else { "" }, value as i64)
}

/// A move of a share, in percentage points: "+5 pp", "0 pp". "pts" would read
/// as ticket points on these pages.
pub(super) fn pp_text(value: i64) -> String {
    format!("{}{} pp", if value > 0 { "+" } else { "" }, value)
}

/// The four cards of a period. `comparison` decides whether they say how they
/// moved; with the previous period missing, none does.
pub(super) fn stat_cards(stats: &TileStats, comparison: &Comparison) -> Vec<StatCard> {
    let before = match comparison {
        Comparison::Ready { stats, .. } => Some(stats),
        _ => None,
    };
    let versus = comparison.versus();
    let moved = |now: f64, then: Option<f64>| {
        let value = percent_change(now, then?)?;
        change(Some(value), percent_text(value), versus.clone())
    };
    let unscored = stats.delivered.saturating_sub(stats.scored);
    let coverage = before
        .filter(|b| b.delivered > 0)
        .map(|b| stats.coverage_percent - b.coverage_percent)
        .and_then(|d| change(Some(d as f64), pp_text(d), versus.clone()));
    vec![
        StatCard {
            id: "delivered",
            selector: "ticket-stat-delivered",
            label: "Delivered tickets",
            value: stats.delivered.to_string(),
            sub: None,
            flag: None,
            change: moved(stats.delivered as f64, before.map(|b| b.delivered as f64)),
        },
        StatCard {
            id: "points",
            selector: "ticket-stat-points",
            label: "Team points",
            value: fmt(stats.points),
            sub: Some("scored tickets only".into()),
            flag: None,
            change: moved(stats.points, before.map(|b| b.points)),
        },
        StatCard {
            id: "scored",
            selector: "ticket-stat-scored",
            label: "Scored",
            value: if stats.delivered > 0 {
                format!("{}%", stats.coverage_percent)
            } else {
                "-".into()
            },
            sub: Some(format!("{} of {}", stats.scored, stats.delivered)),
            flag: (unscored > 0).then(|| format!("{unscored} not scored")),
            change: coverage,
        },
        StatCard {
            id: "median",
            selector: "ticket-stat-median",
            label: "Median points per person",
            value: stats.median.map_or_else(|| "-".into(), fmt),
            sub: Some("among scored work".into()),
            flag: None,
            change: stats
                .median
                .and_then(|now| moved(now, before.and_then(|b| b.median))),
        },
    ]
}

impl Comparison {
    /// "vs Sep 2026", or "vs the same days of Sep 2026".
    pub(super) fn versus(&self) -> Option<String> {
        match self {
            Self::Ready { against, .. } => Some(format!("vs {against}")),
            _ => None,
        }
    }

    /// Why the cards do not say how they moved, when that is something to
    /// explain: the previous period is not in the store.
    pub(super) fn note(&self) -> Option<&str> {
        match self {
            Self::Missing { note } => Some(note),
            _ => None,
        }
    }
}

/// The colors of the members of a board: the palette, spread so that no two
/// share one, then the ones the user picked.
pub(super) fn member_colors(board: &Board, picked: &ColorOverrides) -> ColorOverrides {
    let ids: Vec<&str> = board.members.iter().map(|m| m.id.as_str()).collect();
    let mut colors = person_color_map(&ids);
    for (id, hex) in picked {
        if let Some(own) = colors.get_mut(id) {
            *own = hex;
        }
    }
    colors
}

/// The trend chart's input: one series of points per member, the busiest five
/// on their own and the rest as "Others".
pub(super) fn trend_data(trend: &MemberTrend, colors: &ColorOverrides) -> ChartData {
    let buckets: Vec<ChartBucket> = trend
        .buckets
        .iter()
        .map(|b| ChartBucket {
            label: b.title.clone(),
            short: b.label.clone(),
        })
        .collect();
    let totals: Vec<f64> = trend.buckets.iter().map(|b| b.points).collect();
    let series = trend
        .members
        .iter()
        .map(|member| Series {
            id: member.id.clone(),
            name: member.name.clone(),
            color: ui::hex_color(colors.get(&member.id).copied().unwrap_or_else(|| {
                monocode_core::contributors::model::palette::color_for_person(&member.id)
            })),
            values: member.points.clone(),
        })
        .collect();
    ChartData {
        summary: chart::summary("Points", trend.unit.as_str(), "points", &buckets, &totals),
        series: chart::top_series(series),
        buckets,
    }
}

/// The ink of a heatmap cell, 0 to 1: no ink for an empty cell, then five steps.
pub(super) fn heat_tone(shade: Option<usize>) -> f32 {
    shade.map_or(0., |s| (s.min(4) + 1) as f32 / 5.)
}

/// What a heatmap cell says on hover and to a screen reader.
pub(super) fn heat_label(name: &str, level: Level, cell: &MatrixCell, unassigned: bool) -> String {
    if cell.delivered == 0 {
        return format!("{name}, {}: no tickets", level.as_str());
    }
    if unassigned {
        return format!(
            "{name}, {}: {}",
            level.as_str(),
            plural(cell.delivered as f64, "ticket")
        );
    }
    format!(
        "{name}, {}: {}, {} points",
        level.as_str(),
        plural(cell.delivered as f64, "ticket"),
        fmt(cell.points)
    )
}

// ---- state ------------------------------------------------------------------------

/// The trend chart, made when the board is first drawn and kept: it repaints
/// itself on hover, and learns about new data from `push_board`.
#[derive(Default)]
pub(super) struct OverviewUi {
    pub(super) chart: Option<Entity<ChartView>>,
    /// The heatmap, the leaderboard and the member's tickets, drawn from a cache.
    pub(super) parts: [Option<Entity<super::part::Part>>; 3],
    /// What the chart was last given: the board it came from and the colors.
    pushed: Option<(u64, u64)>,
    _subscriptions: Vec<Subscription>,
}

impl TicketView {
    /// The colors of the members now: the palette and what the user picked.
    pub(super) fn colors(&self) -> ColorOverrides {
        member_colors(&self.derived.board, &self.colors)
    }

    fn trend_chart(&mut self, cx: &mut Context<Self>) -> Entity<ChartView> {
        if let Some(chart) = &self.overview.chart {
            return chart.clone();
        }
        let data = Arc::new(trend_data(&self.derived.trend, &self.colors()));
        let kind = self.chart_prefs.read(cx).kind;
        let chart = cx.new(|cx| ChartView::new(TREND, data, kind, cx));
        let me = cx.entity();
        self.overview._subscriptions = vec![
            cx.subscribe(&chart, |this, _, event: &chart::ChartEvent, cx| {
                this.chart_event(event, cx)
            }),
            // Everything the views are given is given after this view notifies, never
            // while it draws (a notification during a draw is dropped).
            cx.observe(&me, |this, _, cx| this.push_board(cx)),
        ];
        self.overview.pushed = Some((self.derived.seq, self.colors_gen));
        self.overview.chart = Some(chart.clone());
        chart
    }

    /// Hands the trend chart, and the detail page's views, what changed. Each
    /// notifies itself only when its input differs. Never call this while drawing.
    pub(super) fn push_board(&mut self, cx: &mut Context<Self>) {
        self.push_parts(cx);
        if let Some(chart) = self.overview.chart.clone() {
            let stamp = (self.derived.seq, self.colors_gen);
            if self.overview.pushed != Some(stamp) {
                self.overview.pushed = Some(stamp);
                let data = Arc::new(trend_data(&self.derived.trend, &self.colors()));
                chart.update(cx, |chart, cx| chart.set_data(data, cx));
            }
            let kind = self.chart_prefs.read(cx).kind;
            chart.update(cx, |chart, cx| chart.set_kind(kind, cx));
        }
        self.push_detail(cx);
    }

    pub(super) fn chart_event(&mut self, event: &chart::ChartEvent, cx: &mut Context<Self>) {
        match event {
            chart::ChartEvent::Kind(kind) => self.set_chart_kind(*kind, cx),
            chart::ChartEvent::Color { id, hex } => self.set_member_color(id, hex, cx),
        }
    }

    // ---- drawing ---------------------------------------------------------------------

    /// The cards, the trend and the heatmap of the period.
    pub(super) fn overview(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let cards = stat_cards(&self.derived.stats, &self.derived.comparison);
        let mut out = vec![
            div()
                .id("ticket-stats")
                .debug_selector(|| "ticket-stats".into())
                .role(Role::Group)
                .aria_label("Period statistics")
                .mb_4()
                .flex()
                .flex_wrap()
                .gap(px(10.))
                .children(cards.iter().map(stat_card))
                .into_any_element(),
        ];
        if let Some(note) = self.derived.comparison.note() {
            out.push(
                div()
                    .id("ticket-comparison-note")
                    .debug_selector(|| "ticket-comparison-note".into())
                    .role(Role::Status)
                    .aria_label(note.to_string())
                    .mb_4()
                    .text_size(px(12.))
                    .text_color(content(0.5))
                    .child(note.to_string())
                    .into_any_element(),
            );
        }
        let chart = self.trend_chart(cx);
        out.push(
            div()
                .mb_4()
                .child(section(
                    "Points trend",
                    format!("per {}", self.derived.trend.unit.as_str()),
                    chart,
                ))
                .into_any_element(),
        );
        if !self.derived.matrix.is_empty() {
            let heat = self.part(super::part::PartKind::Heat, cx);
            out.push(
                div()
                    .mb_4()
                    .child(section(
                        "Difficulty mix",
                        "tickets by member and level".into(),
                        heat,
                    ))
                    .into_any_element(),
            );
        }
        out
    }
}

fn stat_card(card: &StatCard) -> Div {
    let id = card.id;
    let value = div()
        .flex()
        .items_baseline()
        .child(card.value.clone())
        .children(card.sub.clone().map(|sub| {
            div()
                .ml(px(6.))
                .text_size(px(12.))
                .font_weight(gpui::FontWeight::NORMAL)
                .text_color(content(0.5))
                .child(sub)
        }))
        .children(card.flag.clone().map(|flag| {
            div()
                .id(SharedString::from(format!("ticket-stat-{id}-flag")))
                .debug_selector(move || format!("ticket-stat-{id}-flag"))
                .ml(px(8.))
                .rounded_full()
                .border_1()
                .border_color(accent_color())
                .px(px(6.))
                .text_size(px(11.))
                .font_weight(gpui::FontWeight::NORMAL)
                .text_color(accent_color())
                .child(flag)
        }));
    kpi(card.selector, card.label, value, card.change.as_ref())
}

// ---- the heatmap ------------------------------------------------------------------

const NAME_W: f32 = 180.;
const CELL_W: f32 = 64.;
const CELL_H: f32 = 30.;
/// A square of 8 dither cells.
const SQUARE: f32 = 8. * CELL;

/// Members down, levels across. Each cell is a dithered square, inked by how
/// many points the member earned at that level, and the number of tickets.
pub(super) fn heatmap(rows: &[MatrixRow]) -> Stateful<Div> {
    div()
        .id("ticket-levels")
        .debug_selector(|| "ticket-levels".into())
        .role(Role::Table)
        .aria_label("Tickets by member and level")
        .overflow_x_scroll()
        .child(
            div()
                .min_w(px(NAME_W + CELL_W * 6.))
                .px(px(12.))
                .py(px(8.))
                .child(
                    div()
                        .flex()
                        .text_size(px(11.))
                        .text_color(content(0.5))
                        .child(div().w(px(NAME_W)).flex_shrink_0().child("Member"))
                        .children(
                            LEVELS.iter().map(|level| {
                                div().w(px(CELL_W)).flex_shrink_0().child(level.as_str())
                            }),
                        )
                        .child(div().w(px(CELL_W)).flex_shrink_0().child("Total")),
                )
                .children(rows.iter().map(heat_row)),
        )
}

fn heat_row(row: &MatrixRow) -> Stateful<Div> {
    let key = if row.unassigned {
        "unassigned".to_string()
    } else {
        row.id.clone()
    };
    let selector = format!("ticket-levels-row-{key}");
    div()
        .id(SharedString::from(selector.clone()))
        .debug_selector({
            let selector = selector.clone();
            move || selector.clone()
        })
        .role(Role::Row)
        .aria_label(row.name.clone())
        .h(px(CELL_H))
        .flex()
        .items_center()
        .text_size(px(12.))
        .text_color(content(if row.faded || row.unassigned {
            0.5
        } else {
            0.9
        }))
        .child(
            div()
                .w(px(NAME_W))
                .flex_shrink_0()
                .pr(px(8.))
                .truncate()
                .child(row.name.clone()),
        )
        .children(row.cells.iter().enumerate().map(|(index, cell)| {
            heat_cell(
                &key,
                &row.name,
                row.unassigned,
                row.faded,
                LEVELS[index],
                cell,
            )
        }))
        .child(
            div()
                .w(px(CELL_W))
                .flex_shrink_0()
                .font_features(tabular())
                .child(row.total.delivered.to_string()),
        )
}

fn heat_cell(
    key: &str,
    name: &str,
    unassigned: bool,
    faded: bool,
    level: Level,
    cell: &MatrixCell,
) -> Stateful<Div> {
    let selector = format!("ticket-levels-{key}-{}", level.as_str());
    let label = SharedString::from(heat_label(name, level, cell, unassigned));
    let tone = heat_tone(cell.shade);
    let tint = if unassigned {
        content(0.5)
    } else {
        accent_color()
    };
    let tip = label.clone();
    div()
        .id(SharedString::from(selector.clone()))
        .debug_selector(move || selector.clone())
        .role(Role::Cell)
        .aria_label(label)
        .w(px(CELL_W))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(px(6.))
        .when(faded, |d| d.opacity(0.6))
        .tooltip(move |_, cx| cx.new(|_| Tip(tip.clone())).into())
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    let track = content(0.07);
                    let grid = Grid::new(bounds.origin, SQUARE, SQUARE);
                    grid.paint(window, |col, row| {
                        Some(if tone > 0. && inked(col, row, tone) {
                            tint
                        } else {
                            track
                        })
                    });
                },
            )
            .size(px(SQUARE))
            .flex_shrink_0(),
        )
        .child(
            div()
                .font_features(tabular())
                .text_color(content(if cell.delivered == 0 { 0. } else { 0.7 }))
                .child(cell.delivered.to_string()),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use monocode_core::contributors::tickets::{
        board::{MemberRow, build_board},
        report::{MemberSeries, TrendBucket, TrendUnit, tile_stats},
        tickets::DEFAULT_TICKET_POINTS,
    };

    fn stats(delivered: usize, scored: usize, points: f64, median: Option<f64>) -> TileStats {
        TileStats {
            delivered,
            scored,
            points,
            median,
            coverage_percent: (scored * 100).checked_div(delivered).unwrap_or(0) as i64,
        }
    }

    fn ready(before: TileStats) -> Comparison {
        Comparison::Ready {
            against: "Sep 2026".into(),
            stats: before,
            board: build_board(&[], &DEFAULT_TICKET_POINTS, &[]),
        }
    }

    fn card<'a>(cards: &'a [StatCard], id: &str) -> &'a StatCard {
        cards.iter().find(|c| c.id == id).unwrap()
    }

    #[test]
    fn the_cards_carry_the_numbers_of_the_period() {
        let cards = stat_cards(&stats(4, 3, 40., Some(20.)), &Comparison::Off);
        assert_eq!(
            cards.iter().map(|c| c.id).collect::<Vec<_>>(),
            ["delivered", "points", "scored", "median"]
        );
        assert_eq!(card(&cards, "delivered").value, "4");
        assert_eq!(card(&cards, "points").value, "40");
        assert_eq!(card(&cards, "scored").value, "75%");
        assert_eq!(card(&cards, "scored").sub.as_deref(), Some("3 of 4"));
        assert_eq!(card(&cards, "median").value, "20");
        assert!(
            cards.iter().all(|c| c.change.is_none()),
            "no comparison, no change"
        );
    }

    #[test]
    fn unscored_tickets_are_flagged_and_a_full_period_is_not() {
        let partial = stat_cards(&stats(4, 3, 40., None), &Comparison::Off);
        assert_eq!(
            card(&partial, "scored").flag.as_deref(),
            Some("1 not scored")
        );
        let full = stat_cards(&stats(4, 4, 40., None), &Comparison::Off);
        assert_eq!(card(&full, "scored").flag, None);
        assert_eq!(card(&full, "median").value, "-");
    }

    #[test]
    fn a_share_moves_in_percentage_points_not_ticket_points() {
        assert_eq!(pp_text(5), "+5 pp");
        assert_eq!(pp_text(-25), "-25 pp");
        assert_eq!(pp_text(0), "0 pp");
    }

    #[test]
    fn each_card_says_how_it_moved_against_the_previous_period() {
        let cards = stat_cards(
            &stats(6, 3, 60., Some(30.)),
            &ready(stats(4, 3, 40., Some(40.))),
        );
        let text = |id: &str| card(&cards, id).change.clone().map(|c| (c.text, c.suffix));
        assert_eq!(
            text("delivered"),
            Some(("▲ +50%".into(), Some("vs Sep 2026".into())))
        );
        assert_eq!(text("points").map(|t| t.0), Some("▲ +50%".into()));
        // 50% scored against 75%.
        assert_eq!(text("scored").map(|t| t.0), Some("▼ -25 pp".into()));
        assert_eq!(text("median").map(|t| t.0), Some("▼ -25%".into()));
    }

    #[test]
    fn nothing_to_compare_with_means_no_change_on_that_card() {
        let cards = stat_cards(&stats(3, 3, 30., Some(10.)), &ready(stats(0, 0, 0., None)));
        assert!(cards.iter().all(|c| c.change.is_none()));
        let flat = stat_cards(
            &stats(3, 3, 30., Some(10.)),
            &ready(stats(3, 3, 30., Some(10.))),
        );
        assert_eq!(card(&flat, "delivered").change.as_ref().unwrap().text, "0%");
    }

    #[test]
    fn a_missing_previous_period_is_explained_and_pending_is_not() {
        let missing = Comparison::Missing {
            note: "No comparison with Sep 2026: its tickets were not fully fetched.".into(),
        };
        assert_eq!(
            missing.note(),
            Some("No comparison with Sep 2026: its tickets were not fully fetched.")
        );
        assert_eq!(Comparison::Pending.note(), None);
        assert_eq!(Comparison::Off.note(), None);
        assert_eq!(
            ready(stats(1, 1, 1., None)).versus().as_deref(),
            Some("vs Sep 2026")
        );
        assert_eq!(missing.versus(), None);
    }

    fn series(id: &str, points: &[f64]) -> MemberSeries {
        MemberSeries {
            id: id.into(),
            name: format!("User {id}"),
            points: points.to_vec(),
            tickets: vec![0; points.len()],
        }
    }

    fn trend(members: Vec<MemberSeries>) -> MemberTrend {
        let len = members.first().map_or(0, |m| m.points.len());
        MemberTrend {
            unit: TrendUnit::Week,
            buckets: (0..len)
                .map(|i| TrendBucket {
                    label: format!("{} Oct", i + 1),
                    title: format!("Week of {} Oct", i + 1),
                    points: members.iter().map(|m| m.points[i]).sum(),
                })
                .collect(),
            members,
        }
    }

    #[test]
    fn the_trend_keeps_the_top_five_by_points_and_sums_the_rest() {
        let members = (1..=7)
            .map(|n| series(&n.to_string(), &[n as f64, 1.]))
            .collect();
        let data = trend_data(&trend(members), &ColorOverrides::new());
        let ids: Vec<&str> = data.series.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["7", "6", "5", "4", "3", "others"]);
        // Members 1 and 2: (1 + 2) in the first bucket, (1 + 1) in the second.
        assert_eq!(data.series[5].values, [3., 2.]);
        assert_eq!(data.series[5].name, "Others");
        assert_eq!(data.buckets[0].label, "Week of 1 Oct");
        assert_eq!(data.buckets[0].short, "1 Oct");
        assert!(
            data.summary.starts_with("Points per week: "),
            "{}",
            data.summary
        );
    }

    #[test]
    fn five_members_or_fewer_have_no_others() {
        let data = trend_data(
            &trend(vec![series("1", &[1.]), series("2", &[2.])]),
            &ColorOverrides::new(),
        );
        assert_eq!(data.series.len(), 2);
        assert!(data.series.iter().all(|s| s.id != "others"));
    }

    #[test]
    fn a_picked_color_wins_over_the_palette_one() {
        let members = vec![series("1", &[1.]), series("2", &[2.])];
        let mut picked = ColorOverrides::new();
        picked.insert("1".into(), "#3fb950");
        let data = trend_data(&trend(members), &picked);
        let one = data.series.iter().find(|s| s.id == "1").unwrap();
        assert_eq!(one.color, ui::hex_color("#3fb950"));
    }

    fn member(id: &str) -> MemberRow {
        MemberRow {
            id: id.into(),
            name: format!("User {id}"),
            points: 0.,
            tickets: 0,
            assessed: 0,
            active: 0,
            levels: monocode_core::contributors::tickets::tickets::ByLevel([0; 5]),
            ticket_list: vec![],
        }
    }

    #[test]
    fn member_colors_are_distinct_and_only_members_of_the_board_can_be_picked() {
        let mut board = build_board(&[], &DEFAULT_TICKET_POINTS, &[]);
        board.members = vec![member("1"), member("2"), member("3")];
        let plain = member_colors(&board, &ColorOverrides::new());
        let distinct: std::collections::HashSet<_> = plain.values().collect();
        assert_eq!(distinct.len(), 3);
        let mut picked = ColorOverrides::new();
        picked.insert("2".into(), "#f97316");
        picked.insert("ghost".into(), "#f97316");
        let colors = member_colors(&board, &picked);
        assert_eq!(colors["2"], "#f97316");
        assert_eq!(colors["1"], plain["1"]);
        assert!(!colors.contains_key("ghost"));
    }

    #[test]
    fn a_heat_cell_is_inked_by_its_shade() {
        assert_eq!(heat_tone(None), 0.);
        assert_eq!(heat_tone(Some(0)), 0.2);
        assert_eq!(heat_tone(Some(4)), 1.);
        assert_eq!(heat_tone(Some(9)), 1.);
    }

    #[test]
    fn a_heat_cell_says_what_is_in_it() {
        let cell = |points, delivered| MatrixCell {
            points,
            delivered,
            scored: delivered,
            shade: None,
        };
        assert_eq!(
            heat_label("Ann", Level::M, &cell(16., 2), false),
            "Ann, M: 2 tickets, 16 points"
        );
        assert_eq!(
            heat_label("Ann", Level::Xs, &cell(3., 1), false),
            "Ann, XS: 1 ticket, 3 points"
        );
        assert_eq!(
            heat_label("Ann", Level::L, &cell(0., 0), false),
            "Ann, L: no tickets"
        );
        assert_eq!(
            heat_label("Unassigned", Level::S, &cell(0., 3), true),
            "Unassigned, S: 3 tickets"
        );
    }

    #[test]
    fn tile_stats_of_an_empty_board_make_dashes() {
        let cards = stat_cards(
            &tile_stats(&build_board(&[], &DEFAULT_TICKET_POINTS, &[])),
            &Comparison::Off,
        );
        assert_eq!(card(&cards, "scored").value, "-");
        assert_eq!(card(&cards, "delivered").value, "0");
    }
}
