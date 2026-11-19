//! What a conversation says about files, reduced to the path texts the rules compare.
//!
//! Two kinds of evidence, kept apart because they prove different things:
//!
//! * **Mentions**: the file mentions of the ASSISTANT's messages (see [`super::mentions`]). A
//!   user message never counts, a control device can type any path into one.
//! * **Tool paths**: the STRUCTURED path parameters of the TOOL rows of tools that operate on
//!   files that COMPLETED (`file_path`, `path`, `notebook_path`, `filename`, `output_path`, `paths[]`, the
//!   headers of a patch). Outputs, free-text command lines and question tools never count: a
//!   control device can make a shell command echo a path or type one as the answer to a
//!   question. A file outside every project the device selected needs both kinds (see
//!   [`super::serve`]), so asking the assistant to "reply with `~/x`" does not make `~/x`
//!   readable. Residual: an agent prompted (or prompt-injected) into reading or writing a file
//!   with a file tool makes that file readable.
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

/// The file paths one tool row names through STRUCTURED parameters: the `file_path`, `path`,
/// `notebook_path`, `filename`, `output_path` and `paths[]` fields of the tool input, and the
/// `*** Add File:` / `*** Update File:` headers of a patch tool. Never free text (a shell
/// command line, a tool output, a question's answer) and never a question tool: a control
/// device can make the agent echo or be answered with any text, but it cannot choose which
/// parameter of a file tool the agent fills in. A row that is not a tool call gives nothing.
pub fn tool_paths(row_text: &str) -> Vec<String> {
    // The whole row is parsed: a cut would lose the `file_path` of a big Write that is stored
    // before its content.
    let Ok(call) = serde_json::from_str::<crate::tool::ToolCall>(row_text) else {
        return Vec::new();
    };
    // A call the user denied, that failed, was cancelled or still runs did not touch the file.
    if call.state != crate::tool::ToolState::Completed || !is_file_tool(&call.name) {
        return Vec::new();
    }
    let patch_tool = call.name.to_ascii_lowercase().contains("patch");
    let mut out: Vec<String> = Vec::new();
    let mut push = |value: &str| {
        if let Some(path) = argument_token(value.trim()) {
            if out.len() < MAX_ENTRIES && !out.contains(&path) {
                out.push(path);
            }
        }
    };
    match serde_json::from_str::<Value>(&call.input) {
        Ok(Value::Object(map)) => {
            for key in PATH_KEYS {
                if let Some(Value::String(value)) = map.get(*key) {
                    push(value);
                }
            }
            if let Some(Value::Array(items)) = map.get("paths") {
                for item in items.iter().take(MAX_PATHS) {
                    if let Value::String(value) = item {
                        push(value);
                    }
                }
            }
            if patch_tool {
                for value in map.values() {
                    if let Value::String(text) = value {
                        patch_headers(text, &mut push);
                    }
                }
            }
        }
        // A patch tool may take the patch itself as its whole input.
        _ if patch_tool => patch_headers(&call.input, &mut push),
        _ => {}
    }
    out
}

/// Input fields that hold one file path.
const PATH_KEYS: &[&str] = &[
    "file_path",
    "path",
    "notebook_path",
    "filename",
    "output_path",
];
/// Entries of a `paths` array read.
const MAX_PATHS: usize = 1_000;

/// The tools that read or change one file by path, by the names the providers use (Claude,
/// Codex, OpenCode, editor tools). An allow-list: a search tool (`Glob`, `Grep`, `LS`,
/// `list_dir`) takes a root folder, a question tool carries text the user typed, and an MCP
/// tool (`server:tool`) is anything; none of them proves a file was opened.
fn is_file_tool(name: &str) -> bool {
    const FILE_TOOLS: &[&str] = &[
        "read",
        "write",
        "edit",
        "multiedit",
        "notebookedit",
        "notebookread",
        "filechange",
        "apply_patch",
        "applypatch",
        "patch",
        "str_replace_editor",
        "str_replace_based_edit_tool",
        "read_file",
        "write_file",
        "edit_file",
        "create_file",
        "read_text_file",
        "write_text_file",
        "readfile",
        "writefile",
        "editfile",
        "view",
    ];
    FILE_TOOLS.contains(&name.to_ascii_lowercase().as_str())
}

/// The files a patch adds, updates or moves to.
fn patch_headers(text: &str, push: &mut impl FnMut(&str)) {
    for line in text.lines() {
        let line = line.trim();
        for header in ["*** Add File:", "*** Update File:", "*** Move to:"] {
            if let Some(path) = line.strip_prefix(header) {
                push(path);
            }
        }
    }
}

/// A whole parameter value of a tool call: any single-line text without a URL scheme may be the
/// file the tool was given (`Makefile`, `NOTES`, `/Users/me/My Docs/a.html`). It is only ever
/// compared with the file that was requested, so being loose costs nothing.
fn argument_token(value: &str) -> Option<String> {
    if value.is_empty() || value.contains("://") && !value.starts_with("file://") {
        return None;
    }
    mentions::clean_path(value).filter(|path| path.chars().count() <= MAX_TOKEN_CHARS)
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
        call.state = crate::tool::ToolState::Completed;
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
    fn tool_paths_come_only_from_structured_path_parameters() {
        let evidence = Evidence::from_rows(&[
            tool_row(
                "Write",
                r#"{"file_path":"/Users/me/My Docs/report.html","content":"x"}"#,
                "",
            ),
            tool_row("Read", r#"{"path":"docs/a.md"}"#, ""),
            tool_row("NotebookEdit", r#"{"notebook_path":"~/n.ipynb"}"#, ""),
            tool_row(
                "write_file",
                r#"{"filename":"Makefile","output_path":"./out/b.txt"}"#,
                "",
            ),
            tool_row(
                "fileChange",
                r#"{"path":"a/x.rs","paths":["a/x.rs","b/y.rs"]}"#,
                "",
            ),
        ]);
        for expected in [
            "/Users/me/My Docs/report.html",
            "docs/a.md",
            "~/n.ipynb",
            "Makefile",
            "./out/b.txt",
            "a/x.rs",
            "b/y.rs",
        ] {
            assert!(
                evidence.tool.iter().any(|t| t == expected),
                "{expected} missing from {:?}",
                evidence.tool
            );
        }
    }

    #[test]
    fn command_lines_outputs_and_other_free_text_do_not_count() {
        let evidence = Evidence::from_rows(&[
            tool_row(
                "Bash",
                r#"{"command":"echo ~/x/c.html && cat docs/a.md"}"#,
                "",
            ),
            tool_row(
                "Bash",
                "{}",
                "wrote /tmp/shot.png (12 KB)\nsee notes.md:12\nupdated Makefile and LICENSE\n",
            ),
            tool_row(
                "WebFetch",
                r#"{"url":"https://example.com/a.html","prompt":"~/y.md"}"#,
                "read ~/z.md",
            ),
            tool_row(
                "Write",
                r#"{"content":"/etc/hosts","description":"~/w.md"}"#,
                "",
            ),
        ]);
        assert!(evidence.tool.is_empty(), "{:?}", evidence.tool);
    }

    #[test]
    fn a_question_tool_never_counts_even_with_a_path_field() {
        for name in [
            "AskUserQuestion",
            "item/tool/requestUserInput",
            "cursor/ask_question",
            "_x.ai/ask_user_question",
        ] {
            let evidence = Evidence::from_rows(&[tool_row(
                name,
                r#"{"path":"~/x","questions":[{"question":"~/x"}]}"#,
                "~/x",
            )]);
            assert!(evidence.tool.is_empty(), "{name}: {:?}", evidence.tool);
        }
    }

    #[test]
    fn patch_headers_count_for_a_patch_tool_only() {
        let patch = "*** Begin Patch\n*** Add File: new/a.md\n+x\n*** Update File: src/b.rs\n@@\n*** End Patch";
        let evidence = Evidence::from_rows(&[
            tool_row(
                "apply_patch",
                &serde_json::json!({ "input": patch }).to_string(),
                "",
            ),
            tool_row("apply_patch", patch, ""),
        ]);
        assert_eq!(evidence.tool, ["new/a.md", "src/b.rs"]);
        // The same text in a shell command is only text.
        let evidence = Evidence::from_rows(&[tool_row(
            "Bash",
            &serde_json::json!({ "command": patch }).to_string(),
            patch,
        )]);
        assert!(evidence.tool.is_empty());
    }

    #[test]
    fn a_call_that_did_not_complete_gives_nothing() {
        for state in [
            crate::tool::ToolState::Running,
            crate::tool::ToolState::Failed,
            crate::tool::ToolState::Cancelled,
        ] {
            let mut call = ToolCall::new("1", "Read");
            call.input = r#"{"file_path":"/tmp/x/a.md"}"#.into();
            call.state = state.clone();
            let text = serde_json::to_string(&call).unwrap();
            assert!(tool_paths(&text).is_empty(), "{state:?}");
        }
        assert_eq!(
            tool_paths(&tool_row("Read", r#"{"file_path":"/tmp/x/a.md"}"#, "").text),
            ["/tmp/x/a.md"]
        );
    }

    #[test]
    fn only_file_tools_count_not_search_roots_or_mcp_tools() {
        for name in [
            "Glob",
            "Grep",
            "LS",
            "list_dir",
            "mcp__srv__read",
            "srv:read_file",
            "WebFetch",
            "Bash",
        ] {
            let row = tool_row(name, r#"{"path":"/tmp/x","file_path":"/tmp/x/a.md"}"#, "");
            assert!(tool_paths(&row.text).is_empty(), "{name}");
        }
        for name in [
            "Read",
            "Write",
            "Edit",
            "MultiEdit",
            "NotebookEdit",
            "fileChange",
            "apply_patch",
            "str_replace_editor",
            "read",
            "patch",
        ] {
            let row = tool_row(name, r#"{"file_path":"/tmp/x/a.md"}"#, "");
            assert_eq!(tool_paths(&row.text), ["/tmp/x/a.md"], "{name}");
        }
    }

    #[test]
    fn the_path_of_a_huge_write_is_not_lost() {
        let content = "x".repeat(5 * 1024 * 1024);
        let input = serde_json::json!({ "file_path": "/tmp/x/big.md", "content": content });
        let row = tool_row("Write", &input.to_string(), "");
        assert_eq!(tool_paths(&row.text), ["/tmp/x/big.md"]);
    }

    #[test]
    fn a_row_that_is_not_a_tool_call_gives_nothing() {
        assert!(tool_paths("created /tmp/x/y.md").is_empty());
        assert!(tool_paths("nothing here").is_empty());
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
