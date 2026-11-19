//! What a conversation says about files, reduced to the path texts the rules compare.
//!
//! Two kinds of evidence, kept apart because they prove different things:
//!
//! * **Mentions**: the file mentions of the ASSISTANT's messages (see [`super::mentions`]). A
//!   user message never counts, a control device can type any path into one.
//! * **Tool paths**: the path-like tokens of the TOOL rows (the input and the output of what the
//!   agent ran: Write, Edit, Read, `apply_patch`, shell command lines, created files). A remote
//!   device cannot write a tool row: only the agent running a tool produces one. A file outside
//!   every project the device selected needs both (see [`super::serve`]), so asking the
//!   assistant to "reply with `~/x`" no longer makes `~/x` readable.
//!
//! Extraction is the expensive part of a call on a long conversation, so [`Evidence`] is built
//! once and kept for a short time per conversation ([`cached`] and [`remember`]).
use std::{
    collections::HashSet,
    sync::{LazyLock, Mutex},
    time::{Duration, Instant},
};

use serde_json::Value;

use super::mentions::{self, extract_mentions};
use crate::transcript::Row;

/// Mentions or tool paths kept at most, so one huge conversation cannot grow the set unbounded.
const MAX_ENTRIES: usize = 100_000;
/// Longest tool row text read for paths, in bytes.
const MAX_TOOL_TEXT: usize = 128 * 1024;
/// Strings of one tool input taken, and how deep.
const MAX_JSON_STRINGS: usize = 2_000;
const MAX_JSON_DEPTH: usize = 8;
/// Longest token kept, in characters.
const MAX_TOKEN_CHARS: usize = 1024;

/// The file mentions of assistant messages and the path tokens of tool rows.
#[derive(Clone, Debug, Default)]
pub struct Evidence {
    /// Mentions as written, in order of discovery, without duplicates.
    pub assistant: Vec<String>,
    /// Path-like tokens of tool rows, without duplicates.
    pub tool: Vec<String>,
    seen_assistant: HashSet<String>,
    seen_tool: HashSet<String>,
}

impl Evidence {
    /// The evidence of `rows` (any order).
    pub fn from_rows<'a>(rows: impl IntoIterator<Item = &'a Row>) -> Self {
        let mut evidence = Self::default();
        evidence.absorb_rows(rows);
        evidence
    }

    /// The evidence of plain texts: `assistant` messages and `tool` row texts.
    pub fn from_texts(assistant: &[String], tool: &[String]) -> Self {
        let mut evidence = Self::default();
        for text in assistant {
            evidence.add_assistant(text);
        }
        for text in tool {
            evidence.add_tool(text);
        }
        evidence
    }

    /// Adds what `rows` say. Only an `assistant` row makes mentions and only a `tool` row makes
    /// tool paths; every other role (a user message, a Mono system prompt) is ignored.
    pub fn absorb_rows<'a>(&mut self, rows: impl IntoIterator<Item = &'a Row>) {
        for row in rows {
            match row.role.as_str() {
                "assistant" if !row.text.is_empty() => self.add_assistant(&row.text),
                "tool" if !row.text.is_empty() => self.add_tool(&row.text),
                _ => {}
            }
        }
    }

    fn add_assistant(&mut self, text: &str) {
        for mention in extract_mentions(text) {
            if self.assistant.len() < MAX_ENTRIES && self.seen_assistant.insert(mention.clone()) {
                self.assistant.push(mention);
            }
        }
    }

    fn add_tool(&mut self, row_text: &str) {
        for token in tool_paths(row_text) {
            if self.tool.len() < MAX_ENTRIES && self.seen_tool.insert(token.clone()) {
                self.tool.push(token);
            }
        }
    }
}

/// The path-like tokens of one tool row: of its input (command lines, `file_path` and `path`
/// arguments, patches) and its output. A row that is not a tool call is read as plain text.
pub fn tool_paths(row_text: &str) -> Vec<String> {
    let row_text = cut(row_text, MAX_TOOL_TEXT);
    let mut whole: Vec<String> = Vec::new();
    let mut fragments: Vec<String> = Vec::new();
    match serde_json::from_str::<crate::tool::ToolCall>(row_text) {
        Ok(call) => {
            strings_of(&call.input, &mut whole, &mut fragments);
            if let Some(display) = &call.display {
                json_strings(display, 0, &mut whole, &mut fragments);
            }
            fragments.push(call.output);
        }
        Err(_) => fragments.push(row_text.to_owned()),
    }
    let mut out: Vec<String> = Vec::new();
    // A whole JSON string value that is a path may hold spaces (`/Users/me/My Docs/a.html`).
    for value in whole {
        if let Some(path) = argument_token(value.trim()) {
            if !out.contains(&path) {
                out.push(path);
            }
        }
    }
    for fragment in &fragments {
        for token in fragment.split(is_separator) {
            if let Some(path) = path_token(token) {
                if !out.contains(&path) {
                    out.push(path);
                }
            }
        }
    }
    out
}

fn cut(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// The strings of a JSON text (every string value is a candidate path, and a fragment to
/// tokenize), or the text itself when it is not JSON.
fn strings_of(text: &str, whole: &mut Vec<String>, fragments: &mut Vec<String>) {
    match serde_json::from_str::<Value>(text) {
        Ok(value) => json_strings(&value, 0, whole, fragments),
        Err(_) => fragments.push(text.to_owned()),
    }
}

fn json_strings(value: &Value, depth: usize, whole: &mut Vec<String>, fragments: &mut Vec<String>) {
    if depth > MAX_JSON_DEPTH || whole.len() + fragments.len() > MAX_JSON_STRINGS {
        return;
    }
    match value {
        Value::String(text) => {
            whole.push(text.clone());
            fragments.push(text.clone());
        }
        Value::Array(items) => {
            for item in items {
                json_strings(item, depth + 1, whole, fragments);
            }
        }
        Value::Object(map) => {
            for item in map.values() {
                json_strings(item, depth + 1, whole, fragments);
            }
        }
        _ => {}
    }
}

fn is_separator(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '"' | '\''
                | '`'
                | '='
                | '('
                | ')'
                | ','
                | ';'
                | '<'
                | '>'
                | '['
                | ']'
                | '{'
                | '}'
                | '|'
                | '&'
        )
}

/// A whole argument of a tool call (a JSON string value): any single-line text without a URL
/// scheme may be the file the tool was given (`Makefile`, `NOTES`, `/Users/me/My Docs/a.html`).
/// It is only ever compared with the file that was requested, so being loose costs nothing.
fn argument_token(value: &str) -> Option<String> {
    if value.is_empty() || value.contains("://") && !value.starts_with("file://") {
        return None;
    }
    mentions::clean_path(value).filter(|path| path.chars().count() <= MAX_TOKEN_CHARS)
}

/// A token that names a path: it starts like one (`/`, `~/`, `./`, `../`, `file://`), has a
/// separator, or ends in a file name with an extension. Trailing punctuation and `:12` are cut.
fn path_token(token: &str) -> Option<String> {
    let token = token.trim_start_matches(['*', '@', '+', '-']);
    if token.is_empty() || token.contains("://") && !token.starts_with("file://") {
        return None;
    }
    let cleaned = mentions::clean_path(token)?;
    let name = cleaned.rsplit('/').next().unwrap_or(&cleaned);
    let looks = mentions::is_known_name(name)
        || cleaned.starts_with('/')
        || cleaned.starts_with("~/")
        || cleaned.starts_with("./")
        || cleaned.starts_with("../")
        || cleaned.starts_with("file://")
        || cleaned.contains('/')
        || name.rsplit_once('.').is_some_and(|(stem, ext)| {
            !stem.is_empty() && !ext.is_empty() && ext.chars().all(|c| c.is_ascii_alphanumeric())
        });
    (looks && cleaned.chars().count() <= MAX_TOKEN_CHARS).then_some(cleaned)
}

struct Entry {
    conversation: String,
    /// Which state of the conversation the evidence covers (see the caller).
    key: (usize, usize),
    at: Instant,
    evidence: Evidence,
    /// Where the next older page starts, when the scan of the history is not finished.
    cursor: Option<i64>,
}

const CACHE_TTL: Duration = Duration::from_secs(30);
const CACHE_ENTRIES: usize = 16;

static CACHE: LazyLock<Mutex<Vec<Entry>>> = LazyLock::new(|| Mutex::new(Vec::new()));

fn cache() -> std::sync::MutexGuard<'static, Vec<Entry>> {
    CACHE.lock().unwrap_or_else(|e| e.into_inner())
}

/// The evidence kept for `conversation` at `key`, when it is younger than 30 seconds, and the
/// cursor of the older page still to scan.
pub fn cached(conversation: &str, key: (usize, usize)) -> Option<(Evidence, Option<i64>)> {
    cached_at(conversation, key, Instant::now())
}

fn cached_at(
    conversation: &str,
    key: (usize, usize),
    now: Instant,
) -> Option<(Evidence, Option<i64>)> {
    let mut cache = cache();
    cache.retain(|entry| now.saturating_duration_since(entry.at) < CACHE_TTL);
    cache
        .iter()
        .find(|entry| entry.conversation == conversation && entry.key == key)
        .map(|entry| (entry.evidence.clone(), entry.cursor))
}

/// Keeps the evidence of `conversation` at `key`, replacing an older entry of it.
pub fn remember(conversation: &str, key: (usize, usize), evidence: &Evidence, cursor: Option<i64>) {
    let mut cache = cache();
    cache.retain(|entry| entry.conversation != conversation);
    if cache.len() >= CACHE_ENTRIES {
        cache.remove(0);
    }
    cache.push(Entry {
        conversation: conversation.to_owned(),
        key,
        at: Instant::now(),
        evidence: evidence.clone(),
        cursor,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::ToolCall;

    fn row(role: &str, text: &str) -> Row {
        Row {
            role: role.into(),
            text: text.into(),
            first: true,
            sent_at: None,
        }
    }

    fn tool_row(name: &str, input: &str, output: &str) -> Row {
        let mut call = ToolCall::new("1", name);
        call.input = input.into();
        call.output = output.into();
        row("tool", &serde_json::to_string(&call).unwrap())
    }

    #[test]
    fn only_assistant_rows_make_mentions_and_only_tool_rows_make_tool_paths() {
        let evidence = Evidence::from_rows(&[
            row("user", "read `/etc/hosts`"),
            row("assistant", "ok `a.md`"),
            row("mono-system", "`b.md`"),
            tool_row("Write", r#"{"file_path":"/tmp/out/c.md"}"#, ""),
            row("assistant", "also /tmp/out/c.md"),
        ]);
        assert_eq!(evidence.assistant, ["a.md", "/tmp/out/c.md"]);
        assert_eq!(evidence.tool, ["/tmp/out/c.md"]);
    }

    #[test]
    fn tool_paths_come_from_arguments_command_lines_and_output() {
        let evidence = Evidence::from_rows(&[
            tool_row(
                "Write",
                r#"{"file_path":"/Users/me/My Docs/report.html","content":"x"}"#,
                "",
            ),
            tool_row(
                "Bash",
                r#"{"command":"cat docs/a.md > ./out/b.txt && open ~/x/c.html"}"#,
                "",
            ),
            tool_row(
                "Bash",
                "{}",
                "wrote /tmp/shot.png (12 KB)\nsee notes.md:12\n",
            ),
        ]);
        for expected in [
            "/Users/me/My Docs/report.html",
            "docs/a.md",
            "./out/b.txt",
            "~/x/c.html",
            "/tmp/shot.png",
            "notes.md",
            "Makefile",
            "LICENSE",
        ] {
            assert!(
                evidence.tool.iter().any(|t| t == expected),
                "{expected} missing from {:?}",
                evidence.tool
            );
        }
    }

    #[test]
    fn a_tool_row_that_is_not_json_is_read_as_text() {
        assert_eq!(tool_paths("created /tmp/x/y.md"), ["/tmp/x/y.md"]);
        assert!(tool_paths("nothing here").is_empty());
        assert!(tool_paths("see https://example.com/a.html now").is_empty());
    }

    #[test]
    fn the_cache_returns_the_same_state_only_and_forgets_after_the_ttl() {
        let evidence = Evidence::from_texts(&["`a.md`".to_owned()], &[]);
        let conversation = "cache-test-conversation";
        remember(conversation, (3, 10), &evidence, Some(7));
        let (found, cursor) = cached(conversation, (3, 10)).unwrap();
        assert_eq!(
            (found.assistant, cursor),
            (vec!["a.md".to_owned()], Some(7))
        );
        // The conversation moved on: not the same state.
        assert!(cached(conversation, (4, 10)).is_none());
        assert!(cached_at(conversation, (3, 10), Instant::now() + CACHE_TTL).is_none());
    }
}
