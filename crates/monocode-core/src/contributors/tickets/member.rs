//! One member's page of the Ticket tab: what they delivered in the period, how
//! it compares with the previous one, and the days and hours it happened on.
//! Pure; everything comes from the board the tab already built.

use std::collections::BTreeMap;

use chrono::{Datelike, NaiveDate, Timelike};

use super::board::Board;
use super::report::{percent, percent_change};
use super::tickets::{delivered_at, ByLevel, Level, LevelCounts, Points, LEVELS};
use crate::contributors::model::period::parse_iso;

/// Tickets delivered by local weekday (Monday first) and hour of the day.
pub type Rhythm = [[u32; 24]; 7];

/// How many recent tickets the page lists.
pub const RECENT_LIMIT: usize = 10;

/// What a member delivered on one local day.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DayDelivery {
    pub tickets: u32,
    /// The member's share of the points of those tickets.
    pub points: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RecentTicket {
    pub id: String,
    pub title: String,
    /// `None`: not scored.
    pub level: Option<Level>,
    /// The member's part of the ticket's points.
    pub share: f64,
    /// When it was delivered, as stored.
    pub delivered_at: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MemberDetail {
    pub id: String,
    pub name: String,
    pub points: f64,
    pub delivered: usize,
    pub scored: usize,
    /// In working status, not scored.
    pub active: usize,
    /// Scored out of delivered, whole percent; 0 without delivered tickets.
    pub coverage_percent: i64,
    /// Change against the previous period, whole percent; `None` without
    /// anything to compare with.
    pub points_change: Option<f64>,
    pub delivered_change: Option<f64>,
    /// Scored tickets, whole percent.
    pub scored_change: Option<f64>,
    /// Coverage, in percentage points; `None` when the member delivered
    /// nothing in the previous period.
    pub coverage_change: Option<i64>,
    pub levels: LevelCounts,
    /// Points the member earned at each level.
    pub level_points: Points,
    pub days: BTreeMap<NaiveDate, DayDelivery>,
    pub rhythm: Rhythm,
    /// Newest delivery first.
    pub recent: Vec<RecentTicket>,
}

/// The page of member `id`, `None` when the board has no such member.
/// `previous` is the board of the previous period, when it is known.
pub fn member_detail(board: &Board, id: &str, previous: Option<&Board>) -> Option<MemberDetail> {
    let row = board.members.iter().find(|m| m.id == id)?;
    let before = previous.and_then(|b| b.members.iter().find(|m| m.id == id));
    let (before_points, before_delivered, before_scored) = before.map_or((0.0, 0.0, 0.0), |m| {
        (m.points, m.tickets as f64, m.assessed as f64)
    });
    let coverage_percent = percent(row.assessed as f64, row.tickets as f64);
    let coverage_change = before
        .filter(|m| previous.is_some() && m.tickets > 0)
        .map(|m| coverage_percent - percent(m.assessed as f64, m.tickets as f64));
    let mut days: BTreeMap<NaiveDate, DayDelivery> = BTreeMap::new();
    let mut rhythm: Rhythm = [[0; 24]; 7];
    let mut level_points: Points = ByLevel([0.0; 5]);
    let mut dated: Vec<(i64, &super::board::MemberTicket)> = Vec::new();
    for entry in &row.ticket_list {
        let at = delivered_at(&entry.ticket).at;
        let when = parse_iso(at);
        if let Some(when) = when {
            let day = days.entry(when.date_naive()).or_default();
            day.tickets += 1;
            day.points += entry.share;
            rhythm[when.weekday().num_days_from_monday() as usize][when.hour() as usize] += 1;
        }
        if let Some(level) = entry.level {
            level_points[level] += entry.share;
        }
        dated.push((when.map_or(i64::MIN, |t| t.timestamp_millis()), entry));
    }
    dated.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.ticket.id.cmp(&b.1.ticket.id))
    });
    let recent = dated
        .into_iter()
        .take(RECENT_LIMIT)
        .map(|(_, entry)| RecentTicket {
            id: entry.ticket.id.clone(),
            title: entry.ticket.title.clone(),
            level: entry.level,
            share: entry.share,
            delivered_at: delivered_at(&entry.ticket).at.to_string(),
        })
        .collect();
    Some(MemberDetail {
        id: row.id.clone(),
        name: row.name.clone(),
        points: row.points,
        delivered: row.tickets,
        scored: row.assessed,
        active: row.active,
        coverage_percent,
        points_change: previous.and_then(|_| percent_change(row.points, before_points)),
        delivered_change: previous
            .and_then(|_| percent_change(row.tickets as f64, before_delivered)),
        scored_change: previous.and_then(|_| percent_change(row.assessed as f64, before_scored)),
        coverage_change,
        levels: row.levels,
        level_points,
        days,
        rhythm,
        recent,
    })
}

/// The levels with their counts and points, for a breakdown. Levels the member
/// has no scored ticket at are left out.
pub fn level_breakdown(detail: &MemberDetail) -> Vec<(Level, usize, f64)> {
    LEVELS
        .iter()
        .filter(|level| detail.levels[**level] > 0)
        .map(|level| (*level, detail.levels[*level], detail.level_points[*level]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contributors::model::js_iso;
    use crate::contributors::model::period::{current_period, PeriodKind};
    use crate::contributors::model::test_fixtures::local_at;
    use crate::contributors::tickets::board::{build_board, period_tickets};
    use crate::contributors::tickets::test_fixtures::{named, scope, ticket_with};
    use crate::contributors::tickets::tickets::{TicketRecord, DEFAULT_TICKET_POINTS};

    fn on(id: &str, month0: u32, day: u32, hour: u32, level: &str, who: &[&str]) -> TicketRecord {
        ticket_with(id, |t| {
            let when = js_iso(&local_at(2026, month0, day, hour, 30));
            t.done_at = Some(when.clone());
            t.updated_at = when;
            t.override_level = (!level.is_empty()).then(|| level.to_string());
            t.assignees = who.iter().map(|w| named(w, &format!("User {w}"))).collect();
        })
    }

    fn board(tickets: &[TicketRecord], month0: i32) -> Board {
        let now = local_at(2026, 9, 18, 12, 0);
        let mut period = current_period(PeriodKind::Month, &now, None);
        if month0 < 9 {
            period = crate::contributors::model::period::previous_period(&period).unwrap();
        }
        let delivered = period_tickets(tickets, &scope("w1"), &period);
        build_board(&delivered, &DEFAULT_TICKET_POINTS, &[])
    }

    fn october() -> Vec<TicketRecord> {
        vec![
            on("a", 9, 5, 10, "S", &["1"]),
            on("b", 9, 6, 14, "M", &["1", "2"]),
            on("c", 9, 6, 16, "", &["1"]),
            on("d", 9, 13, 9, "XL", &["2"]),
        ]
    }

    #[test]
    fn a_member_is_summed_from_their_row() {
        let detail = member_detail(&board(&october(), 9), "1", None).unwrap();
        assert_eq!(detail.name, "User 1");
        // S (8) + half of M (8); the unscored one is worth nothing.
        assert_eq!(detail.points, 16.0);
        assert_eq!((detail.delivered, detail.scored), (3, 2));
        assert_eq!(detail.coverage_percent, 67);
        assert_eq!(detail.levels[Level::S], 1);
        assert_eq!(detail.level_points[Level::M], 8.0);
        assert_eq!(detail.points_change, None, "nothing to compare with");
        assert_eq!(member_detail(&board(&october(), 9), "nope", None), None);
    }

    #[test]
    fn coverage_never_rounds_across_50_or_100() {
        let all = [on("x", 9, 5, 10, "S", &["1"])];
        assert_eq!(
            member_detail(&board(&all, 9), "1", None)
                .unwrap()
                .coverage_percent,
            100
        );
        let mut three: Vec<TicketRecord> = (0..199)
            .map(|i| on(&format!("u{i}"), 9, 5, 10, "", &["1"]))
            .collect();
        three.push(on("s", 9, 5, 10, "S", &["1"]));
        // 1 of 200 is 0.5%: shown as 1, not 0.
        let detail = member_detail(&board(&three, 9), "1", None).unwrap();
        assert_eq!(detail.coverage_percent, 1);
        let mut near: Vec<TicketRecord> = (0..199)
            .map(|i| on(&format!("s{i}"), 9, 5, 10, "S", &["1"]))
            .collect();
        near.push(on("u", 9, 5, 10, "", &["1"]));
        assert_eq!(
            member_detail(&board(&near, 9), "1", None)
                .unwrap()
                .coverage_percent,
            99,
            "199 of 200 is not 100%"
        );
    }

    #[test]
    fn the_change_against_the_previous_period_needs_something_to_compare() {
        let mut tickets = october();
        tickets.push(on("p1", 8, 3, 10, "S", &["1"]));
        tickets.push(on("p2", 8, 4, 10, "S", &["1"]));
        let now_board = board(&tickets, 9);
        let before = board(&tickets, 8);
        let detail = member_detail(&now_board, "1", Some(&before)).unwrap();
        // 16 points against 16, 3 tickets against 2.
        assert_eq!(detail.points_change, Some(0.0));
        assert_eq!(detail.delivered_change, Some(50.0));
        // 2 scored against 2, and 67% coverage against 100%.
        assert_eq!(detail.scored_change, Some(0.0));
        assert_eq!(detail.coverage_change, Some(-33));
        // Member 2 was not there last month: no change to show.
        let two = member_detail(&now_board, "2", Some(&before)).unwrap();
        assert_eq!((two.points_change, two.delivered_change), (None, None));
        assert_eq!((two.scored_change, two.coverage_change), (None, None));
    }

    #[test]
    fn deliveries_are_counted_per_local_day_and_weekday_hour() {
        let detail = member_detail(&board(&october(), 9), "1", None).unwrap();
        let day = |d| NaiveDate::from_ymd_opt(2026, 10, d).unwrap();
        assert_eq!(detail.days.len(), 2);
        assert_eq!(
            detail.days[&day(5)],
            DayDelivery {
                tickets: 1,
                points: 8.0
            }
        );
        assert_eq!(
            detail.days[&day(6)],
            DayDelivery {
                tickets: 2,
                points: 8.0
            }
        );
        // Oct 5, 2026 is a Monday, Oct 6 a Tuesday.
        assert_eq!(detail.rhythm[0][10], 1);
        assert_eq!(detail.rhythm[1][14], 1);
        assert_eq!(detail.rhythm[1][16], 1);
        assert_eq!(detail.rhythm.iter().flatten().sum::<u32>(), 3);
    }

    #[test]
    fn recent_tickets_come_newest_first_and_stop_at_the_limit() {
        let detail = member_detail(&board(&october(), 9), "1", None).unwrap();
        let ids: Vec<&str> = detail.recent.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["c", "b", "a"]);
        assert_eq!(detail.recent[1].level, Some(Level::M));
        assert_eq!(detail.recent[1].share, 8.0);
        assert_eq!(detail.recent[0].level, None);
        let many: Vec<TicketRecord> = (1..=15)
            .map(|d| on(&format!("t{d}"), 9, d, 10, "S", &["1"]))
            .collect();
        let detail = member_detail(&board(&many, 9), "1", None).unwrap();
        assert_eq!(detail.recent.len(), RECENT_LIMIT);
        assert_eq!(detail.recent[0].id, "t15");
    }

    #[test]
    fn the_breakdown_lists_the_levels_the_member_has() {
        let detail = member_detail(&board(&october(), 9), "2", None).unwrap();
        assert_eq!(
            level_breakdown(&detail),
            [(Level::M, 1, 8.0), (Level::Xl, 1, 48.0)]
        );
    }
}
