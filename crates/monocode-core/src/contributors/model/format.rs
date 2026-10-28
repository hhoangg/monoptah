//! Number and time formatting for the Contributors views.

use super::js_round;
use super::period::parse_iso;
use chrono::{DateTime, Local};

/// `Intl.NumberFormat("en-US", { maximumFractionDigits: 0 })` of a rounded score.
pub fn format_score(n: f64) -> String {
    if n.is_nan() {
        return "NaN".into();
    }
    if n.is_infinite() {
        return if n > 0.0 { "∞" } else { "-∞" }.into();
    }
    // `+ 0.0` turns -0 into 0 so a tiny negative never prints as "-0".
    let rounded = js_round(n) + 0.0;
    let digits = format!("{:.0}", rounded.abs());
    let mut grouped = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    if rounded < 0.0 {
        format!("-{grouped}")
    } else {
        grouped
    }
}

/// A fraction as a signed percentage: 0.18 -> "+18%".
pub fn format_delta(frac: f64) -> String {
    let pct = js_round(frac * 100.0);
    if pct == 0.0 {
        return "0%".into();
    }
    format!("{}{}%", if pct > 0.0 { "+" } else { "-" }, pct.abs())
}

const MINUTE: i64 = 60_000;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;

/// "5m ago", "2h ago", "12d ago", "3mo ago", "1y ago"; "" for an unparseable timestamp.
pub fn relative_time(iso: &str, now: &DateTime<Local>) -> String {
    let Some(then) = parse_iso(iso) else {
        return String::new();
    };
    let diff = now.timestamp_millis() - then.timestamp_millis();
    if diff < MINUTE {
        return "just now".into();
    }
    if diff < HOUR {
        return format!("{}m ago", diff / MINUTE);
    }
    if diff < DAY {
        return format!("{}h ago", diff / HOUR);
    }
    let days = diff / DAY;
    if days < 60 {
        format!("{days}d ago")
    } else if days < 365 {
        format!("{}mo ago", days / 30)
    } else {
        format!("{}y ago", days / 365)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contributors::model::test_fixtures::local_at;
    use chrono::{DateTime, Duration, Local};

    #[test]
    fn format_score_rounds_and_groups_thousands() {
        assert_eq!(format_score(612.4), "612");
        assert_eq!(format_score(612.5), "613");
        assert_eq!(format_score(1234.6), "1,235");
        assert_eq!(format_score(1_234_567.0), "1,234,567");
        assert_eq!(format_score(0.0), "0");
    }

    #[test]
    fn format_score_never_prints_a_negative_zero() {
        assert_eq!(format_score(-0.2), "0");
        assert_eq!(format_score(-0.0), "0");
    }

    #[test]
    fn format_score_rounds_halves_up_like_js_math_round() {
        assert_eq!(format_score(-2.5), "-2");
        assert_eq!(format_score(-1234.5), "-1,234");
        assert_eq!(format_score(2.5), "3");
    }

    #[test]
    fn format_delta_prints_a_signed_percentage() {
        assert_eq!(format_delta(0.18), "+18%");
        assert_eq!(format_delta(-0.06), "-6%");
        assert_eq!(format_delta(1.5), "+150%");
    }

    #[test]
    fn format_delta_prints_0_percent_without_a_sign() {
        assert_eq!(format_delta(0.0), "0%");
        assert_eq!(format_delta(0.004), "0%");
        assert_eq!(format_delta(-0.004), "0%");
    }

    fn now() -> DateTime<Local> {
        DateTime::parse_from_rfc3339("2026-10-15T12:00:00Z")
            .unwrap()
            .with_timezone(&Local)
    }

    fn ago(ms: i64) -> String {
        (now() - Duration::milliseconds(ms)).to_utc().to_rfc3339()
    }

    const MIN: i64 = 60_000;
    const HOUR: i64 = 60 * MIN;
    const DAY: i64 = 24 * HOUR;

    #[test]
    fn relative_time_says_just_now_under_a_minute_and_for_the_future() {
        assert_eq!(relative_time(&ago(0), &now()), "just now");
        assert_eq!(relative_time(&ago(59_000), &now()), "just now");
        assert_eq!(relative_time(&ago(-5 * MIN), &now()), "just now");
    }

    #[test]
    fn relative_time_uses_minutes_hours_and_days() {
        assert_eq!(relative_time(&ago(5 * MIN), &now()), "5m ago");
        assert_eq!(relative_time(&ago(2 * HOUR + 10 * MIN), &now()), "2h ago");
        assert_eq!(relative_time(&ago(23 * HOUR), &now()), "23h ago");
        assert_eq!(relative_time(&ago(DAY), &now()), "1d ago");
        assert_eq!(relative_time(&ago(12 * DAY + 5 * HOUR), &now()), "12d ago");
    }

    #[test]
    fn relative_time_falls_back_to_months_and_years_for_old_dates() {
        assert_eq!(relative_time(&ago(59 * DAY), &now()), "59d ago");
        assert_eq!(relative_time(&ago(90 * DAY), &now()), "3mo ago");
        assert_eq!(relative_time(&ago(400 * DAY), &now()), "1y ago");
    }

    #[test]
    fn relative_time_returns_an_empty_string_for_an_invalid_date() {
        assert_eq!(relative_time("not a date", &now()), "");
        let _ = local_at(2026, 9, 15, 12, 0);
    }
}
