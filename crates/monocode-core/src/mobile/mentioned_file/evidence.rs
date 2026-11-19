//! What a conversation says about files, reduced to the path texts the rules compare.
//!
//! Two kinds of evidence, kept apart because they prove different things:
//!
//! * **Mentions**: the file mentions of the ASSISTANT's messages (see [`super::mentions`]). A
//!   user message never counts, a control device can type any path into one.
//! * **Tool paths**: the STRUCTURED path parameters of the TOOL rows of tools that operate on
//!   files that COMPLETED (`file_path`, `path`, `notebook_path`, `filename`, `output_path`, `paths[]`, the
//!   headers of a patch).
//! * **Loose tool paths**: path-like words in the COMPLETED calls of two families of tools only
//!   (an allow-list, [`ToolFamily`]). A SHELL tool (Bash, Codex `commandExecution`, ...): its
//!   command line and its output. A SUBAGENT tool (Task, Agent, Codex collab calls): its OUTPUT
//!   only (its prompt is text the parent agent wrote from what a device said). A report written
//!   by a script never goes through a file tool, but the command that wrote it, or the output
//!   that announced it, names it. Every other tool (todo lists, web fetch, MCP tools, a file
//!   tool's description or output, question tools) gives nothing: their text is typed by a
//!   device or comes from a third party. A call that did not complete never counts. Scanning is
//!   bounded per row ([`MAX_LOOSE_SCAN_BYTES`], [`MAX_ROW_ENTRIES`]) and per conversation
//!   ([`MAX_ENTRIES`]); when the conversation cap is reached the NEWER rows win (rows are read
//!   newest first, see [`Evidence::absorb_rows`]), so an old huge `find` cannot hide a later
//!   report.
//!
//! A file outside every project the device selected needs a mention and one of the tool kinds
//! (or the freshness rule, see [`super::serve`]). Residual: an agent prompted into running a
//! shell command that names a file (a control device can ask for it; in auto or bypass mode no
//! approval is asked), or whose output carries injected text (`cat` of an untrusted file), makes
//! that file readable; the deny-list and the protected folders are the last defence.
//!
//! The earliest plausible send time of the rows seen is kept too ([`Evidence::started_ms`]), the
//! fallback start of the conversation for the freshness rule.
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
#[cfg(not(test))]
const MAX_ENTRIES: usize = 100_000;
#[cfg(test)]
const MAX_ENTRIES: usize = 200;
/// Loose tokens kept from one tool row, a tenth of the conversation's, so one row cannot fill it.
const MAX_ROW_ENTRIES: usize = MAX_ENTRIES / 10;
/// Send times before this (2020-01-01 UTC, in milliseconds) are not real: they are ignored.
pub const MIN_PLAUSIBLE_MS: u64 = 1_577_836_800_000;
/// Longest token kept, in characters.
const MAX_TOKEN_CHARS: usize = 1024;
/// Bytes of one tool call's input, and of its output, searched for loose paths.
pub const MAX_LOOSE_SCAN_BYTES: usize = 256 * 1024;

/// The file mentions of assistant messages and the path tokens of tool rows.
#[derive(Clone, Debug, Default)]
pub struct Evidence {
    /// Mentions as written, in order of discovery, without duplicates.
    pub assistant: Vec<String>,
    /// Path-like tokens of the structured file parameters of tool rows, without duplicates.
    pub tool: Vec<String>,
    /// Path-like words of the input and output of completed tool calls, without duplicates.
    pub loose: Vec<String>,
    /// The earliest send time (Unix milliseconds) of any row absorbed: when the conversation
    /// began, as far as the history read tells.
    pub started_ms: Option<u64>,
    seen_assistant: HashSet<String>,
    seen_tool: HashSet<String>,
    seen_loose: HashSet<String>,
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
    ///
    /// A message the transcript cut into several rows (a continuation row has `first == false`)
    /// is read whole, so a table or a link is not torn apart at a row boundary. `rows` run oldest
    /// to newest; the messages are read newest first, and a caller with several pages reads the
    /// newest page first, so the entry cap drops the oldest evidence, not the newest.
    pub fn absorb_rows<'a>(&mut self, rows: impl IntoIterator<Item = &'a Row>) {
        let mut messages: Vec<(&str, String)> = Vec::new();
        for row in rows {
            if let Some(at) = row.sent_at {
                self.note_started(at);
            }
            match messages.last_mut() {
                Some((role, text)) if !row.first && *role == row.role => text.push_str(&row.text),
                _ => messages.push((row.role.as_str(), row.text.clone())),
            }
        }
        for (role, text) in messages.iter().rev() {
            self.add_message(role, text);
        }
    }

    fn add_message(&mut self, role: &str, text: &str) {
        if text.is_empty() {
            return;
        }
        match role {
            "assistant" => self.add_assistant(text),
            "tool" => self.add_tool(text),
            _ => {}
        }
    }

    /// Records a send time: the earliest one seen is when the conversation began.
    pub fn note_started(&mut self, sent_at_ms: u64) {
        if sent_at_ms < MIN_PLAUSIBLE_MS {
            return;
        }
        self.started_ms = Some(
            self.started_ms
                .map_or(sent_at_ms, |known| known.min(sent_at_ms)),
        );
    }

    fn add_assistant(&mut self, text: &str) {
        for mention in extract_mentions(text) {
            if self.assistant.len() < MAX_ENTRIES && self.seen_assistant.insert(mention.clone()) {
                self.assistant.push(mention);
            }
        }
    }

    fn add_tool(&mut self, row_text: &str) {
        let Some(call) = completed_call(row_text) else {
            return;
        };
        for token in tool_paths_of(&call) {
            if self.tool.len() < MAX_ENTRIES && self.seen_tool.insert(token.clone()) {
                self.tool.push(token);
            }
        }
        for token in loose_paths_of(&call) {
            if self.loose.len() < MAX_ENTRIES && self.seen_loose.insert(token.clone()) {
                self.loose.push(token);
            }
        }
    }

    /// Every tool token, structured first: what a tool row of the conversation names.
    pub fn tool_tokens(&self) -> impl Iterator<Item = &String> {
        self.tool.iter().chain(self.loose.iter())
    }
}

/// The tool call a row holds, when it COMPLETED. A call the user denied, that failed, was
/// cancelled or still runs did not touch anything, and a row that is not a tool call is no
/// evidence. The whole row is parsed: a cut would lose the `file_path` of a big Write that is
/// stored before its content.
fn completed_call(row_text: &str) -> Option<crate::tool::ToolCall> {
    let call = serde_json::from_str::<crate::tool::ToolCall>(row_text).ok()?;
    (call.state == crate::tool::ToolState::Completed).then_some(call)
}

/// The families of tools whose calls give loose evidence (an allow-list: any other tool, a
/// question tool included, gives none).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ToolFamily {
    /// Runs a command: the command line and the output are scanned.
    Shell,
    /// Runs a subagent: only the output is scanned.
    Subagent,
}

/// The family of the tool `name`, by the names the providers use: Claude (`Bash`, `Task`,
/// `Agent`), Codex (`commandExecution`, `exec_command`, `local_shell`, `collabAgentToolCall`),
/// OpenCode and editors (`bash`, `shell`, `run_terminal_cmd`, `execute`, `terminal`) and the
/// Mono import of a `kind: execute` call (`Bash`).
fn tool_family(name: &str) -> Option<ToolFamily> {
    const SHELL: &[&str] = &[
        "bash",
        "bashoutput",
        "shell",
        "local_shell",
        "shell_command",
        "exec_command",
        "commandexecution",
        "run_terminal_cmd",
        "run_shell_command",
        "execute",
        "execute_bash",
        "terminal",
    ];
    const SUBAGENT: &[&str] = &[
        "task",
        "agent",
        "subagent",
        "subagentactivity",
        "collabagenttoolcall",
        "spawn_agent",
        "delegate_task",
    ];
    let name = name.to_ascii_lowercase();
    if SHELL.contains(&name.as_str()) {
        Some(ToolFamily::Shell)
    } else if SUBAGENT.contains(&name.as_str()) {
        Some(ToolFamily::Subagent)
    } else {
        None
    }
}

/// Input fields that hold the command line of a shell tool.
const COMMAND_KEYS: &[&str] = &["command", "cmd", "script", "commands"];

/// The path-like words of a completed shell or subagent call (see the module docs). Any other
/// tool, a call that did not complete and a row that is not a tool call give nothing.
#[cfg(test)]
pub fn loose_paths(row_text: &str) -> Vec<String> {
    completed_call(row_text).map_or_else(Vec::new, |call| loose_paths_of(&call))
}

fn loose_paths_of(call: &crate::tool::ToolCall) -> Vec<String> {
    // ACP providers (Gemini, Cursor, ...) put a prose title in the name; the semantic kind of
    // the call says it runs a command.
    let acp_shell = call
        .display
        .as_ref()
        .and_then(|display| display["kind"].as_str())
        .is_some_and(|kind| matches!(kind, "execute" | "shell" | "bash"));
    let Some(family) = tool_family(&call.name).or(acp_shell.then_some(ToolFamily::Shell)) else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    if family == ToolFamily::Shell {
        let mut budget = MAX_LOOSE_SCAN_BYTES;
        match serde_json::from_str::<Value>(&call.input) {
            Ok(Value::Object(map)) => {
                for key in COMMAND_KEYS {
                    if let Some(value) = map.get(*key) {
                        scan_command(value, &mut budget, &mut out, &mut seen);
                    }
                }
            }
            // A bare command line, or a JSON string or argv array.
            Ok(value @ (Value::String(_) | Value::Array(_))) => {
                scan_command(&value, &mut budget, &mut out, &mut seen);
            }
            _ => scan_text(&call.input, &mut budget, &mut out, &mut seen),
        }
    }
    let mut budget = MAX_LOOSE_SCAN_BYTES;
    scan_text(&call.output, &mut budget, &mut out, &mut seen);
    out
}

/// A command given as a string or as an argv array of strings.
fn scan_command(
    value: &Value,
    budget: &mut usize,
    out: &mut Vec<String>,
    seen: &mut HashSet<String>,
) {
    match value {
        Value::String(text) => scan_text(text, budget, out, seen),
        Value::Array(items) => {
            for item in items {
                if let Value::String(text) = item {
                    scan_text(text, budget, out, seen);
                }
            }
        }
        _ => {}
    }
}

/// Characters that end a path-like word in a command line or an output.
fn is_separator(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '"' | '\''
                | '`'
                | '='
                | '<'
                | '>'
                | '('
                | ')'
                | ','
                | ';'
                | '|'
                | '&'
                | '['
                | ']'
                | '{'
                | '}'
        )
}

fn scan_text(text: &str, budget: &mut usize, out: &mut Vec<String>, seen: &mut HashSet<String>) {
    let mut end = text.len().min(*budget);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    *budget -= end;
    for word in text[..end].split(is_separator) {
        if out.len() >= MAX_ROW_ENTRIES {
            return;
        }
        if let Some(token) = loose_token(word) {
            if seen.insert(token.clone()) {
                out.push(token);
            }
        }
    }
}

/// A word that looks like a file path: a `/`, or a file extension, or a well-known bare name.
/// `$HOME/x` reads as `~/x`. Loose on purpose: a token is only ever compared with the file
/// that was requested.
fn loose_token(word: &str) -> Option<String> {
    let word = word.trim();
    if word.is_empty()
        || word.starts_with('-')
        || (word.contains("://") && !word.starts_with("file://"))
    {
        return None;
    }
    let word = ["${HOME}/", "$HOME/"]
        .iter()
        .find_map(|prefix| word.strip_prefix(prefix).map(|rest| format!("~/{rest}")))
        .unwrap_or_else(|| word.to_owned());
    let token = mentions::clean_path(&word)?;
    if token.chars().count() > MAX_TOKEN_CHARS {
        return None;
    }
    let name = token.rsplit('/').next().unwrap_or(&token);
    let has_extension = name.rsplit_once('.').is_some_and(|(stem, extension)| {
        !stem.is_empty()
            && extension.len() <= 12
            && extension.starts_with(|c: char| c.is_ascii_alphabetic())
            && extension.chars().all(|c| c.is_ascii_alphanumeric())
    });
    (token.contains('/') || has_extension || mentions::is_known_name(name)).then_some(token)
}

/// The file paths one tool row names through STRUCTURED parameters: the `file_path`, `path`,
/// `notebook_path`, `filename`, `output_path` and `paths[]` fields of the tool input, and the
/// `*** Add File:` / `*** Update File:` headers of a patch tool. Never free text (a shell
/// command line, a tool output, a question's answer) and never a question tool: a control
/// device can make the agent echo or be answered with any text, but it cannot choose which
/// parameter of a file tool the agent fills in. A row that is not a tool call gives nothing.
pub fn tool_paths(row_text: &str) -> Vec<String> {
    completed_call(row_text).map_or_else(Vec::new, |call| tool_paths_of(&call))
}

fn tool_paths_of(call: &crate::tool::ToolCall) -> Vec<String> {
    if !is_file_tool(&call.name) {
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
        // Messages are read newest first.
        assert_eq!(evidence.assistant, ["/tmp/out/c.md", "a.md"]);
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
    fn shell_command_lines_and_outputs_and_subagent_outputs_are_loose_evidence() {
        let evidence = Evidence::from_rows(&[
            tool_row(
                "Bash",
                r#"{"command":"echo ~/x/c.html && cat docs/a.md > $HOME/out/b.html"}"#,
                "",
            ),
            tool_row(
                "Bash",
                "{}",
                "wrote /tmp/shot.png (12 KB)\nsee notes.md:12\nupdated Makefile and LICENSE 1.5 v2\n",
            ),
            tool_row(
                "commandExecution",
                r#"{"command":"python3 gen.py > ~/r/codex.html","cwd":"/ignored/cwd.txt"}"#,
                "",
            ),
            tool_row("exec_command", r#"{"cmd":["sh","-c","cp a.md ~/r/argv.md"]}"#, ""),
            tool_row(
                "Task",
                r#"{"prompt":"look at ~/p/prompt.md"}"#,
                "The subagent wrote ~/s/answer.html.",
            ),
        ]);
        // Structured evidence is another thing: none of this is a file tool.
        assert!(evidence.tool.is_empty(), "{:?}", evidence.tool);
        for expected in [
            "~/x/c.html",
            "docs/a.md",
            "~/out/b.html",
            "/tmp/shot.png",
            "notes.md",
            "Makefile",
            "LICENSE",
            "~/r/codex.html",
            "~/r/argv.md",
            "~/s/answer.html",
        ] {
            assert!(
                evidence.loose.iter().any(|t| t == expected),
                "{expected} missing from {:?}",
                evidence.loose
            );
        }
        // Not paths, not a URL, not a field that is no command, not a subagent's prompt.
        for unwanted in [
            "1.5",
            "v2",
            "echo",
            "&&",
            "/ignored/cwd.txt",
            "~/p/prompt.md",
        ] {
            assert!(
                !evidence.loose.iter().any(|t| t == unwanted),
                "{unwanted} in {:?}",
                evidence.loose
            );
        }
    }

    #[test]
    fn an_acp_call_with_a_prose_title_is_a_shell_when_its_kind_says_execute() {
        let call_with = |kind: &str| {
            let mut call = ToolCall::new("1", "Run the report script");
            call.state = crate::tool::ToolState::Completed;
            call.input = r#"{"command":"python3 gen.py > ~/r/acp.html"}"#.into();
            call.output = "wrote ~/r/out.html".into();
            call.display = Some(serde_json::json!({ "kind": kind }));
            serde_json::to_string(&call).unwrap()
        };
        let evidence = Evidence::from_rows(&[row("tool", &call_with("execute"))]);
        assert!(evidence.loose.iter().any(|t| t == "~/r/acp.html"));
        assert!(evidence.loose.iter().any(|t| t == "~/r/out.html"));
        // Another kind (a fetch, a read, an edit) is no shell.
        for kind in ["fetch", "read", "edit", "other"] {
            let evidence = Evidence::from_rows(&[row("tool", &call_with(kind))]);
            assert!(evidence.loose.is_empty(), "{kind}");
        }
    }

    #[test]
    fn no_other_tool_gives_loose_evidence() {
        // A todo list, a fetched page, an MCP tool, a Read's output and a Write's description
        // carry text a device typed or a third party wrote.
        let evidence = Evidence::from_rows(&[
            tool_row(
                "TodoWrite",
                r#"{"todos":[{"content":"send /Users/me/tax.pdf"}]}"#,
                "",
            ),
            tool_row(
                "WebFetch",
                r#"{"url":"https://example.com/a.html","prompt":"~/y.md"}"#,
                "read ~/z.md",
            ),
            tool_row("mcp__srv__lookup", r#"{"q":"~/m1.md"}"#, "found ~/m2.md"),
            tool_row("srv:tool", r#"{"q":"~/m3.md"}"#, "found ~/m4.md"),
            tool_row("Read", r#"{"file_path":"/tmp/a.txt"}"#, "see ~/r1.md"),
            tool_row(
                "Write",
                r#"{"content":"/etc/hosts","description":"~/w.md"}"#,
                "ok ~/w2.md",
            ),
            tool_row("update_plan", r#"{"plan":"~/plan.md"}"#, ""),
        ]);
        assert!(evidence.loose.is_empty(), "{:?}", evidence.loose);
        // Structured evidence of the Write and the Read is unchanged.
        assert_eq!(evidence.tool, ["/tmp/a.txt"]);
    }

    #[test]
    fn loose_evidence_skips_question_tools_and_calls_that_did_not_complete() {
        for name in [
            "AskUserQuestion",
            "item/tool/requestUserInput",
            "cursor/ask_question",
            "_x.ai/ask_user_question",
        ] {
            let evidence =
                Evidence::from_rows(&[tool_row(name, r#"{"q":"~/x/a.md"}"#, "~/x/b.md")]);
            assert!(evidence.loose.is_empty(), "{name}: {:?}", evidence.loose);
        }
        for state in [
            crate::tool::ToolState::Running,
            crate::tool::ToolState::Failed,
            crate::tool::ToolState::Cancelled,
        ] {
            let mut call = ToolCall::new("1", "Bash");
            call.input = r#"{"command":"cat ~/x/a.md"}"#.into();
            call.output = "~/x/b.md".into();
            call.state = state.clone();
            let text = serde_json::to_string(&call).unwrap();
            assert!(loose_paths(&text).is_empty(), "{state:?}");
        }
        assert!(loose_paths("created /tmp/x/y.md").is_empty());
    }

    #[test]
    fn loose_scanning_is_bounded_per_row() {
        let filler = "x ".repeat(MAX_LOOSE_SCAN_BYTES);
        let output = format!("{filler}\n/tmp/late/report.html");
        let evidence = Evidence::from_rows(&[tool_row("Bash", "{}", &output)]);
        assert!(evidence.loose.is_empty(), "{:?}", evidence.loose);
        let early = format!("/tmp/early/report.html\n{filler}");
        let evidence = Evidence::from_rows(&[tool_row("Bash", "{}", &early)]);
        assert_eq!(evidence.loose, ["/tmp/early/report.html"]);
        // A command with thousands of distinct words keeps only the row's share.
        let many: String = (0..MAX_ENTRIES * 2)
            .map(|i| format!("/tmp/f/{i}.txt "))
            .collect();
        let evidence = Evidence::from_rows(&[tool_row("Bash", "{}", &many)]);
        assert_eq!(evidence.loose.len(), MAX_ROW_ENTRIES);
    }

    #[test]
    fn when_the_cap_is_reached_newer_rows_win_over_older_ones() {
        // An old `find` fills the cap; a later report must still be known.
        let mut rows: Vec<Row> = Vec::new();
        for chunk in 0..(MAX_ENTRIES / MAX_ROW_ENTRIES + 3) {
            let listing: String = (0..MAX_ROW_ENTRIES)
                .map(|i| format!("/old/{chunk}/{i}.txt\n"))
                .collect();
            rows.push(tool_row("Bash", r#"{"command":"find /old"}"#, &listing));
        }
        rows.push(tool_row(
            "Bash",
            r#"{"command":"python3 gen.py"}"#,
            "wrote /new/report.html",
        ));
        let evidence = Evidence::from_rows(&rows);
        assert!(evidence.loose.len() <= MAX_ENTRIES);
        assert!(evidence.loose.iter().any(|t| t == "/new/report.html"));
        // The oldest listing is what was dropped.
        assert!(!evidence.loose.iter().any(|t| t == "/old/0/0.txt"));
        // Older pages read later do not push the newer evidence out either.
        let mut paged = Evidence::from_rows(&rows[rows.len() - 1..]);
        paged.absorb_rows(&rows[..rows.len() - 1]);
        assert!(paged.loose.iter().any(|t| t == "/new/report.html"));
        assert!(paged.loose.len() <= MAX_ENTRIES);
    }

    #[test]
    fn the_earliest_plausible_send_time_and_continuation_rows_are_kept() {
        let mut first = row("user", "hi");
        first.sent_at = Some(1_700_000_005_000);
        let mut later = row("assistant", "see [a](");
        later.sent_at = Some(1_700_000_009_000);
        let mut bogus = row("user", "x");
        bogus.sent_at = Some(5);
        let mut rest = row("assistant", "casium/reports/a.html)");
        rest.first = false;
        let evidence = Evidence::from_rows(&[later, rest, bogus, first]);
        assert_eq!(evidence.started_ms, Some(1_700_000_005_000));
        // The link cut at a row boundary is one mention.
        assert_eq!(evidence.assistant, ["casium/reports/a.html"]);
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
