//! Compact numbers for tiles and axes.

use super::js_round;

/// 38200 -> "38.2k", 2500000 -> "2.5M"; small numbers stay whole.
pub fn format_compact(n: f64) -> String {
    if !n.is_finite() {
        return "0".into();
    }
    let value = js_round(n);
    let abs = value.abs();
    let sign = if value < 0.0 { "-" } else { "" };
    if abs < 1000.0 {
        return format!("{}", value + 0.0);
    }
    let trim = |x: f64| format!("{}", js_round(x * 10.0) / 10.0);
    if abs < 999_950.0 {
        format!("{sign}{}k", trim(abs / 1000.0))
    } else {
        format!("{sign}{}M", trim(abs / 1_000_000.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_compact_keeps_small_numbers_whole() {
        assert_eq!(format_compact(0.0), "0");
        assert_eq!(format_compact(412.0), "412");
        assert_eq!(format_compact(999.0), "999");
    }

    #[test]
    fn format_compact_abbreviates_thousands_with_one_decimal_dropping_a_trailing_zero() {
        assert_eq!(format_compact(1000.0), "1k");
        assert_eq!(format_compact(1234.0), "1.2k");
        assert_eq!(format_compact(38_200.0), "38.2k");
        assert_eq!(format_compact(12_949.0), "12.9k");
    }

    #[test]
    fn format_compact_rolls_over_to_millions_instead_of_printing_1000k() {
        assert_eq!(format_compact(999_999.0), "1M");
        assert_eq!(format_compact(2_500_000.0), "2.5M");
    }

    #[test]
    fn format_compact_never_prints_a_negative_zero_or_nan() {
        assert_eq!(format_compact(-0.0), "0");
        assert_eq!(format_compact(f64::NAN), "0");
    }

    #[test]
    fn format_compact_keeps_the_sign_of_negative_numbers() {
        assert_eq!(format_compact(-412.0), "-412");
        assert_eq!(format_compact(-38_200.0), "-38.2k");
        assert_eq!(format_compact(f64::INFINITY), "0");
    }
}
