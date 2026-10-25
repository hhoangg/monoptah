//! Periods on the local calendar. A period is a span of whole local days; its
//! end is exclusive and is a local midnight. All calendar math goes through
//! dates, never through 24 hour offsets, so a DST change cannot shift a day.

use chrono::{
    DateTime, Datelike, Duration, Local, LocalResult, NaiveDate, NaiveDateTime, TimeZone, Utc,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PeriodKind {
    Days7,
    Month,
    Quarter,
    Year,
    All,
}

impl PeriodKind {
    /// The text used in saved preferences: `7d`, `month`, `quarter`, `year`, `all`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Days7 => "7d",
            Self::Month => "month",
            Self::Quarter => "quarter",
            Self::Year => "year",
            Self::All => "all",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        PERIOD_KINDS
            .iter()
            .map(|(kind, _)| *kind)
            .find(|kind| kind.as_str() == text)
    }
}

/// A span of whole local days. `end` is exclusive and is a local midnight.
#[derive(Clone, Debug, PartialEq)]
pub struct Period {
    pub kind: PeriodKind,
    /// `7d`, `2026-09`, `2026-Q3`, `2026` or `all`.
    pub id: String,
    /// `Last 7 days`, `Sep 2026`, `Q3 2026`, `2026` or `All time`.
    pub label: String,
    pub start: DateTime<Local>,
    pub end: DateTime<Local>,
}

pub const PERIOD_KINDS: [(PeriodKind, &str); 5] = [
    (PeriodKind::Days7, "7d"),
    (PeriodKind::Month, "Month"),
    (PeriodKind::Quarter, "Quarter"),
    (PeriodKind::Year, "Year"),
    (PeriodKind::All, "All time"),
];

pub const DEFAULT_PERIOD_KIND: PeriodKind = PeriodKind::Days7;

pub(crate) const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// `new Date(y, m, d)`: local midnight of a calendar day. A midnight that does
/// not exist (a DST change at 00:00) moves forward to the first instant that does.
pub(crate) fn midnight(date: NaiveDate) -> DateTime<Local> {
    let start = date.and_hms_opt(0, 0, 0).expect("midnight");
    for hours in 0..=3 {
        match Local.from_local_datetime(&(start + Duration::hours(hours))) {
            LocalResult::Single(t) | LocalResult::Ambiguous(t, _) => return t,
            LocalResult::None => {}
        }
    }
    Local.from_utc_datetime(&start)
}

pub(crate) fn start_of_day(t: &DateTime<Local>) -> DateTime<Local> {
    midnight(t.date_naive())
}

pub(crate) fn add_days(t: &DateTime<Local>, days: i64) -> DateTime<Local> {
    midnight(t.date_naive() + Duration::days(days))
}

/// First of a month given a 0-based month index that may overflow (-1, 12).
pub(crate) fn first_of_month(year: i32, month0: i32) -> NaiveDate {
    let total = year * 12 + month0;
    NaiveDate::from_ymd_opt(total.div_euclid(12), total.rem_euclid(12) as u32 + 1, 1)
        .unwrap_or(NaiveDate::MIN)
}

/// `YYYY-MM-DD` of a local calendar day.
pub(crate) fn day_key_of(t: &DateTime<Local>) -> String {
    let d = t.date_naive();
    format!("{}-{:02}-{:02}", d.year(), d.month(), d.day())
}

/// The last 7 days ending the day before `end_exclusive`.
fn last_seven_days(end_exclusive: DateTime<Local>) -> Period {
    Period {
        kind: PeriodKind::Days7,
        id: "7d".into(),
        label: "Last 7 days".into(),
        start: add_days(&end_exclusive, -7),
        end: end_exclusive,
    }
}

// Month, quarter and year are built from a month index; overflow such as month
// -1 or 12 is normalized.
fn month_period(year: i32, month0: i32) -> Period {
    let first = first_of_month(year, month0);
    let (y, m) = (first.year(), first.month0());
    Period {
        kind: PeriodKind::Month,
        id: format!("{y}-{:02}", m + 1),
        label: format!("{} {y}", MONTHS[m as usize]),
        start: midnight(first),
        end: midnight(first_of_month(y, m as i32 + 1)),
    }
}

fn quarter_period(year: i32, quarter: i32) -> Period {
    let first = first_of_month(year, quarter * 3);
    let (y, m) = (first.year(), first.month0());
    let q = m / 3;
    Period {
        kind: PeriodKind::Quarter,
        id: format!("{y}-Q{}", q + 1),
        label: format!("Q{} {y}", q + 1),
        start: midnight(first),
        end: midnight(first_of_month(y, m as i32 + 3)),
    }
}

fn year_period(year: i32) -> Period {
    Period {
        kind: PeriodKind::Year,
        id: year.to_string(),
        label: year.to_string(),
        start: midnight(first_of_month(year, 0)),
        end: midnight(first_of_month(year + 1, 0)),
    }
}

/// Everything from the day of the first commit to today. It has no period
/// before it, so there is nothing to compare with.
fn all_time_period(first_commit: Option<&DateTime<Local>>, now: &DateTime<Local>) -> Period {
    let today = start_of_day(now);
    let first = first_commit.map_or(today, start_of_day);
    Period {
        kind: PeriodKind::All,
        id: "all".into(),
        label: "All time".into(),
        start: if first < today { first } else { today },
        end: add_days(now, 1),
    }
}

/// The period of `kind` that contains `now` (for 7d, the 7 days ending today).
/// All time starts on the day of `first_commit`; without one (not known yet, or
/// a repository without commits) it covers today only.
pub fn current_period(
    kind: PeriodKind,
    now: &DateTime<Local>,
    first_commit: Option<&DateTime<Local>>,
) -> Period {
    let (year, month0) = (now.year(), now.month0() as i32);
    match kind {
        PeriodKind::Days7 => last_seven_days(add_days(now, 1)),
        PeriodKind::Month => month_period(year, month0),
        PeriodKind::Quarter => quarter_period(year, month0 / 3),
        PeriodKind::Year => year_period(year),
        PeriodKind::All => all_time_period(first_commit, now),
    }
}

/// The period of the same kind right before `p`; `None` for All time, which has none.
pub fn previous_period(p: &Period) -> Option<Period> {
    let (year, month0) = (p.start.year(), p.start.month0() as i32);
    Some(match p.kind {
        PeriodKind::Days7 => last_seven_days(p.start),
        PeriodKind::Month => month_period(year, month0 - 1),
        PeriodKind::Quarter => quarter_period(year, month0 / 3 - 1),
        PeriodKind::Year => year_period(year - 1),
        PeriodKind::All => return None,
    })
}

/// The period after `p`, or `None` when it would start after today (and always for 7d and All time).
pub fn next_period(p: &Period, now: &DateTime<Local>) -> Option<Period> {
    let (year, month0) = (p.start.year(), p.start.month0() as i32);
    let next = match p.kind {
        PeriodKind::Days7 | PeriodKind::All => return None,
        PeriodKind::Month => month_period(year, month0 + 1),
        PeriodKind::Quarter => quarter_period(year, month0 / 3 + 1),
        PeriodKind::Year => year_period(year + 1),
    };
    (next.start <= start_of_day(now)).then_some(next)
}

/// Exactly `n` ASCII digits as a number.
fn ascii_number(text: &str, n: usize) -> Option<i32> {
    (text.len() == n && text.bytes().all(|b| b.is_ascii_digit()))
        .then(|| text.parse().ok())
        .flatten()
}

/// The period a saved id stands for, or `None` when the id is not one of
/// `kind` or names a period that starts after today. The last 7 days and All
/// time have nothing to pin, so no id stands for either.
pub fn period_from_id(kind: PeriodKind, id: &str, now: &DateTime<Local>) -> Option<Period> {
    let found = match kind {
        PeriodKind::All => return None,
        PeriodKind::Days7 => (id == "7d").then(|| current_period(PeriodKind::Days7, now, None))?,
        PeriodKind::Month => {
            let (year, month) = id.split_once('-')?;
            let month = ascii_number(month, 2).filter(|m| (1..=12).contains(m))?;
            month_period(ascii_number(year, 4)?, month - 1)
        }
        PeriodKind::Quarter => {
            let (year, quarter) = id.split_once("-Q")?;
            let quarter = ascii_number(quarter, 1).filter(|q| (1..=4).contains(q))?;
            quarter_period(ascii_number(year, 4)?, quarter - 1)
        }
        PeriodKind::Year => year_period(ascii_number(id, 4)?),
    };
    (found.id == id && found.start <= start_of_day(now)).then_some(found)
}

#[derive(Clone, Debug, PartialEq)]
pub struct SyncWindow {
    pub from: DateTime<Local>,
    pub to: DateTime<Local>,
}

/// The days a period needs commits for: from the start of the previous period
/// (rank movement and KPI deltas compare with it) to the end of `p`, capped at
/// tomorrow 00:00 so the current period does not reach into the future. All
/// time has no previous period, so it needs only its own days.
pub fn sync_window(p: &Period, now: &DateTime<Local>) -> SyncWindow {
    let tomorrow = add_days(now, 1);
    SyncWindow {
        from: previous_period(p).unwrap_or_else(|| p.clone()).start,
        to: if p.end < tomorrow { p.end } else { tomorrow },
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Span {
    pub start: DateTime<Local>,
    pub end: DateTime<Local>,
}

/// The span of the previous period that matches how much of `p` has elapsed, so
/// a partly elapsed period is compared with the same days of the one before it:
/// the 3rd of October against Sep 1-3, not the whole of September. A complete
/// period compares whole with whole. Built from calendar days, so a DST change
/// does not shift the end. `None` when there is no previous period (All time).
pub fn previous_comparable_span(p: &Period, now: &DateTime<Local>) -> Option<Span> {
    let before = previous_period(p)?;
    let elapsed_end = sync_window(p, now).to;
    let days = (elapsed_end.date_naive() - p.start.date_naive()).num_days();
    let end = add_days(&before.start, days);
    Some(Span {
        start: before.start,
        end: if end < before.end { end } else { before.end },
    })
}

/// A timestamp as JS `new Date(text)` reads the common shapes: RFC 3339, a
/// date-time without an offset (local time) and a bare date (UTC midnight).
pub fn parse_iso(text: &str) -> Option<DateTime<Local>> {
    let text = text.trim();
    if let Ok(t) = DateTime::parse_from_rfc3339(text) {
        return Some(t.with_timezone(&Local));
    }
    for format in [
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M:%S%.f",
    ] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(text, format) {
            return Local
                .from_local_datetime(&naive)
                .earliest()
                .or_else(|| Some(Local.from_utc_datetime(&naive)));
        }
    }
    let date = NaiveDate::parse_from_str(text, "%Y-%m-%d").ok()?;
    Some(
        Utc.from_utc_datetime(&date.and_hms_opt(0, 0, 0)?)
            .with_timezone(&Local),
    )
}

pub fn is_in_period(p: &Period, iso: &str) -> bool {
    parse_iso(iso).is_some_and(|t| t >= p.start && t < p.end)
}

/// "YYYY-MM-DD" of the local calendar day, or "" for an unparseable timestamp.
pub fn local_day_key(iso: &str) -> String {
    parse_iso(iso).map_or_else(String::new, |t| day_key_of(&t))
}

pub fn is_weekend(day_key: &str) -> bool {
    let mut parts = day_key.split('-');
    let (Some(y), Some(m), Some(d), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    let (Some(y), Some(m), Some(d)) = (ascii_number(y, 4), ascii_number(m, 2), ascii_number(d, 2))
    else {
        return false;
    };
    // `new Date(y, m - 1, d)` rolls an overflowing month or day over.
    let date = first_of_month(y, m - 1) + Duration::days(i64::from(d) - 1);
    matches!(date.weekday(), chrono::Weekday::Sat | chrono::Weekday::Sun)
}

/// Local day keys of the period, oldest first, not beyond today.
pub fn period_day_keys(p: &Period, now: &DateTime<Local>) -> Vec<String> {
    let today = start_of_day(now);
    let mut keys = Vec::new();
    let mut d = p.start;
    while d < p.end {
        if d > today {
            break;
        }
        keys.push(day_key_of(&d));
        d = add_days(&d, 1);
    }
    keys
}

/// What the user selected. A `None` id follows the current period of the kind,
/// so a window left open across midnight (or a month change) moves with the
/// calendar; an id pins a past period.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeriodPick {
    pub kind: PeriodKind,
    pub id: Option<String>,
}

/// The pick that selects `p`: pinned, unless `p` is the current period of its kind.
pub fn pick_of(p: &Period, now: &DateTime<Local>) -> PeriodPick {
    PeriodPick {
        kind: p.kind,
        // All time is always the current one: its id is a constant, not a place on the calendar.
        id: (p.kind != PeriodKind::All && p.id != current_period(p.kind, now, None).id)
            .then(|| p.id.clone()),
    }
}

/// The period a pick stands for today; a pinned id that is no longer valid
/// reads as the current one. `first_commit` is where All time starts.
pub fn resolve_pick(
    pick: &PeriodPick,
    now: &DateTime<Local>,
    first_commit: Option<&DateTime<Local>>,
) -> Period {
    pick.id
        .as_deref()
        .and_then(|id| period_from_id(pick.kind, id, now))
        .unwrap_or_else(|| current_period(pick.kind, now, first_commit))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contributors::model::test_fixtures::{
        iso, local_at, local_date, local_dt, prior_of, with_tz,
    };
    use chrono::{DateTime, Datelike, Local, Timelike};

    fn now() -> DateTime<Local> {
        local_at(2026, 8, 15, 14, 30) // Tue 2026-09-15
    }

    fn ymd(d: &DateTime<Local>) -> [i32; 3] {
        [d.year(), d.month() as i32, d.day() as i32]
    }

    fn ms(d: &DateTime<Local>) -> i64 {
        d.timestamp_millis()
    }

    fn pick(kind: PeriodKind, id: Option<&str>) -> PeriodPick {
        PeriodPick {
            kind,
            id: id.map(String::from),
        }
    }

    #[test]
    fn kinds_lists_the_five_kinds_all_time_last_and_defaults_to_7d() {
        let listed: Vec<(&str, &str)> = PERIOD_KINDS
            .iter()
            .map(|(kind, label)| (kind.as_str(), *label))
            .collect();
        assert_eq!(
            listed,
            [
                ("7d", "7d"),
                ("month", "Month"),
                ("quarter", "Quarter"),
                ("year", "Year"),
                ("all", "All time"),
            ]
        );
        assert_eq!(DEFAULT_PERIOD_KIND, PeriodKind::Days7);
    }

    // currentPeriod

    #[test]
    fn current_period_7d_is_today_and_the_six_days_before_ending_tomorrow_midnight() {
        let p = current_period(PeriodKind::Days7, &now(), None);
        assert_eq!(p.kind, PeriodKind::Days7);
        assert_eq!(p.id, "7d");
        assert_eq!(p.label, "Last 7 days");
        assert_eq!(ms(&p.start), ms(&local_date(2026, 8, 9)));
        assert_eq!(ms(&p.end), ms(&local_date(2026, 8, 16)));
    }

    #[test]
    fn current_period_month_covers_the_calendar_month() {
        let p = current_period(PeriodKind::Month, &now(), None);
        assert_eq!(p.id, "2026-09");
        assert_eq!(p.label, "Sep 2026");
        assert_eq!(ms(&p.start), ms(&local_date(2026, 8, 1)));
        assert_eq!(ms(&p.end), ms(&local_date(2026, 9, 1)));
    }

    #[test]
    fn current_period_puts_the_first_and_last_local_instants_of_a_month_in_the_same_period() {
        let id = |d: DateTime<Local>| current_period(PeriodKind::Month, &d, None).id;
        assert_eq!(id(local_dt(2026, 0, 1, 0, 0, 0, 0)), "2026-01");
        assert_eq!(id(local_dt(2026, 0, 31, 23, 59, 59, 999)), "2026-01");
        assert_eq!(id(local_date(2026, 1, 1)), "2026-02");
    }

    #[test]
    fn current_period_quarter_covers_three_months() {
        let p = current_period(PeriodKind::Quarter, &now(), None);
        assert_eq!(p.id, "2026-Q3");
        assert_eq!(p.label, "Q3 2026");
        assert_eq!(ymd(&p.start), [2026, 7, 1]);
        assert_eq!(ymd(&p.end), [2026, 10, 1]);
        let q = |d| current_period(PeriodKind::Quarter, &d, None).id;
        assert_eq!(q(local_date(2026, 2, 31)), "2026-Q1");
        assert_eq!(q(local_date(2026, 3, 1)), "2026-Q2");
    }

    #[test]
    fn current_period_year_covers_the_calendar_year() {
        let p = current_period(PeriodKind::Year, &now(), None);
        assert_eq!(p.id, "2026");
        assert_eq!(p.label, "2026");
        assert_eq!(ymd(&p.start), [2026, 1, 1]);
        assert_eq!(ymd(&p.end), [2027, 1, 1]);
    }

    #[test]
    fn current_period_knows_the_length_of_a_leap_february() {
        let leap = current_period(PeriodKind::Month, &local_date(2028, 1, 10), None);
        assert_eq!(period_day_keys(&leap, &local_date(2028, 2, 20)).len(), 29);
        let plain = current_period(PeriodKind::Month, &local_date(2027, 1, 10), None);
        assert_eq!(period_day_keys(&plain, &local_date(2027, 2, 20)).len(), 28);
        assert_eq!(ymd(&leap.end), [2028, 3, 1]);
    }

    // previousPeriod

    #[test]
    fn previous_period_7d_is_the_7_days_before() {
        let prev = prior_of(&current_period(PeriodKind::Days7, &now(), None));
        assert_eq!(prev.kind, PeriodKind::Days7);
        assert_eq!(ymd(&prev.start), [2026, 9, 2]);
        assert_eq!(ymd(&prev.end), [2026, 9, 9]);
    }

    #[test]
    fn previous_period_month_rolls_jan_back_to_the_previous_december() {
        let prev = prior_of(&current_period(
            PeriodKind::Month,
            &local_date(2027, 0, 10),
            None,
        ));
        assert_eq!(prev.id, "2026-12");
        assert_eq!(prev.label, "Dec 2026");
        assert_eq!(ymd(&prev.end), [2027, 1, 1]);
    }

    #[test]
    fn previous_period_quarter_rolls_q1_back_to_q4_of_the_previous_year() {
        let prev = prior_of(&current_period(
            PeriodKind::Quarter,
            &local_date(2027, 1, 1),
            None,
        ));
        assert_eq!(prev.id, "2026-Q4");
        assert_eq!(prev.label, "Q4 2026");
        assert_eq!(ymd(&prev.start), [2026, 10, 1]);
        assert_eq!(ymd(&prev.end), [2027, 1, 1]);
    }

    #[test]
    fn previous_period_year_is_the_year_before() {
        let prev = prior_of(&current_period(PeriodKind::Year, &now(), None));
        assert_eq!(prev.id, "2025");
        assert_eq!(ymd(&prev.start), [2025, 1, 1]);
    }

    #[test]
    fn previous_period_walks_back_over_a_leap_february() {
        let mar = current_period(PeriodKind::Month, &local_date(2028, 2, 5), None);
        let feb = prior_of(&mar);
        assert_eq!(feb.id, "2028-02");
        assert_eq!(ymd(&feb.end), [2028, 3, 1]);
    }

    // nextPeriod

    #[test]
    fn next_period_is_none_for_the_current_period_of_every_kind() {
        for (kind, _) in PERIOD_KINDS {
            assert!(next_period(&current_period(kind, &now(), None), &now()).is_none());
        }
    }

    #[test]
    fn next_period_steps_forward_from_a_past_period() {
        let id = |p: &Period| next_period(p, &now()).map(|n| n.id);
        let aug = prior_of(&current_period(PeriodKind::Month, &now(), None));
        assert_eq!(id(&aug).as_deref(), Some("2026-09"));
        let q2 = prior_of(&current_period(PeriodKind::Quarter, &now(), None));
        assert_eq!(id(&q2).as_deref(), Some("2026-Q3"));
        let y = prior_of(&current_period(PeriodKind::Year, &now(), None));
        assert_eq!(id(&y).as_deref(), Some("2026"));
    }

    #[test]
    fn next_period_crosses_the_year_boundary() {
        let later = local_date(2027, 0, 5);
        let dec = current_period(PeriodKind::Month, &local_date(2026, 11, 5), None);
        assert_eq!(next_period(&dec, &later).unwrap().id, "2027-01");
        let q4 = current_period(PeriodKind::Quarter, &local_date(2026, 11, 5), None);
        assert_eq!(next_period(&q4, &later).unwrap().id, "2027-Q1");
    }

    #[test]
    fn next_period_allows_a_period_that_starts_today() {
        let nov = current_period(PeriodKind::Month, &local_date(2026, 10, 20), None);
        let first_second = local_dt(2026, 11, 1, 0, 0, 1, 0);
        assert_eq!(next_period(&nov, &first_second).unwrap().id, "2026-12");
        assert!(next_period(&nov, &local_at(2026, 10, 30, 23, 59)).is_none());
    }

    // periodFromId

    #[test]
    fn period_from_id_round_trips_every_kind() {
        for (kind, _) in PERIOD_KINDS {
            // All time has no id to pin; it is covered in its own block.
            if kind == PeriodKind::All {
                continue;
            }
            let p = current_period(kind, &now(), None);
            let back = period_from_id(kind, &p.id, &now()).unwrap();
            assert_eq!(back.id, p.id);
            assert_eq!(ms(&back.start), ms(&p.start));
            assert_eq!(ms(&back.end), ms(&p.end));
            let prev = prior_of(&p);
            let from_prev = period_from_id(kind, &prev.id, &now()).unwrap();
            let expected = if kind == PeriodKind::Days7 {
                &p.start
            } else {
                &prev.start
            };
            assert_eq!(ms(&from_prev.start), ms(expected));
        }
    }

    #[test]
    fn period_from_id_rejects_ids_that_are_not_of_the_kind() {
        let n = now();
        for (kind, id) in [
            (PeriodKind::Days7, "30d"),
            (PeriodKind::Days7, "2026-09"),
            (PeriodKind::Month, "2026-13"),
            (PeriodKind::Month, "2026-00"),
            (PeriodKind::Month, "2026-9"),
            (PeriodKind::Month, "2026-Q3"),
            (PeriodKind::Quarter, "2026-Q5"),
            (PeriodKind::Quarter, "2026-Q0"),
            (PeriodKind::Quarter, "2026-09"),
            (PeriodKind::Year, "26"),
            (PeriodKind::Year, "2026-09"),
            (PeriodKind::Year, ""),
            (PeriodKind::Month, "garbage"),
        ] {
            assert!(period_from_id(kind, id, &n).is_none(), "{kind:?} {id}");
        }
    }

    #[test]
    fn period_from_id_rejects_a_period_that_starts_after_today() {
        let n = now();
        assert!(period_from_id(PeriodKind::Month, "2026-10", &n).is_none());
        assert!(period_from_id(PeriodKind::Quarter, "2026-Q4", &n).is_none());
        assert!(period_from_id(PeriodKind::Year, "2027", &n).is_none());
        assert_eq!(
            period_from_id(PeriodKind::Month, "2026-09", &n).unwrap().id,
            "2026-09"
        );
    }

    #[test]
    fn period_from_id_does_not_read_non_ascii_digits() {
        let n = now();
        assert!(period_from_id(PeriodKind::Year, "\u{0662}\u{0660}\u{0662}\u{0666}", &n).is_none());
        assert!(period_from_id(PeriodKind::Month, "2026-\u{0660}9", &n).is_none());
    }

    // pickOf and resolvePick

    #[test]
    fn pick_of_follows_the_current_period_with_a_none_id_and_pins_a_past_one() {
        for (kind, _) in PERIOD_KINDS {
            assert_eq!(
                pick_of(&current_period(kind, &now(), None), &now()),
                pick(kind, None)
            );
        }
        let aug = prior_of(&current_period(PeriodKind::Month, &now(), None));
        assert_eq!(
            pick_of(&aug, &now()),
            pick(PeriodKind::Month, Some("2026-08"))
        );
    }

    #[test]
    fn resolve_pick_resolves_a_none_id_to_the_current_period_as_the_days_pass() {
        let p = pick(PeriodKind::Month, None);
        assert_eq!(resolve_pick(&p, &now(), None).id, "2026-09");
        assert_eq!(
            resolve_pick(&p, &local_date(2026, 9, 2), None).id,
            "2026-10"
        );
    }

    #[test]
    fn resolve_pick_resolves_a_pinned_id_or_the_current_period_when_it_is_not_valid() {
        let n = now();
        assert_eq!(
            resolve_pick(&pick(PeriodKind::Quarter, Some("2026-Q1")), &n, None).label,
            "Q1 2026"
        );
        assert_eq!(
            resolve_pick(&pick(PeriodKind::Quarter, Some("2027-Q1")), &n, None).id,
            "2026-Q3"
        );
        assert_eq!(
            resolve_pick(&pick(PeriodKind::Year, Some("oops")), &n, None).id,
            "2026"
        );
    }

    // syncWindow

    #[test]
    fn sync_window_caps_the_current_period_at_tomorrow_midnight_and_reaches_back_to_the_previous_start(
    ) {
        let n = now();
        let month = sync_window(&current_period(PeriodKind::Month, &n, None), &n);
        assert_eq!(ymd(&month.from), [2026, 8, 1]);
        assert_eq!(ms(&month.to), ms(&local_date(2026, 8, 16)));

        let week = sync_window(&current_period(PeriodKind::Days7, &n, None), &n);
        assert_eq!(ymd(&week.from), [2026, 9, 2]);
        assert_eq!(ms(&week.to), ms(&local_date(2026, 8, 16)));

        let year = sync_window(&current_period(PeriodKind::Year, &n, None), &n);
        assert_eq!(ymd(&year.from), [2025, 1, 1]);
        assert_eq!(ms(&year.to), ms(&local_date(2026, 8, 16)));
    }

    #[test]
    fn sync_window_ends_a_past_period_at_its_own_end() {
        let aug = prior_of(&current_period(PeriodKind::Month, &now(), None));
        let w = sync_window(&aug, &now());
        assert_eq!(ymd(&w.from), [2026, 7, 1]);
        assert_eq!(ymd(&w.to), [2026, 9, 1]);
    }

    // previousComparableSpan

    #[test]
    fn previous_comparable_span_covers_the_same_elapsed_days_of_the_previous_month() {
        let n = local_at(2026, 9, 3, 15, 0); // Oct 3
        let span =
            previous_comparable_span(&current_period(PeriodKind::Month, &n, None), &n).unwrap();
        assert_eq!(ymd(&span.start), [2026, 9, 1]);
        assert_eq!(ymd(&span.end), [2026, 9, 4]);
    }

    #[test]
    fn previous_comparable_span_is_capped_at_the_end_of_a_shorter_previous_month() {
        let n = local_at(2026, 2, 31, 10, 0); // Mar 31 vs 28 days of Feb
        let span =
            previous_comparable_span(&current_period(PeriodKind::Month, &n, None), &n).unwrap();
        assert_eq!(ymd(&span.start), [2026, 2, 1]);
        assert_eq!(ymd(&span.end), [2026, 3, 1]);
    }

    #[test]
    fn previous_comparable_span_is_the_whole_previous_period_for_a_complete_past_period() {
        let quarter = prior_of(&current_period(PeriodKind::Quarter, &now(), None));
        let span = previous_comparable_span(&quarter, &now()).unwrap();
        let before = prior_of(&quarter);
        assert_eq!(ms(&span.start), ms(&before.start));
        assert_eq!(ms(&span.end), ms(&before.end));
    }

    #[test]
    fn previous_comparable_span_is_the_previous_7_days_for_7d() {
        let n = now();
        let span =
            previous_comparable_span(&current_period(PeriodKind::Days7, &n, None), &n).unwrap();
        assert_eq!(ymd(&span.start), [2026, 9, 2]);
        assert_eq!(ymd(&span.end), [2026, 9, 9]);
    }

    #[test]
    fn previous_comparable_span_covers_the_elapsed_part_of_the_previous_quarter_and_year() {
        let n = local_at(2026, 9, 10, 9, 0); // 10 days into Q4
        let q =
            previous_comparable_span(&current_period(PeriodKind::Quarter, &n, None), &n).unwrap();
        assert_eq!(ymd(&q.start), [2026, 7, 1]);
        assert_eq!(ymd(&q.end), [2026, 7, 11]);
        let y = previous_comparable_span(&current_period(PeriodKind::Year, &now(), None), &now())
            .unwrap();
        assert_eq!(ymd(&y.start), [2025, 1, 1]);
        assert_eq!(ymd(&y.end), [2025, 9, 16]);
    }

    #[test]
    fn previous_comparable_span_counts_calendar_days_across_a_dst_change() {
        with_tz(
            "Europe/Berlin",
            module_path!(),
            "previous_comparable_span_counts_calendar_days_across_a_dst_change",
            || {
                if !crate::contributors::model::test_fixtures::has_dst() {
                    return;
                }
                let n = local_at(2026, 3, 2, 10, 0); // Apr 2, after the March change
                let span =
                    previous_comparable_span(&current_period(PeriodKind::Month, &n, None), &n)
                        .unwrap();
                assert_eq!(ymd(&span.start), [2026, 3, 1]);
                assert_eq!(ymd(&span.end), [2026, 3, 3]);
                assert_eq!(span.end.hour(), 0);
            },
        );
    }

    // isInPeriod

    #[test]
    fn is_in_period_includes_the_start_and_excludes_the_end() {
        let sep = current_period(PeriodKind::Month, &now(), None);
        assert!(is_in_period(&sep, &iso(&local_at(2026, 8, 1, 0, 0))));
        assert!(is_in_period(&sep, &iso(&local_at(2026, 8, 30, 23, 59))));
        assert!(!is_in_period(&sep, &iso(&local_at(2026, 9, 1, 0, 0))));
        assert!(!is_in_period(&sep, &iso(&local_at(2026, 7, 31, 23, 59))));
        assert!(!is_in_period(&sep, "not a date"));
    }

    #[test]
    fn is_in_period_treats_a_quarter_across_a_year_boundary_by_local_time() {
        let q4 = current_period(PeriodKind::Quarter, &local_date(2026, 11, 20), None);
        assert!(is_in_period(&q4, &iso(&local_at(2026, 11, 31, 23, 59))));
        assert!(!is_in_period(&q4, &iso(&local_at(2027, 0, 1, 0, 0))));
    }

    // localDayKey and isWeekend

    #[test]
    fn local_day_key_formats_the_local_day_and_rejects_bad_timestamps() {
        assert_eq!(
            local_day_key(&iso(&local_at(2026, 9, 5, 23, 30))),
            "2026-10-05"
        );
        assert_eq!(local_day_key("nope"), "");
    }

    #[test]
    fn is_weekend_flags_saturday_and_sunday_only() {
        assert!(is_weekend("2026-10-10"));
        assert!(is_weekend("2026-10-11"));
        assert!(!is_weekend("2026-10-12"));
        assert!(!is_weekend("garbage"));
    }

    // periodDayKeys

    #[test]
    fn period_day_keys_lists_every_day_of_a_past_period() {
        let aug = prior_of(&current_period(PeriodKind::Month, &now(), None));
        let keys = period_day_keys(&aug, &now());
        assert_eq!(keys.len(), 31);
        assert_eq!(keys[0], "2026-08-01");
        assert_eq!(keys[30], "2026-08-31");
    }

    #[test]
    fn period_day_keys_stops_at_today_for_the_current_period() {
        let keys = period_day_keys(&current_period(PeriodKind::Month, &now(), None), &now());
        assert_eq!(keys.len(), 15);
        assert_eq!(keys[14], "2026-09-15");
        let year = period_day_keys(&current_period(PeriodKind::Year, &now(), None), &now());
        assert_eq!(year.len(), 31 + 28 + 31 + 30 + 31 + 30 + 31 + 31 + 15);
    }

    #[test]
    fn period_day_keys_has_seven_days_for_the_7d_period() {
        assert_eq!(
            period_day_keys(&current_period(PeriodKind::Days7, &now(), None), &now()),
            [
                "2026-09-09",
                "2026-09-10",
                "2026-09-11",
                "2026-09-12",
                "2026-09-13",
                "2026-09-14",
                "2026-09-15",
            ]
        );
    }

    // daylight saving time

    #[test]
    fn dst_counts_7_day_keys_in_the_week_of_the_march_change_in_europe_berlin() {
        with_tz(
            "Europe/Berlin",
            module_path!(),
            "dst_counts_7_day_keys_in_the_week_of_the_march_change_in_europe_berlin",
            || {
                if !crate::contributors::model::test_fixtures::has_dst() {
                    return;
                }
                // DST starts Sunday 2026-03-29: that day has 23 hours.
                let n = local_at(2026, 2, 30, 10, 0);
                let p = current_period(PeriodKind::Days7, &n, None);
                assert_eq!(p.start.hour(), 0);
                assert_eq!(p.end.hour(), 0);
                assert_eq!(
                    period_day_keys(&p, &n),
                    [
                        "2026-03-24",
                        "2026-03-25",
                        "2026-03-26",
                        "2026-03-27",
                        "2026-03-28",
                        "2026-03-29",
                        "2026-03-30",
                    ]
                );
                // The week that ends on the change day still has 7 keys.
                let sunday = local_at(2026, 2, 29, 12, 0);
                assert_eq!(
                    period_day_keys(&current_period(PeriodKind::Days7, &sunday, None), &sunday)
                        .len(),
                    7
                );
                assert_eq!(period_day_keys(&prior_of(&p), &n).len(), 7);
            },
        );
    }

    #[test]
    fn dst_keeps_month_boundaries_at_local_midnight_across_a_dst_change() {
        with_tz(
            "America/New_York",
            module_path!(),
            "dst_keeps_month_boundaries_at_local_midnight_across_a_dst_change",
            || {
                if !crate::contributors::model::test_fixtures::has_dst() {
                    return;
                }
                // DST ends 2026-11-01 in New York: November starts on a 25 hour day.
                let nov = current_period(PeriodKind::Month, &local_date(2026, 10, 15), None);
                assert_eq!(nov.start.hour(), 0);
                assert_eq!(nov.end.hour(), 0);
                assert_eq!(period_day_keys(&nov, &local_date(2026, 11, 20)).len(), 30);
                assert_eq!(prior_of(&nov).id, "2026-10");
            },
        );
    }

    #[test]
    fn dst_midnight_that_does_not_exist_moves_forward_like_js() {
        // Lebanon skips 00:00-01:00 on 2026-03-29, like `new Date(2026, 2, 29)` in JS
        // the day starts at 01:00.
        with_tz(
            "Asia/Beirut",
            module_path!(),
            "dst_midnight_that_does_not_exist_moves_forward_like_js",
            || {
                // Return unless Beirut is really in effect: `TZ` is ignored on
                // Windows and tzdata may be missing. Check the gap itself:
                // Beirut has no 00:30 on 2026-03-29.
                let gap = NaiveDate::from_ymd_opt(2026, 3, 29)
                    .and_then(|d| d.and_hms_opt(0, 30, 0))
                    .unwrap();
                if !matches!(Local.from_local_datetime(&gap), LocalResult::None) {
                    return;
                }
                let day = local_date(2026, 2, 29);
                // the same instant `new Date(2026, 2, 29)` gives: 01:00 local
                assert_eq!(
                    day.with_timezone(&Local).hour(),
                    1,
                    "the zone database changed: pick another zone with a midnight DST gap"
                );
                assert_eq!(day.date_naive().day(), 29);
                let n = local_at(2026, 2, 29, 12, 0);
                let p = current_period(PeriodKind::Days7, &n, None);
                let keys = period_day_keys(&p, &n);
                assert_eq!(keys.len(), 7);
                assert_eq!(keys[0], "2026-03-23");
                assert_eq!(keys[6], "2026-03-29");
                assert_eq!(p.end.day(), 30);
                assert_eq!(p.end.hour(), 0);
                assert_eq!(local_day_key(&iso(&day)), "2026-03-29");
                let month = current_period(PeriodKind::Month, &n, None);
                assert_eq!(month.id, "2026-03");
                assert_eq!(period_day_keys(&month, &n).len(), 29);
            },
        );
    }

    // all time

    fn first() -> DateTime<Local> {
        local_at(2023, 4, 17, 15, 40) // Wed 2023-05-17 15:40
    }

    #[test]
    fn all_time_starts_on_the_local_day_of_the_first_commit_and_ends_tomorrow_midnight() {
        let p = current_period(PeriodKind::All, &now(), Some(&first()));
        assert_eq!(p.kind, PeriodKind::All);
        assert_eq!(p.id, "all");
        assert_eq!(p.label, "All time");
        assert_eq!(ms(&p.start), ms(&local_date(2023, 4, 17)));
        assert_eq!(ms(&p.end), ms(&local_date(2026, 8, 16)));
    }

    #[test]
    fn all_time_covers_today_only_until_the_first_commit_is_known_or_when_there_is_none() {
        let p = current_period(PeriodKind::All, &now(), None);
        assert_eq!(ms(&p.start), ms(&local_date(2026, 8, 15)));
        assert_eq!(ms(&p.end), ms(&local_date(2026, 8, 16)));
    }

    #[test]
    fn all_time_never_starts_after_today_even_for_a_first_commit_dated_in_the_future() {
        let p = current_period(PeriodKind::All, &now(), Some(&local_date(2027, 0, 5)));
        assert_eq!(ms(&p.start), ms(&local_date(2026, 8, 15)));
    }

    #[test]
    fn all_time_has_no_previous_period_and_no_next_one() {
        let p = current_period(PeriodKind::All, &now(), Some(&first()));
        assert!(previous_period(&p).is_none());
        assert!(next_period(&p, &now()).is_none());
    }

    #[test]
    fn all_time_syncs_exactly_start_to_tomorrow_without_a_doubled_window() {
        let p = current_period(PeriodKind::All, &now(), Some(&first()));
        let w = sync_window(&p, &now());
        assert_eq!(ms(&w.from), ms(&local_date(2023, 4, 17)));
        assert_eq!(ms(&w.to), ms(&local_date(2026, 8, 16)));
    }

    #[test]
    fn all_time_has_nothing_to_compare_with() {
        assert!(previous_comparable_span(
            &current_period(PeriodKind::All, &now(), Some(&first())),
            &now()
        )
        .is_none());
    }

    #[test]
    fn all_time_has_no_id_to_pin_the_id_never_resolves_to_a_period() {
        assert!(period_from_id(PeriodKind::All, "all", &now()).is_none());
        assert_eq!(
            pick_of(
                &current_period(PeriodKind::All, &now(), Some(&first())),
                &now()
            ),
            pick(PeriodKind::All, None)
        );
    }

    #[test]
    fn all_time_resolves_a_pick_of_all_through_the_first_commit() {
        let p = resolve_pick(&pick(PeriodKind::All, None), &now(), Some(&first()));
        assert_eq!(ms(&p.start), ms(&local_date(2023, 4, 17)));
        assert_eq!(p.label, "All time");
    }

    #[test]
    fn all_time_counts_its_day_keys_from_the_first_day_to_today() {
        let first = local_at(2026, 8, 12, 9, 0);
        assert_eq!(
            period_day_keys(
                &current_period(PeriodKind::All, &now(), Some(&first)),
                &now()
            ),
            ["2026-09-12", "2026-09-13", "2026-09-14", "2026-09-15"]
        );
    }

    #[test]
    fn all_time_contains_commits_from_the_first_day_on_and_none_after_tomorrow() {
        let p = current_period(PeriodKind::All, &now(), Some(&first()));
        assert!(is_in_period(&p, &iso(&local_at(2023, 4, 17, 0, 1))));
        assert!(!is_in_period(&p, &iso(&local_at(2023, 4, 16, 23, 59))));
        assert!(is_in_period(&p, &iso(&now())));
    }

    // not in the TS suite: instants that are not full RFC 3339

    #[test]
    fn parse_iso_accepts_the_shapes_js_date_accepts() {
        let full = parse_iso("2026-10-05T10:00:00.000Z").unwrap();
        assert_eq!(full.timestamp(), 1_791_194_400);
        assert_eq!(
            parse_iso("2026-10-05T10:00:00+07:00").unwrap().timestamp(),
            1_791_169_200
        );
        assert_eq!(parse_iso("2026-10-05").unwrap().timestamp(), 1_791_158_400);
        assert!(parse_iso("2026-10-05T10:00:00").is_some());
        assert!(parse_iso("").is_none());
        assert!(parse_iso("2026-13-45").is_none());
    }
}
