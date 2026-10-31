//! Pure Contributors model: config, identities, periods and small formatters.
//! Port of `src/features/contributors/model/`; no I/O besides the preference
//! helpers in `prefs`.

pub mod activity;
pub mod agent_session;
pub mod ask;
pub mod axis;
pub mod compact;
pub mod config;
pub mod format;
pub mod identity;
pub mod leaderboard;
pub mod member;
pub mod palette;
pub mod patterns;
pub mod period;
pub mod pipeline;
pub mod prefs;
pub mod scoring;
pub mod snapshot;
pub mod stats;
pub mod sync_indicator;
pub mod sync_jobs;
pub mod types;
pub mod unmapped;

#[cfg(test)]
pub mod test_fixtures;

/// JS `\s` (and `String.prototype.trim`) whitespace, which differs a little
/// from `char::is_whitespace`: U+FEFF counts, U+0085 does not.
pub(crate) fn js_space(c: char) -> bool {
    (c.is_whitespace() && c != '\u{0085}') || c == '\u{feff}'
}

/// `String.prototype.trim`.
pub(crate) fn js_trim(s: &str) -> &str {
    s.trim_matches(js_space)
}

/// `String.prototype.length`: UTF-16 code units.
pub(crate) fn js_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// The order of the default `Array.prototype.sort` on strings: UTF-16 code units.
pub(crate) fn js_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

/// A close stand-in for `String.prototype.localeCompare` on the names and ids
/// the model sorts: case-insensitive, and for text equal ignoring case the
/// lowercase form first, as ICU does. Accents and scripts are not collated.
pub fn locale_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    a.to_lowercase()
        .cmp(&b.to_lowercase())
        .then_with(|| b.cmp(a))
}

/// `Date.prototype.toISOString` of an instant.
pub(crate) fn js_iso(t: &chrono::DateTime<chrono::Local>) -> String {
    t.with_timezone(&chrono::Utc)
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// A number printed like `JSON.stringify`: the text of `Number.prototype.toString`.
pub(crate) fn serialize_number<S: serde::Serializer>(
    value: &f64,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    if !value.is_finite() {
        // `JSON.stringify(NaN)` is `null`
        return serializer.serialize_none();
    }
    if value.fract() == 0.0 && value.abs() < 9.0e15 {
        return serializer.serialize_i64(*value as i64);
    }
    // serde_json's own float text switches to exponent form at other
    // magnitudes than JS, so the JS text is written as it is.
    serde::Serialize::serialize(&raw_number(*value), serializer)
}

fn raw_number(value: f64) -> Box<serde_json::value::RawValue> {
    serde_json::value::RawValue::from_string(js_number_text(value))
        .expect("a finite number is valid JSON")
}

/// `serialize_number` for a nullable number.
pub(crate) fn serialize_opt_number<S: serde::Serializer>(
    value: &Option<f64>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match value {
        Some(v) => serialize_number(v, serializer),
        None => serializer.serialize_none(),
    }
}

/// `Number.prototype.toString`: the shortest digits that read back to the same
/// number, plain up to 21 integer digits and down to 1e-6, else `1e-7` or
/// `1.5e+21`. Used for template strings and JSON alike.
pub fn js_number_text(value: f64) -> String {
    if value.is_nan() {
        return "NaN".into();
    }
    if value == 0.0 {
        return "0".into();
    }
    let sign = if value < 0.0 { "-" } else { "" };
    if value.is_infinite() {
        return format!("{sign}Infinity");
    }
    // Rust prints the shortest round-trip digits as `d.ddde<exp>`.
    let sci = format!("{:e}", value.abs());
    let (mantissa, exp) = sci.split_once('e').expect("exponent form");
    let mut digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let exp10 = exp.parse::<i32>().expect("exponent");
    if let Some(even) = even_tie(&digits, exp10, value.abs()) {
        digits = even;
    }
    let k = digits.len() as i32;
    // the decimal point sits after `n` digits
    let n = exp10 + 1;
    let body = if k <= n && n <= 21 {
        format!("{digits}{}", "0".repeat((n - k) as usize))
    } else if 0 < n && n <= 21 {
        format!("{}.{}", &digits[..n as usize], &digits[n as usize..])
    } else if -6 < n && n <= 0 {
        format!("0.{}{digits}", "0".repeat((-n) as usize))
    } else {
        let e = n - 1;
        let e = format!("{}{}", if e < 0 { '-' } else { '+' }, e.abs());
        if k == 1 {
            format!("{digits}e{e}")
        } else {
            format!("{}.{}e{e}", &digits[..1], &digits[1..])
        }
    };
    format!("{sign}{body}")
}

/// Two shortest digit strings can both read back to `value` when it sits
/// exactly between them (16 or 17 digits, e.g. 172652608482167.125). JS takes
/// the even one; Rust takes the larger. Returns the even string when `digits`
/// is the odd, larger one of such a pair.
fn even_tie(digits: &str, exp10: i32, value: f64) -> Option<String> {
    let k = digits.len();
    let last = digits.as_bytes()[k - 1];
    if (last - b'0') & 1 == 0 {
        return None;
    }
    let mut alt = digits.as_bytes().to_vec();
    alt[k - 1] -= 1;
    let alt = String::from_utf8(alt).expect("ascii digits");
    let reads_back = format!("{alt}e{}", exp10 - (k as i32 - 1))
        .parse::<f64>()
        .is_ok_and(|v| v == value);
    if !reads_back {
        return None;
    }
    // exactly in the middle: the alternative, a 5, then nothing but zeros
    let exact = format!("{:.*e}", k + 30, value);
    let mantissa: String = exact
        .split_once('e')?
        .0
        .chars()
        .filter(|c| *c != '.')
        .collect();
    let (head, tail) = mantissa.split_at(k);
    (head == alt && tail.starts_with('5') && tail[1..].bytes().all(|b| b == b'0')).then_some(alt)
}

/// `Math.round`: halves go toward +infinity (Rust `round` goes away from zero).
pub(crate) fn js_round(x: f64) -> f64 {
    let floor = x.floor();
    if x - floor >= 0.5 {
        floor + 1.0
    } else {
        floor
    }
}

/// A map that iterates in insertion order, like a JS `Map`.
pub(crate) struct OrderedMap<V> {
    index: std::collections::HashMap<String, usize>,
    items: Vec<(String, V)>,
}

impl<V> OrderedMap<V> {
    pub(crate) fn new() -> Self {
        Self {
            index: std::collections::HashMap::new(),
            items: Vec::new(),
        }
    }

    pub(crate) fn entry(&mut self, key: &str, make: impl FnOnce() -> V) -> &mut V {
        let i = match self.index.get(key) {
            Some(&i) => i,
            None => {
                self.items.push((key.to_string(), make()));
                self.index.insert(key.to_string(), self.items.len() - 1);
                self.items.len() - 1
            }
        };
        &mut self.items[i].1
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (&str, &V)> {
        self.items.iter().map(|(k, v)| (k.as_str(), v))
    }

    pub(crate) fn into_values(self) -> impl Iterator<Item = V> {
        self.items.into_iter().map(|(_, v)| v)
    }

    pub(crate) fn into_pairs(self) -> impl Iterator<Item = (String, V)> {
        self.items.into_iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering;

    #[test]
    fn locale_cmp_ignores_case_and_puts_lowercase_first_on_a_tie() {
        assert_eq!(locale_cmp("amy", "Bob"), Ordering::Less);
        assert_eq!(locale_cmp("Zed", "amy"), Ordering::Greater);
        assert_eq!(locale_cmp("ana", "Ana"), Ordering::Less);
        assert_eq!(locale_cmp("Ana", "Ana"), Ordering::Equal);
        let mut names = vec!["bob", "Cat", "Ana", "ana"];
        names.sort_by(|a, b| locale_cmp(a, b));
        assert_eq!(names, ["ana", "Ana", "bob", "Cat"]);
    }

    #[test]
    fn js_iso_prints_utc_with_milliseconds() {
        use chrono::TimeZone;
        let t = chrono::Utc
            .with_ymd_and_hms(2026, 10, 5, 7, 8, 9)
            .unwrap()
            .with_timezone(&chrono::Local);
        assert_eq!(js_iso(&t), "2026-10-05T07:08:09.000Z");
    }

    #[test]
    fn js_number_text_switches_to_exponent_form_like_number_to_string() {
        for (value, text) in [
            (1e-5, "0.00001"),
            (6.666666666666667e-6, "0.000006666666666666667"),
            (1e-6, "0.000001"),
            (1.5e-6, "0.0000015"),
            (1e-7, "1e-7"),
            (1.5e-7, "1.5e-7"),
            (-1.25e-7, "-1.25e-7"),
            (1e21, "1e+21"),
            (1.5e21, "1.5e+21"),
            (-1.5e21, "-1.5e+21"),
            (123456789012345680000.0, "123456789012345680000"),
            (1e20, "100000000000000000000"),
            (1e16, "10000000000000000"),
            (0.0, "0"),
            (-0.0, "0"),
            (0.1 + 0.2, "0.30000000000000004"),
            (-2.5, "-2.5"),
            (123.456, "123.456"),
            (f64::MAX, "1.7976931348623157e+308"),
            (5e-324, "5e-324"),
        ] {
            assert_eq!(js_number_text(value), text, "{value:e}");
        }
        // two shortest strings read back: JS takes the even one
        for (value, text) in [
            (172_652_608_482_167.0 + 0.125, "172652608482167.12"),
            (730_717_865_284_532.0 + 0.25, "730717865284532.2"),
            (142_783_806_426_450.0 + 0.625, "142783806426450.62"),
            (-(730_717_865_284_532.0 + 0.25), "-730717865284532.2"),
            (0.5, "0.5"),
            (4.35, "4.35"),
        ] {
            assert_eq!(js_number_text(value), text, "{value}");
        }
        assert_eq!(js_number_text(f64::NAN), "NaN");
        assert_eq!(js_number_text(f64::INFINITY), "Infinity");
        assert_eq!(js_number_text(f64::NEG_INFINITY), "-Infinity");
    }

    #[test]
    fn serialize_number_uses_the_same_text_as_number_to_string() {
        #[derive(serde::Serialize)]
        struct N(#[serde(serialize_with = "serialize_number")] f64);
        for (value, text) in [
            (1e-5, "0.00001"),
            (6.666666666666667e-6, "0.000006666666666666667"),
            (1e-6, "0.000001"),
            (1e-7, "1e-7"),
            (1e21, "1e+21"),
            (123456789012345680000.0, "123456789012345680000"),
            (-0.0, "0"),
            (0.1 + 0.2, "0.30000000000000004"),
            (12.0, "12"),
            (-3.0, "-3"),
            (0.5, "0.5"),
            (f64::NAN, "null"),
            (f64::INFINITY, "null"),
        ] {
            assert_eq!(serde_json::to_string(&N(value)).unwrap(), text, "{value:e}");
        }
        // the raw text also survives `to_value` and a pretty print
        let v = serde_json::to_value(N(1e-7)).unwrap();
        assert_eq!(v.as_f64(), Some(1e-7));
        assert_eq!(serde_json::to_string_pretty(&N(1e-7)).unwrap(), "1e-7");
        #[derive(serde::Serialize)]
        struct M(#[serde(serialize_with = "serialize_opt_number")] Option<f64>);
        assert_eq!(serde_json::to_string(&M(Some(1e-7))).unwrap(), "1e-7");
    }

    #[test]
    fn js_number_text_prints_whole_numbers_without_a_fraction() {
        assert_eq!(js_number_text(10.0), "10");
        assert_eq!(js_number_text(0.8), "0.8");
        assert_eq!(js_number_text(1.5), "1.5");
        assert_eq!(js_number_text(0.15 * 100.0), "15");
    }

    #[test]
    fn serialize_number_prints_whole_numbers_like_json_stringify() {
        #[derive(serde::Serialize)]
        struct N(#[serde(serialize_with = "serialize_number")] f64);
        assert_eq!(serde_json::to_string(&N(3.0)).unwrap(), "3");
        assert_eq!(serde_json::to_string(&N(0.1)).unwrap(), "0.1");
        assert_eq!(serde_json::to_string(&N(-2.0)).unwrap(), "-2");
        #[derive(serde::Serialize)]
        struct M(#[serde(serialize_with = "serialize_opt_number")] Option<f64>);
        assert_eq!(serde_json::to_string(&M(None)).unwrap(), "null");
        assert_eq!(serde_json::to_string(&M(Some(4.0))).unwrap(), "4");
    }
}
