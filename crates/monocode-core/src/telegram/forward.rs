//! Which questions go to Telegram, when, and with what payload. Port of
//! `src/features/telegram/model/telegramForward.ts` over plain structs, so the model is
//! testable without GPUI. The summary of what the agent said is built here as well.
//!
//! Lengths are counted in Unicode scalar values where the TypeScript counts UTF-16 units;
//! the two only differ for text outside the Basic Multilingual Plane.
use super::render::{ForwardPayload, OptionIn, QuestionIn};
use crate::surface::Surface;
use regex::Regex;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

/// The wait before a question is sent when the project's setting is not known.
pub const FORWARD_DELAY_MS: u64 = 5 * 60_000;

const TOPIC_TITLE_MAX: usize = 128;
/// Hard cap on the summary, in characters. The summary aims lower, at `SUMMARY_TARGET`.
pub const CONTEXT_MAX: usize = 300;
const SUMMARY_TARGET: usize = 280;

/// One transcript block that can carry the agent's last words.
#[derive(Clone, Debug, Default)]
pub struct Block {
    pub role: String,
    pub text: String,
    pub internal: bool,
}

/// Where a workflow node's session came from. Empty until workflows exist natively.
#[derive(Clone, Debug, Default)]
pub struct Workflow {
    pub workflow_name: String,
    pub node_title: String,
    pub item_label: Option<String>,
    pub item_id: Option<String>,
    pub round: Option<u64>,
}

/// The question a session is waiting on.
#[derive(Clone, Debug)]
pub struct Pending {
    /// The provider's id for the question, handed back untouched when an answer is applied.
    pub id: String,
    pub request_id: u64,
    pub questions: Vec<QuestionIn>,
    /// The provider auto-skips it after a deadline, so nobody needs the ping.
    pub auto_resolve: bool,
}

impl Pending {
    /// The question the provider asked, as Telegram sees it. Native questions carry no
    /// per-question header, and Telegram option ids are the native option ids.
    pub fn from_question(question: &crate::agent::Question) -> Self {
        let keep = |text: &str| Some(text.to_string()).filter(|t| !t.is_empty());
        Self {
            id: question.id.clone(),
            request_id: request_id_of(&question.id),
            auto_resolve: question.auto_resolve_at.is_some(),
            questions: question
                .items
                .iter()
                .map(|item| QuestionIn {
                    id: item.id.clone(),
                    header: None,
                    prompt: item.prompt.clone(),
                    multi_select: item.multiple,
                    allow_custom: item.allow_custom,
                    options: item
                        .options
                        .iter()
                        .map(|option| OptionIn {
                            id: option.id.clone(),
                            label: option.label.clone(),
                            description: keep(&option.description),
                        })
                        .collect(),
                })
                .collect(),
        }
    }
}

/// The ticket id of a session started from an inbox item: the tracker's identifier, else
/// `#<number>` (App.tsx `onStartInboxItem`).
pub fn inbox_ticket(item: &crate::inbox::Item) -> String {
    let identifier = item.identifier.trim();
    if crate::inbox::is_tracker(&item.provider) && !identifier.is_empty() {
        identifier.to_string()
    } else {
        format!("#{}", item.number)
    }
}

/// Everything the forwarding model reads about one session.
#[derive(Clone, Debug, Default)]
pub struct SessionInfo {
    /// The persisted id (`Store::persisted_session_id`), which is the key of Telegram topics.
    pub id: String,
    pub cwd: String,
    /// The stored title, with the provider prefix.
    pub title: String,
    pub provider: String,
    pub surface: Surface,
    pub orchestration_worker: bool,
    pub inbox_ask: bool,
    pub ephemeral: bool,
    pub mono: bool,
    /// Ticket id remembered when the session was started from an inbox item.
    pub ticket: Option<String>,
    /// Identifier of the inbox card the session still shows.
    pub inbox_identifier: Option<String>,
    pub workflow: Option<Workflow>,
    /// Number of the linked GitHub issue or pull request.
    pub linked_number: Option<u64>,
    pub blocks: Vec<Block>,
    pub pending: Option<Pending>,
}

/// `"<session>#<request>"`: the key of one question everywhere in the forwarding state.
pub fn question_key(session_id: &str, request_id: u64) -> String {
    format!("{session_id}#{request_id}")
}

/// Telegram's numeric request id for a native question id: a stable hash that stays
/// below 2^53 and above zero (the same recipe as the shared store's handles).
pub fn request_id_of(question_id: &str) -> u64 {
    let bytes = Sha256::digest(question_id.as_bytes());
    (u64::from_le_bytes(bytes[..8].try_into().unwrap()) & ((1 << 53) - 1)).max(1)
}

/// The project's key in `telegram-config.json`, shared with the Tauri app.
pub fn project_key(cwd: &str) -> String {
    crate::files::path_key(cwd)
}

fn is_remote(cwd: &str) -> bool {
    cwd.replace('\\', "/")
        .starts_with(crate::remote::REMOTE_PATH_PREFIX)
}

/// Whether a session could ever post questions to Telegram, whatever it is asking now.
pub fn is_topic_linkable(session: &SessionInfo) -> bool {
    !(session.orchestration_worker
        || session.inbox_ask
        || session.ephemeral
        || session.surface.is_tui()
        || session.mono
        || is_remote(&session.cwd))
}

/// Sessions whose questions go to Telegram: ones a person would answer in a project tab.
pub fn is_forwardable(session: &SessionInfo) -> bool {
    // Questions that auto-skip never block the agent, so nobody needs the ping.
    is_topic_linkable(session) && !session.pending.as_ref().is_some_and(|p| p.auto_resolve)
}

fn nonblank(text: &Option<String>) -> Option<&str> {
    text.as_deref().map(str::trim).filter(|t| !t.is_empty())
}

pub fn session_ticket_id(session: &SessionInfo) -> Option<String> {
    if let Some(ticket) = nonblank(&session.ticket) {
        return Some(ticket.to_string());
    }
    if let Some(identifier) = nonblank(&session.inbox_identifier) {
        return Some(identifier.to_string());
    }
    if let Some(item) = session.workflow.as_ref().and_then(|w| nonblank(&w.item_id)) {
        return Some(item.to_string());
    }
    session.linked_number.map(|number| format!("#{number}"))
}

/// Inbox sessions are titled "<ticket> <title>" (native: "<ticket> · <title>"); the ticket is
/// shown on its own, so drop it here.
fn title(session: &SessionInfo, ticket: Option<&str>) -> String {
    let shown = crate::session_title::display_title(&session.title, &session.provider);
    if let Some(ticket) = ticket {
        for separator in [" \u{b7} ", " "] {
            if let Some(rest) = shown.strip_prefix(&format!("{ticket}{separator}")) {
                return rest.to_string();
            }
        }
    }
    shown.to_string()
}

pub fn topic_title(session: &SessionInfo) -> String {
    let ticket = session_ticket_id(session);
    let full = match &ticket {
        Some(ticket) => format!("[{ticket}] {}", title(session, Some(ticket))),
        None => title(session, None),
    };
    full.chars().take(TOPIC_TITLE_MAX).collect()
}

// ---------------------------------------------------------------------------------------
// The summary of the agent's last message.
// ---------------------------------------------------------------------------------------

const CUT_MARK: &str = "\u{2026} ";
const SENTENCE_END: &str = ".!?\u{2026}";
const CJK_SENTENCE_END: &str = "\u{3002}\u{ff01}\u{ff1f}";
const SENTENCE_CLOSERS: &str = "\"')]\u{201d}\u{2019}\u{300d}\u{ff09}";
/// Characters a JavaScript `.` does not match.
const LINE_TERMINATORS: &str = "\n\r\u{2028}\u{2029}";

/// One sentence, heading, table row or list item of the cleaned message.
struct Unit {
    text: String,
    list: bool,
}

/// Brackets of a code span placeholder: private-use characters, removed from the input.
const SPAN_OPEN: char = '\u{E000}';
const SPAN_CLOSE: char = '\u{E001}';

/// How much of the end of a message is looked at; longer messages are cut before parsing.
const SCAN_MAX: usize = 4000;
/// Longest tail of a message that is cleaned at all; the rest is discarded first.
const CLEAN_MAX: usize = 200_000;

struct Patterns {
    quote_prefix: Regex,
    fence_opener: Regex,
    table_rule: Regex,
    list_item: Regex,
    heading_start: Regex,
    heading: Regex,
    code_newline: Regex,
    placeholder: Regex,
    backticks: Regex,
    image: Regex,
    link: Regex,
    autolink: Regex,
    html: Regex,
    strike: Regex,
    star: Regex,
    spaces: Regex,
    not_alnum: Regex,
}

fn patterns() -> &'static Patterns {
    static PATTERNS: OnceLock<Patterns> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        let re = |pattern: &str| Regex::new(pattern).unwrap();
        let dot = format!("[^{LINE_TERMINATORS}]");
        Patterns {
            quote_prefix: re(r"^\s*(?:>\s?)+"),
            fence_opener: re(&format!("^(`{{3,}}|~{{3,}})({dot}*)$")),
            table_rule: re(r"^\|?[\s:|-]*-[\s:|-]*$"),
            list_item: re(&format!(r"^\s*(?:[-*+]|\d+[.)])\s+({dot}*)$")),
            heading_start: re(r"^#{1,6}\s+"),
            heading: re(&format!(r"^#{{1,6}}\s+({dot}*?)(?:\s+#+)?$")),
            code_newline: re(r"\s*\r?\n\s*"),
            placeholder: re(&format!("{SPAN_OPEN}(\\d+){SPAN_CLOSE}")),
            backticks: re("`+"),
            image: re(r"!\[[^\]]*\]\([^)]*\)"),
            link: re(r"\[([^\]]+)\]\([^)]*\)"),
            autolink: re(r"<(https?://[^>\s]+)>"),
            html: re(r"</?[A-Za-z][^<>]*>"),
            strike: re(&format!(r"~~(\S(?:{dot}*?\S)?)~~")),
            star: re(r"\*(\S(?:[^*]*\S)?)\*"),
            spaces: re(r"\s+"),
            not_alnum: re(r"[^\p{L}\p{N}]+"),
        }
    })
}

/// Replaces each inline code span (any run of backticks, closed by the next run of the
/// same length) with a placeholder and records its contents, so no later step touches
/// them. An unclosed run of backticks stays as plain text. Linear in the text length.
fn hide_code_spans(text: &str, spans: &mut Vec<String>) -> String {
    let p = patterns();
    let runs: Vec<(usize, usize)> = p
        .backticks
        .find_iter(text)
        .map(|m| (m.start(), m.len()))
        .collect();
    // For each run, the next run of the same length: the one that would close it.
    let mut closer = vec![usize::MAX; runs.len()];
    let mut next_of_size: HashMap<usize, usize> = HashMap::new();
    for k in (0..runs.len()).rev() {
        closer[k] = next_of_size.get(&runs[k].1).copied().unwrap_or(usize::MAX);
        next_of_size.insert(runs[k].1, k);
    }
    let mut out = String::new();
    let mut from = 0;
    let mut k = 0;
    while k < runs.len() {
        let close = closer[k];
        if close == usize::MAX {
            k += 1;
            continue;
        }
        let start = runs[k].0 + runs[k].1;
        let mut content = p
            .code_newline
            .replace_all(&text[start..runs[close].0], " ")
            .into_owned();
        // A single space on both sides only keeps backticks apart from the fence.
        if content.len() >= 2
            && content.starts_with(' ')
            && content.ends_with(' ')
            && !content.trim().is_empty()
        {
            content = content[1..content.len() - 1].to_string();
        }
        out.push_str(&text[from..runs[k].0]);
        out.push(SPAN_OPEN);
        out.push_str(&spans.len().to_string());
        out.push(SPAN_CLOSE);
        spans.push(content);
        from = runs[close].0 + runs[close].1;
        k = close + 1;
    }
    out.push_str(&text[from..]);
    out
}

fn show_code_spans(text: &str, spans: &[String]) -> String {
    patterns()
        .placeholder
        .replace_all(text, |caps: &regex::Captures| {
            caps[1]
                .parse::<usize>()
                .ok()
                .and_then(|index| spans.get(index))
                .cloned()
                .unwrap_or_default()
        })
        .into_owned()
}

fn is_term(c: char) -> bool {
    LINE_TERMINATORS.contains(c)
}

fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// The match of `(\*\*|__)(\S(?:.*?\S)?)\1` starting at `i`, as (group start, group end,
/// end of match). The optional part is tried first and grows lazily, like a backtracking
/// engine would.
fn strong_at(c: &[char], i: usize) -> Option<(usize, usize, usize)> {
    let delimiter = c[i];
    let group = i + 2;
    if group >= c.len() || c[group].is_whitespace() {
        return None;
    }
    let closes = |at: usize| at + 1 < c.len() && c[at] == delimiter && c[at + 1] == delimiter;
    let mut k = group + 1;
    while k < c.len() {
        if !c[k].is_whitespace() && closes(k + 1) {
            return Some((group, k + 1, k + 3));
        }
        if is_term(c[k]) {
            break;
        }
        k += 1;
    }
    closes(group + 1).then_some((group, group + 1, group + 3))
}

/// `(\*\*|__)(\S(?:.*?\S)?)\1` replaced by its group.
fn strip_strong(text: &str) -> String {
    let c: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < c.len() {
        let doubled = i + 1 < c.len() && (c[i] == '*' || c[i] == '_') && c[i + 1] == c[i];
        if let Some((a, b, end)) = doubled.then(|| strong_at(&c, i)).flatten() {
            out.extend(&c[a..b]);
            i = end;
            continue;
        }
        out.push(c[i]);
        i += 1;
    }
    out
}

/// The part of `_(\S(?:[^_]*\S)?)_(?!\w)` after the leading `(^|\W)`, with `at` on the
/// first underscore: (group start, group end, end of match).
fn underscore_at(c: &[char], at: usize) -> Option<(usize, usize, usize)> {
    if at >= c.len() || c[at] != '_' {
        return None;
    }
    let q = at + 1;
    if q >= c.len() || c[q].is_whitespace() {
        return None;
    }
    let closes =
        |u: usize| u < c.len() && c[u] == '_' && !c.get(u + 1).is_some_and(|n| is_word(*n));
    let mut run = 0;
    while q + 1 + run < c.len() && c[q + 1 + run] != '_' {
        run += 1;
    }
    for tail in (0..=run).rev() {
        let last = q + 1 + tail;
        if last < c.len() && !c[last].is_whitespace() && closes(last + 1) {
            return Some((q, last + 1, last + 2));
        }
    }
    closes(q + 1).then_some((q, q + 1, q + 2))
}

/// `(^|\W)_(\S(?:[^_]*\S)?)_(?!\w)` replaced by its two groups (`\w` is ASCII only).
fn strip_underscore(text: &str) -> String {
    let c: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut s = 0;
    while s < c.len() {
        let found = if s == 0 && c[0] == '_' {
            underscore_at(&c, 0).map(|m| (None, m))
        } else {
            None
        }
        .or_else(|| {
            (!is_word(c[s]))
                .then(|| underscore_at(&c, s + 1).map(|m| (Some(c[s]), m)))
                .flatten()
        });
        if let Some((lead, (a, b, end))) = found {
            out.extend(lead);
            out.extend(&c[a..b]);
            s = end;
        } else {
            out.push(c[s]);
            s += 1;
        }
    }
    out
}

/// Inline markdown and html reduced to the words they carry. Code spans are already
/// hidden as placeholders, so they pass through untouched.
fn strip_markup(text: &str) -> String {
    let p = patterns();
    let text = p.image.replace_all(text, "");
    let text = p.link.replace_all(&text, "$1");
    let text = p.autolink.replace_all(&text, "$1");
    let text = p.html.replace_all(&text, "");
    let text = strip_strong(&text);
    let text = p.strike.replace_all(&text, "$1");
    let text = p.star.replace_all(&text, "$1");
    let text = strip_underscore(&text);
    p.spaces.replace_all(&text, " ").trim().to_string()
}

fn split_sentences(text: &str) -> Vec<String> {
    let all: Vec<char> = text.chars().collect();
    let mut sentences = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < all.len() {
        let ch = all[i];
        let mut end = None;
        if CJK_SENTENCE_END.contains(ch) {
            end = Some(i + 1);
        } else if SENTENCE_END.contains(ch) {
            let mut j = i + 1;
            while j < all.len() && SENTENCE_END.contains(all[j]) {
                j += 1;
            }
            while j < all.len() && SENTENCE_CLOSERS.contains(all[j]) {
                j += 1;
            }
            if j >= all.len() || all[j].is_whitespace() {
                end = Some(j);
            }
        }
        let Some(end) = end else {
            i += 1;
            continue;
        };
        sentences.push(
            all[start..end]
                .iter()
                .collect::<String>()
                .trim()
                .to_string(),
        );
        start = end;
        i = end;
    }
    sentences.push(all[start..].iter().collect::<String>().trim().to_string());
    sentences.retain(|s| !s.is_empty());
    sentences
}

/// The lines of the message without fenced code (a blank line stands in for each block).
fn lines_without_code(raw: &str) -> Vec<String> {
    let p = patterns();
    let mut lines = Vec::new();
    let mut fence: Option<String> = None;
    let parts: Vec<&str> = raw.split('\n').collect();
    for (ix, source) in parts.iter().enumerate() {
        let source = if ix + 1 < parts.len() {
            source.strip_suffix('\r').unwrap_or(source)
        } else {
            source
        };
        let line = p.quote_prefix.replace(source, "").into_owned();
        let trimmed = line.trim();
        if let Some(open) = &fence {
            if trimmed.starts_with(open.as_str()) {
                fence = None;
            }
            continue;
        }
        if let Some(opener) = p.fence_opener.captures(trimmed) {
            // ```x``` on one line is inline code, not a fence.
            if !(opener[1].starts_with('`') && opener[2].contains('`')) {
                lines.push(String::new());
                fence = Some(opener[1].to_string());
                continue;
            }
        }
        lines.push(line);
    }
    lines
}

#[derive(Clone, Copy, PartialEq)]
enum LineKind {
    Break,
    Heading,
    Item,
    Table,
    Prose,
}

/// `^([-*_])(\s*\1){2,}$`
fn horizontal_rule(trimmed: &str) -> bool {
    let mut chars = trimmed.chars();
    let Some(mark) = chars.next().filter(|c| matches!(c, '-' | '*' | '_')) else {
        return false;
    };
    let rest: Vec<char> = chars.collect();
    rest.iter().all(|c| *c == mark || c.is_whitespace())
        && rest.last() == Some(&mark)
        && rest.iter().filter(|c| **c == mark).count() >= 2
}

fn is_break(trimmed: &str) -> bool {
    trimmed.is_empty() || horizontal_rule(trimmed) || patterns().table_rule.is_match(trimmed)
}

/// What a line is, by the same rules `message_units` reads it with.
fn line_kind(line: &str) -> LineKind {
    let p = patterns();
    let trimmed = line.trim();
    if is_break(trimmed) {
        LineKind::Break
    } else if p.heading_start.is_match(trimmed) {
        LineKind::Heading
    } else if p.list_item.is_match(line) {
        LineKind::Item
    } else if trimmed.starts_with('|') {
        LineKind::Table
    } else {
        LineKind::Prose
    }
}

/// Hides the code spans of each block: a paragraph, list item (with its indented
/// continuation lines), heading or table row. A span never crosses a block boundary, so
/// a stray backtick cannot pair with one further down. Done on the whole message, before
/// any window is cut, so a window can never start inside a span.
fn hide_spans_by_block(lines: &[String], spans: &mut Vec<String>) -> Vec<String> {
    fn flush(
        block: &mut Vec<&str>,
        open: &mut Option<LineKind>,
        out: &mut Vec<String>,
        spans: &mut Vec<String>,
    ) {
        if block.is_empty() {
            return;
        }
        let joined: String = block
            .join("\n")
            .chars()
            .filter(|c| *c != SPAN_OPEN && *c != SPAN_CLOSE)
            .collect();
        out.extend(
            hide_code_spans(&joined, spans)
                .split('\n')
                .map(str::to_string),
        );
        block.clear();
        *open = None;
    }
    let mut out = Vec::new();
    let mut block: Vec<&str> = Vec::new();
    let mut open: Option<LineKind> = None;
    for line in lines {
        let kind = line_kind(line);
        let indented = line.starts_with(char::is_whitespace);
        let continues = kind == LineKind::Prose
            && (open == Some(LineKind::Prose) || (open == Some(LineKind::Item) && indented));
        if !continues {
            flush(&mut block, &mut open, &mut out, spans);
        }
        if kind == LineKind::Break {
            out.push(line.clone());
            continue;
        }
        block.push(line);
        if !continues {
            open = Some(kind);
        }
        if matches!(kind, LineKind::Heading | LineKind::Table) {
            flush(&mut block, &mut open, &mut out, spans);
        }
    }
    flush(&mut block, &mut open, &mut out, spans);
    out
}

/// The last `n` characters of `text`.
fn tail_chars(text: &str, n: usize) -> &str {
    match text.char_indices().rev().nth(n.saturating_sub(1)) {
        Some((at, _)) if text.chars().count() > n => &text[at..],
        _ => text,
    }
}

/// The last lines that fit `CLEAN_MAX` characters; a single longer line is cut at a space.
fn last_chars(lines: Vec<String>) -> (Vec<String>, bool) {
    let mut kept: Vec<String> = Vec::new();
    let mut room = CLEAN_MAX;
    for line in lines.into_iter().rev() {
        let size = line.chars().count();
        if size < room {
            room -= size + 1;
            kept.push(line);
            continue;
        }
        if kept.is_empty() {
            let tail = tail_chars(&line, CLEAN_MAX - 1);
            let tail = tail.find(' ').map_or(tail, |space| &tail[space + 1..]);
            return (vec![tail.to_string()], true);
        }
        kept.reverse();
        return (kept, true);
    }
    kept.reverse();
    (kept, false)
}

/// The end of the (span-hiding) lines that fits `SCAN_MAX` characters, so the cost of
/// cleaning a huge message is bounded. It starts at a line boundary, or after the first
/// blank line of the window when there is one; only a last line that is too long by
/// itself is cut, at a word boundary (a placeholder holds no space, so none is split).
/// The flag tells that something before was left out.
fn last_lines(lines: Vec<String>) -> (Vec<String>, bool) {
    let mut room = SCAN_MAX;
    let mut first = lines.len();
    while first > 0 {
        let size = lines[first - 1].chars().count();
        if size >= room {
            break;
        }
        room -= size + 1;
        first -= 1;
    }
    if first == 0 {
        return (lines, false);
    }
    let kept = &lines[first..];
    if kept.is_empty() {
        let tail = tail_chars(&lines[first - 1], SCAN_MAX - 1);
        let tail = match tail.find(' ') {
            Some(space) => &tail[space + 1..],
            None => {
                // No word boundary: at least do not start in the middle of a placeholder.
                match (tail.find(SPAN_CLOSE), tail.find(SPAN_OPEN)) {
                    (Some(close), open) if open.is_none_or(|open| close < open) => {
                        &tail[close + SPAN_CLOSE.len_utf8()..]
                    }
                    _ => tail,
                }
            }
        };
        let lines = if tail.trim().is_empty() {
            Vec::new()
        } else {
            vec![tail.to_string()]
        };
        return (lines, true);
    }
    let from_paragraph = match kept.iter().position(|line| line.trim().is_empty()) {
        Some(blank) => &kept[blank + 1..],
        None => kept,
    };
    let chosen = if from_paragraph.iter().any(|line| !line.trim().is_empty()) {
        from_paragraph
    } else {
        kept
    };
    (chosen.to_vec(), true)
}

fn flush_prose(prose: &mut Vec<String>, units: &mut Vec<Unit>, spans: &[String]) {
    // Sentences are split with code spans still hidden, so a full stop inside one never
    // ends a sentence; the text is restored before anything is measured.
    for sentence in split_sentences(&strip_markup(&prose.join(" "))) {
        let text = show_code_spans(&sentence, spans);
        if !text.is_empty() {
            units.push(Unit { text, list: false });
        }
    }
    prose.clear();
}

/// The message as units, with code, rules and markup noise left out.
fn message_units(raw: &str) -> (Vec<Unit>, bool) {
    let p = patterns();
    let mut spans = Vec::new();
    let (bounded, bounded_cut) = last_chars(lines_without_code(raw));
    let (scanned, scanned_cut) = last_lines(hide_spans_by_block(&bounded, &mut spans));
    let strip_inline = |text: &str, spans: &[String]| show_code_spans(&strip_markup(text), spans);
    let mut units: Vec<Unit> = Vec::new();
    let mut prose: Vec<String> = Vec::new();
    let mut last_item: Option<usize> = None;
    for line in &scanned {
        let trimmed = line.trim();
        if is_break(trimmed) {
            flush_prose(&mut prose, &mut units, &spans);
            last_item = None;
            continue;
        }
        if let Some(heading) = p.heading.captures(trimmed) {
            flush_prose(&mut prose, &mut units, &spans);
            last_item = None;
            let text = strip_inline(&heading[1], &spans);
            if !text.is_empty() {
                units.push(Unit { text, list: false });
            }
            continue;
        }
        if let Some(item) = p.list_item.captures(line) {
            flush_prose(&mut prose, &mut units, &spans);
            let body = strip_inline(&item[1], &spans);
            last_item = if body.is_empty() {
                None
            } else {
                units.push(Unit {
                    text: format!("- {body}"),
                    list: true,
                });
                Some(units.len() - 1)
            };
            continue;
        }
        if let Some(ix) = last_item.filter(|_| line.starts_with(char::is_whitespace)) {
            let more = strip_inline(trimmed, &spans);
            units[ix].text.push(' ');
            units[ix].text.push_str(&more);
            continue;
        }
        last_item = None;
        if trimmed.starts_with('|') {
            flush_prose(&mut prose, &mut units, &spans);
            let row = trimmed.strip_prefix('|').unwrap_or(trimmed);
            let row = row.strip_suffix('|').unwrap_or(row);
            let cells: Vec<String> = row
                .split('|')
                .map(|cell| strip_inline(cell, &spans))
                .filter(|cell| !cell.is_empty())
                .collect();
            let text = cells.join(", ");
            if !text.is_empty() {
                units.push(Unit { text, list: false });
            }
            continue;
        }
        prose.push(trimmed.to_string());
    }
    flush_prose(&mut prose, &mut units, &spans);
    (units, scanned_cut || bounded_cut)
}

fn normalized(text: &str) -> String {
    patterns()
        .not_alnum
        .replace_all(&text.to_lowercase(), "")
        .into_owned()
}

fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// A unit that says no more than the question prompt itself.
fn repeats_prompt(unit: &Unit, prompts: &[&str]) -> bool {
    let said = normalized(&unit.text);
    if utf16_len(&said) < 6 {
        return false;
    }
    prompts.iter().any(|prompt| {
        let asked = normalized(prompt);
        if utf16_len(&asked) < 6 {
            return false;
        }
        asked.contains(&said)
            || (said.contains(&asked) && utf16_len(&said) <= utf16_len(&asked) + 10)
    })
}

fn joiner_of(before: &Unit, after: &Unit) -> &'static str {
    if before.list || after.list {
        "\n"
    } else if before
        .text
        .chars()
        .last()
        .is_some_and(|c| CJK_SENTENCE_END.contains(c))
    {
        ""
    } else {
        " "
    }
}

fn join_units(units: &[&Unit]) -> String {
    let mut text = String::new();
    for (index, unit) in units.iter().enumerate() {
        if index > 0 {
            text.push_str(joiner_of(units[index - 1], unit));
        }
        text.push_str(&unit.text);
    }
    text
}

/// The end of one long unit, from a word boundary, behind the cut marker.
fn clip_tail(text: &str, budget: usize) -> String {
    let room = budget - CUT_MARK.chars().count();
    let words: Vec<&str> = text.split(' ').collect();
    let mut kept: Vec<&str> = Vec::new();
    let mut used = 0;
    for word in words.iter().rev() {
        let cost = word.chars().count() + usize::from(!kept.is_empty());
        if used + cost > room {
            break;
        }
        kept.push(word);
        used += cost;
    }
    kept.reverse();
    // A single unbroken run longer than the room has no word boundary to cut at.
    if kept.is_empty() {
        kept.push(tail_chars(words[words.len() - 1], room));
    }
    format!("{CUT_MARK}{}", kept.join(" "))
}

/// A short summary of what the agent said just before asking: the whole sentences closest
/// to the question, up to about `SUMMARY_TARGET` characters, as plain text. Deterministic
/// and local (no model call). It starts with an ellipsis when earlier text was left out.
pub fn question_context(session: &SessionInfo) -> String {
    for block in session.blocks.iter().rev() {
        if block.role != "assistant" || block.internal || block.text.trim().is_empty() {
            continue;
        }
        // A message that is only code has no summary; do not fall back to an older one.
        let (mut units, cut) = message_units(block.text.trim_end());
        if units.is_empty() {
            return String::new();
        }
        // Sentences at the end that only repeat the question add nothing; drop them when
        // something else was said.
        let prompts: Vec<&str> = session
            .pending
            .iter()
            .flat_map(|p| p.questions.iter().map(|q| q.prompt.as_str()))
            .collect();
        let mut repeats = 0;
        while repeats < units.len() && repeats_prompt(&units[units.len() - 1 - repeats], &prompts) {
            repeats += 1;
        }
        if repeats < units.len() {
            units.truncate(units.len() - repeats);
        }
        let mut picked: Vec<&Unit> = Vec::new();
        let mut used = 0;
        let mut index = units.len();
        while index > 0 {
            let unit = &units[index - 1];
            let cost = unit.text.chars().count()
                + picked
                    .first()
                    .map_or(0, |next| joiner_of(unit, next).chars().count());
            if used + cost > SUMMARY_TARGET {
                if picked.is_empty() {
                    return clip_tail(&unit.text, SUMMARY_TARGET);
                }
                break;
            }
            picked.insert(0, unit);
            used += cost;
            index -= 1;
        }
        let mark = if index > 0 || cut { CUT_MARK } else { "" };
        return format!("{mark}{}", join_units(&picked));
    }
    String::new()
}

// ---------------------------------------------------------------------------------------
// Header, payload and timing.
// ---------------------------------------------------------------------------------------

/// The project's configured wait in milliseconds; 5 minutes while its status is unknown.
pub fn forward_delay_ms(delay_secs: Option<u64>) -> u64 {
    delay_secs.map_or(FORWARD_DELAY_MS, |secs| secs.saturating_mul(1000))
}

/// The folder name shown for a project; `~` for the home folder.
pub fn project_name(cwd: &str) -> String {
    let windows = {
        let b = cwd.as_bytes();
        (b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'\\' | b'/'))
            || cwd.starts_with("\\\\")
            || cwd.starts_with("//")
    };
    let slashed = if windows {
        cwd.replace('\\', "/")
    } else {
        cwd.to_string()
    };
    let trimmed = match slashed.trim_end_matches('/') {
        "" if slashed.is_empty() => return "~".into(),
        "" => "/",
        t => t,
    };
    let parts: Vec<&str> = trimmed.split('/').filter(|p| !p.is_empty()).collect();
    let drive = |part: &str| {
        part.len() == 2 && part.as_bytes()[0].is_ascii_alphabetic() && part.ends_with(':')
    };
    let home = trimmed == "~"
        || (parts.len() == 2 && matches!(parts[0], "Users" | "home"))
        || (parts.len() == 3 && drive(parts[0]) && parts[1] == "Users");
    if home {
        return "~".into();
    }
    if drive(trimmed) {
        return trimmed.to_string();
    }
    parts.last().map_or(trimmed, |last| last).to_string()
}

pub fn header_of(session: &SessionInfo) -> String {
    let ticket = session_ticket_id(session);
    let first = [
        project_name(&session.cwd),
        title(session, ticket.as_deref()),
        ticket.clone().unwrap_or_default(),
    ]
    .into_iter()
    .filter(|part| !part.is_empty())
    .collect::<Vec<_>>()
    .join(" \u{b7} ");
    let Some(workflow) = &session.workflow else {
        return first;
    };
    let second = [
        Some(format!("Workflow {}", workflow.workflow_name)),
        Some(workflow.node_title.clone()),
        workflow.item_label.clone(),
        workflow.round.map(|round| format!("round {round}")),
    ]
    .into_iter()
    .flatten()
    .filter(|part| !part.is_empty())
    .collect::<Vec<_>>()
    .join(" \u{b7} ");
    format!("{first}\n{second}")
}

pub fn forward_payload(
    session: &SessionInfo,
    project: &str,
    delay_secs: u64,
) -> Option<ForwardPayload> {
    let pending = session.pending.as_ref()?;
    let keep = |text: &Option<String>| text.clone().filter(|t| !t.is_empty());
    Some(ForwardPayload {
        project: project.to_string(),
        session_id: session.id.clone(),
        request_id: pending.request_id,
        topic_title: topic_title(session),
        header: header_of(session),
        context: question_context(session),
        delay_secs,
        questions: pending
            .questions
            .iter()
            .map(|question| QuestionIn {
                id: question.id.clone(),
                header: keep(&question.header),
                prompt: question.prompt.clone(),
                multi_select: question.multi_select,
                allow_custom: question.allow_custom,
                options: question
                    .options
                    .iter()
                    .map(|option| OptionIn {
                        id: option.id.clone(),
                        label: option.label.clone(),
                        description: keep(&option.description),
                    })
                    .collect(),
            })
            .collect(),
    })
}

/// Sessions whose current question has waited as long as their project's delay and was
/// not sent yet. A question that was never seen (it appeared while forwarding was off) is
/// never due, whatever the delay.
pub fn due_forwards<'a>(
    sessions: impl IntoIterator<Item = &'a SessionInfo>,
    first_seen: &HashMap<String, u64>,
    sent: &HashSet<String>,
    now: u64,
    delay_ms_for: impl Fn(&SessionInfo) -> u64,
) -> Vec<&'a SessionInfo> {
    sessions
        .into_iter()
        .filter(|session| {
            let Some(pending) = &session.pending else {
                return false;
            };
            if !is_forwardable(session) {
                return false;
            }
            let key = question_key(&session.id, pending.request_id);
            if sent.contains(&key) {
                return false;
            }
            first_seen
                .get(&key)
                .is_some_and(|seen| now.saturating_sub(*seen) >= delay_ms_for(session))
        })
        .collect()
}

#[cfg(test)]
mod tests;
