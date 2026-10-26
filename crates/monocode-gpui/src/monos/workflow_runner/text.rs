//! Pure text helpers of a run: UTF-16 slicing (lengths count like JS), the output and error caps
//! and the run folder name.
use monocode_core::mono::workflows::{
    ordered::{Json, OrderedMap},
    template::js_space,
};

/// Longest string kept in a node output, so a run record stays small (UTF-16 units, like JS).
const OUTPUT_STRING_MAX: usize = 16_000;
const ERROR_MAX: usize = 1_000;

pub(super) fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// The longest start of `text` of at most `units` UTF-16 units. A character is never split.
pub(super) fn utf16_head(text: &str, units: usize) -> &str {
    let mut used = 0;
    for (at, c) in text.char_indices() {
        used += c.len_utf16();
        if used > units {
            return &text[..at];
        }
    }
    text
}

/// The longest end of `text` of at most `units` UTF-16 units.
pub(super) fn utf16_tail(text: &str, units: usize) -> &str {
    let mut used = 0;
    for (at, c) in text.char_indices().rev() {
        used += c.len_utf16();
        if used > units {
            return &text[at + c.len_utf8()..];
        }
    }
    text
}

/// Shortens a long string, saying where it cut.
pub(super) fn cap_string(text: &str) -> String {
    let length = utf16_len(text);
    if length <= OUTPUT_STRING_MAX {
        return text.to_string();
    }
    format!(
        "{}\n[truncated, {} more characters]",
        utf16_head(text, OUTPUT_STRING_MAX),
        length - OUTPUT_STRING_MAX
    )
}

/// Shortens every long string inside a node output, saying so where it cut (`capOutput`).
pub(super) fn cap_output(value: &Json) -> Json {
    fn walk(item: &Json, depth: usize) -> Json {
        match item {
            Json::String(text) => Json::String(cap_string(text)),
            Json::Array(items) if depth <= 24 => {
                Json::Array(items.iter().map(|entry| walk(entry, depth + 1)).collect())
            }
            Json::Object(map) if depth <= 24 => Json::Object(
                map.iter()
                    .map(|(key, entry)| (key.clone(), walk(entry, depth + 1)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }
    walk(value, 0)
}

pub(super) fn cap_map(map: &OrderedMap<Json>) -> OrderedMap<Json> {
    map.iter()
        .map(|(key, value)| (key.clone(), cap_output(value)))
        .collect()
}

/// An error kept short: the end of it, which is where a failure says what went wrong.
pub(super) fn clip(text: &str) -> String {
    if utf16_len(text) > ERROR_MAX {
        format!("…{}", utf16_tail(text, ERROR_MAX))
    } else {
        text.to_string()
    }
}

/// "<workflow name> · 2026-10-22 14:05", within the folder name limit.
pub(super) fn run_folder_name(workflow_name: &str, at: u64) -> String {
    use chrono::{Local, TimeZone};
    let stamp = Local
        .timestamp_millis_opt(at as i64)
        .single()
        .map(|time| time.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_default();
    let suffix = format!(" · {stamp}");
    let mut name = String::new();
    let mut gap = false;
    for c in workflow_name.chars() {
        if js_space(c) {
            gap = true;
        } else {
            if gap && !name.is_empty() {
                name.push(' ');
            }
            gap = false;
            name.push(c);
        }
    }
    if name.is_empty() {
        name = "Workflow".into();
    }
    let room = 100usize.saturating_sub(utf16_len(&suffix));
    format!("{}{suffix}", utf16_head(&name, room))
}
