//! Settings > Web app > Activity log: the newest lines of the desktop's audit
//! log (`mobile-audit.jsonl`). The page that hosts it calls [`activity_log`].
use crate::{settings::widgets::note, theme::*};
use gpui::{AnyElement, div, prelude::*, px};
use monocode_core::mobile::audit;
use serde_json::Value;
use std::path::Path;

/// How many lines the viewer shows.
const SHOWN: usize = 200;

/// One audit line as the viewer shows it.
#[derive(Debug, PartialEq)]
struct Row {
    time: String,
    device: String,
    transport: String,
    what: String,
    outcome: String,
    path: Option<String>,
}

/// `YYYY-MM-DD HH:MM:SS` in UTC, from unix milliseconds.
fn time_text(ms: u64) -> String {
    let secs = ms / 1000;
    let (days, rest) = ((secs / 86_400) as i64, secs % 86_400);
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

fn text(line: &Value, key: &str) -> String {
    line[key].as_str().unwrap_or_default().to_string()
}

fn row(line: &Value) -> Row {
    let event = text(line, "event");
    let method = text(line, "method");
    Row {
        time: time_text(line["ts"].as_u64().unwrap_or(0)),
        device: text(line, "deviceId"),
        transport: text(line, "transport"),
        what: if event == "call" && !method.is_empty() {
            method
        } else {
            event
        },
        outcome: text(line, "outcome"),
        path: line["path"].as_str().map(str::to_string),
    }
}

fn rows(lines: &[Value]) -> Vec<Row> {
    lines.iter().take(SHOWN).map(row).collect()
}

fn cell(text: String, width: f32, alpha: f32) -> AnyElement {
    div()
        .w(px(width))
        .flex_none()
        .truncate()
        .text_size(px(12.))
        .text_color(content(alpha))
        .child(text)
        .into_any_element()
}

/// The activity list for the given audit lines (newest first): up to 200 rows
/// of time, device, transport, event or method, outcome and, for file reads,
/// the path; or an empty state.
pub(crate) fn activity_list(lines: &[Value]) -> AnyElement {
    let shown = rows(lines);
    if shown.is_empty() {
        return note(
            "No remote activity yet. Calls that change things, refusals and file reads appear here.",
        );
    }
    div()
        .mt_5()
        .overflow_hidden()
        .rounded_xl()
        .border_1()
        .border_color(content(0.1))
        .flex()
        .flex_col()
        .children(shown.into_iter().enumerate().map(|(ix, row)| {
            let denied = row.outcome != "ok";
            div()
                .when(ix > 0, |d| d.border_t_1().border_color(content(0.1)))
                .px_4()
                .py_2()
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .gap_3()
                        .child(cell(row.time, 150., 0.45))
                        .child(cell(row.device, 110., 0.6))
                        .child(cell(row.transport, 44., 0.45))
                        .child(cell(row.what, 180., 0.9))
                        .child(cell(row.outcome, 140., if denied { 0.9 } else { 0.45 })),
                )
                .children(
                    row.path
                        .map(|path| cell(format!("Path: {path}"), 640., 0.6)),
                )
        }))
        .into_any_element()
}

/// The viewer for the audit log in the host's data directory `dir`.
#[allow(dead_code)] // Called by the Web app page once it is registered.
pub(crate) fn activity_log(dir: &Path) -> AnyElement {
    activity_list(&audit::read_recent(dir, SHOWN))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_call_line_becomes_a_row() {
        let line = json!({"ts": 1_700_000_000_000u64, "deviceId": "d1", "transport": "web",
            "event": "call", "method": "project.file", "outcome": "denied:path_denied",
            "path": ".env"});
        assert_eq!(
            row(&line),
            Row {
                time: "2023-11-14 22:13:20".into(),
                device: "d1".into(),
                transport: "web".into(),
                what: "project.file".into(),
                outcome: "denied:path_denied".into(),
                path: Some(".env".into()),
            }
        );
    }

    #[test]
    fn a_lifecycle_line_shows_its_event() {
        let line = json!({"ts": 0, "deviceId": "d1", "transport": "quic",
            "event": "revoke", "method": null, "outcome": "ok"});
        assert_eq!(row(&line).what, "revoke");
    }

    #[test]
    fn at_most_two_hundred_rows_are_shown() {
        let lines = vec![json!({"event": "connect"}); 250];
        assert_eq!(rows(&lines).len(), SHOWN);
        assert!(rows(&[]).is_empty());
    }
}
