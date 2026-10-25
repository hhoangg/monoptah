//! The Ticket Contributors report: the numbers behind it (`build_report`) and
//! a self-contained HTML page (`render_report_html`) with inline SVG charts.
//! Port of `ticket-contributors/model/report.ts`; the output keeps the same
//! structure, CSS and escaping.

use std::collections::HashSet;

use chrono::{DateTime, Datelike, Local, TimeZone};

use super::board::{
    active_tickets, build_board, excluded_members_of, period_tickets, statuses_of, Board,
};
use super::tickets::{
    delivered_at, effective_level, AiConfigStatus, Level, LevelCounts, Points, TicketRecord,
    WorkspaceScope, CRITERIA, LEVELS,
};
use crate::contributors::model::period::{
    add_days, first_of_month, midnight, parse_iso, previous_period as previous_period_of, Period,
    PeriodKind,
};
use crate::contributors::model::{js_len, js_number_text, js_round, js_space, js_trim, locale_cmp};

pub struct ReportInput {
    pub workspace_name: String,
    pub period: Period,
    pub previous_period: Option<Period>,
    pub scope: WorkspaceScope,
    pub points: Points,
    pub tickets: Vec<TicketRecord>,
    pub previous_tickets: Vec<TicketRecord>,
    pub ai: Option<AiConfigStatus>,
    pub generated_at: DateTime<Local>,
    /// The previous Period's tickets were not fully fetched, so
    /// `previous_tickets` was left out; the report then says why there is no
    /// comparison.
    pub previous_incomplete: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReportNote {
    pub headline: String,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReportTile {
    /// `delivered`, `scored`, `points`, `contributors`, `active` or `median`.
    pub id: &'static str,
    pub label: &'static str,
    /// Display text, already formatted.
    pub value: String,
    /// Small line under the value; `None` leaves it out.
    pub sub: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReportMember {
    pub id: String,
    pub name: String,
    pub points: f64,
    /// Whole percent of the team's points.
    pub share_percent: i64,
    pub delivered: usize,
    pub scored: usize,
    /// Scored tickets out of delivered tickets, 0 to 1.
    pub coverage: f64,
    /// Coverage as shown: whole percent, capped like the Scored tile.
    pub coverage_percent: i64,
    /// Delivered tickets with coverage below 50%: the totals are incomplete and drawn faded.
    pub faded: bool,
    pub levels: LevelCounts,
    /// Points per scored ticket; `None` without scored tickets.
    pub avg_points: Option<f64>,
    pub active: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrendUnit {
    Day,
    Week,
    Month,
    Year,
}

impl TrendUnit {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Week => "week",
            Self::Month => "month",
            Self::Year => "year",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TrendBucket {
    pub label: String,
    pub title: String,
    pub points: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Trend {
    pub unit: TrendUnit,
    pub buckets: Vec<TrendBucket>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HardestTicket {
    pub id: String,
    pub title: String,
    /// Only an `https://` URL; otherwise `None` and the title is plain text.
    pub url: Option<String>,
    pub members: String,
    pub level: Level,
    pub points: f64,
    pub edited: bool,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MatrixCell {
    pub points: f64,
    pub delivered: usize,
    pub scored: usize,
    /// 0 to 4 on the blue ramp by points against the table's busiest cell; `None` means no background.
    pub shade: Option<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MatrixRow {
    pub id: String,
    pub name: String,
    /// The row of delivered tickets without an assignee: counts only, no points.
    pub unassigned: bool,
    /// Overall coverage below 50%: the row is drawn faded, like the overview table.
    pub faded: bool,
    pub cells: Vec<MatrixCell>,
    pub total: MatrixCell,
}

/// Delivered tickets without an assignee, and how many of them are scored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unassigned {
    pub delivered: usize,
    pub scored: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReportMonth {
    /// `2026-10`.
    pub key: String,
    /// `Oct`.
    pub label: &'static str,
    /// `October 2026`.
    pub title: String,
    pub delivered: usize,
    pub points: f64,
    pub tiles: Vec<ReportTile>,
    pub members: Vec<ReportMember>,
    pub unassigned: Option<Unassigned>,
    pub hardest: Vec<HardestTicket>,
    /// Coverage note of this month alone; `None` at 100% or without tickets.
    pub note: Option<ReportNote>,
}

/// Month by month view of a quarter or a year.
#[derive(Clone, Debug, PartialEq)]
pub struct ReportMonthly {
    pub months: Vec<ReportMonth>,
    pub rows: Vec<MatrixRow>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LoadRow {
    pub id: String,
    pub name: String,
    pub delivered: usize,
    pub active: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Statuses {
    pub delivered: Vec<String>,
    pub excluded: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReportData {
    pub workspace_name: String,
    pub period_label: String,
    pub generated_label: String,
    /// For example `claude-sonnet-5-5 (Claude Code)`; `None` without AI config.
    pub scored_with: Option<String>,
    pub notes: Vec<ReportNote>,
    /// Why there is no comparison with the previous Period; `None` when there is none to explain.
    pub comparison_note: Option<String>,
    pub tiles: Vec<ReportTile>,
    pub members: Vec<ReportMember>,
    /// Delivered tickets without an assignee; `None` when there are none.
    pub unassigned: Option<Unassigned>,
    pub trend: Trend,
    pub load: Vec<LoadRow>,
    pub hardest: Vec<HardestTicket>,
    /// Only for quarter and year periods.
    pub monthly: Option<ReportMonthly>,
    pub statuses: Statuses,
    /// Names of the members left off the board; empty when nobody is.
    pub excluded_members: Vec<String>,
    pub points: Points,
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

const MONTH_NAMES: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

// Series, level ramp and surfaces. The same values feed the CSS and the SVG
// attributes, which cannot rely on CSS variables.
const COLOR_PAGE: &str = "#f9f9f7";
const COLOR_SURFACE: &str = "#fcfcfb";
const COLOR_TEXT_PRIMARY: &str = "#0b0b0b";
const COLOR_TEXT_SECONDARY: &str = "#52514e";
const COLOR_TEXT_MUTED: &str = "#898781";
const COLOR_GRID: &str = "#e1e0d9";
const COLOR_AXIS: &str = "#c3c2b7";
const COLOR_BORDER: &str = "rgba(11, 11, 11, 0.10)";
const COLOR_SERIES1: &str = "#2a78d6";
const COLOR_SERIES2: &str = "#eb6834";
const COLOR_WARN_BG: &str = "#fff6e0";
const COLOR_WARN_INK: &str = "#7a5300";
/// Muted ink for text on a shaded matrix cell: #52514e falls below 4.5:1 on the darker steps.
const COLOR_INK_ON_SHADE: &str = "#2b2a28";

/// Light steps of the level ramp: backgrounds under dark ink in the month matrix.
const CELL_RAMP: [&str; 5] = ["#cde2fb", "#b7d3f6", "#9ec5f4", "#86b6ef", "#6da7ec"];

fn level_color(level: Level) -> &'static str {
    match level {
        Level::Xs => "#86b6ef",
        Level::S => "#5598e7",
        Level::M => "#2a78d6",
        Level::L => "#1c5cab",
        Level::Xl => "#104281",
    }
}

const FADE_BELOW: f64 = 0.5;
pub const HARDEST_LIMIT: usize = 10;
const MONTH_HARDEST_LIMIT: usize = 3;

// ---------------------------------------------------------------- formatting

/// Escapes text for HTML content and attribute values.
fn esc(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Whole numbers stay whole; a split share keeps one decimal, thousands are
/// grouped (`toLocaleString("en-US", { maximumFractionDigits: 1 })`).
fn fmt(n: f64) -> String {
    let rounded = js_round(n * 10.0) / 10.0;
    if !rounded.is_finite() {
        return if rounded.is_nan() {
            "NaN".into()
        } else if rounded < 0.0 {
            "-∞".into()
        } else {
            "∞".into()
        };
    }
    let text = format!("{:.1}", rounded.abs());
    let (int, frac) = text.split_once('.').unwrap_or((&text, "0"));
    let mut grouped = String::new();
    for (i, digit) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    let sign = if rounded < 0.0 { "-" } else { "" };
    if frac == "0" {
        format!("{sign}{grouped}")
    } else {
        format!("{sign}{grouped}.{frac}")
    }
}

/// Rounded percent that never crosses a boundary by rounding: below 100% it
/// stays at 99 or less, below 50% at 49 or less, and above 0 it is at least 1.
fn percent(part: f64, whole: f64) -> i64 {
    if whole <= 0.0 || part <= 0.0 {
        return 0;
    }
    if part >= whole {
        return 100;
    }
    let rounded = js_round((part / whole) * 100.0).max(1.0);
    let cap = if part / whole < FADE_BELOW {
        49.0
    } else {
        99.0
    };
    rounded.min(cap) as i64
}

fn day_label(d: &DateTime<Local>) -> String {
    format!("{} {}", d.day(), MONTHS[d.month0() as usize])
}

/// `Number.prototype.toFixed(2)` for a finite number: the nearest value, and
/// the larger one when two are as near (Rust rounds such a tie to even).
fn to_fixed2(n: f64) -> String {
    let sign = if n < 0.0 { "-" } else { "" };
    // Rust prints the exact binary value, so the digits after the second
    // decimal say how to round.
    let exact = format!("{:.80}", n.abs());
    let (int, frac) = exact.split_once('.').expect("decimal point");
    let mut digits: Vec<u8> = format!("{int}{}", &frac[..2]).into_bytes();
    if frac.as_bytes()[2] >= b'5' {
        let mut i = digits.len();
        loop {
            if i == 0 {
                digits.insert(0, b'1');
                break;
            }
            i -= 1;
            if digits[i] == b'9' {
                digits[i] = b'0';
            } else {
                digits[i] += 1;
                break;
            }
        }
    }
    let text = String::from_utf8(digits).expect("ascii digits");
    let (head, tail) = text.split_at(text.len() - 2);
    format!("{sign}{head}.{tail}")
}

/// Compact number for SVG attributes: `String(Number(n.toFixed(2)))`.
fn num(n: f64) -> String {
    if !n.is_finite() {
        return js_number_text(n);
    }
    js_number_text(to_fixed2(n).parse::<f64>().unwrap_or(n))
}

/// A number interpolated into a template string.
fn n(value: f64) -> String {
    js_number_text(value)
}

fn clip(text: &str, max: usize) -> String {
    if js_len(text) > max {
        let keep: Vec<u16> = text
            .encode_utf16()
            .take(max.saturating_sub(3).max(1))
            .collect();
        format!("{}...", String::from_utf16_lossy(&keep))
    } else {
        text.to_string()
    }
}

fn runner_label(runner: &str) -> &'static str {
    if runner == "claude-code" {
        "Claude Code"
    } else {
        "API"
    }
}

// --------------------------------------------------------------------- build

fn slug(text: &str) -> String {
    let mut out = String::new();
    for c in text.to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let s = out.trim_matches('-');
    if s.is_empty() {
        "workspace".into()
    } else {
        s.to_string()
    }
}

pub fn report_file_name(workspace_name: &str, period: &Period) -> String {
    format!(
        "ticket-contributors-{}-{}.html",
        slug(workspace_name),
        period.id
    )
}

fn ms(t: &DateTime<Local>) -> i64 {
    t.timestamp_millis()
}

fn from_ms(millis: i64) -> DateTime<Local> {
    Local
        .timestamp_millis_opt(millis)
        .single()
        .expect("a representable instant")
}

/// `new Date(y, m, 1)`; an overflowing month rolls over.
fn month_start(year: i32, month0: i32) -> DateTime<Local> {
    midnight(first_of_month(year, month0))
}

/// Monday 00:00 of the local week that contains `d`.
fn monday_of(d: &DateTime<Local>) -> DateTime<Local> {
    add_days(d, -i64::from(d.weekday().num_days_from_monday()))
}

struct BucketPlan {
    unit: TrendUnit,
    starts: Vec<DateTime<Local>>,
}

fn month_range(from: &DateTime<Local>, end_exclusive: &DateTime<Local>) -> Vec<DateTime<Local>> {
    let mut starts = Vec::new();
    let mut d = month_start(from.year(), from.month0() as i32);
    while d < *end_exclusive {
        starts.push(d);
        d = month_start(d.year(), d.month0() as i32 + 1);
    }
    starts
}

/// `Date.parse` of a delivery date; `None` for text JS reads as NaN.
fn parse_ms(text: &str) -> Option<i64> {
    parse_iso(text).map(|t| ms(&t))
}

/// Buckets for the trend: days for 7d, ISO weeks for a month, months for a
/// quarter or a year; All time uses months up to 24 of them, else years.
fn plan_buckets(period: &Period, delivered: &[TicketRecord]) -> BucketPlan {
    match period.kind {
        PeriodKind::Days7 => {
            let mut starts = Vec::new();
            let mut d = period.start;
            while d < period.end {
                starts.push(d);
                d = add_days(&d, 1);
            }
            BucketPlan {
                unit: TrendUnit::Day,
                starts,
            }
        }
        PeriodKind::Month => {
            let mut starts = Vec::new();
            let mut d = monday_of(&period.start);
            while d < period.end {
                starts.push(d);
                d = add_days(&d, 7);
            }
            BucketPlan {
                unit: TrendUnit::Week,
                starts,
            }
        }
        PeriodKind::Quarter | PeriodKind::Year => BucketPlan {
            unit: TrendUnit::Month,
            starts: month_range(&period.start, &period.end),
        },
        PeriodKind::All => {
            let times: Vec<i64> = delivered
                .iter()
                .filter_map(|t| parse_ms(delivered_at(t).at))
                .collect();
            let (Some(&min), Some(&max)) = (times.iter().min(), times.iter().max()) else {
                return BucketPlan {
                    unit: TrendUnit::Month,
                    starts: vec![],
                };
            };
            let (first, last) = (from_ms(min), from_ms(max));
            let months = (last.year() - first.year()) * 12 + last.month0() as i32
                - first.month0() as i32
                + 1;
            if months <= 24 {
                return BucketPlan {
                    unit: TrendUnit::Month,
                    starts: month_range(
                        &first,
                        &month_start(last.year(), last.month0() as i32 + 1),
                    ),
                };
            }
            BucketPlan {
                unit: TrendUnit::Year,
                starts: (first.year()..=last.year())
                    .map(|y| month_start(y, 0))
                    .collect(),
            }
        }
    }
}

fn bucket_labels(unit: TrendUnit, start: &DateTime<Local>, period: &Period) -> (String, String) {
    match unit {
        TrendUnit::Day => (day_label(start), day_label(start)),
        TrendUnit::Week => (day_label(start), format!("Week of {}", day_label(start))),
        TrendUnit::Month => {
            let month = MONTHS[start.month0() as usize];
            let full = format!("{month} {}", start.year());
            (
                if period.kind == PeriodKind::All {
                    full.clone()
                } else {
                    month.to_string()
                },
                full,
            )
        }
        TrendUnit::Year => (start.year().to_string(), start.year().to_string()),
    }
}

fn median(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = sorted.len() / 2;
    Some(if sorted.len() % 2 == 1 {
        sorted[mid]
    } else {
        (sorted[mid - 1] + sorted[mid]) / 2.0
    })
}

/// Reason of the highest-scoring criterion; criteria order breaks ties.
fn main_reason(t: &TicketRecord) -> String {
    if t.override_level.as_deref().is_some_and(|l| !l.is_empty()) {
        let note = t.override_note.as_deref().map_or("", js_trim);
        return if note.is_empty() { "Set by hand" } else { note }.to_string();
    }
    let Some(assessment) = &t.assessment else {
        return String::new();
    };
    let mut best = 0;
    for key in 0..CRITERIA.len() {
        if assessment.criteria.get(key) > assessment.criteria.get(best) {
            best = key;
        }
    }
    assessment.reasons.get(best).to_string()
}

fn delivered_time(t: &TicketRecord) -> f64 {
    parse_ms(delivered_at(t).at).map_or(0.0, |m| m as f64)
}

fn distinct_names(t: &TicketRecord) -> Vec<&str> {
    let mut seen: Vec<(&str, &str)> = Vec::new();
    for a in &t.assignees {
        if !seen.iter().any(|(id, _)| *id == a.id) {
            seen.push((&a.id, &a.name));
        }
    }
    seen.into_iter().map(|(_, name)| name).collect()
}

fn unassigned_of(board: &Board) -> Option<Unassigned> {
    (board.unassigned.tickets > 0).then_some(Unassigned {
        delivered: board.unassigned.tickets,
        scored: board.unassigned.assessed,
    })
}

/// Index of the last start at or before `time`; `None` before the first.
fn bucket_index(starts: &[DateTime<Local>], time: i64) -> Option<usize> {
    let mut index = None;
    for (i, start) in starts.iter().enumerate() {
        if ms(start) <= time {
            index = Some(i);
        } else {
            break;
        }
    }
    index
}

fn report_members(board: &Board) -> (Vec<ReportMember>, f64) {
    let team_points = board.members.iter().fold(0.0, |sum, m| sum + m.points);
    let members = board
        .members
        .iter()
        .map(|m| {
            let coverage = if m.tickets > 0 {
                m.assessed as f64 / m.tickets as f64
            } else {
                0.0
            };
            ReportMember {
                id: m.id.clone(),
                name: m.name.clone(),
                points: m.points,
                share_percent: if team_points > 0.0 {
                    js_round((m.points / team_points) * 100.0) as i64
                } else {
                    0
                },
                delivered: m.tickets,
                scored: m.assessed,
                coverage,
                coverage_percent: percent(m.assessed as f64, m.tickets as f64),
                // Coverage means nothing for a member with no delivered tickets.
                faded: m.tickets > 0 && coverage < FADE_BELOW,
                levels: m.levels,
                avg_points: (m.assessed > 0).then(|| m.points / m.assessed as f64),
                active: m.active,
            }
        })
        .collect();
    (members, team_points)
}

fn scored_tile(board: &Board) -> ReportTile {
    ReportTile {
        id: "scored",
        label: "Scored",
        value: if board.total > 0 {
            format!("{}%", percent(board.assessed as f64, board.total as f64))
        } else {
            "-".into()
        },
        sub: Some(format!("{} of {}", board.assessed, board.total)),
    }
}

fn points_tile(team_points: f64) -> ReportTile {
    ReportTile {
        id: "points",
        label: "Team points",
        value: fmt(team_points),
        sub: Some("scored tickets only".into()),
    }
}

fn active_tile(count: usize) -> ReportTile {
    ReportTile {
        id: "active",
        label: "Active tickets",
        value: count.to_string(),
        sub: Some("in a working status".into()),
    }
}

/// Coverage note when some delivered tickets are not scored; `where_` narrows it to a span.
fn coverage_note(board: &Board, where_: &str) -> Option<ReportNote> {
    if !(board.total > 0 && board.assessed < board.total) {
        return None;
    }
    Some(ReportNote {
        headline: format!(
            "Scoring coverage is {}%.",
            percent(board.assessed as f64, board.total as f64)
        ),
        text: format!(
            "{} of {} delivered tickets{where_} are scored. Faded members have less than 50% of their tickets scored, so their totals are incomplete.",
            board.assessed, board.total
        ),
    })
}

/// Delivered and scored tickets, by points then newest delivery.
fn hardest_of(delivered: &[TicketRecord], points: &Points, limit: usize) -> Vec<HardestTicket> {
    struct Scored<'a> {
        t: &'a TicketRecord,
        level: Level,
        worth: f64,
        time: f64,
    }
    let mut scored: Vec<Scored> = delivered
        .iter()
        .filter_map(|t| {
            effective_level(t).map(|level| Scored {
                t,
                level,
                worth: points[level],
                time: delivered_time(t),
            })
        })
        .collect();
    let by = |a: f64, b: f64| {
        b.partial_cmp(&a)
            .filter(|o| *o != std::cmp::Ordering::Equal)
            .unwrap_or(std::cmp::Ordering::Equal)
    };
    scored.sort_by(|a, b| {
        by(a.worth, b.worth)
            .then_with(|| by(a.time, b.time))
            .then_with(|| locale_cmp(&a.t.id, &b.t.id))
    });
    scored
        .into_iter()
        .take(limit)
        .map(
            |Scored {
                 t, level, worth, ..
             }| {
                let names = distinct_names(t);
                HardestTicket {
                    id: t.id.clone(),
                    title: t.title.clone(),
                    url: t.url.starts_with("https://").then(|| t.url.clone()),
                    members: if names.is_empty() {
                        "Unassigned".into()
                    } else {
                        names.join(", ")
                    },
                    level,
                    points: worth,
                    edited: t.override_level.is_some(),
                    reason: main_reason(t),
                }
            },
        )
        .collect()
}

/// One month per column of a quarter or a year, up to the month of
/// `generated_at`. A ticket belongs to the month of its delivery date (an
/// active one, of its update date); one dated after the last month joins it,
/// as in the trend. Rows follow the overview member table.
fn build_monthly(
    period: &Period,
    delivered: &[TicketRecord],
    active: &[TicketRecord],
    overview: (&[ReportMember], Option<Unassigned>),
    points: &Points,
    generated_at: &DateTime<Local>,
) -> ReportMonthly {
    let starts: Vec<DateTime<Local>> = month_range(&period.start, &period.end)
        .into_iter()
        .filter(|start| ms(start) <= ms(generated_at))
        .collect();
    let mut delivered_by: Vec<Vec<TicketRecord>> = starts.iter().map(|_| vec![]).collect();
    let mut active_by: Vec<Vec<TicketRecord>> = starts.iter().map(|_| vec![]).collect();
    for t in delivered {
        if let Some(i) = bucket_index(&starts, delivered_time(t) as i64) {
            delivered_by[i].push(t.clone());
        }
    }
    for t in active {
        if let Some(i) = parse_ms(&t.updated_at).and_then(|m| bucket_index(&starts, m)) {
            active_by[i].push(t.clone());
        }
    }

    let months: Vec<ReportMonth> = starts
        .iter()
        .enumerate()
        .map(|(i, start)| {
            let board = build_board(&delivered_by[i], points, &active_by[i]);
            let (members, team_points) = report_members(&board);
            let name = MONTH_NAMES[start.month0() as usize];
            let title = format!("{name} {}", start.year());
            ReportMonth {
                key: format!("{}-{:02}", start.year(), start.month0() + 1),
                label: MONTHS[start.month0() as usize],
                delivered: board.total,
                points: team_points,
                tiles: vec![
                    ReportTile {
                        id: "delivered",
                        label: "Delivered tickets",
                        value: board.total.to_string(),
                        sub: None,
                    },
                    scored_tile(&board),
                    points_tile(team_points),
                    active_tile(active_by[i].len()),
                ],
                members,
                unassigned: unassigned_of(&board),
                hardest: hardest_of(&delivered_by[i], points, MONTH_HARDEST_LIMIT),
                note: coverage_note(&board, &format!(" in {title}")),
                title,
            }
        })
        .collect();

    let cell_of = |points: f64, delivered: usize, scored: usize| MatrixCell {
        points,
        delivered,
        scored,
        shade: None,
    };
    let (overview_members, overview_unassigned) = overview;
    let mut rows: Vec<MatrixRow> = overview_members
        .iter()
        .map(|m| MatrixRow {
            id: m.id.clone(),
            name: m.name.clone(),
            unassigned: false,
            faded: m.faded,
            cells: months
                .iter()
                .map(|month| match month.members.iter().find(|x| x.id == m.id) {
                    Some(own) => cell_of(own.points, own.delivered, own.scored),
                    None => cell_of(0.0, 0, 0),
                })
                .collect(),
            total: cell_of(m.points, m.delivered, m.scored),
        })
        .collect();
    if let Some(unassigned) = overview_unassigned {
        rows.push(MatrixRow {
            id: String::new(),
            name: "Unassigned".into(),
            unassigned: true,
            faded: false,
            cells: months
                .iter()
                .map(|month| {
                    let own = month.unassigned;
                    cell_of(
                        0.0,
                        own.map_or(0, |u| u.delivered),
                        own.map_or(0, |u| u.scored),
                    )
                })
                .collect(),
            total: cell_of(0.0, unassigned.delivered, unassigned.scored),
        });
    }

    // Shade by points against the busiest month cell of any member.
    let max = rows
        .iter()
        .filter(|r| !r.unassigned)
        .flat_map(|r| r.cells.iter().map(|c| c.points))
        .fold(0.0_f64, f64::max);
    for row in &mut rows {
        for cell in &mut row.cells {
            if max > 0.0 && cell.points > 0.0 {
                let step = ((cell.points / max) * CELL_RAMP.len() as f64).ceil() as i64 - 1;
                cell.shade = Some(step.clamp(0, CELL_RAMP.len() as i64 - 1) as usize);
            }
        }
    }
    ReportMonthly { months, rows }
}

pub fn build_report(input: &ReportInput) -> ReportData {
    let (period, scope, points, generated_at) = (
        &input.period,
        &input.scope,
        &input.points,
        &input.generated_at,
    );
    let delivered = period_tickets(&input.tickets, scope, period);
    let active = active_tickets(&input.tickets, scope, period);
    let board = build_board(&delivered, points, &active);

    let (members, team_points) = report_members(&board);
    let unassigned = unassigned_of(&board);

    // Comparison with the previous Period: not for All time, nor from zero. A
    // Period still running is compared with the same elapsed span of the
    // previous one; the rolling 7 days and a finished Period compare whole.
    let before = input
        .previous_period
        .clone()
        .or_else(|| previous_period_of(period));
    let mut comparison: Option<String> = None;
    let mut comparison_note: Option<String> = None;
    if let (true, Some(before)) = (period.kind != PeriodKind::All, &before) {
        let label = if before.kind == PeriodKind::Days7 {
            "the previous 7 days".to_string()
        } else {
            before.label.clone()
        };
        if input.previous_incomplete {
            comparison_note = Some(format!(
                "No comparison with {label}: its tickets were not fully fetched. Open that period in Ticket Contributors once to include it."
            ));
        } else {
            let running = period.kind != PeriodKind::Days7 && ms(generated_at) < ms(&period.end);
            let mut against = label;
            let mut span = before.clone();
            if running {
                let elapsed = ms(generated_at) - ms(&period.start);
                let end = ms(&before.end).min(ms(&before.start) + elapsed.max(0));
                span.end = from_ms(end);
                against = format!("the same days of {}", before.label);
            }
            let previous = period_tickets(&input.previous_tickets, scope, &span).len();
            if previous > 0 {
                let change =
                    js_round(((board.total as f64 - previous as f64) / previous as f64) * 100.0);
                comparison = Some(format!(
                    "{}{}% vs {against}",
                    if change > 0.0 { "+" } else { "" },
                    js_number_text(change)
                ));
            }
        }
    }

    let scored_median = median(
        &members
            .iter()
            .filter(|m| m.scored > 0)
            .map(|m| m.points)
            .collect::<Vec<_>>(),
    );
    let tiles = vec![
        ReportTile {
            id: "delivered",
            label: "Delivered tickets",
            value: board.total.to_string(),
            sub: comparison,
        },
        scored_tile(&board),
        points_tile(team_points),
        ReportTile {
            id: "contributors",
            label: "Contributors",
            value: members
                .iter()
                .filter(|m| m.delivered > 0)
                .count()
                .to_string(),
            sub: Some("with delivered work".into()),
        },
        active_tile(active.len()),
        ReportTile {
            id: "median",
            label: "Median points per person",
            value: scored_median.map_or_else(|| "-".into(), fmt),
            sub: Some("among scored work".into()),
        },
    ];

    let mut notes: Vec<ReportNote> = Vec::new();
    if let Some(note) = coverage_note(&board, "") {
        notes.push(note);
    }
    if board.dates_approximate {
        notes.push(ReportNote {
            headline: "Some delivery dates are approximate.".into(),
            text: "Tickets without a done date or status history use their last update date."
                .into(),
        });
    }

    // Trend: team points (member shares) by delivery date. Buckets that start
    // after the report was generated are still to come.
    let planned = plan_buckets(period, &delivered);
    let plan = BucketPlan {
        unit: planned.unit,
        starts: planned
            .starts
            .into_iter()
            .filter(|start| ms(start) <= ms(generated_at))
            .collect(),
    };
    let mut totals = vec![0.0; plan.starts.len()];
    for member in &board.members {
        for entry in &member.ticket_list {
            if let Some(index) = bucket_index(&plan.starts, delivered_time(&entry.ticket) as i64) {
                totals[index] += entry.share;
            }
        }
    }
    let buckets: Vec<TrendBucket> = plan
        .starts
        .iter()
        .enumerate()
        .map(|(i, start)| {
            let (label, title) = bucket_labels(plan.unit, start, period);
            TrendBucket {
                label,
                title,
                points: totals[i],
            }
        })
        .collect();

    // Hardest tickets: delivered and scored, by points then newest delivery.
    let hardest = hardest_of(&delivered, points, HARDEST_LIMIT);

    // Delivered statuses seen in the period plus those chosen in the scope.
    let mut delivered_names: Vec<String> = statuses_of(&delivered)
        .into_iter()
        .map(|s| s.name)
        .collect();
    let mut known: HashSet<String> = delivered_names.iter().map(|n| n.to_lowercase()).collect();
    for name in &scope.delivered_statuses {
        let trimmed = js_trim(name);
        if !trimmed.is_empty() && known.insert(trimmed.to_lowercase()) {
            delivered_names.push(trimmed.to_string());
        }
    }

    ReportData {
        workspace_name: input.workspace_name.clone(),
        period_label: period.label.clone(),
        generated_label: format!(
            "{} {} {}",
            generated_at.day(),
            MONTHS[generated_at.month0() as usize],
            generated_at.year()
        ),
        scored_with: input
            .ai
            .as_ref()
            .map(|ai| format!("{} ({})", ai.model, runner_label(&ai.runner))),
        notes,
        comparison_note,
        tiles,
        members: members.clone(),
        unassigned,
        trend: Trend {
            unit: plan.unit,
            buckets,
        },
        load: members
            .iter()
            .map(|m| LoadRow {
                id: m.id.clone(),
                name: m.name.clone(),
                delivered: m.delivered,
                active: m.active,
            })
            .collect(),
        hardest,
        monthly: matches!(period.kind, PeriodKind::Quarter | PeriodKind::Year).then(|| {
            build_monthly(
                period,
                &delivered,
                &active,
                (&members, unassigned),
                points,
                generated_at,
            )
        }),
        statuses: Statuses {
            delivered: delivered_names,
            excluded: scope
                .excluded_statuses
                .iter()
                .map(|s| js_trim(s).to_string())
                .filter(|s| !s.is_empty())
                .collect(),
        },
        excluded_members: excluded_members_of(&input.tickets, scope)
            .iter()
            .map(|m| m.name.clone())
            .collect(),
        points: *points,
    }
}

// ----------------------------------------------------------------- SVG parts

const W: f64 = 460.0;
const ROW: f64 = 30.0;
const BAR: f64 = 18.0;

/// Rounded data end only, square at the baseline.
fn hbar_path(x: f64, y: f64, w: f64, h: f64, r: f64) -> String {
    let rr = r.min(w).min(h / 2.0);
    format!(
        "M{},{}H{}Q{},{} {},{}V{}Q{},{} {},{}H{}Z",
        num(x),
        num(y),
        num(x + w - rr),
        num(x + w),
        num(y),
        num(x + w),
        num(y + rr),
        num(y + h - rr),
        num(x + w),
        num(y + h),
        num(x + w - rr),
        num(y + h),
        num(x)
    )
}

fn vbar_path(x: f64, y: f64, w: f64, h: f64, r: f64) -> String {
    let rr = r.min(h).min(w / 2.0);
    format!(
        "M{},{}V{}Q{},{} {},{}H{}Q{},{} {},{}V{}Z",
        num(x),
        num(y + h),
        num(y + rr),
        num(x),
        num(y),
        num(x + rr),
        num(y),
        num(x + w - rr),
        num(x + w),
        num(y),
        num(x + w),
        num(y + rr),
        num(y + h)
    )
}

/// Axis ticks from 0 up to a round value at or above `max`.
fn nice_ticks(max: f64, target: u32) -> Vec<f64> {
    if max.is_nan() || max <= 0.0 {
        return vec![0.0, 1.0];
    }
    let raw = (max / f64::from(target)).max(1.0);
    let pow = 10f64.powi(raw.log10().floor() as i32);
    let step = [1.0, 2.0, 5.0, 10.0]
        .iter()
        .map(|k| k * pow)
        .find(|s| *s >= raw)
        .unwrap_or(10.0 * pow);
    let mut ticks = Vec::new();
    let mut v = 0.0;
    while v < max + step {
        ticks.push(v);
        if v >= max {
            break;
        }
        v += step;
    }
    ticks
}

fn svg_open(h: f64, label: &str) -> String {
    format!(
        r#"<svg viewBox="0 0 {} {}" width="100%" role="img" aria-label="{}">"#,
        n(W),
        n(h),
        esc(label)
    )
}

fn empty(text: &str) -> String {
    format!(r#"<p class="empty">{}</p>"#, esc(text))
}

/// Characters of a member name that fit the 100px left margin of the bar charts.
const NAME_MAX: usize = 14;

fn name_text(x: f64, y: f64, name: &str, max: usize) -> String {
    let clipped = clip(name, max);
    let title = if clipped == name {
        String::new()
    } else {
        format!("<title>{}</title>", esc(name))
    };
    format!(
        r#"<text x="{}" y="{}" text-anchor="end">{}{title}</text>"#,
        n(x),
        n(y),
        esc(&clipped)
    )
}

/// `Math.max(...values)`; `-Infinity` for none.
fn max_of(values: impl Iterator<Item = f64>) -> f64 {
    values.fold(f64::NEG_INFINITY, f64::max)
}

fn points_chart(members: &[ReportMember]) -> String {
    if members.is_empty() {
        return empty("No delivered tickets in this period.");
    }
    let l = 100.0;
    let r = if members.iter().any(|m| m.faded && m.points > 0.0) {
        118.0
    } else {
        48.0
    };
    let h = members.len() as f64 * ROW + 8.0;
    let max = {
        let max = max_of(members.iter().map(|m| m.points));
        if max == 0.0 || max.is_nan() {
            1.0
        } else {
            max
        }
    };
    let mut out = vec![
        svg_open(h, "Points by member, horizontal bars"),
        format!(
            r#"<line x1="{}" x2="{}" y1="0" y2="{}" stroke="{COLOR_AXIS}"/>"#,
            n(l),
            n(l),
            n(h - 4.0)
        ),
    ];
    for (i, m) in members.iter().enumerate() {
        let y = i as f64 * ROW + 6.0;
        let w = ((W - l - r) * m.points) / max;
        let pct = m.coverage_percent;
        out.push(name_text(l - 8.0, y + 14.0, &m.name, NAME_MAX));
        if m.points > 0.0 {
            out.push(format!(
                r#"<path d="{}" fill="{COLOR_SERIES1}"{}/>"#,
                hbar_path(l, y, w, BAR, 4.0),
                if m.faded { r#" opacity="0.35""# } else { "" }
            ));
            out.push(format!(
                r#"<rect x="{}" y="{}" width="{}" height="26" fill="transparent"><title>{}</title></rect>"#,
                n(l),
                n(y - 4.0),
                n(W - l),
                esc(&format!("{}: {} points, coverage {pct}%", m.name, fmt(m.points)))
            ));
        }
        let label = if m.delivered == 0 {
            "no delivered work".to_string()
        } else if m.points > 0.0 {
            format!(
                "{}{}",
                fmt(m.points),
                if m.faded {
                    format!(" ({pct}% scored)")
                } else {
                    String::new()
                }
            )
        } else {
            "not scored yet".to_string()
        };
        out.push(format!(
            r#"<text x="{}" y="{}" class="{}">{}</text>"#,
            num(l + w + 6.0),
            n(y + 14.0),
            if m.faded || m.delivered == 0 {
                "muted"
            } else {
                "val"
            },
            esc(&label)
        ));
    }
    out.push("</svg>".into());
    out.concat()
}

fn levels_chart(members: &[ReportMember]) -> String {
    let scored: Vec<&ReportMember> = members.iter().filter(|m| m.scored > 0).collect();
    if scored.is_empty() {
        return empty("No scored tickets yet.");
    }
    let l = 100.0;
    let r = 30.0;
    let h = scored.len() as f64 * ROW + 8.0;
    let total_of = |m: &ReportMember| LEVELS.iter().map(|lv| m.levels[*lv]).sum::<usize>();
    let max = {
        let max = max_of(scored.iter().map(|m| total_of(m) as f64));
        if max == 0.0 || max.is_nan() {
            1.0
        } else {
            max
        }
    };
    let mut out = vec![
        svg_open(h, "Scored tickets per level per member, stacked bars"),
        format!(
            r#"<line x1="{}" x2="{}" y1="0" y2="{}" stroke="{COLOR_AXIS}"/>"#,
            n(l),
            n(l),
            n(h - 4.0)
        ),
    ];
    for (i, m) in scored.iter().enumerate() {
        let y = i as f64 * ROW + 6.0;
        let mut x = l;
        out.push(name_text(l - 8.0, y + 14.0, &m.name, NAME_MAX));
        let segments: Vec<Level> = LEVELS
            .iter()
            .copied()
            .filter(|lv| m.levels[*lv] > 0)
            .collect();
        for (j, level) in segments.iter().enumerate() {
            let count = m.levels[*level];
            let w = ((W - l - r) * count as f64) / max;
            let title = format!(
                "<title>{}</title>",
                esc(&format!(
                    "{}: {} {count} {}",
                    m.name,
                    level.as_str(),
                    if count == 1 { "ticket" } else { "tickets" }
                ))
            );
            if j == segments.len() - 1 {
                out.push(format!(
                    r#"<path d="{}" fill="{}">{title}</path>"#,
                    hbar_path(x, y, w, BAR, 4.0),
                    level_color(*level)
                ));
            } else {
                out.push(format!(
                    r#"<rect x="{}" y="{}" width="{}" height="{}" fill="{}">{title}</rect>"#,
                    num(x),
                    n(y),
                    num((w - 2.0).max(0.0)),
                    n(BAR),
                    level_color(*level)
                ));
            }
            x += w;
        }
        out.push(format!(
            r#"<text x="{}" y="{}" class="val">{}</text>"#,
            num(x + 6.0),
            n(y + 14.0),
            total_of(m)
        ));
    }
    out.push("</svg>".into());
    out.concat()
}

fn trend_chart(trend: &Trend) -> String {
    let buckets = &trend.buckets;
    if buckets.is_empty() {
        return empty("No delivered tickets in this period.");
    }
    let h = 220.0;
    let l = 40.0;
    let r = 26.0;
    let t = 14.0;
    let b = 28.0;
    let ticks = nice_ticks(max_of(buckets.iter().map(|b| b.points)), 4);
    let top = *ticks.last().expect("ticks");
    let y_of = |v: f64| t + (h - t - b) * (1.0 - v / top);
    let plot_w = W - l - r;
    let count = buckets.len();
    let step = if count > 1 {
        plot_w / (count as f64 - 1.0)
    } else {
        plot_w
    };
    let x_of = |i: usize| {
        if count > 1 {
            l + i as f64 * step
        } else {
            l + plot_w / 2.0
        }
    };
    let mut out = vec![svg_open(
        h,
        &format!("Team points per {}, line chart", trend.unit.as_str()),
    )];
    for v in &ticks {
        out.push(format!(
            r#"<line x1="{}" x2="{}" y1="{}" y2="{}" stroke="{}"/>"#,
            n(l),
            n(W - r),
            num(y_of(*v)),
            num(y_of(*v)),
            if *v == 0.0 { COLOR_AXIS } else { COLOR_GRID }
        ));
        out.push(format!(
            r#"<text x="{}" y="{}" text-anchor="end" class="muted">{}</text>"#,
            n(l - 6.0),
            num(y_of(*v) + 4.0),
            fmt(*v)
        ));
    }
    let coords: Vec<String> = buckets
        .iter()
        .enumerate()
        .map(|(i, bk)| format!("{},{}", num(x_of(i)), num(y_of(bk.points))))
        .collect();
    if count > 1 {
        out.push(format!(
            r#"<path d="M{}" fill="none" stroke="{COLOR_SERIES1}" stroke-width="2" stroke-linejoin="round"/>"#,
            coords.join("L")
        ));
    }
    // Skip labels when they would run into each other.
    let widest = buckets.iter().map(|b| js_len(&b.label)).max().unwrap_or(0);
    let every = (((widest as f64 * 7.0 + 8.0) / step).ceil().max(1.0)) as usize;
    for (i, bk) in buckets.iter().enumerate() {
        let x = x_of(i);
        if i % every == 0 {
            out.push(format!(
                r#"<text x="{}" y="{}" text-anchor="middle" class="muted">{}</text>"#,
                num(x),
                n(h - 8.0),
                esc(&bk.label)
            ));
        }
        out.push(format!(
            r#"<circle cx="{}" cy="{}" r="4" fill="{COLOR_SERIES1}" stroke="{COLOR_SURFACE}" stroke-width="2"/>"#,
            num(x),
            num(y_of(bk.points))
        ));
        out.push(format!(
            r#"<rect x="{}" y="{}" width="{}" height="{}" fill="transparent"><title>{}</title></rect>"#,
            num(x - if count > 1 { step / 2.0 } else { plot_w / 2.0 }),
            n(t),
            num(if count > 1 { step } else { plot_w }),
            n(h - t - b),
            esc(&format!("{}: {} points", bk.title, fmt(bk.points)))
        ));
    }
    let last_index = count - 1;
    let last_y = y_of(buckets[last_index].points);
    out.push(format!(
        r#"<text x="{}" y="{}" text-anchor="end" class="val">{}</text>"#,
        num(x_of(last_index) - 4.0),
        num(if last_y - 10.0 < t {
            last_y + 18.0
        } else {
            last_y - 10.0
        }),
        fmt(buckets[last_index].points)
    ));
    out.push("</svg>".into());
    out.concat()
}

fn load_chart(load: &[LoadRow]) -> String {
    if load.is_empty() {
        return empty("No work to show in this period.");
    }
    let h = 220.0;
    let l = 30.0;
    let r = 8.0;
    let t = 14.0;
    let b = 34.0;
    let ticks = nice_ticks(
        max_of(
            load.iter()
                .flat_map(|m| [m.delivered as f64, m.active as f64]),
        ),
        2,
    );
    let top = *ticks.last().expect("ticks");
    let y_of = |v: f64| t + (h - t - b) * (1.0 - v / top);
    let band = (W - l - r) / load.len() as f64;
    let bw = 2.0_f64.max(14.0_f64.min((band - 6.0) / 2.0));
    let mut out = vec![svg_open(
        h,
        "Delivered and active tickets per member, grouped bars",
    )];
    for v in &ticks {
        out.push(format!(
            r#"<line x1="{}" x2="{}" y1="{}" y2="{}" stroke="{}"/>"#,
            n(l),
            n(W - r),
            num(y_of(*v)),
            num(y_of(*v)),
            if *v == 0.0 { COLOR_AXIS } else { COLOR_GRID }
        ));
        out.push(format!(
            r#"<text x="{}" y="{}" text-anchor="end" class="muted">{}</text>"#,
            n(l - 6.0),
            num(y_of(*v) + 4.0),
            fmt(*v)
        ));
    }
    let max_chars = 2.0_f64.max((band / 6.5).floor()) as usize;
    for (i, m) in load.iter().enumerate() {
        let cx = l + band * i as f64 + band / 2.0;
        let bars: [(&str, usize, &str, f64); 2] = [
            ("Delivered", m.delivered, COLOR_SERIES1, -bw - 1.0),
            ("Active", m.active, COLOR_SERIES2, 1.0),
        ];
        for (label, value, color, dx) in bars {
            if value == 0 {
                continue;
            }
            let y = y_of(value as f64);
            out.push(format!(
                r#"<path d="{}" fill="{color}"><title>{}</title></path>"#,
                vbar_path(cx + dx, y, bw, y_of(0.0) - y, 4.0),
                esc(&format!(
                    "{}: {label} {value} {}",
                    m.name,
                    if value == 1 { "ticket" } else { "tickets" }
                ))
            ));
        }
        // `name.trim().split(/\s+/)[0]`
        let first: String = js_trim(&m.name)
            .chars()
            .take_while(|c| !js_space(*c))
            .collect();
        out.push(format!(
            r#"<text x="{}" y="{}" text-anchor="middle" class="muted">{}<title>{}</title></text>"#,
            num(cx),
            n(h - 16.0),
            esc(&clip(&first, max_chars)),
            esc(&m.name)
        ));
    }
    out.push("</svg>".into());
    out.concat()
}

// ---------------------------------------------------------------------- HTML

const CSS_TEMPLATE: &str = r#"
  .viz-root {
    color-scheme: light;
    --page: @page@;
    --surface-1: @surface@;
    --text-primary: @textPrimary@;
    --text-secondary: @textSecondary@;
    --text-muted: @textMuted@;
    --grid: @grid@;
    --axis: @axis@;
    --border: @border@;
    --series-1: @series1@;
    --series-2: @series2@;
    --warn-bg: @warnBg@;
    --warn-ink: @warnInk@;
    --ink-on-shade: @inkOnShade@;
  }
  * { box-sizing: border-box; }
  body { margin: 0; background: var(--page); color: var(--text-primary);
    font: 14px/1.5 system-ui, -apple-system, "Segoe UI", sans-serif; }
  .page { max-width: 1040px; margin: 0 auto; padding: 32px 24px 64px; }
  header h1 { font-size: 22px; margin: 0 0 4px; }
  header .meta { color: var(--text-secondary); font-size: 13px; }
  .notes { display: flex; flex-direction: column; gap: 6px; margin: 16px 0 24px; }
  .note { background: var(--warn-bg); color: var(--warn-ink); border-radius: 8px; padding: 8px 12px; font-size: 13px; }
  .note b::before { content: "\26A0  "; }
  .tiles { display: grid; grid-template-columns: repeat(auto-fit, minmax(150px, 1fr)); gap: 12px; margin: 24px 0; }
  .notes + .tiles { margin-top: 0; }
  .tile { background: var(--surface-1); border: 1px solid var(--border); border-radius: 12px; padding: 14px 16px; }
  .tile .label { color: var(--text-secondary); font-size: 12px; }
  .tile .value { font-size: 26px; font-weight: 600; margin-top: 2px; }
  .tile .sub { color: var(--text-muted); font-size: 12px; }
  .grid2 { display: grid; grid-template-columns: 1fr 1fr; gap: 16px; }
  @media (max-width: 820px) { .grid2 { grid-template-columns: 1fr; } }
  .card { background: var(--surface-1); border: 1px solid var(--border); border-radius: 12px; padding: 16px; margin-bottom: 16px; }
  .card h2 { font-size: 15px; margin: 0 0 2px; }
  .card .desc { color: var(--text-secondary); font-size: 12px; margin-bottom: 10px; }
  .empty { color: var(--text-muted); font-size: 13px; margin: 8px 0; }
  .legend { display: flex; gap: 14px; flex-wrap: wrap; font-size: 12px; color: var(--text-secondary); margin-bottom: 6px; }
  .legend span::before { content: ""; display: inline-block; width: 10px; height: 10px; border-radius: 3px;
    background: var(--c); margin-right: 6px; vertical-align: -1px; }
  svg text { font: 12px system-ui, -apple-system, "Segoe UI", sans-serif; fill: var(--text-secondary); }
  svg .val { fill: var(--text-primary); font-variant-numeric: tabular-nums; }
  svg .muted { fill: var(--text-muted); }
  table { width: 100%; border-collapse: collapse; font-size: 13px; }
  th, td { padding: 7px 8px; border-bottom: 1px solid var(--grid); text-align: right; font-variant-numeric: tabular-nums; }
  th:first-child, td:first-child { text-align: left; }
  th { color: var(--text-secondary); font-weight: 600; font-size: 12px; }
  tr.faded td, tr.unassigned td { color: var(--text-muted); }
  .pill { display: inline-block; min-width: 26px; padding: 1px 6px; border-radius: 6px; font-size: 11px;
    font-weight: 600; color: #fff; text-align: center; }
  .edited { color: var(--text-muted); font-size: 11px; }
  table.top td:nth-child(2), table.top th:nth-child(2) { text-align: left; }
  td.why, th.why { color: var(--text-secondary); font-size: 12px; text-align: left; }
  table.top a { color: inherit; }
  .method { color: var(--text-secondary); font-size: 13px; }
  .method table { width: auto; }
  .months .desc { margin-bottom: 12px; }
  table.matrix th, table.matrix td { text-align: center; }
  table.matrix th:first-child, table.matrix td:first-child { text-align: left; }
  table.matrix td.cell { color: var(--text-primary); }
  table.matrix td.cell .sub, table.matrix td.total .sub { color: var(--text-secondary); font-size: 11px; }
  table.matrix td.shaded .sub { color: var(--ink-on-shade); }
  table.matrix tr.faded td:not(:first-child) { opacity: 0.6; }
  table.matrix td.total { font-weight: 600; border-left: 1px solid var(--axis); }
  table.matrix td.none { color: var(--text-muted); }
  table.matrix tr.unassigned td, table.matrix tr.faded td:first-child { color: var(--text-muted); }
  details.month { border-top: 1px solid var(--grid); margin-top: 12px; padding-top: 8px; }
  details.month summary { cursor: pointer; font-weight: 600; font-size: 14px; padding: 4px 0; }
  details.month summary .meta { color: var(--text-secondary); font-weight: 400; }
  details.month h3 { font-size: 13px; margin: 14px 0 6px; color: var(--text-secondary); }
  .tiles.mini { margin: 12px 0; gap: 8px; }
  .tiles.mini .tile { padding: 8px 12px; }
  .tiles.mini .value { font-size: 20px; }
  @media print { .page { padding: 0; } .card, .tile { break-inside: avoid; } }
"#;

fn css() -> String {
    [
        ("@page@", COLOR_PAGE),
        ("@surface@", COLOR_SURFACE),
        ("@textPrimary@", COLOR_TEXT_PRIMARY),
        ("@textSecondary@", COLOR_TEXT_SECONDARY),
        ("@textMuted@", COLOR_TEXT_MUTED),
        ("@grid@", COLOR_GRID),
        ("@axis@", COLOR_AXIS),
        ("@border@", COLOR_BORDER),
        ("@series1@", COLOR_SERIES1),
        ("@series2@", COLOR_SERIES2),
        ("@warnBg@", COLOR_WARN_BG),
        ("@warnInk@", COLOR_WARN_INK),
        ("@inkOnShade@", COLOR_INK_ON_SHADE),
    ]
    .iter()
    .fold(CSS_TEMPLATE.to_string(), |css, (key, value)| {
        css.replace(key, value)
    })
}

fn legend_of_levels() -> String {
    format!(
        r#"<div class="legend">{}</div>"#,
        LEVELS
            .iter()
            .map(|l| format!(
                r#"<span style="--c:{}">{}</span>"#,
                level_color(*l),
                l.as_str()
            ))
            .collect::<String>()
    )
}

fn members_table(
    members: &[ReportMember],
    unassigned: Option<Unassigned>,
    empty_text: &str,
) -> String {
    if members.is_empty() && unassigned.is_none() {
        return empty(empty_text);
    }
    let level_heads: String = LEVELS
        .iter()
        .map(|l| format!("<th>{}</th>", l.as_str()))
        .collect();
    let head = format!(
        "<tr><th>Member</th><th>Points</th><th>Share</th><th>Delivered</th><th>Coverage</th>{level_heads}<th>Avg / ticket</th><th>Active</th></tr>"
    );
    let mut rows: Vec<String> = members
        .iter()
        .map(|m| {
            let levels: String = LEVELS.iter().map(|l| format!("<td>{}</td>", m.levels[*l])).collect();
            format!(
                "<tr{}><td>{}</td><td>{}</td><td>{}%</td><td>{}</td><td>{}/{}</td>{levels}<td>{}</td><td>{}</td></tr>",
                if m.faded { r#" class="faded""# } else { "" },
                esc(&m.name),
                fmt(m.points),
                m.share_percent,
                m.delivered,
                m.scored,
                m.delivered,
                m.avg_points.map_or_else(|| "-".to_string(), fmt),
                m.active
            )
        })
        .collect();
    if let Some(u) = unassigned {
        rows.push(format!(
            r#"<tr class="unassigned"><td>Unassigned</td><td>-</td><td>-</td><td>{}</td><td>{}/{}</td><td colspan="{}"></td></tr>"#,
            u.delivered,
            u.scored,
            u.delivered,
            LEVELS.len() + 2
        ));
    }
    format!("<table>{head}{}</table>", rows.concat())
}

fn hardest_table(hardest: &[HardestTicket], empty_text: &str) -> String {
    if hardest.is_empty() {
        return empty(empty_text);
    }
    let head = r#"<tr><th>Ticket</th><th>Member</th><th>Level</th><th>Points</th><th class="why">Main reason</th></tr>"#;
    let rows: String = hardest
        .iter()
        .map(|h| {
            let title = match &h.url {
                Some(url) => format!(
                    r#"<a href="{}" target="_blank" rel="noopener noreferrer">{}</a>"#,
                    esc(url),
                    esc(&h.title)
                ),
                None => esc(&h.title),
            };
            format!(
                r#"<tr><td>{title}</td><td>{}</td><td><span class="pill" style="background:{}">{}</span>{}</td><td>{}</td><td class="why">{}</td></tr>"#,
                esc(&h.members),
                level_color(h.level),
                h.level.as_str(),
                if h.edited { r#" <span class="edited">edited</span>"# } else { "" },
                fmt(h.points),
                esc(&h.reason)
            )
        })
        .collect();
    format!(r#"<table class="top">{head}{rows}</table>"#)
}

/// Points with the delivered count small and muted next to them.
fn matrix_cell(cell: &MatrixCell, unassigned: bool, title: &str, total: bool) -> String {
    let class = if total {
        "total"
    } else if cell.shade.is_none() {
        "cell"
    } else {
        "cell shaded"
    };
    if cell.delivered == 0 && cell.points == 0.0 {
        return format!(
            r#"<td class="none{}" title="{}">-</td>"#,
            if total { " total" } else { "" },
            esc(title)
        );
    }
    let style = cell.shade.map_or_else(String::new, |s| {
        format!(r#" style="background:{}""#, CELL_RAMP[s])
    });
    let body = if unassigned {
        cell.delivered.to_string()
    } else {
        format!(
            r#"{} <span class="sub">&middot; {}</span>"#,
            fmt(cell.points),
            cell.delivered
        )
    };
    format!(
        r#"<td class="{class}"{style} title="{}">{body}</td>"#,
        esc(title)
    )
}

fn matrix_table(monthly: &ReportMonthly, period_label: &str) -> String {
    if monthly.rows.is_empty() {
        return empty("No delivered tickets in this period.");
    }
    let month_heads: String = monthly
        .months
        .iter()
        .map(|m| format!("<th>{}</th>", esc(m.label)))
        .collect();
    let head = format!("<tr><th>Member</th>{month_heads}<th>Total</th></tr>");
    let text = |c: &MatrixCell, unassigned: bool| {
        format!(
            "{}{} delivered, {} scored",
            if unassigned {
                String::new()
            } else {
                format!("{} points, ", fmt(c.points))
            },
            c.delivered,
            c.scored
        )
    };
    let rows: String = monthly
        .rows
        .iter()
        .map(|row| {
            let mut cells: Vec<String> = row
                .cells
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    matrix_cell(
                        c,
                        row.unassigned,
                        &format!(
                            "{}, {}: {}",
                            row.name,
                            monthly.months[i].title,
                            text(c, row.unassigned)
                        ),
                        false,
                    )
                })
                .collect();
            cells.push(matrix_cell(
                &row.total,
                row.unassigned,
                &format!(
                    "{}, {period_label}: {}",
                    row.name,
                    text(&row.total, row.unassigned)
                ),
                true,
            ));
            let row_class = if row.unassigned {
                "unassigned"
            } else if row.faded {
                "faded"
            } else {
                ""
            };
            format!(
                "<tr{}><td>{}</td>{}</tr>",
                if row_class.is_empty() {
                    String::new()
                } else {
                    format!(r#" class="{row_class}""#)
                },
                esc(&row.name),
                cells.concat()
            )
        })
        .collect();
    format!(r#"<table class="matrix">{head}{rows}</table>"#)
}

fn tile_html(t: &ReportTile) -> String {
    format!(
        r#"<div class="tile"><div class="label">{}</div><div class="value">{}</div>{}</div>"#,
        esc(t.label),
        esc(&t.value),
        match t.sub.as_deref().filter(|s| !s.is_empty()) {
            Some(sub) => format!(r#"<div class="sub">{}</div>"#, esc(sub)),
            None => String::new(),
        }
    )
}

fn month_block(month: &ReportMonth) -> String {
    let tiles: String = month.tiles.iter().map(tile_html).collect();
    let note = match &month.note {
        Some(n) => format!(
            r#"<div class="notes"><div class="note"><b>{}</b> {}</div></div>"#,
            esc(&n.headline),
            esc(&n.text)
        ),
        None => String::new(),
    };
    format!(
        r#"<details class="month">
      <summary>{} <span class="meta">&middot; {} delivered &middot; {} points</span></summary>
      {note}
      <div class="tiles mini">{tiles}</div>
      <h3>Member table</h3>
      {}
      <h3>Hardest tickets</h3>
      {}
    </details>"#,
        esc(&month.title),
        month.delivered,
        fmt(month.points),
        members_table(
            &month.members,
            month.unassigned,
            "No members to show in this month."
        ),
        hardest_table(&month.hardest, "No scored tickets in this month.")
    )
}

fn monthly_section(data: &ReportData) -> String {
    let Some(monthly) = data.monthly.as_ref().filter(|m| !m.months.is_empty()) else {
        return String::new();
    };
    let blocks = if monthly.rows.is_empty() {
        String::new()
    } else {
        monthly
            .months
            .iter()
            .map(month_block)
            .collect::<Vec<_>>()
            .join("\n    ")
    };
    format!(
        r#"<section class="card months">
    <h2>Month by month</h2>
    {}
    {}
    {blocks}
  </section>
"#,
        if blocks.is_empty() {
            ""
        } else {
            r#"<div class="desc">Click a month to see its details. Expand the months you want before printing.</div>"#
        },
        matrix_table(monthly, &data.period_label)
    )
}

fn method_section(data: &ReportData) -> String {
    let sums = ["0-1", "2-3", "4-5", "6-7", "8-10"];
    let delivered = if data.statuses.delivered.is_empty() {
        "none in this period".to_string()
    } else {
        data.statuses.delivered.join(", ")
    };
    let excluded = if data.statuses.excluded.is_empty() {
        String::new()
    } else {
        format!(" Excluded statuses: {}.", data.statuses.excluded.join(", "))
    };
    let members = if data.excluded_members.is_empty() {
        String::new()
    } else {
        format!(
            " Excluded from this board: {}.",
            data.excluded_members.join(", ")
        )
    };
    let sum_heads: String = sums.iter().map(|s| format!("<th>{s}</th>")).collect();
    let level_cells: String = LEVELS
        .iter()
        .map(|l| format!("<td>{}</td>", l.as_str()))
        .collect();
    let point_cells: String = LEVELS
        .iter()
        .map(|l| format!("<td>{}</td>", fmt(data.points[*l])))
        .collect();
    format!(
        r#"<section class="card method">
    <h2>How the score works</h2>
    <p>Each delivered ticket is judged by an AI on five criteria, 0 to 2 each: ambiguity, investigation, system scope, risk and coordination. The app sums them (0 to 10) and maps the sum to a level, which earns points. Difficulty is about the problem, not the size of the code change. Due dates are not used.</p>
    <table>
      <tr><th>Sum</th>{sum_heads}</tr>
      <tr><td>Level</td>{level_cells}</tr>
      <tr><td>Points</td>{point_cells}</tr>
    </table>
    <p>Delivered statuses: {}.{}{} Subtasks count; a parent with subtasks does not. Active tickets are shown for context and never scored.{}</p>
    <p><b>Read this as a conversation starter, not a performance rating.</b> Work outside ClickUp, mentoring and unscored tickets are not in these numbers.</p>
  </section>"#,
        esc(&delivered),
        esc(&excluded),
        "",
        esc(&members)
    )
}

pub fn render_report_html(data: &ReportData) -> String {
    let mut meta_parts = vec![
        format!("Workspace <b>{}</b>", esc(&data.workspace_name)),
        format!("Period <b>{}</b>", esc(&data.period_label)),
        format!("Generated {}", esc(&data.generated_label)),
    ];
    if let Some(scored_with) = &data.scored_with {
        meta_parts.push(format!("Scored with {}", esc(scored_with)));
    }
    meta_parts.push("Monoptah".into());
    let meta = meta_parts.join(" &middot; ");
    let mut note_rows: Vec<String> = Vec::new();
    if let Some(note) = &data.comparison_note {
        note_rows.push(format!(r#"<div class="note">{}</div>"#, esc(note)));
    }
    for n in &data.notes {
        note_rows.push(format!(
            r#"<div class="note"><b>{}</b> {}</div>"#,
            esc(&n.headline),
            esc(&n.text)
        ));
    }
    let notes = if note_rows.is_empty() {
        String::new()
    } else {
        format!(r#"<div class="notes">{}</div>"#, note_rows.concat())
    };
    let tiles: String = data.tiles.iter().map(tile_html).collect();
    let unit = data.trend.unit.as_str();

    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>{title}</title>
<meta name="viewport" content="width=device-width, initial-scale=1">
<style>{css}</style>
</head>
<body class="viz-root">
<div class="page">
  <header>
    <h1>Ticket Contributors report</h1>
    <div class="meta">{meta}</div>
  </header>
  {notes}
  <section class="tiles" aria-label="Summary">{tiles}</section>

  <div class="grid2">
    <section class="card">
      <h2>Points by member</h2>
      <div class="desc">Sum of ticket points, split equally between assignees.</div>
      {points_chart}
    </section>
    <section class="card">
      <h2>Difficulty mix by member</h2>
      <div class="desc">Scored tickets by level, light = small, dark = hard.</div>
      {legend}
      {levels_chart}
    </section>
  </div>

  <div class="grid2">
    <section class="card">
      <h2>Team points per {unit}</h2>
      <div class="desc">By delivery {unit}, scored tickets only.</div>
      {trend_chart}
    </section>
    <section class="card">
      <h2>Delivered and active work</h2>
      <div class="desc">Delivered tickets in the period next to tickets still in a working status.</div>
      <div class="legend"><span style="--c:{series1}">Delivered</span><span style="--c:{series2}">Active</span></div>
      {load_chart}
    </section>
  </div>

  {monthly}
  <section class="card">
    <h2>Member table</h2>
    <div class="desc">Every number behind the charts. Coverage is scored tickets out of delivered tickets. A ticket with several assignees counts for each of them; unassigned tickets have their own row.</div>
    {members_table}
  </section>

  <section class="card">
    <h2>Hardest tickets this period</h2>
    <div class="desc">Top {limit} by level, with the AI's main reason. Overridden levels are marked "edited".</div>
    {hardest_table}
  </section>

  {method}
</div>
</body>
</html>
"#,
        title = esc(&format!(
            "Ticket Contributors report - {} - {}",
            data.workspace_name, data.period_label
        )),
        css = css(),
        points_chart = points_chart(&data.members),
        legend = legend_of_levels(),
        levels_chart = levels_chart(&data.members),
        trend_chart = trend_chart(&data.trend),
        series1 = COLOR_SERIES1,
        series2 = COLOR_SERIES2,
        load_chart = load_chart(&data.load),
        monthly = monthly_section(data),
        members_table = members_table(
            &data.members,
            data.unassigned,
            "No members to show in this period."
        ),
        limit = HARDEST_LIMIT,
        hardest_table = hardest_table(&data.hardest, "No scored tickets in this period."),
        method = method_section(data),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contributors::model::js_iso;
    use crate::contributors::model::period::{current_period, previous_period, Period, PeriodKind};
    use crate::contributors::model::test_fixtures::{local_at, local_date};
    use crate::contributors::ticket_store::Criteria;
    use crate::contributors::tickets::test_fixtures::{
        assessment_of, named, strings, ticket_with, uniform,
    };
    use crate::contributors::tickets::tickets::{
        ExcludedMember, TicketAssignee, DEFAULT_TICKET_POINTS,
    };
    use regex::Regex;
    use std::collections::BTreeSet;

    fn now() -> DateTime<Local> {
        local_at(2026, 9, 18, 12, 0)
    }

    fn october() -> Period {
        current_period(PeriodKind::Month, &now(), None)
    }

    fn september() -> Period {
        previous_period(&october()).unwrap()
    }

    fn scope() -> WorkspaceScope {
        WorkspaceScope {
            workspace_id: "w1".into(),
            name: "Casium".into(),
            space_ids: None,
            excluded_statuses: vec![],
            delivered_statuses: vec![],
            excluded_members: vec![],
        }
    }

    fn user(id: &str) -> TicketAssignee {
        named(id, &format!("User {id}"))
    }

    fn assessment(
        criteria: Criteria,
        reason: &str,
    ) -> crate::contributors::tickets::tickets::TicketAssessment {
        assessment_of(criteria, reason)
    }

    /// A local-time noon date as an ISO string, away from day edges.
    fn at(y: i32, m: u32, d: u32) -> String {
        js_iso(&local_at(y, m - 1, d, 12, 0))
    }

    fn ticket(id: &str, patch: impl FnOnce(&mut TicketRecord)) -> TicketRecord {
        ticket_with(id, |t| {
            t.updated_at = at(2026, 10, 7);
            t.done_at = Some(at(2026, 10, 7));
            patch(t);
        })
    }

    fn level(t: &mut TicketRecord, level: &str) {
        t.override_level = Some(level.into());
    }

    fn done(t: &mut TicketRecord, when: String) {
        t.done_at = Some(when);
    }

    fn input(tickets: Vec<TicketRecord>, patch: impl FnOnce(&mut ReportInput)) -> ReportInput {
        let mut i = ReportInput {
            workspace_name: "Casium".into(),
            period: october(),
            previous_period: Some(september()),
            scope: scope(),
            points: DEFAULT_TICKET_POINTS,
            tickets,
            previous_tickets: vec![],
            ai: Some(AiConfigStatus {
                runner: "claude-code".into(),
                model: "claude-sonnet-5-5".into(),
                has_api_key: false,
            }),
            generated_at: now(),
            previous_incomplete: false,
        };
        patch(&mut i);
        i
    }

    fn report(tickets: Vec<TicketRecord>) -> ReportData {
        build_report(&input(tickets, |_| {}))
    }

    fn html_of(tickets: Vec<TicketRecord>) -> String {
        render_report_html(&report(tickets))
    }

    fn tile<'a>(data: &'a ReportData, id: &str) -> &'a ReportTile {
        data.tiles.iter().find(|t| t.id == id).unwrap()
    }

    fn sub(data: &ReportData, id: &str) -> Option<String> {
        tile(data, id).sub.clone()
    }

    fn count(haystack: &str, needle: &str) -> usize {
        haystack.matches(needle).count()
    }

    // reportFileName

    #[test]
    fn report_file_name_slugs_the_workspace_and_appends_the_period_id() {
        assert_eq!(
            report_file_name("Casium Dev / EU!", &october()),
            "ticket-contributors-casium-dev-eu-2026-10.html"
        );
        assert_eq!(
            report_file_name("  ", &current_period(PeriodKind::Days7, &now(), None)),
            "ticket-contributors-workspace-7d.html"
        );
    }

    // renderReportHtml safety

    const EVIL: &str = r#"<script>alert(1)</script> "quoted" & 'single'"#;

    fn evil_tickets() -> Vec<TicketRecord> {
        vec![
            ticket("a", |t| {
                t.title = EVIL.into();
                t.url = r#"https://app.clickup.com/t/a?x=1&y="2""#.into();
                t.status = EVIL.into();
                t.assignees = vec![named("1", EVIL)];
                t.assessment = Some(assessment(uniform(2), EVIL));
            }),
            ticket("b", |t| {
                t.title = "plain http link".into();
                t.url = "http://insecure.example.com/t/b".into();
                t.assignees = vec![named("1", EVIL)];
                level(t, "L");
                t.override_note = Some(EVIL.into());
            }),
        ]
    }

    fn evil_report_input() -> ReportInput {
        input(evil_tickets(), |i| {
            i.workspace_name = EVIL.into();
            i.scope.delivered_statuses = strings(&[EVIL]);
            i.scope.excluded_statuses = strings(&["<i>excluded</i>"]);
            i.ai = Some(AiConfigStatus {
                runner: "api".into(),
                model: EVIL.into(),
                has_api_key: true,
            });
        })
    }

    fn evil_html() -> String {
        render_report_html(&build_report(&evil_report_input()))
    }

    #[test]
    fn render_report_html_safety_escapes_every_clickup_and_ai_string_and_has_no_script() {
        let out = evil_html();
        assert!(!out.contains("<script"));
        assert!(!out.contains("alert(1)</"));
        assert!(out.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(out.contains("&quot;quoted&quot;"));
        assert!(out.contains("&#39;single&#39;"));
        assert!(out.contains("Excluded statuses: &lt;i&gt;excluded&lt;/i&gt;"));
        assert!(!out.contains("<i>"));
    }

    #[test]
    fn render_report_html_safety_uses_no_http_and_no_external_host_besides_https_ticket_links() {
        let out = evil_html();
        assert!(!out.contains("http://"));
        let hosts: BTreeSet<String> = Regex::new(r#"https://([^/"'\s]+)"#)
            .unwrap()
            .captures_iter(&out)
            .map(|c| c[1].to_string())
            .collect();
        assert_eq!(hosts, BTreeSet::from(["app.clickup.com".to_string()]));
        assert!(!Regex::new(r"(?i)<(link|img|iframe|object|embed)\b")
            .unwrap()
            .is_match(&out));
        assert!(!out.contains("@import"));
        assert!(!Regex::new(r"url\(").unwrap().is_match(&out));
    }

    #[test]
    fn render_report_html_safety_has_no_event_handler_attribute_even_with_a_hostile_url() {
        let hostile = ticket("h", |t| {
            t.title = r#"x" onmouseover="alert(1)"#.into();
            t.url = r#"https://a.example.com/" onmouseover="alert(2)" x=""#.into();
            level(t, "XL");
            t.assignees = vec![named("1", r#"n" onclick="alert(3)"#)];
        });
        let mut tickets = evil_tickets();
        tickets.push(hostile);
        let out = html_of(tickets);
        assert!(out.contains("&quot; onmouseover=&quot;alert(2)"));
        // Drop quoted attribute values, then look for on...= inside any tag.
        let bare = Regex::new(r#""[^"]*""#).unwrap().replace_all(&out, "\"\"");
        assert!(!Regex::new(r"(?i)<[^<>]*\son[a-z]+\s*=")
            .unwrap()
            .is_match(&bare));
    }

    #[test]
    fn render_report_html_safety_links_a_ticket_only_when_its_url_is_https() {
        let data = report(evil_tickets());
        let secure = data.hardest.iter().find(|h| h.id == "a").unwrap();
        let insecure = data.hardest.iter().find(|h| h.id == "b").unwrap();
        assert_eq!(
            secure.url.as_deref(),
            Some(r#"https://app.clickup.com/t/a?x=1&y="2""#)
        );
        assert_eq!(insecure.url, None);
        let out = render_report_html(&data);
        assert!(out.contains(r#"href="https://app.clickup.com/t/a?x=1&amp;y=&quot;2&quot;""#));
        assert!(!out.contains("insecure.example.com"));
        assert!(out.contains("plain http link"));
    }

    // buildReport tiles

    fn tile_tickets() -> Vec<TicketRecord> {
        vec![
            ticket("a", |t| {
                t.assignees = vec![user("1"), user("2")];
                t.assessment = Some(assessment(uniform(1), "reason")); // M = 16, split 8 + 8
            }),
            ticket("b", |t| {
                t.assignees = vec![user("1")];
                level(t, "XL"); // 48
            }),
            ticket("c", |t| t.assignees = vec![user("2")]), // unscored
            ticket("d", |t| {
                t.assignees = vec![user("3")];
                t.assessment = Some(assessment(uniform(0), "reason")); // XS = 3
            }),
            ticket("out", |t| {
                done(t, at(2026, 9, 12));
                t.assignees = vec![user("1")];
            }),
        ]
    }

    #[test]
    fn build_report_tiles_match_the_member_table_sums() {
        let data = report(tile_tickets());
        let team_points: f64 = data.members.iter().map(|m| m.points).sum();
        assert_eq!(tile(&data, "points").value, team_points.to_string());
        assert_eq!(team_points, (8 + 48 + 8 + 3) as f64);
        assert_eq!(tile(&data, "delivered").value, "4");
        // ticket a counts for two members
        assert_eq!(data.members.iter().map(|m| m.delivered).sum::<usize>(), 5);
        assert_eq!(tile(&data, "scored").value, "75%");
        assert_eq!(sub(&data, "scored").as_deref(), Some("3 of 4"));
        assert_eq!(
            tile(&data, "contributors").value,
            data.members
                .iter()
                .filter(|m| m.delivered > 0)
                .count()
                .to_string()
        );
        assert_eq!(tile(&data, "median").value, "8"); // points 56, 8 and 3
    }

    #[test]
    fn build_report_tiles_count_active_tickets_and_members_with_only_active_work() {
        let mut tickets = tile_tickets();
        tickets.push(ticket("w", |t| {
            t.status = "in progress".into();
            t.status_type = "custom".into();
            t.done_at = None;
            t.updated_at = at(2026, 10, 10);
            t.assignees = vec![named("9", "Only Active")];
        }));
        let data = report(tickets);
        assert_eq!(tile(&data, "active").value, "1");
        assert_eq!(tile(&data, "contributors").value, "3");
        let busy = data.load.iter().find(|m| m.id == "9").unwrap();
        assert_eq!((busy.delivered, busy.active), (0, 1));
        assert!(render_report_html(&data).contains("Only Active"));
    }

    #[test]
    fn build_report_tiles_work_with_nothing_delivered() {
        let data = report(vec![]);
        assert!(data.notes.is_empty());
        assert_eq!(tile(&data, "delivered").value, "0");
        assert_eq!(tile(&data, "scored").value, "-");
        assert_eq!(tile(&data, "median").value, "-");
        let points: Vec<f64> = data.trend.buckets.iter().map(|b| b.points).collect();
        assert_eq!(points, [0.0, 0.0, 0.0]);
        let html = render_report_html(&data);
        assert!(html.contains("</html>"));
        assert!(!html.contains("NaN"));
    }

    // notes

    #[test]
    fn notes_show_coverage_only_below_100_percent() {
        let full = report(vec![ticket("a", |t| {
            t.assignees = vec![user("1")];
            level(t, "S");
        })]);
        assert!(full.notes.is_empty());
        assert!(!render_report_html(&full).contains("Scoring coverage"));

        let partial = report(vec![
            ticket("a", |t| {
                t.assignees = vec![user("1")];
                level(t, "S");
            }),
            ticket("b", |t| t.assignees = vec![user("1")]),
            ticket("c", |t| t.assignees = vec![user("1")]),
        ]);
        assert_eq!(partial.notes.len(), 1);
        assert_eq!(partial.notes[0].headline, "Scoring coverage is 33%.");
        assert!(partial.notes[0].text.contains("1 of 3 delivered tickets"));
        assert!(render_report_html(&partial).contains("Scoring coverage is 33%."));
    }

    #[test]
    fn notes_never_round_a_partial_coverage_up_to_100_percent() {
        let tickets = (0..300)
            .map(|i| {
                ticket(&format!("t{i}"), |t| {
                    t.assignees = vec![user("1")];
                    if i != 0 {
                        level(t, "S");
                    }
                })
            })
            .collect();
        assert_eq!(
            report(tickets).notes[0].headline,
            "Scoring coverage is 99%."
        );
    }

    #[test]
    fn notes_show_the_approximate_dates_note_only_when_flagged() {
        let exact = report(vec![ticket("a", |t| {
            t.assignees = vec![user("1")];
            level(t, "S");
        })]);
        assert!(!exact
            .notes
            .iter()
            .any(|n| n.headline == "Some delivery dates are approximate."));
        let approx = build_report(&input(
            vec![ticket("a", |t| {
                t.assignees = vec![user("1")];
                level(t, "S");
                t.status = "ready for testing".into();
                t.status_type = "custom".into();
                t.done_at = None;
                t.status_since = None;
                t.updated_at = at(2026, 10, 8);
            })],
            |i| i.scope.delivered_statuses = strings(&["ready for testing"]),
        ));
        let headlines: Vec<&str> = approx.notes.iter().map(|n| n.headline.as_str()).collect();
        assert_eq!(headlines, ["Some delivery dates are approximate."]);
        assert!(render_report_html(&approx).contains("approximate"));
    }

    // comparison tile

    fn current() -> Vec<TicketRecord> {
        (0..6)
            .map(|i| ticket(&format!("c{i}"), |t| t.assignees = vec![user("1")]))
            .collect()
    }

    fn earlier(n: usize, day: u32) -> Vec<TicketRecord> {
        (0..n)
            .map(|i| {
                ticket(&format!("p{i}"), |t| {
                    done(t, at(2026, 9, day));
                    t.assignees = vec![user("1")];
                })
            })
            .collect()
    }

    fn renamed(tickets: Vec<TicketRecord>, prefix: &str) -> Vec<TicketRecord> {
        tickets
            .into_iter()
            .map(|mut t| {
                t.id = format!("{prefix}{}", t.id);
                t
            })
            .collect()
    }

    #[test]
    fn comparison_tile_compares_a_running_period_with_the_same_days_of_the_previous_one() {
        // NOW is 18 Oct 12:00, so only Sep 1 to 18 12:00 counts.
        let mut previous = earlier(5, 10);
        previous.extend(renamed(earlier(4, 25), "late"));
        let data = build_report(&input(current(), |i| i.previous_tickets = previous));
        assert_eq!(
            sub(&data, "delivered").as_deref(),
            Some("+20% vs the same days of Sep 2026")
        );
        let down = build_report(&input(current(), |i| i.previous_tickets = earlier(8, 10)));
        assert_eq!(
            sub(&down, "delivered").as_deref(),
            Some("-25% vs the same days of Sep 2026")
        );
        let same = build_report(&input(current(), |i| i.previous_tickets = earlier(6, 10)));
        assert_eq!(
            sub(&same, "delivered").as_deref(),
            Some("0% vs the same days of Sep 2026")
        );
    }

    #[test]
    fn comparison_tile_compares_a_finished_period_whole() {
        let mut previous = earlier(5, 10);
        previous.extend(renamed(earlier(5, 25), "late"));
        let data = build_report(&input(current(), |i| {
            i.previous_tickets = previous;
            i.generated_at = local_at(2026, 10, 5, 12, 0);
        }));
        assert_eq!(sub(&data, "delivered").as_deref(), Some("-40% vs Sep 2026"));
    }

    #[test]
    fn comparison_tile_compares_the_rolling_7_days_whole() {
        let week = current_period(PeriodKind::Days7, &now(), None);
        let before = previous_period(&week).unwrap();
        let prev: Vec<TicketRecord> = (0..3)
            .map(|i| {
                ticket(&format!("p{i}"), |t| {
                    done(
                        t,
                        js_iso(&(before.end - chrono::Duration::milliseconds(3_600_000))),
                    );
                    t.assignees = vec![user("1")];
                })
            })
            .collect();
        let data = build_report(&input(
            vec![ticket("a", |t| {
                done(t, at(2026, 10, 17));
                t.assignees = vec![user("1")];
            })],
            |i| {
                i.period = week;
                i.previous_period = Some(before);
                i.previous_tickets = prev;
            },
        ));
        assert_eq!(
            sub(&data, "delivered").as_deref(),
            Some("-67% vs the previous 7 days")
        );
    }

    #[test]
    fn comparison_tile_is_left_out_for_all_time_or_when_the_previous_period_is_empty() {
        let zero = build_report(&input(current(), |_| {}));
        assert_eq!(sub(&zero, "delivered"), None);
        // Only after the elapsed span: nothing to compare with yet.
        let late = build_report(&input(current(), |i| i.previous_tickets = earlier(5, 28)));
        assert_eq!(sub(&late, "delivered"), None);
        let all = build_report(&input(current(), |i| {
            i.period = current_period(PeriodKind::All, &now(), Some(&local_date(2026, 0, 1)));
            i.previous_period = None;
            i.previous_tickets = earlier(5, 10);
        }));
        assert_eq!(sub(&all, "delivered"), None);
        assert!(!render_report_html(&zero).contains(" vs "));
    }

    // trend buckets

    fn labels(data: &ReportData) -> Vec<&str> {
        data.trend
            .buckets
            .iter()
            .map(|b| b.label.as_str())
            .collect()
    }

    fn trend_points(data: &ReportData) -> Vec<f64> {
        data.trend.buckets.iter().map(|b| b.points).collect()
    }

    fn owned(id: &str, when: String, lvl: &str, members: &[&str]) -> TicketRecord {
        ticket(id, |t| {
            done(t, when);
            t.assignees = members.iter().map(|m| user(m)).collect();
            level(t, lvl);
        })
    }

    #[test]
    fn trend_buckets_use_iso_weeks_labelled_by_monday_for_a_month() {
        let data = report(vec![
            owned("a", at(2026, 10, 1), "XS", &["1"]),
            owned("b", at(2026, 10, 7), "S", &["1"]),
            owned("c", at(2026, 10, 8), "M", &["1"]),
            owned("d", at(2026, 10, 14), "XL", &["1"]),
        ]);
        assert_eq!(data.trend.unit, TrendUnit::Week);
        // NOW is 18 Oct: the weeks of 19 and 26 Oct have not started.
        assert_eq!(labels(&data), ["28 Sep", "5 Oct", "12 Oct"]);
        assert_eq!(trend_points(&data), [3.0, 24.0, 48.0]);
        assert_eq!(data.trend.buckets[1].title, "Week of 5 Oct");
    }

    #[test]
    fn trend_buckets_keep_every_week_of_a_finished_month() {
        let data = build_report(&input(
            vec![owned("d", at(2026, 10, 28), "XL", &["1"])],
            |i| i.generated_at = local_at(2026, 10, 5, 12, 0),
        ));
        assert_eq!(
            labels(&data),
            ["28 Sep", "5 Oct", "12 Oct", "19 Oct", "26 Oct"]
        );
        assert_eq!(trend_points(&data), [0.0, 0.0, 0.0, 0.0, 48.0]);
    }

    #[test]
    fn trend_buckets_use_months_for_a_year_split_points_included() {
        let year = current_period(PeriodKind::Year, &now(), None);
        let data = build_report(&input(
            vec![
                owned("a", at(2026, 1, 15), "S", &["1"]),
                owned("b", at(2026, 10, 5), "M", &["1", "2"]),
            ],
            |i| {
                i.previous_period = previous_period(&year);
                i.period = year;
            },
        ));
        assert_eq!(data.trend.unit, TrendUnit::Month);
        // Nov and Dec have not started on 18 Oct.
        assert_eq!(data.trend.buckets.len(), 10);
        assert_eq!(
            (
                data.trend.buckets[0].label.as_str(),
                data.trend.buckets[0].points
            ),
            ("Jan", 8.0)
        );
        assert_eq!(
            (
                data.trend.buckets[9].label.as_str(),
                data.trend.buckets[9].points
            ),
            ("Oct", 16.0)
        );
    }

    #[test]
    fn trend_buckets_use_months_for_a_quarter_and_days_for_7d() {
        let quarter = build_report(&input(vec![], |i| {
            i.period = current_period(PeriodKind::Quarter, &now(), None)
        }));
        assert_eq!(quarter.trend.unit, TrendUnit::Month);
        assert_eq!(labels(&quarter), ["Oct"]);
        let finished = build_report(&input(vec![], |i| {
            i.period = current_period(PeriodKind::Quarter, &now(), None);
            i.generated_at = local_at(2027, 0, 5, 12, 0);
        }));
        assert_eq!(labels(&finished), ["Oct", "Nov", "Dec"]);
        let week = build_report(&input(vec![], |i| {
            i.period = current_period(PeriodKind::Days7, &now(), None)
        }));
        assert_eq!(week.trend.unit, TrendUnit::Day);
        assert_eq!(week.trend.buckets.len(), 7);
    }

    #[test]
    fn trend_buckets_use_months_for_all_time_within_24_months_else_years() {
        let all = current_period(PeriodKind::All, &now(), Some(&local_date(2020, 0, 1)));
        let short = build_report(&input(
            vec![
                owned("a", at(2025, 11, 5), "S", &["1"]),
                owned("b", at(2026, 10, 5), "S", &["1"]),
            ],
            |i| {
                i.period = all.clone();
                i.previous_period = None;
            },
        ));
        assert_eq!(short.trend.unit, TrendUnit::Month);
        assert_eq!(short.trend.buckets.len(), 12);
        assert_eq!(short.trend.buckets[0].label, "Nov 2025");

        let long = build_report(&input(
            vec![
                owned("a", at(2023, 3, 5), "S", &["1"]),
                owned("b", at(2026, 10, 5), "S", &["1"]),
            ],
            |i| {
                i.period = all;
                i.previous_period = None;
            },
        ));
        assert_eq!(long.trend.unit, TrendUnit::Year);
        assert_eq!(labels(&long), ["2023", "2024", "2025", "2026"]);
        assert_eq!(long.trend.buckets[0].points, 8.0);
        assert_eq!(long.trend.buckets[3].points, 8.0);
    }

    // hardest tickets

    fn strongest() -> Criteria {
        Criteria {
            ambiguity: 0,
            investigation: 2,
            scope: 2,
            risk: 1,
            coordination: 0,
        }
    }

    #[test]
    fn hardest_tickets_order_by_points_then_newest_delivery_keep_10() {
        let mut tickets = vec![
            ticket("s", |t| {
                level(t, "S");
                t.assignees = vec![user("1")];
            }),
            ticket("xl-old", |t| {
                level(t, "XL");
                done(t, at(2026, 10, 2));
                t.assignees = vec![user("1")];
            }),
            ticket("xl-new", |t| {
                level(t, "XL");
                done(t, at(2026, 10, 9));
                t.assignees = vec![user("1")];
            }),
            ticket("l", |t| {
                level(t, "L");
                t.assignees = vec![user("1")];
            }),
            ticket("unscored", |t| t.assignees = vec![user("1")]),
        ];
        for i in 0..10 {
            tickets.push(ticket(&format!("xs{i}"), |t| {
                level(t, "XS");
                t.assignees = vec![user("1")];
            }));
        }
        let data = report(tickets);
        assert_eq!(data.hardest.len(), 10);
        let top: Vec<&str> = data.hardest.iter().take(4).map(|h| h.id.as_str()).collect();
        assert_eq!(top, ["xl-new", "xl-old", "l", "s"]);
        assert!(!data.hardest.iter().any(|h| h.id == "unscored"));
        assert_eq!(
            (data.hardest[0].level, data.hardest[0].points),
            (Level::Xl, 48.0)
        );
    }

    #[test]
    fn hardest_tickets_pick_the_highest_criterion_reason_earliest_on_ties() {
        let data = report(vec![ticket("a", |t| {
            t.assignees = vec![user("1")];
            t.assessment = Some(assessment(strongest(), "why"));
        })]);
        let h = &data.hardest[0];
        assert_eq!(h.reason, "why-investigation");
        assert!(!h.edited);
        assert_eq!(h.level, Level::M); // sum 5
    }

    #[test]
    fn hardest_tickets_mark_overrides_as_edited_and_use_the_note_or_set_by_hand() {
        let data = report(vec![
            ticket("noted", |t| {
                t.assignees = vec![user("1")];
                t.assessment = Some(assessment(strongest(), "reason"));
                level(t, "XL");
                t.override_note = Some("  took a week ".into());
            }),
            ticket("silent", |t| {
                done(t, at(2026, 10, 3));
                t.assignees = vec![user("1")];
                level(t, "XL");
            }),
            ticket("blank", |t| {
                done(t, at(2026, 10, 1));
                t.assignees = vec![user("1")];
                level(t, "XL");
                t.override_note = Some("   ".into());
            }),
        ]);
        let by_id = |id: &str| data.hardest.iter().find(|h| h.id == id).unwrap();
        assert!(by_id("noted").edited);
        assert_eq!(by_id("noted").level, Level::Xl);
        assert_eq!(by_id("noted").reason, "took a week");
        assert!(by_id("silent").edited);
        assert_eq!(by_id("silent").reason, "Set by hand");
        assert_eq!(by_id("blank").reason, "Set by hand");
        assert!(render_report_html(&data).contains(r#"<span class="edited">edited</span>"#));
    }

    #[test]
    fn hardest_tickets_list_all_distinct_assignees_or_unassigned() {
        let data = report(vec![
            ticket("a", |t| {
                level(t, "XL");
                t.assignees = vec![named("1", "Ann"), named("2", "Bo"), named("1", "Ann")];
            }),
            ticket("b", |t| level(t, "L")),
        ]);
        let members: Vec<&str> = data.hardest.iter().map(|h| h.members.as_str()).collect();
        assert_eq!(members, ["Ann, Bo", "Unassigned"]);
    }

    // faded members

    #[test]
    fn faded_members_fade_members_below_50_percent_coverage_in_the_data_table_and_chart() {
        let tickets = vec![
            ticket("a", |t| {
                t.assignees = vec![named("1", "Solid")];
                level(t, "M");
            }),
            ticket("b", |t| {
                t.assignees = vec![named("2", "Thin")];
                level(t, "M");
            }),
            ticket("c", |t| t.assignees = vec![named("2", "Thin")]),
            ticket("d", |t| t.assignees = vec![named("2", "Thin")]),
            ticket("e", |t| {
                t.assignees = vec![named("3", "Half")];
                level(t, "M");
            }),
            ticket("f", |t| t.assignees = vec![named("3", "Half")]),
        ];
        let data = report(tickets);
        let faded = |name: &str| data.members.iter().find(|m| m.name == name).unwrap().faded;
        assert!(!faded("Solid"));
        assert!(faded("Thin"));
        assert!(!faded("Half"));
        let html = render_report_html(&data);
        assert_eq!(count(&html, r#"<tr class="faded">"#), 1);
        assert!(html.contains(r#"opacity="0.35""#));
        assert!(html.contains("(33% scored)"));
    }

    // members with no delivered tickets

    #[test]
    fn members_with_no_delivered_tickets_are_not_faded_coverage_means_nothing_for_them() {
        let data = report(vec![
            ticket("a", |t| {
                t.assignees = vec![named("1", "Solid")];
                level(t, "M");
            }),
            ticket("w", |t| {
                t.status = "in progress".into();
                t.status_type = "custom".into();
                t.done_at = None;
                t.assignees = vec![named("2", "Busy")];
            }),
        ]);
        let busy = data.members.iter().find(|m| m.name == "Busy").unwrap();
        assert_eq!(busy.delivered, 0);
        assert_eq!(busy.active, 1);
        assert!(!busy.faded);
        let html = render_report_html(&data);
        assert!(!html.contains(r#"<tr class="faded">"#));
        assert!(!html.contains(r#"opacity="0.35""#));
        assert!(html.contains("no delivered work"));
    }

    // member names in the charts

    #[test]
    fn member_names_in_the_charts_clip_a_16_character_name_and_keep_the_full_name_in_a_title() {
        let long = "Nguyen Van Hoang";
        let html = html_of(vec![ticket("a", |t| {
            t.assignees = vec![named("1", long)];
            level(t, "M");
        })]);
        assert!(html.contains(&format!(">Nguyen Van ...<title>{long}</title></text>")));
        assert!(!html.contains(&format!(">{long}</text>")));
    }

    #[test]
    fn member_names_in_the_charts_leave_a_name_of_14_characters_whole() {
        let name = "Nguyen Van Hoa";
        assert_eq!(name.len(), 14);
        let html = html_of(vec![ticket("a", |t| {
            t.assignees = vec![named("1", name)];
            level(t, "M");
        })]);
        assert!(html.contains(&format!(r#"text-anchor="end">{name}</text>"#)));
    }

    // comparison note

    fn prev(id: &str) -> TicketRecord {
        ticket(id, |t| {
            done(t, at(2026, 9, 5));
            t.updated_at = at(2026, 9, 5);
        })
    }

    fn one_current() -> Vec<TicketRecord> {
        vec![ticket("a", |t| t.assignees = vec![named("1", "Ann")])]
    }

    #[test]
    fn comparison_note_is_absent_when_the_previous_period_was_fully_fetched_and_the_comparison_shows(
    ) {
        let data = build_report(&input(one_current(), |i| {
            i.previous_tickets = vec![prev("p1"), prev("p2")]
        }));
        assert_eq!(data.comparison_note, None);
        assert!(sub(&data, "delivered").unwrap().contains("vs "));
        assert!(!render_report_html(&data).contains("No comparison with"));
    }

    #[test]
    fn comparison_note_explains_the_missing_comparison_when_the_previous_period_was_not_fully_fetched(
    ) {
        let data = build_report(&input(one_current(), |i| i.previous_incomplete = true));
        assert_eq!(
            data.comparison_note.as_deref(),
            Some("No comparison with Sep 2026: its tickets were not fully fetched. Open that period in Ticket Contributors once to include it.")
        );
        assert_eq!(sub(&data, "delivered"), None);
        let html = render_report_html(&data);
        assert!(html.contains("No comparison with Sep 2026: its tickets"));
        // At the top, before the tiles.
        assert!(html.find("No comparison with").unwrap() < html.find(r#"class="tiles""#).unwrap());
    }

    #[test]
    fn comparison_note_has_no_note_for_all_time_which_has_no_previous_period() {
        let data = build_report(&input(one_current(), |i| {
            i.period = current_period(PeriodKind::All, &now(), None);
            i.previous_period = None;
            i.previous_incomplete = true;
        }));
        assert_eq!(data.comparison_note, None);
    }

    #[test]
    fn comparison_note_escapes_the_previous_period_label() {
        let data = build_report(&input(one_current(), |i| {
            i.previous_period = Some(Period {
                label: "<b>Sep</b> & co".into(),
                ..september()
            });
            i.previous_incomplete = true;
        }));
        let html = render_report_html(&data);
        assert!(html.contains("&lt;b&gt;Sep&lt;/b&gt; &amp; co"));
        assert!(!html.contains("<b>Sep</b>"));
    }

    // renderReportHtml layout

    #[test]
    fn render_report_html_layout_renders_header_sections_svg_titles_and_the_method_table() {
        let html = html_of(vec![ticket("a", |t| {
            t.assignees = vec![named("1", "Ann Lee")];
            level(t, "L");
        })]);
        assert!(html.contains("<title>Ticket Contributors report - Casium - Oct 2026</title>"));
        assert!(html.contains("Scored with claude-sonnet-5-5 (Claude Code)"));
        for heading in [
            "Points by member",
            "Difficulty mix by member",
            "Team points per week",
            "Delivered and active work",
            "Member table",
            "Hardest tickets this period",
            "How the score works",
        ] {
            assert!(html.contains(&format!("<h2>{heading}</h2>")), "{heading}");
        }
        assert_eq!(count(&html, "<svg "), 4);
        assert!(html.contains("<title>Ann Lee: 32 points, coverage 100%</title>"));
        for color in [
            "#2a78d6", "#eb6834", "#86b6ef", "#5598e7", "#1c5cab", "#104281",
        ] {
            assert!(html.contains(color), "{color}");
        }
        assert!(
            html.contains("<td>Points</td><td>3</td><td>8</td><td>16</td><td>32</td><td>48</td>")
        );
        assert!(html.contains("Delivered statuses: closed."));
    }

    #[test]
    fn render_report_html_layout_omits_the_scored_with_part_without_ai_config() {
        let html = render_report_html(&build_report(&input(vec![], |i| i.ai = None)));
        assert!(!html.contains("Scored with"));
    }

    // unassigned tickets

    fn unassigned_tickets() -> Vec<TicketRecord> {
        vec![
            ticket("a", |t| {
                t.assignees = vec![named("1", "Ann")];
                level(t, "S");
            }),
            ticket("u1", |t| level(t, "M")),
            ticket("u2", |_| {}),
        ]
    }

    #[test]
    fn unassigned_tickets_get_their_own_table_row_and_still_count_in_the_tiles() {
        let data = report(unassigned_tickets());
        assert_eq!(
            data.unassigned,
            Some(Unassigned {
                delivered: 2,
                scored: 1
            })
        );
        assert_eq!(tile(&data, "delivered").value, "3");
        assert_eq!(sub(&data, "scored").as_deref(), Some("2 of 3"));
        let html = render_report_html(&data);
        assert!(html.contains(
            r#"<tr class="unassigned"><td>Unassigned</td><td>-</td><td>-</td><td>2</td><td>1/2</td>"#
        ));
        assert!(html.contains("A ticket with several assignees counts for each of them; unassigned tickets have their own row."));
    }

    #[test]
    fn unassigned_tickets_have_no_row_without_unassigned_tickets() {
        let data = report(vec![ticket("a", |t| {
            t.assignees = vec![user("1")];
            level(t, "S");
        })]);
        assert_eq!(data.unassigned, None);
        assert!(!render_report_html(&data).contains(r#"class="unassigned""#));
    }

    #[test]
    fn unassigned_tickets_render_the_table_when_only_unassigned_tickets_exist() {
        let html = html_of(vec![ticket("u", |_| {})]);
        assert!(html.contains("<td>Unassigned</td>"));
        assert!(!html.contains("No members to show"));
    }

    // coverage text

    #[test]
    fn coverage_text_never_rounds_a_faded_member_up_to_50_percent_or_a_partial_one_up_to_100_percent(
    ) {
        // 124 of 250 scored is 49.6%; 299 of 300 is 99.67%.
        let mut tickets: Vec<TicketRecord> = (0..250)
            .map(|i| {
                ticket(&format!("f{i}"), |t| {
                    t.assignees = vec![named("1", "Faded")];
                    if i < 124 {
                        level(t, "S");
                    }
                })
            })
            .collect();
        tickets.extend((0..300).map(|i| {
            ticket(&format!("n{i}"), |t| {
                t.assignees = vec![named("2", "Nearly")];
                if i < 299 {
                    level(t, "S");
                }
            })
        }));
        let data = report(tickets);
        let by_name = |name: &str| data.members.iter().find(|m| m.name == name).unwrap();
        assert!(by_name("Faded").faded);
        assert_eq!(by_name("Faded").coverage_percent, 49);
        assert_eq!(by_name("Nearly").coverage_percent, 99);
        let html = render_report_html(&data);
        assert!(html.contains("Faded: 992 points, coverage 49%"));
        assert!(html.contains("Nearly: 2,392 points, coverage 99%"));
        assert!(!html.contains("coverage 50%"));
        assert!(!html.contains("coverage 100%"));
    }

    // month by month

    fn q4() -> Period {
        current_period(PeriodKind::Quarter, &now(), None)
    }

    fn q3() -> Period {
        previous_period(&q4()).unwrap()
    }

    fn year() -> Period {
        current_period(PeriodKind::Year, &now(), None)
    }

    const MONTH_EVIL: &str = r#"<b>"Evil"</b> & 'Co'"#;

    // Q3 2026: July, August, September.
    fn q3_tickets() -> Vec<TicketRecord> {
        vec![
            ticket("j1", |t| {
                done(t, at(2026, 7, 5));
                t.assignees = vec![named("1", "Ann"), named("2", "Bob")];
                level(t, "M"); // 16 split 8 + 8
            }),
            ticket("j2", |t| {
                done(t, at(2026, 7, 20));
                t.assignees = vec![named("1", "Ann")];
                level(t, "XL"); // 48
            }),
            ticket("a1", |t| {
                done(t, at(2026, 8, 3));
                t.assignees = vec![named("2", "Bob")]; // unscored
            }),
            ticket("a2", |t| {
                done(t, at(2026, 8, 9));
                t.assignees = vec![named("2", "Bob")];
                level(t, "L");
            }),
            ticket("s1", |t| {
                done(t, at(2026, 9, 30));
                t.assignees = vec![named("1", "Ann")];
                level(t, "XS");
            }),
            ticket("u1", |t| {
                done(t, at(2026, 9, 2));
                level(t, "S"); // unassigned
            }),
            ticket("wip", |t| {
                t.status = "in progress".into();
                t.status_type = "custom".into();
                t.done_at = None;
                t.updated_at = at(2026, 8, 15);
                t.assignees = vec![named("3", "Cy")];
            }),
        ]
    }

    fn quarter() -> ReportData {
        build_report(&input(q3_tickets(), |i| {
            i.period = q3();
            i.previous_period = None;
        }))
    }

    fn monthly(data: &ReportData) -> &ReportMonthly {
        data.monthly.as_ref().unwrap()
    }

    fn row<'a>(m: &'a ReportMonthly, name: &str) -> &'a MatrixRow {
        m.rows.iter().find(|r| r.name == name).unwrap()
    }

    #[test]
    fn month_by_month_adds_nothing_for_a_month_7_days_or_all_time() {
        for period in [
            october(),
            current_period(PeriodKind::Days7, &now(), None),
            current_period(PeriodKind::All, &now(), None),
        ] {
            let data = build_report(&input(q3_tickets(), |i| i.period = period));
            assert!(data.monthly.is_none());
            let out = render_report_html(&data);
            assert!(!out.contains("Month by month"));
            assert!(!out.contains("<details"));
        }
    }

    #[test]
    fn month_by_month_has_one_column_per_month_of_a_finished_quarter_after_the_trend() {
        let data = quarter();
        let months: Vec<&str> = monthly(&data).months.iter().map(|m| m.label).collect();
        assert_eq!(months, ["Jul", "Aug", "Sep"]);
        let out = render_report_html(&data);
        let section = &out[out.find("Month by month").unwrap()..];
        assert!(out.find("Team points per month").unwrap() < out.find("Month by month").unwrap());
        assert!(out.find("Month by month").unwrap() < out.find("<h2>Member table</h2>").unwrap());
        assert!(section.contains(
            "<tr><th>Member</th><th>Jul</th><th>Aug</th><th>Sep</th><th>Total</th></tr>"
        ));
        assert!(out.contains(
            "Click a month to see its details. Expand the months you want before printing."
        ));
    }

    #[test]
    fn month_by_month_stops_at_the_month_of_the_report_date_in_an_unfinished_quarter_and_year() {
        let q = build_report(&input(q3_tickets(), |i| i.period = q4()));
        let months: Vec<&str> = monthly(&q).months.iter().map(|m| m.label).collect();
        assert_eq!(months, ["Oct"]);
        let y = build_report(&input(q3_tickets(), |i| i.period = year()));
        let titles: Vec<&str> = monthly(&y)
            .months
            .iter()
            .map(|m| m.title.as_str())
            .collect();
        let want: Vec<String> = [
            "January",
            "February",
            "March",
            "April",
            "May",
            "June",
            "July",
            "August",
            "September",
            "October",
        ]
        .iter()
        .map(|n| format!("{n} 2026"))
        .collect();
        assert_eq!(titles, want);
        // The year reaches back, so July's work shows in its July cell.
        let ann = row(monthly(&y), "Ann");
        let cell = &ann.cells[6];
        assert_eq!((cell.points, cell.delivered, cell.scored), (56.0, 2, 2));
    }

    #[test]
    fn month_by_month_matrix_cells_and_totals_equal_the_overview_member_table() {
        let data = quarter();
        let monthly = monthly(&data);
        let mut names: Vec<&str> = data.members.iter().map(|m| m.name.as_str()).collect();
        names.push("Unassigned");
        assert_eq!(
            monthly
                .rows
                .iter()
                .map(|r| r.name.as_str())
                .collect::<Vec<_>>(),
            names
        );
        for member in &data.members {
            let row = monthly.rows.iter().find(|r| r.id == member.id).unwrap();
            assert_eq!(row.total.points, member.points);
            assert_eq!(row.total.delivered, member.delivered);
            assert_eq!(row.total.scored, member.scored);
            assert_eq!(
                row.cells.iter().map(|c| c.points).sum::<f64>(),
                member.points
            );
            assert_eq!(
                row.cells.iter().map(|c| c.delivered).sum::<usize>(),
                member.delivered
            );
        }
        let unassigned = monthly.rows.last().unwrap();
        let want = data.unassigned.unwrap();
        assert_eq!(
            (unassigned.total.delivered, unassigned.total.scored),
            (want.delivered, want.scored)
        );
        let ann = row(monthly, "Ann");
        let cells: Vec<(f64, usize)> = ann.cells.iter().map(|c| (c.points, c.delivered)).collect();
        assert_eq!(cells, [(56.0, 2), (0.0, 0), (3.0, 1)]);
        assert_eq!(
            tile(&data, "points").value,
            monthly
                .months
                .iter()
                .map(|m| m.points)
                .sum::<f64>()
                .to_string()
        );
        let out = render_report_html(&data);
        assert!(out.contains("Ann, July 2026: 56 points, 2 delivered, 2 scored"));
        assert!(out.contains(r#"<span class="sub">&middot; 2</span>"#));
    }

    #[test]
    fn month_by_month_shades_by_points_against_the_busiest_cell_and_leaves_zero_cells_plain() {
        let data = quarter();
        let ann = row(monthly(&data), "Ann");
        assert_eq!(ann.cells[0].shade, Some(4));
        assert_eq!(ann.cells[1].shade, None);
        assert_eq!(ann.cells[2].shade, Some(0));
        assert_eq!(ann.total.shade, None);
        let out = render_report_html(&data);
        assert!(out.contains("background:#6da7ec"));
        assert!(out.contains("background:#cde2fb"));
    }

    #[test]
    fn month_by_month_draws_the_count_on_a_shaded_cell_in_the_darker_ink() {
        let out = render_report_html(&quarter());
        assert!(
            Regex::new(r"td\.shaded \.sub \{ color: var\(--ink-on-shade\)")
                .unwrap()
                .is_match(&out)
        );
        assert!(out.contains("--ink-on-shade: #2b2a28;"));
        assert!(out.contains(
            r#"<td class="cell shaded" style="background:#6da7ec" title="Ann, July 2026"#
        ));
        // Blank and total cells keep the lighter ink: they carry no shaded class.
        assert!(out.contains(r#"<td class="total" title="Ann, Q3 2026"#));
        assert!(!out.contains(r#"class="cell" style="#));
    }

    #[test]
    fn month_by_month_fades_the_matrix_row_of_a_member_below_50_percent_overall_coverage() {
        let data = quarter();
        let rows: Vec<(&str, bool)> = monthly(&data)
            .rows
            .iter()
            .map(|r| (r.name.as_str(), r.faded))
            .collect();
        assert_eq!(
            rows,
            [
                ("Ann", false),
                ("Bob", false),
                ("Cy", false),
                ("Unassigned", false)
            ]
        );
        let low = |id: &str, day: u32, scored: bool| {
            ticket(id, |t| {
                done(t, at(2026, 8, day));
                t.assignees = vec![named("1", "Low")];
                if scored {
                    level(t, "S");
                }
            })
        };
        let data = build_report(&input(
            vec![low("f1", 3, false), low("f2", 4, false), low("f3", 5, true)],
            |i| {
                i.period = q3();
                i.previous_period = None;
            },
        ));
        assert!(data.members[0].faded);
        assert!(monthly(&data).rows[0].faded);
        assert!(render_report_html(&data).contains(r#"<tr class="faded"><td>Low</td>"#));
    }

    #[test]
    fn month_by_month_folds_a_ticket_delivered_after_the_last_month_into_that_month() {
        let late = ticket("late", |t| {
            done(t, at(2026, 11, 5)); // inside Q4, after the report date
            t.assignees = vec![named("1", "Ann")];
            level(t, "L");
        });
        let data = build_report(&input(vec![late], |i| i.period = q4()));
        let m = monthly(&data);
        assert_eq!(
            m.months.iter().map(|m| m.label).collect::<Vec<_>>(),
            ["Oct"]
        );
        assert_eq!((m.months[0].delivered, m.months[0].points), (1, 32.0));
        assert_eq!(
            (m.rows[0].cells[0].points, m.rows[0].cells[0].delivered),
            (32.0, 1)
        );
        assert_eq!(m.rows[0].total.points, 32.0);
        let hardest: Vec<&str> = m.months[0].hardest.iter().map(|h| h.id.as_str()).collect();
        assert_eq!(hardest, ["late"]);
    }

    #[test]
    fn month_by_month_hides_the_help_line_when_there_are_no_month_blocks_but_keeps_empty_months() {
        let empty = render_report_html(&build_report(&input(vec![], |i| {
            i.period = q3();
            i.previous_period = None;
        })));
        assert!(empty.contains("Month by month"));
        assert!(!empty.contains("Click a month"));
        let data = build_report(&input(q3_tickets(), |i| i.period = year()));
        assert_eq!(monthly(&data).months.len(), 10);
        assert_eq!(monthly(&data).months[0].delivered, 0);
        assert!(render_report_html(&data).contains("Click a month"));
    }

    #[test]
    fn month_by_month_closes_every_month_block_by_default() {
        let out = render_report_html(&quarter());
        assert_eq!(count(&out, r#"<details class="month">"#), 3);
        assert!(!Regex::new(r"<details[^>]*\bopen\b").unwrap().is_match(&out));
        assert!(out.contains("<summary>July 2026 "));
        assert!(
            Regex::new(r#"July 2026 <span class="meta">&middot; 2 delivered"#)
                .unwrap()
                .is_match(&out)
        );
        assert!(!out.contains("<script"));
    }

    #[test]
    fn month_by_month_lists_the_3_hardest_tickets_of_each_month() {
        let mut tickets: Vec<TicketRecord> = ["a", "b", "c", "d"]
            .iter()
            .zip(["XS", "S", "M", "L"])
            .enumerate()
            .map(|(i, (id, lvl))| {
                ticket(id, |t| {
                    done(t, at(2026, 8, 3 + i as u32));
                    t.assignees = vec![named("1", "Ann")];
                    level(t, lvl);
                })
            })
            .collect();
        tickets.push(ticket("july", |t| {
            done(t, at(2026, 7, 3));
            t.assignees = vec![named("1", "Ann")];
            level(t, "XS");
        }));
        let data = build_report(&input(tickets, |i| {
            i.period = q3();
            i.previous_period = None;
        }));
        let ids = |m: usize| -> Vec<&str> {
            monthly(&data).months[m]
                .hardest
                .iter()
                .map(|h| h.id.as_str())
                .collect()
        };
        assert_eq!(ids(1), ["d", "c", "b"]);
        assert_eq!(ids(0), ["july"]);
        assert!(ids(2).is_empty());
    }

    #[test]
    fn month_by_month_builds_the_tiles_and_member_table_of_each_month_on_their_own() {
        let data = quarter();
        let aug = &monthly(&data).months[1];
        assert_eq!(aug.delivered, 2);
        let tiles: Vec<(&str, &str)> = aug.tiles.iter().map(|t| (t.id, t.value.as_str())).collect();
        assert_eq!(
            tiles,
            [
                ("delivered", "2"),
                ("scored", "50%"),
                ("points", "32"),
                ("active", "1")
            ]
        );
        let bob = aug.members.iter().find(|m| m.name == "Bob").unwrap();
        assert_eq!((bob.points, bob.delivered, bob.scored), (32.0, 2, 1));
        let cy = aug.members.iter().find(|m| m.name == "Cy").unwrap();
        assert_eq!((cy.delivered, cy.active), (0, 1));
    }

    #[test]
    fn month_by_month_escapes_names_and_titles_inside_the_month_section() {
        let data = build_report(&input(
            vec![ticket("x", |t| {
                t.title = MONTH_EVIL.into();
                done(t, at(2026, 8, 3));
                t.assignees = vec![named("1", MONTH_EVIL)];
                level(t, "XL");
                t.override_note = Some(MONTH_EVIL.into());
            })],
            |i| {
                i.period = q3();
                i.previous_period = None;
            },
        ));
        let out = render_report_html(&data);
        let section =
            &out[out.find("Month by month").unwrap()..out.find("<h2>Member table</h2>").unwrap()];
        assert!(section.contains("&lt;b&gt;&quot;Evil&quot;&lt;/b&gt; &amp; &#39;Co&#39;"));
        assert!(!section.contains(r#"<b>"Evil""#));
        assert!(!section.contains("<b>&quot;"));
        assert!(section.contains(
            r#"title="&lt;b&gt;&quot;Evil&quot;&lt;/b&gt; &amp; &#39;Co&#39;, August 2026"#
        ));
    }

    #[test]
    fn month_by_month_shows_a_months_coverage_note_only_when_that_month_is_below_100_percent() {
        let data = quarter();
        let months = &monthly(&data).months;
        assert_eq!(months[0].note, None);
        assert_eq!(
            months[1].note.as_ref().map(|n| n.headline.as_str()),
            Some("Scoring coverage is 50%.")
        );
        assert!(months[1]
            .note
            .as_ref()
            .unwrap()
            .text
            .contains("1 of 2 delivered tickets in August 2026"));
        assert_eq!(months[2].note, None);
        let out = render_report_html(&data);
        let blocks: Vec<&str> = out.split(r#"<details class="month">"#).skip(1).collect();
        assert!(!blocks[0].contains("Scoring coverage"));
        assert!(blocks[1].contains("Scoring coverage is 50%."));
        assert!(!blocks[2].contains("Scoring coverage"));
    }

    #[test]
    fn month_by_month_works_with_nothing_delivered() {
        let data = build_report(&input(vec![], |i| {
            i.period = q3();
            i.previous_period = None;
        }));
        let out = render_report_html(&data);
        assert!(out.contains("Month by month"));
        assert!(!out.contains("<details"));
        assert!(!out.contains("NaN"));
    }

    // excluded members

    const EX_EVIL: &str = r#"<b>"Ex"</b> & 'Co'"#;

    fn excluded_scope() -> WorkspaceScope {
        let mut s = scope();
        s.excluded_members = vec![
            ExcludedMember {
                id: "2".into(),
                name: "Old Two".into(),
            },
            ExcludedMember {
                id: "9".into(),
                name: "Left Lee".into(),
            },
        ];
        s
    }

    fn excluded_tickets() -> Vec<TicketRecord> {
        vec![
            ticket("own", |t| {
                t.assignees = vec![named("1", "Ann")];
                level(t, "S"); // 8
            }),
            ticket("shared", |t| {
                t.assignees = vec![named("1", "Ann"), named("2", EX_EVIL)];
                level(t, "M"); // 16 goes to Ann alone
            }),
            ticket("theirs", |t| {
                t.title = "Only theirs".into();
                t.assignees = vec![named("2", EX_EVIL)];
                level(t, "XL");
            }),
            ticket("nobody", |t| level(t, "XS")),
            ticket("busy", |t| {
                t.status = "in progress".into();
                t.status_type = "custom".into();
                t.done_at = None;
                t.updated_at = at(2026, 10, 8);
                t.assignees = vec![named("2", EX_EVIL)];
            }),
        ]
    }

    #[test]
    fn excluded_members_leave_excluded_members_out_of_every_number_and_table() {
        let data = build_report(&input(excluded_tickets(), |i| i.scope = excluded_scope()));
        let members: Vec<(&str, usize, f64)> = data
            .members
            .iter()
            .map(|m| (m.name.as_str(), m.delivered, m.points))
            .collect();
        assert_eq!(members, [("Ann", 2, 24.0)]);
        assert_eq!(tile(&data, "delivered").value, "3");
        assert_eq!(tile(&data, "points").value, "24");
        assert_eq!(tile(&data, "active").value, "0");
        assert_eq!(
            data.unassigned,
            Some(Unassigned {
                delivered: 1,
                scored: 1
            })
        );
        assert_eq!(
            data.load
                .iter()
                .map(|l| l.name.as_str())
                .collect::<Vec<_>>(),
            ["Ann"]
        );
        assert!(!data.hardest.iter().any(|h| h.id == "theirs"));
        assert_eq!(
            data.hardest
                .iter()
                .find(|h| h.id == "shared")
                .unwrap()
                .members,
            "Ann"
        );
        let out = render_report_html(&data);
        assert!(!out.contains("Only theirs"));
        // Only the method section names them.
        assert_eq!(out.split("Ex&quot;").count(), 2);
    }

    #[test]
    fn excluded_members_keep_them_out_of_the_month_by_month_matrix() {
        let data = build_report(&input(excluded_tickets(), |i| {
            i.scope = excluded_scope();
            i.period = q4();
            i.previous_period = None;
        }));
        let names: Vec<&str> = monthly(&data)
            .rows
            .iter()
            .map(|r| r.name.as_str())
            .collect();
        assert_eq!(names, ["Ann", "Unassigned"]);
        assert_eq!(render_report_html(&data).split("Ex&quot;").count(), 2);
    }

    #[test]
    fn excluded_members_are_listed_by_name_escaped_in_the_method_section() {
        let out = render_report_html(&build_report(&input(excluded_tickets(), |i| {
            i.scope = excluded_scope()
        })));
        assert!(out.contains(
            "Excluded from this board: &lt;b&gt;&quot;Ex&quot;&lt;/b&gt; &amp; &#39;Co&#39;, Left Lee."
        ));
        assert!(!out.contains(r#"<b>"Ex""#));
    }

    #[test]
    fn excluded_members_add_no_line_when_nobody_is_excluded() {
        let out = html_of(excluded_tickets());
        assert!(!out.contains("Excluded from this board"));
    }

    // not in the TS suite: the helpers behind the text and the SVG

    #[test]
    fn fmt_keeps_whole_numbers_whole_one_decimal_and_groups_thousands_like_en_us() {
        for (value, text) in [
            (8.0, "8"),
            (0.0, "0"),
            (8.25, "8.3"),
            (0.04, "0"),
            (0.05, "0.1"),
            (2392.0, "2,392"),
            (1234567.89, "1,234,567.9"),
            (999.96, "1,000"),
            (12.5, "12.5"),
        ] {
            assert_eq!(fmt(value), text, "{value}");
        }
    }

    #[test]
    fn percent_never_crosses_a_boundary_by_rounding() {
        assert_eq!(percent(0.0, 10.0), 0);
        assert_eq!(percent(5.0, 0.0), 0);
        assert_eq!(percent(10.0, 10.0), 100);
        assert_eq!(percent(1.0, 300.0), 1);
        assert_eq!(percent(299.0, 300.0), 99);
        assert_eq!(percent(124.0, 250.0), 49);
        assert_eq!(percent(125.0, 250.0), 50);
        assert_eq!(percent(1.0, 3.0), 33);
    }

    #[test]
    fn num_rounds_to_two_decimals_like_to_fixed_and_drops_trailing_zeros() {
        for (value, text) in [
            (10.0, "10"),
            (3.456, "3.46"),
            (0.125, "0.13"),
            (0.375, "0.38"),
            (-0.125, "-0.13"),
            (2.675, "2.67"),
            (1.005, "1"),
            (0.001, "0"),
            (-0.001, "0"),
            (12.5, "12.5"),
            (1e-7, "0"),
            (123456.789, "123456.79"),
        ] {
            assert_eq!(num(value), text, "{value}");
        }
    }

    #[test]
    fn clip_cuts_by_utf16_units_and_marks_the_cut() {
        assert_eq!(clip("abcdefghijklmnop", 14), "abcdefghijk...");
        assert_eq!(clip("short", 14), "short");
        assert_eq!(clip("abcdef", 3), "a...");
        assert_eq!(clip("abcdef", 6), "abcdef");
    }

    #[test]
    fn nice_ticks_run_from_zero_to_a_round_value_at_or_above_the_max() {
        assert_eq!(nice_ticks(0.0, 4), [0.0, 1.0]);
        assert_eq!(nice_ticks(f64::NAN, 4), [0.0, 1.0]);
        assert_eq!(nice_ticks(10.0, 4), [0.0, 5.0, 10.0]);
        assert_eq!(nice_ticks(7.0, 4), [0.0, 2.0, 4.0, 6.0, 8.0]);
        assert_eq!(nice_ticks(3.0, 2), [0.0, 2.0, 4.0]);
        assert_eq!(nice_ticks(100.0, 4), [0.0, 50.0, 100.0]);
        assert_eq!(nice_ticks(1.0, 4), [0.0, 1.0]);
    }

    #[test]
    fn bar_paths_round_only_the_data_end() {
        assert_eq!(
            hbar_path(100.0, 6.0, 120.0, 18.0, 4.0),
            "M100,6H216Q220,6 220,10V20Q220,24 216,24H100Z"
        );
        assert_eq!(
            vbar_path(50.0, 10.0, 14.0, 100.0, 4.0),
            "M50,110V14Q50,10 54,10H60Q64,10 64,14V110Z"
        );
        // a bar shorter than its radius shrinks the radius
        assert_eq!(
            hbar_path(0.0, 0.0, 2.0, 18.0, 4.0),
            "M0,0H0Q2,0 2,2V16Q2,18 0,18H0Z"
        );
    }

    #[test]
    fn esc_escapes_the_five_html_characters() {
        assert_eq!(esc(r#"&<>"'"#), "&amp;&lt;&gt;&quot;&#39;");
        assert_eq!(esc("plain"), "plain");
    }

    #[test]
    fn a_single_bucket_trend_draws_a_point_and_no_line() {
        let data = build_report(&input(
            vec![ticket("a", |t| {
                t.assignees = vec![user("1")];
                level(t, "S");
            })],
            |i| {
                i.generated_at = local_at(2026, 9, 2, 12, 0);
                i.period = Period {
                    kind: PeriodKind::Month,
                    ..october()
                };
            },
        ));
        assert_eq!(data.trend.buckets.len(), 1);
        let svg = trend_chart(&data.trend);
        assert!(!svg.contains("<path"));
        assert!(svg.contains("<circle"));
    }

    #[test]
    fn the_report_names_the_runner_of_the_scoring() {
        let api = build_report(&input(vec![], |i| {
            i.ai = Some(AiConfigStatus {
                runner: "api".into(),
                model: "m".into(),
                has_api_key: true,
            })
        }));
        assert_eq!(api.scored_with.as_deref(), Some("m (API)"));
        assert_eq!(
            report(vec![]).scored_with.as_deref(),
            Some("claude-sonnet-5-5 (Claude Code)")
        );
        assert_eq!(report(vec![]).generated_label, "18 Oct 2026");
    }

    #[test]
    fn the_report_lists_chosen_delivered_statuses_not_seen_in_the_period() {
        let data = build_report(&input(
            vec![ticket("a", |t| t.assignees = vec![user("1")])],
            |i| i.scope.delivered_statuses = strings(&["Ready", " ready ", "closed", ""]),
        ));
        assert_eq!(data.statuses.delivered, ["closed", "Ready"]);
        assert!(data.statuses.excluded.is_empty());
    }

    #[test]
    fn a_report_with_no_work_still_renders_every_chart_as_an_empty_note() {
        let html = html_of(vec![]);
        assert!(html.contains("No delivered tickets in this period."));
        assert!(html.contains("No scored tickets yet."));
        assert!(html.contains("No work to show in this period."));
        assert!(html.contains("No members to show in this period."));
        assert!(html.contains("No scored tickets in this period."));
    }
}
