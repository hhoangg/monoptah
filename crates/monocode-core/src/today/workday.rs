//! Work days: a day rolls over at a configurable local time, not at midnight.
use super::Day;
use chrono::{Duration, Local, NaiveDate, NaiveDateTime, TimeZone};

const FORMAT: &str = "%Y-%m-%d";

/// The work day `now` (epoch ms) falls on, in the machine's local time.
pub fn work_day(now: u64, rollover_minutes: u32) -> Day {
    let millis = i64::try_from(now).unwrap_or(i64::MAX);
    let local = Local
        .timestamp_millis_opt(millis)
        .earliest()
        .map(|time| time.naive_local())
        .unwrap_or_default();
    work_day_of(local, rollover_minutes)
}
/// The work day of a local wall-clock time; the testable core of [`work_day`].
pub fn work_day_of(local: NaiveDateTime, rollover_minutes: u32) -> Day {
    // Wall-clock arithmetic keeps a DST change from moving the boundary.
    let shifted = local
        .checked_sub_signed(Duration::minutes(i64::from(rollover_minutes)))
        .unwrap_or(local);
    shifted.date().format(FORMAT).to_string()
}
/// Whole days from `from` to `to`; 0 when `to` is not after `from` or either is invalid.
pub fn days_between(from: &str, to: &str) -> u32 {
    match (parse(from), parse(to)) {
        (Some(from), Some(to)) => u32::try_from((to - from).num_days().max(0)).unwrap_or(u32::MAX),
        _ => 0,
    }
}
/// `day` moved by `delta` days; `None` when `day` is not a date.
pub fn add_days(day: &str, delta: i64) -> Option<Day> {
    let date = parse(day)?;
    let moved = date.checked_add_signed(Duration::try_days(delta)?)?;
    Some(moved.format(FORMAT).to_string())
}
/// A strict `YYYY-MM-DD` real date.
pub fn is_day(text: &str) -> bool {
    parse(text).is_some()
}
fn parse(text: &str) -> Option<NaiveDate> {
    let date = NaiveDate::parse_from_str(text, FORMAT).ok()?;
    // chrono accepts "2026-1-5"; the contract is the zero padded form only.
    (date.format(FORMAT).to_string() == text).then_some(date)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn at(y: i32, m: u32, d: u32, h: u32, min: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, m, d)
            .unwrap()
            .and_hms_opt(h, min, 0)
            .unwrap()
    }

    #[test]
    fn day_rolls_over_at_four_by_default() {
        assert_eq!(work_day_of(at(2026, 7, 14, 3, 59), 240), "2026-07-13");
        assert_eq!(work_day_of(at(2026, 7, 14, 4, 0), 240), "2026-07-14");
        assert_eq!(work_day_of(at(2026, 7, 14, 23, 59), 240), "2026-07-14");
        assert_eq!(work_day_of(at(2026, 7, 14, 0, 0), 240), "2026-07-13");
    }

    #[test]
    fn custom_rollover_moves_the_boundary() {
        assert_eq!(work_day_of(at(2026, 7, 14, 0, 0), 0), "2026-07-14");
        assert_eq!(work_day_of(at(2026, 7, 14, 6, 29), 390), "2026-07-13");
        assert_eq!(work_day_of(at(2026, 7, 14, 6, 30), 390), "2026-07-14");
        // 12 hours: noon starts the day.
        assert_eq!(work_day_of(at(2026, 7, 14, 11, 59), 720), "2026-07-13");
        assert_eq!(work_day_of(at(2026, 7, 14, 12, 0), 720), "2026-07-14");
    }

    #[test]
    fn rollover_crosses_month_and_year() {
        assert_eq!(work_day_of(at(2027, 1, 1, 3, 0), 240), "2026-12-31");
        assert_eq!(work_day_of(at(2026, 3, 1, 1, 0), 240), "2026-02-28");
    }

    #[test]
    fn work_day_reads_the_local_clock() {
        let local = |h, m| {
            Local
                .with_ymd_and_hms(2026, 7, 14, h, m, 0)
                .earliest()
                .unwrap()
                .timestamp_millis() as u64
        };
        assert_eq!(work_day(local(3, 59), 240), "2026-07-13");
        assert_eq!(work_day(local(4, 0), 240), "2026-07-14");
    }

    #[test]
    fn days_between_counts_whole_days() {
        assert_eq!(days_between("2026-07-10", "2026-07-14"), 4);
        assert_eq!(days_between("2026-07-14", "2026-07-14"), 0);
        assert_eq!(days_between("2026-07-14", "2026-07-10"), 0);
        assert_eq!(days_between("2026-12-30", "2027-01-02"), 3);
        assert_eq!(days_between("nope", "2026-07-10"), 0);
        assert_eq!(days_between("2026-07-10", ""), 0);
    }

    #[test]
    fn add_days_moves_both_ways() {
        assert_eq!(add_days("2026-07-31", 1).as_deref(), Some("2026-08-01"));
        assert_eq!(add_days("2026-03-01", -1).as_deref(), Some("2026-02-28"));
        assert_eq!(add_days("2026-07-14", 0).as_deref(), Some("2026-07-14"));
        assert_eq!(add_days("garbage", 1), None);
    }

    #[test]
    fn is_day_is_strict() {
        assert!(is_day("2026-07-14"));
        assert!(is_day("2028-02-29"));
        assert!(!is_day("2027-02-29"));
        assert!(!is_day("2026-7-14"));
        assert!(!is_day("2026-07-14 "));
        assert!(!is_day("2026-07-14T10:00"));
        assert!(!is_day(""));
    }
}
