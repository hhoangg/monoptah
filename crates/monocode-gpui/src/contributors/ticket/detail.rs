//! A member's page in the Ticket tab: the same layout as the GitHub detail
//! page (`contributors/detail.rs`), filled with points and tickets. The
//! leaderboard row opens it in place of the board, the toolbar stays, and Back
//! (or Escape) puts the list back exactly as it was: scroll, the member whose
//! tickets were open, and the keys. What the page says is computed by the pure
//! functions at the top (unit tested); the `TicketView` methods lay it out.
use super::{
    fmt,
    overview::pp_text,
    period_phrase, plural,
    state::{Comparison, TicketView},
};
use crate::{
    contributors::{
        activity::{CalendarView, RhythmView, window as year_window},
        chart::{self, ChartBucket, ChartData, ChartView, Series},
        detail::{
            BarRow, Header, ListRow, back_bar, bar_list, empty_page, header, list, page, section,
        },
        stats::{ChangeView, card, card_title, change, kpi},
        ui::hex_color,
    },
    theme::*,
    visuals::{
        calendar::{self, Active},
        dither::release_sheet,
        rhythm::{self, Noun},
    },
};
use chrono::{Duration as Days, NaiveDate};
use gpui::{
    AnyElement, Context, Entity, Hsla, Pixels, Point, StyleRefinement, Subscription, Window, div,
    prelude::*, px,
};
use monocode_core::contributors::{
    model::period::{Period, midnight, parse_iso},
    tickets::{
        board::{build_board, period_tickets},
        member::{DayDelivery, MemberDetail, RecentTicket, level_breakdown, member_detail},
        report::MemberTrend,
        tickets::TicketRecord,
    },
};
use std::{
    collections::{BTreeMap, HashSet},
    sync::Arc,
};

/// Selector prefix of the page.
pub(super) const PREFIX: &str = "ticket-detail";
const POINTS_ID: &str = "points";
const TICKETS_ID: &str = "tickets";
/// Weeks of history the calendar shows at most.
const WEEKS: i64 = 53;
const TICKETS: Noun = Noun {
    one: "ticket",
    many: "tickets",
};

// ---- the pure view functions ---------------------------------------------------

/// One stat card of a member: the value, a line under it, and how it moved.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct DetailCard {
    pub selector: &'static str,
    pub label: &'static str,
    pub value: String,
    pub sub: Option<String>,
    pub change: Option<ChangeView>,
}

fn percent_text(value: f64) -> String {
    format!("{}{}%", if value > 0. { "+" } else { "" }, value as i64)
}

/// The cards of a member. A card with nothing behind it is left out, not shown
/// as zero: no points without a scored ticket, no coverage without a delivered
/// one, no Active card without active work. `versus` is what the changes read
/// against; they are only there when the previous period had something.
pub(super) fn detail_cards(d: &MemberDetail, versus: &Option<String>) -> Vec<DetailCard> {
    let moved = |value: Option<f64>| {
        let value = value?;
        change(Some(value), percent_text(value), versus.clone())
    };
    let mut cards = Vec::new();
    if d.scored > 0 {
        cards.push(DetailCard {
            selector: "ticket-detail-kpi-points",
            label: "Points",
            value: fmt(d.points),
            sub: None,
            change: moved(d.points_change),
        });
    }
    if d.delivered > 0 {
        cards.push(DetailCard {
            selector: "ticket-detail-kpi-delivered",
            label: "Delivered tickets",
            value: d.delivered.to_string(),
            sub: None,
            change: moved(d.delivered_change),
        });
        cards.push(DetailCard {
            selector: "ticket-detail-kpi-scored",
            label: "Scored",
            value: d.scored.to_string(),
            sub: Some(format!("of {}", d.delivered)),
            change: moved(d.scored_change),
        });
        cards.push(DetailCard {
            selector: "ticket-detail-kpi-coverage",
            label: "Coverage",
            value: format!("{}%", d.coverage_percent),
            sub: None,
            change: d
                .coverage_change
                .and_then(|pp| change(Some(pp as f64), pp_text(pp), versus.clone())),
        });
    }
    if d.active > 0 {
        cards.push(DetailCard {
            selector: "ticket-detail-kpi-active",
            label: "Active tickets",
            value: d.active.to_string(),
            sub: Some("in a working status".into()),
            change: None,
        });
    }
    cards
}

fn chart_buckets(trend: &MemberTrend) -> Vec<ChartBucket> {
    trend
        .buckets
        .iter()
        .map(|b| ChartBucket {
            label: b.title.clone(),
            short: b.label.clone(),
        })
        .collect()
}

/// One measure of the member per bucket as a chart of its own: points and
/// tickets are different units, so they are never drawn on one axis, stacked
/// or summed.
fn measure_data(
    trend: &MemberTrend,
    id: &str,
    name: &str,
    color: Hsla,
    values: Vec<f64>,
) -> ChartData {
    let buckets = chart_buckets(trend);
    ChartData {
        summary: chart::summary(
            name,
            trend.unit.as_str(),
            &name.to_lowercase(),
            &buckets,
            &values,
        ),
        series: vec![Series {
            id: id.into(),
            name: name.into(),
            color,
            values,
        }],
        buckets,
    }
}

/// The member's points per bucket of the team's trend.
pub(super) fn points_data(trend: &MemberTrend, member: &str, color: Hsla) -> ChartData {
    let values = trend
        .members
        .iter()
        .find(|m| m.id == member)
        .map_or_else(|| vec![0.; trend.buckets.len()], |m| m.points.clone());
    measure_data(trend, POINTS_ID, "Points", color, values)
}

/// The member's delivered tickets per bucket.
pub(super) fn tickets_data(trend: &MemberTrend, member: &str, color: Hsla) -> ChartData {
    let values = trend.members.iter().find(|m| m.id == member).map_or_else(
        || vec![0.; trend.buckets.len()],
        |m| m.tickets.iter().map(|n| *n as f64).collect(),
    );
    measure_data(trend, TICKETS_ID, "Tickets", color, values)
}

/// One day of the deliveries calendar.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct DeliveryDay {
    pub date: NaiveDate,
    pub delivery: DayDelivery,
}
impl Active for DeliveryDay {
    fn date(&self) -> NaiveDate {
        self.date
    }
    fn amount(&self) -> u32 {
        self.delivery.tickets
    }
    fn details(&self, tint: Hsla) -> Vec<(Hsla, String, String)> {
        let mut rows = vec![(
            tint,
            "Delivered".to_string(),
            plural(self.delivery.tickets as f64, "ticket"),
        )];
        if self.delivery.points > 0. {
            rows.push((content(0.3), "Points".into(), fmt(self.delivery.points)));
        }
        rows
    }
}

/// Every day of the last `WEEKS` weeks, from the Sunday that opens the first
/// through today, empty ones included.
pub(super) fn delivery_days(
    days: &BTreeMap<NaiveDate, DayDelivery>,
    today: NaiveDate,
) -> Vec<DeliveryDay> {
    let start = calendar::first_day(today, WEEKS);
    (0..=(today - start).num_days().max(0))
        .map(|i| {
            let date = start + Days::days(i);
            DeliveryDay {
                date,
                delivery: days.get(&date).copied().unwrap_or_default(),
            }
        })
        .collect()
}

/// The line beside the title of the deliveries card: what the store lacks of
/// the year, or why there is no year yet.
pub(super) fn deliveries_note(state: Year) -> &'static str {
    match state {
        Year::Loading => "loading",
        Year::Failed => "could not be read",
        Year::Partial => "last 12 months, only what is stored",
        Year::Whole => "last 12 months",
    }
}

/// How far the read of the year got.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Year {
    Loading,
    Failed,
    /// Read, but the store does not hold the whole year.
    Partial,
    Whole,
}

/// A year of tickets read from the store (no fetch), for one workspace, scope
/// and day.
pub(super) struct YearRead {
    pub key: String,
    pub tickets: Arc<Vec<TicketRecord>>,
    pub covered: bool,
}

/// The levels as bars: how many tickets the member delivered at each.
pub(super) fn level_rows(d: &MemberDetail) -> Vec<BarRow> {
    let rows = level_breakdown(d);
    let most = rows.iter().map(|(_, n, _)| *n).max().unwrap_or(1).max(1);
    rows.into_iter()
        .map(|(level, count, points)| BarRow {
            label: level.as_str().to_string(),
            value: format!("{} - {} pts", plural(count as f64, "ticket"), fmt(points)),
            fraction: count as f32 / most as f32,
        })
        .collect()
}

/// "Oct 8, 2026" of a timestamp, local; the text itself when unreadable.
fn date_text(iso: &str) -> String {
    parse_iso(iso).map_or_else(|| iso.to_string(), |t| t.format("%b %-d, %Y").to_string())
}

pub(super) fn recent_rows(d: &MemberDetail) -> Vec<ListRow> {
    d.recent.iter().map(recent_row).collect()
}

fn recent_row(ticket: &RecentTicket) -> ListRow {
    ListRow {
        lead: ticket
            .level
            .map_or_else(|| "-".into(), |_| fmt(ticket.share)),
        title: ticket.title.clone(),
        tag: Some(
            ticket
                .level
                .map_or("Not scored", |level| level.as_str())
                .to_string(),
        ),
        trail: date_text(&ticket.delivered_at),
    }
}

// ---- state ------------------------------------------------------------------------

struct Shown {
    seq: u64,
    member: String,
    detail: Option<Arc<MemberDetail>>,
}

/// The member's year: the numbers worked out from a year read.
struct YearShown {
    seq: u64,
    member: String,
    key: String,
    detail: Option<Arc<MemberDetail>>,
}

/// What the list had when the page opened, put back by Back.
struct Saved {
    scroll: Point<Pixels>,
    open_member: Option<String>,
    selected: HashSet<String>,
    reasons: HashSet<String>,
}

/// What an open page keeps. The charts, calendar and rhythm are made with the
/// page and dropped with it, so a chart focus never outlives a visit.
#[derive(Default)]
pub(super) struct DetailUi {
    chart: Option<Entity<ChartView>>,
    tickets_chart: Option<Entity<ChartView>>,
    pub(super) calendar: Option<Entity<CalendarView<DeliveryDay>>>,
    pub(super) rhythm: Option<Entity<RhythmView>>,
    /// The page's numbers for one board; dropped when the board is rebuilt.
    shown: Option<Shown>,
    year_shown: Option<YearShown>,
    /// The last year of tickets the store holds, read for this workspace and day.
    pub(super) year: Option<YearRead>,
    /// The key of the read under way, and of the one that failed (not asked again).
    asking: Option<String>,
    failed: Option<String>,
    saved: Option<Saved>,
    _subscription: Option<Subscription>,
}

impl TicketView {
    /// Opens the page of a member in place of the board. The list keeps its
    /// scroll and the member whose tickets were open; the page starts at the
    /// top with this member's tickets listed.
    pub(super) fn open_detail(
        &mut self,
        member: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.detail.is_some() {
            return;
        }
        self.detail_ui.saved = Some(Saved {
            scroll: self.ui.scroll.offset(),
            open_member: self.ui.open_member.clone(),
            selected: std::mem::take(&mut self.ui.selected),
            reasons: std::mem::take(&mut self.ui.reasons),
        });
        self.ui.level_menu = None;
        self.ui.open_member = Some(member.clone());
        self.detail = Some(member);
        self.ui.scroll.set_offset(Point::default());
        window.focus(&self.ui.focus, cx);
        self.ensure_year(cx);
        cx.notify();
    }

    /// Back to the list, with the scroll, the open member and the keys where they were.
    pub(super) fn close_detail(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let saved = self.detail_ui.saved.take();
        if self.detail.take().is_none() {
            return;
        }
        self.drop_detail_ui(cx);
        self.ui.level_menu = None;
        if let Some(saved) = saved {
            self.ui.open_member = saved.open_member;
            self.ui.selected = saved.selected;
            self.ui.reasons = saved.reasons;
            self.ui.scroll.set_offset(saved.scroll);
        }
        window.focus(&self.ui.focus, cx);
        cx.notify();
    }

    /// Frees the images of the page's calendar and rhythm: the atlas keeps a
    /// tile until it is dropped, and dropping the views does not do it.
    pub(super) fn release_detail_images(&mut self, cx: &mut Context<Self>) {
        if let Some(calendar) = &self.detail_ui.calendar {
            release_sheet(&calendar.read(cx).sheet.clone(), cx);
        }
        if let Some(rhythm) = &self.detail_ui.rhythm {
            release_sheet(&rhythm.read(cx).sheet.clone(), cx);
        }
    }

    /// Closes a palette open on the overview or the member's charts.
    pub(super) fn dismiss_palettes(&mut self, cx: &mut Context<Self>) {
        let charts = [
            self.overview.chart.clone(),
            self.detail_ui.chart.clone(),
            self.detail_ui.tickets_chart.clone(),
        ];
        for chart in charts.into_iter().flatten() {
            chart.update(cx, |chart, cx| chart.dismiss_palette(cx));
        }
    }

    /// Releases the images and forgets the page's views.
    pub(super) fn drop_detail_ui(&mut self, cx: &mut Context<Self>) {
        self.release_detail_images(cx);
        self.detail_ui = DetailUi::default();
    }

    /// Escape: closes the member's page; whether there was one. The page calls this too,
    /// so the rule holds wherever the focus is.
    pub(crate) fn close_top_layer(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.detail.is_none() {
            return false;
        }
        self.close_detail(window, cx);
        true
    }

    /// The numbers of the open page, worked out once per board.
    fn member(&mut self) -> Option<Arc<MemberDetail>> {
        let member = self.detail.clone()?;
        let seq = self.derived.seq;
        if let Some(shown) = &self.detail_ui.shown
            && shown.seq == seq
            && shown.member == member
        {
            return shown.detail.clone();
        }
        let before = match &self.derived.comparison {
            Comparison::Ready { board, .. } => Some(board),
            _ => None,
        };
        let detail = member_detail(&self.derived.board, &member, before).map(Arc::new);
        self.detail_ui.shown = Some(Shown {
            seq,
            member,
            detail: detail.clone(),
        });
        detail
    }

    /// The key of the year read now: workspace, spaces and window. Another
    /// workspace's read never matches it.
    fn year_key(&self) -> Option<String> {
        let scope = self.settings.scope.as_ref()?;
        let (_, since, until) = year_window(&self.now);
        Some(format!(
            "{}\0{since}\0{until}",
            monocode_core::contributors::tickets::board_state::scope_fetch_id(scope)
        ))
    }

    /// Starts the read of the last year when the page needs it. It only reads
    /// the store: nothing is fetched, no fetch trigger changes. Safe to call
    /// often; it spawns only when the key is new.
    pub(super) fn ensure_year(&mut self, cx: &mut Context<Self>) {
        if self.detail.is_none() {
            return;
        }
        let (Some(scope), Some(key)) = (self.settings.scope.clone(), self.year_key()) else {
            return;
        };
        let ui = &self.detail_ui;
        if ui.year.as_ref().is_some_and(|y| y.key == key)
            || ui.asking.as_ref() == Some(&key)
            || ui.failed.as_ref() == Some(&key)
        {
            return;
        }
        self.detail_ui.asking = Some(key.clone());
        let (_, since, until) = year_window(&self.now);
        self.spawn_job(
            "year",
            cx,
            move |backend| backend.history(&scope, &since, &until),
            move |this, answer, cx| {
                if this.detail_ui.asking.as_ref() == Some(&key) {
                    this.detail_ui.asking = None;
                }
                // Another workspace or day, or a page that was closed: not for this one.
                if this.detail.is_none() || this.year_key().as_ref() != Some(&key) {
                    this.ensure_year(cx);
                    return;
                }
                match answer {
                    Ok((tickets, covered)) => {
                        this.detail_ui.failed = None;
                        this.detail_ui.year = Some(YearRead {
                            key,
                            tickets: Arc::new(tickets),
                            covered,
                        });
                    }
                    Err(_) => this.detail_ui.failed = Some(key),
                }
                this.detail_ui.year_shown = None;
                this.settle(cx);
            },
        );
    }

    /// The read of the year, dropped: the store may have changed.
    pub(super) fn forget_year(&mut self) {
        self.detail_ui.year = None;
        self.detail_ui.year_shown = None;
        self.detail_ui.failed = None;
    }

    /// Test hook: the days the calendar was last given.
    #[cfg(test)]
    pub(super) fn calendar_days(&self, cx: &gpui::App) -> Option<Arc<Vec<DeliveryDay>>> {
        Some(self.detail_ui.calendar.as_ref()?.read(cx).days.clone())
    }

    /// Test hook: the types the two charts draw.
    #[cfg(test)]
    pub(super) fn detail_chart_kinds(
        &self,
        cx: &gpui::App,
    ) -> (chart::ChartKind, chart::ChartKind) {
        let kind = |c: &Option<Entity<ChartView>>| c.as_ref().map(|c| c.read(cx).kind).unwrap();
        (
            kind(&self.detail_ui.chart),
            kind(&self.detail_ui.tickets_chart),
        )
    }

    pub(super) fn year_state(&self) -> Year {
        let key = self.year_key();
        match &self.detail_ui.year {
            Some(year) if Some(&year.key) == key.as_ref() => {
                if year.covered {
                    Year::Whole
                } else {
                    Year::Partial
                }
            }
            _ if self.detail_ui.failed.is_some() && self.detail_ui.failed == key => Year::Failed,
            _ => Year::Loading,
        }
    }

    /// The member's last year (days and hours), `None` until it is read.
    fn year_member(&mut self) -> Option<Arc<MemberDetail>> {
        let member = self.detail.clone()?;
        let key = self.year_key()?;
        let seq = self.derived.seq;
        if let Some(shown) = &self.detail_ui.year_shown
            && shown.seq == seq
            && shown.member == member
            && shown.key == key
        {
            return shown.detail.clone();
        }
        let year = self.detail_ui.year.as_ref().filter(|y| y.key == key)?;
        let scope = self.settings.scope.as_ref()?;
        let today = self.now.date_naive();
        let span = Period {
            start: midnight(calendar::first_day(today, WEEKS)),
            end: midnight(today + Days::days(1)),
            ..self.period.clone()
        };
        let delivered = period_tickets(&year.tickets, scope, &span);
        let board = build_board(&delivered, &self.settings.points, &[]);
        let detail = member_detail(&board, &member, None).map(Arc::new);
        self.detail_ui.year_shown = Some(YearShown {
            seq,
            member,
            key,
            detail: detail.clone(),
        });
        detail
    }

    /// Test hook: the board number and the delivered count the page worked out last.
    #[cfg(test)]
    pub(super) fn cached_detail(&self) -> Option<(u64, usize)> {
        let shown = self.detail_ui.shown.as_ref()?;
        Some((shown.seq, shown.detail.as_ref()?.delivered))
    }

    fn member_color(&self, member: &str) -> Hsla {
        self.colors()
            .get(member)
            .map_or_else(accent_color, |hex| hex_color(hex))
    }

    /// What the calendar and the rhythm show for the open member: the last
    /// year, empty until it is read.
    fn activity_input(&mut self) -> (Arc<Vec<DeliveryDay>>, Arc<rhythm::Counts>) {
        match self.year_member() {
            Some(year) => (
                Arc::new(delivery_days(&year.days, self.now.date_naive())),
                Arc::new(year.rhythm),
            ),
            None => (Arc::default(), Arc::default()),
        }
    }

    /// Hands the page's views what changed. Never call this while drawing.
    pub(super) fn push_detail(&mut self, cx: &mut Context<Self>) {
        let Some(member) = self.detail.clone() else {
            return;
        };
        let color = self.member_color(&member);
        let kind = self.chart_prefs.read(cx).kind;
        if let Some(chart) = self.detail_ui.chart.clone() {
            let data = Arc::new(points_data(&self.derived.trend, &member, color));
            chart.update(cx, |chart, cx| {
                chart.set_data(data, cx);
                chart.set_kind(kind, cx);
            });
        }
        if let Some(chart) = self.detail_ui.tickets_chart.clone() {
            let data = Arc::new(tickets_data(&self.derived.trend, &member, color));
            chart.update(cx, |chart, cx| {
                chart.set_data(data, cx);
                chart.set_kind(kind, cx);
            });
        }
        let (days, counts) = self.activity_input();
        if let Some(calendar) = self.detail_ui.calendar.clone() {
            calendar.update(cx, |view, cx| view.set(days, color, cx));
        }
        if let Some(rhythm) = self.detail_ui.rhythm.clone() {
            rhythm.update(cx, |view, cx| view.set(counts, color, cx));
        }
    }

    #[allow(clippy::type_complexity)]
    fn detail_views(
        &mut self,
        member: &str,
        cx: &mut Context<Self>,
    ) -> (
        Entity<ChartView>,
        Entity<ChartView>,
        Entity<CalendarView<DeliveryDay>>,
        Entity<RhythmView>,
    ) {
        if let (Some(chart), Some(tickets), Some(calendar), Some(rhythm)) = (
            &self.detail_ui.chart,
            &self.detail_ui.tickets_chart,
            &self.detail_ui.calendar,
            &self.detail_ui.rhythm,
        ) {
            return (
                chart.clone(),
                tickets.clone(),
                calendar.clone(),
                rhythm.clone(),
            );
        }
        let color = self.member_color(member);
        let kind = self.chart_prefs.read(cx).kind;
        let points = Arc::new(points_data(&self.derived.trend, member, color));
        let chart = cx.new(|cx| ChartView::new(PREFIX, points, kind, cx).without_palette());
        let tickets = Arc::new(tickets_data(&self.derived.trend, member, color));
        let tickets_chart = cx.new(|cx| {
            ChartView::new("ticket-detail-tickets", tickets, kind, cx)
                .without_palette()
                .without_tabs()
        });
        let (days, counts) = self.activity_input();
        let calendar = cx.new(|_| {
            CalendarView::new(days, color).labeled(
                "ticket-detail-calendar",
                "Deliveries calendar, last 12 months",
                WEEKS,
            )
        });
        let rhythm =
            cx.new(|_| RhythmView::new(counts, color).labeled("ticket-detail-rhythm", TICKETS));
        self.detail_ui._subscription = Some(
            cx.subscribe(&chart, |this, _, event: &chart::ChartEvent, cx| {
                this.chart_event(event, cx)
            }),
        );
        self.detail_ui.chart = Some(chart.clone());
        self.detail_ui.tickets_chart = Some(tickets_chart.clone());
        self.detail_ui.calendar = Some(calendar.clone());
        self.detail_ui.rhythm = Some(rhythm.clone());
        (chart, tickets_chart, calendar, rhythm)
    }

    /// The page of the open member, drawn in place of the board.
    pub(super) fn detail_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let back = back_bar(
            PREFIX,
            cx.listener(|this, _, window, cx| this.close_detail(window, cx)),
        );
        let (Some(member), Some(detail)) = (self.detail.clone(), self.member()) else {
            return empty_page(PREFIX, back, "No deliveries in this period.").into_any_element();
        };
        let all = self.period.kind == monocode_core::contributors::model::period::PeriodKind::All;
        let phrase = period_phrase(all, &self.period.label);
        let head = header(Header {
            prefix: PREFIX,
            name: detail.name.clone(),
            color: self
                .colors()
                .get(&member)
                .map_or_else(String::new, |hex| hex.to_string()),
            notes: vec![format!(
                "{} {phrase}",
                plural(detail.delivered as f64, "delivered ticket")
            )],
            action: None,
        });
        let versus = self.derived.comparison.versus();
        let cards = detail_cards(&detail, &versus)
            .iter()
            .map(|card| {
                kpi(
                    card.selector,
                    card.label,
                    div()
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
                        })),
                    card.change.as_ref(),
                )
            })
            .collect();
        let (chart, tickets_chart, calendar_view, rhythm_view) = self.detail_views(&member, cx);
        // Replayed frames carry no accessibility nodes: draw them afresh.
        let (calendar_view, rhythm_view) = if self.ui.a11y {
            (
                calendar_view.into_any_element(),
                rhythm_view.into_any_element(),
            )
        } else {
            let fixed = |height: f32| StyleRefinement::default().w_full().h(px(height));
            (
                calendar_view
                    .cached(fixed(calendar::height()))
                    .into_any_element(),
                rhythm_view
                    .cached(fixed(rhythm::height()))
                    .into_any_element(),
            )
        };
        let color = self.member_color(&member);
        let tickets_row = self
            .derived
            .board
            .members
            .iter()
            .find(|m| m.id == member)
            .cloned();
        let unit = self.derived.trend.unit.as_str();
        let mut sections = vec![
            section("Points trend", format!("per {unit}"), chart),
            section("Tickets trend", format!("per {unit}"), tickets_chart),
            card()
                .debug_selector(|| "ticket-detail-activity".into())
                .child(card_title(
                    "Deliveries",
                    deliveries_note(self.year_state()).into(),
                ))
                .child(
                    div()
                        .p(px(12.))
                        .flex()
                        .flex_col()
                        .gap(px(14.))
                        .child(calendar_view)
                        .child(rhythm_view),
                )
                .into_any_element(),
            section(
                "Levels",
                "tickets by difficulty".into(),
                bar_list(
                    "ticket-detail-levels",
                    "Tickets by level",
                    &level_rows(&detail),
                    color,
                    "No scored tickets in this period.",
                ),
            ),
            section(
                "Recent tickets",
                "newest first".into(),
                list(
                    "ticket-detail-recent",
                    "Recent tickets",
                    &recent_rows(&detail),
                    "No delivered tickets in this period.",
                ),
            ),
        ];
        if tickets_row.is_some() {
            let table = self.part(super::part::PartKind::Tickets, cx);
            sections.push(section("Tickets", "scores, levels and notes".into(), table));
        }
        page(PREFIX, back, head, cards, sections).into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contributors::ticket::fake::{at, now, scope, ticket_with, user};
    use chrono::{Datelike, Weekday};
    use monocode_core::contributors::{
        model::period::{PeriodKind, PeriodPick},
        tickets::{
            board::{build_board, period_tickets},
            board_state::period_of,
            report::member_trend,
            tickets::{DEFAULT_TICKET_POINTS, TicketRecord},
        },
    };

    fn october() -> Period {
        period_of(
            &PeriodPick {
                kind: PeriodKind::Month,
                id: None,
            },
            &now(),
        )
    }

    fn delivered(id: &str, day: u32, hour: u32, level: Option<&str>, who: &[&str]) -> TicketRecord {
        ticket_with(id, |t| {
            t.done_at = Some(at(10, day, hour));
            t.updated_at = at(10, day, hour);
            t.override_level = level.map(String::from);
            t.assignees = who.iter().map(|w| user(w, &format!("User {w}"))).collect();
        })
    }

    fn tickets() -> Vec<TicketRecord> {
        vec![
            delivered("a", 5, 10, Some("S"), &["1"]),
            delivered("b", 6, 14, Some("M"), &["1", "2"]),
            delivered("c", 6, 16, None, &["1"]),
            delivered("d", 13, 9, Some("XL"), &["2"]),
        ]
    }

    fn board_in(
        period: &Period,
        tickets: &[TicketRecord],
    ) -> monocode_core::contributors::tickets::board::Board {
        let shown = period_tickets(tickets, &scope(), period);
        build_board(&shown, &DEFAULT_TICKET_POINTS, &[])
    }

    fn detail_of(
        id: &str,
        previous: Option<&monocode_core::contributors::tickets::board::Board>,
    ) -> MemberDetail {
        member_detail(&board_in(&october(), &tickets()), id, previous).unwrap()
    }

    fn card<'a>(cards: &'a [DetailCard], label: &str) -> Option<&'a DetailCard> {
        cards.iter().find(|c| c.label == label)
    }

    #[test]
    fn the_cards_of_a_member_with_scored_work() {
        let cards = detail_cards(&detail_of("1", None), &None);
        let labels: Vec<&str> = cards.iter().map(|c| c.label).collect();
        assert_eq!(
            labels,
            ["Points", "Delivered tickets", "Scored", "Coverage"]
        );
        assert_eq!(card(&cards, "Points").unwrap().value, "16");
        assert_eq!(card(&cards, "Delivered tickets").unwrap().value, "3");
        let scored = card(&cards, "Scored").unwrap();
        assert_eq!(
            (scored.value.as_str(), scored.sub.as_deref()),
            ("2", Some("of 3"))
        );
        assert_eq!(card(&cards, "Coverage").unwrap().value, "67%");
        assert!(
            cards.iter().all(|c| c.change.is_none()),
            "nothing to compare with"
        );
    }

    #[test]
    fn a_card_with_no_data_behind_it_is_left_out_not_shown_as_zero() {
        // Nothing scored: no points. Nothing active: no Active card.
        let board = board_in(&october(), &[delivered("a", 5, 10, None, &["1"])]);
        let cards = detail_cards(&member_detail(&board, "1", None).unwrap(), &None);
        assert!(card(&cards, "Points").is_none());
        assert!(card(&cards, "Active tickets").is_none());
        assert!(card(&cards, "Delivered tickets").is_some());
        // Work in progress alone: the Active card, and no delivered-only cards.
        let working = ticket_with("w", |t| {
            t.status = "doing".into();
            t.status_type = "custom".into();
            t.done_at = None;
            t.updated_at = at(10, 6, 10);
            t.assignees = vec![user("3", "User 3")];
        });
        let active = [working];
        let shown = period_tickets(&active, &scope(), &october());
        let in_progress = monocode_core::contributors::tickets::board::active_tickets(
            &active,
            &scope(),
            &october(),
        );
        let board = build_board(&shown, &DEFAULT_TICKET_POINTS, &in_progress);
        let cards = detail_cards(&member_detail(&board, "3", None).unwrap(), &None);
        let labels: Vec<&str> = cards.iter().map(|c| c.label).collect();
        // Nothing delivered: no zero card for it either.
        assert_eq!(labels, ["Active tickets"]);
    }

    #[test]
    fn the_cards_say_how_they_moved_when_the_previous_period_had_the_member() {
        let mut earlier = tickets();
        earlier.push(ticket_with("p1", |t| {
            t.done_at = Some(at(9, 5, 10));
            t.updated_at = at(9, 5, 10);
            t.override_level = Some("S".into());
            t.assignees = vec![user("1", "User 1")];
        }));
        let september = period_of(
            &PeriodPick {
                kind: PeriodKind::Month,
                id: Some("2026-09".into()),
            },
            &now(),
        );
        let before = board_in(&september, &earlier);
        let cards = detail_cards(
            &detail_of("1", Some(&before)),
            &Some("vs Sep 2026".to_string()),
        );
        // 16 points against 8; 3 tickets against 1.
        let points = card(&cards, "Points").unwrap().change.clone().unwrap();
        assert_eq!(points.text, "▲ +100%");
        assert_eq!(points.suffix.as_deref(), Some("vs Sep 2026"));
        assert_eq!(
            card(&cards, "Delivered tickets")
                .unwrap()
                .change
                .clone()
                .unwrap()
                .text,
            "▲ +200%"
        );
        // 2 scored against 1, and 67% coverage against 100%.
        let scored = card(&cards, "Scored").unwrap().change.clone().unwrap();
        assert_eq!(scored.text, "▲ +100%");
        let coverage = card(&cards, "Coverage").unwrap().change.clone().unwrap();
        assert_eq!(coverage.text, "▼ -33 pp");
        assert_eq!(coverage.suffix.as_deref(), Some("vs Sep 2026"));
        // Member 2 delivered nothing in September: no change, not a bogus one.
        let two = detail_cards(
            &detail_of("2", Some(&before)),
            &Some("vs Sep 2026".to_string()),
        );
        assert!(two.iter().all(|c| c.change.is_none()));
    }

    #[test]
    fn points_and_tickets_are_two_charts_of_one_series_each() {
        let shown = period_tickets(&tickets(), &scope(), &october());
        let board = build_board(&shown, &DEFAULT_TICKET_POINTS, &[]);
        let trend = member_trend(&october(), &shown, &board, &now());
        let points = points_data(&trend, "1", accent_color());
        let tickets = tickets_data(&trend, "1", accent_color());
        assert_eq!(points.series.len(), 1);
        assert_eq!(tickets.series.len(), 1);
        assert_eq!(points.series[0].id, "points");
        assert_eq!(tickets.series[0].id, "tickets");
        // Weeks of Sep 28, Oct 5 and Oct 12: all three tickets closed in the second.
        assert_eq!(points.series[0].values, [0.0, 16.0, 0.0]);
        assert_eq!(tickets.series[0].values, [0.0, 3.0, 0.0]);
        assert_eq!(points.buckets.len(), 3);
        // Each says its own unit, never a sum of the two.
        assert!(
            points
                .summary
                .starts_with("Points per week: 16 points over 3 weeks"),
            "{}",
            points.summary
        );
        assert!(
            tickets
                .summary
                .starts_with("Tickets per week: 3 tickets over 3 weeks"),
            "{}",
            tickets.summary
        );
        // A member the trend does not know has flat series, not a panic.
        let none = points_data(&trend, "nobody", accent_color());
        assert_eq!(none.series[0].values, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn the_calendar_is_the_last_53_weeks_from_a_sunday_to_today() {
        let detail = detail_of("1", None);
        let today = now().date_naive();
        let days = delivery_days(&detail.days, today);
        assert_eq!(days.first().unwrap().date.weekday(), Weekday::Sun);
        assert_eq!(
            days.first().unwrap().date,
            calendar::first_day(today, WEEKS)
        );
        assert_eq!(days.last().unwrap().date, today, "so the glow is on today");
        assert_eq!((days.len() as i64 + 6) / 7, WEEKS);
        let on = |d: u32| {
            days.iter()
                .find(|x| x.date == NaiveDate::from_ymd_opt(2026, 10, d).unwrap())
                .unwrap()
        };
        assert_eq!(on(6).delivery.tickets, 2);
        assert_eq!(on(6).amount(), 2);
        assert_eq!(on(7).delivery.tickets, 0);
        assert_eq!(days.iter().map(|d| d.delivery.tickets).sum::<u32>(), 3);
    }

    #[test]
    fn the_note_says_what_the_store_lacks_of_the_year() {
        assert_eq!(deliveries_note(Year::Loading), "loading");
        assert_eq!(deliveries_note(Year::Failed), "could not be read");
        assert_eq!(deliveries_note(Year::Whole), "last 12 months");
        assert_eq!(
            deliveries_note(Year::Partial),
            "last 12 months, only what is stored"
        );
    }

    #[test]
    fn a_day_says_how_many_tickets_and_how_many_points() {
        let day = DeliveryDay {
            date: NaiveDate::from_ymd_opt(2026, 10, 6).unwrap(),
            delivery: DayDelivery {
                tickets: 2,
                points: 24.0,
            },
        };
        let rows = day.details(accent_color());
        assert_eq!(rows[0].1, "Delivered");
        assert_eq!(rows[0].2, "2 tickets");
        assert_eq!(rows[1].1, "Points");
        assert_eq!(rows[1].2, "24");
        let unscored = DeliveryDay {
            delivery: DayDelivery {
                tickets: 1,
                points: 0.0,
            },
            ..day
        };
        assert_eq!(
            unscored.details(accent_color()).len(),
            1,
            "no zero points row"
        );
    }

    #[test]
    fn levels_are_bars_by_ticket_count() {
        let rows = level_rows(&detail_of("2", None));
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].label, "M");
        assert_eq!(rows[0].value, "1 ticket - 8 pts");
        assert_eq!(rows[1].label, "XL");
        assert_eq!(rows[1].value, "1 ticket - 48 pts");
        assert!(rows.iter().all(|r| r.fraction == 1.0));
        let none = level_rows(
            &member_detail(
                &board_in(&october(), &[delivered("a", 5, 10, None, &["1"])]),
                "1",
                None,
            )
            .unwrap(),
        );
        assert!(none.is_empty());
    }

    #[test]
    fn recent_tickets_list_points_level_and_date() {
        let rows = recent_rows(&detail_of("1", None));
        assert_eq!(rows.len(), 3);
        // Newest first: the unscored one.
        assert_eq!(rows[0].lead, "-");
        assert_eq!(rows[0].tag.as_deref(), Some("Not scored"));
        assert_eq!(rows[1].lead, "8");
        assert_eq!(rows[1].tag.as_deref(), Some("M"));
        assert_eq!(rows[1].title, "Ticket b");
        assert_eq!(rows[1].trail, "Oct 6, 2026");
    }
}
