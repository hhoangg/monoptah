//! The `:emo[<emotion>]` token a Mono writes to ask for a picture, and the code-aware
//! helpers around it. Everything here is pure text work.

use super::{Emoji, Library, Pack};
use std::ops::Range;

const PREFIX: &str = ":emo[";
const MAX_NAME: usize = 64;

/// One token in a plain text run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    /// The whole token, `:emo[` through `]`.
    pub range: Range<usize>,
    /// The emotion name, trimmed. May be empty.
    pub name: String,
}

/// Find the tokens in plain text. The caller passes runs that are not code.
pub fn find(text: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut at = 0;
    while let Some(found) = text[at..].find(PREFIX) {
        let start = at + found;
        let body = start + PREFIX.len();
        let mut end = None;
        for (offset, ch) in text[body..].char_indices() {
            match ch {
                ']' => {
                    end = Some(body + offset);
                    break;
                }
                '[' | '\n' | '\r' => break,
                _ if offset >= MAX_NAME => break,
                _ => {}
            }
        }
        match end.filter(|end| end - body <= MAX_NAME) {
            Some(end) => {
                tokens.push(Token {
                    range: start..end + 1,
                    name: text[body..end].trim().to_owned(),
                });
                at = end + 1;
            }
            None => at = start + 1,
        }
    }
    tokens
}

/// Byte ranges of markdown code: fenced blocks (``` and ~~~) and inline code spans. With
/// `open_ended`, a backtick run with no closing run counts as code up to the end of its
/// paragraph, which is what a streaming view needs.
fn code_ranges(text: &str, open_ended: bool) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut fence: Option<(u8, usize, usize)> = None;
    let mut paragraph = 0;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        let trimmed = line.trim_start_matches(' ');
        let indent = line.len() - trimmed.len();
        let marker = trimmed.as_bytes().first().copied().unwrap_or(0);
        let width = trimmed.bytes().take_while(|b| *b == marker).count();
        let is_fence = indent <= 3 && matches!(marker, b'`' | b'~') && width >= 3;
        if let Some((opened, count, from)) = fence {
            if is_fence && marker == opened && width >= count && trimmed[width..].trim().is_empty()
            {
                ranges.push(from..offset);
                fence = None;
                paragraph = offset;
            }
        } else if is_fence {
            inline_spans(text, paragraph..start, open_ended, &mut ranges);
            fence = Some((marker, width, start));
        } else if line.trim().is_empty() {
            inline_spans(text, paragraph..start, open_ended, &mut ranges);
            paragraph = offset;
        }
    }
    match fence {
        Some((_, _, from)) => ranges.push(from..text.len()),
        None => inline_spans(text, paragraph..text.len(), open_ended, &mut ranges),
    }
    ranges
}

fn inline_spans(text: &str, span: Range<usize>, open_ended: bool, out: &mut Vec<Range<usize>>) {
    let bytes = text.as_bytes();
    let mut i = span.start;
    while i < span.end {
        match bytes[i] {
            b'\\' if bytes.get(i + 1) == Some(&b'`') => i += 2,
            b'`' => {
                let run = bytes[i..span.end]
                    .iter()
                    .take_while(|b| **b == b'`')
                    .count();
                let mut j = i + run;
                let mut closed = None;
                while j < span.end {
                    if bytes[j] == b'`' {
                        let width = bytes[j..span.end]
                            .iter()
                            .take_while(|b| **b == b'`')
                            .count();
                        if width == run {
                            closed = Some(j + width);
                            break;
                        }
                        j += width;
                    } else {
                        j += 1;
                    }
                }
                match closed {
                    Some(end) => {
                        out.push(i..end);
                        i = end;
                    }
                    None if open_ended => {
                        out.push(i..span.end);
                        return;
                    }
                    None => i += run,
                }
            }
            _ => i += 1,
        }
    }
}

fn in_code(ranges: &[Range<usize>], at: usize) -> bool {
    ranges.iter().any(|r| r.contains(&at))
}

/// Remove the tokens from raw markdown, leaving code untouched. A line that held only
/// tokens is removed with its line break; an inline token takes one neighbouring space
/// with it so no double space is left.
pub fn strip(markdown: &str) -> String {
    if !markdown.contains(PREFIX) {
        return markdown.to_owned();
    }
    let code = code_ranges(markdown, false);
    let tokens: Vec<Range<usize>> = find(markdown)
        .into_iter()
        .map(|t| t.range)
        .filter(|r| !in_code(&code, r.start))
        .collect();
    if tokens.is_empty() {
        return markdown.to_owned();
    }
    let mut out = String::with_capacity(markdown.len());
    let mut offset = 0;
    for line in markdown.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        let here: Vec<&Range<usize>> = tokens
            .iter()
            .filter(|r| r.start >= start && r.end <= offset)
            .collect();
        if here.is_empty() {
            out.push_str(line);
            continue;
        }
        let body = line.trim_end_matches(['\n', '\r']);
        let ending = &line[body.len()..];
        let mut kept = String::new();
        let mut at = 0;
        for range in here {
            let (from, to) = (range.start - start, range.end - start);
            kept.push_str(&body[at..from]);
            at = to;
            if kept.ends_with(' ') {
                if body[at..].starts_with(' ') {
                    at += 1;
                } else if at == body.len() {
                    kept.pop();
                }
            }
        }
        kept.push_str(&body[at..]);
        if !kept.trim().is_empty() {
            out.push_str(&kept);
            out.push_str(ending);
        }
    }
    out
}

/// For every token outside code, in source order: its name and whether it is alone on its
/// line (nothing but a quote or list marker before it, nothing after). The markdown parser
/// folds soft breaks into spaces, so a renderer asks this to tell a token on its own line
/// from one inside a sentence.
pub fn standalone(markdown: &str) -> Vec<(String, bool)> {
    if !markdown.contains(PREFIX) {
        return Vec::new();
    }
    let code = code_ranges(markdown, false);
    find(markdown)
        .into_iter()
        .filter(|t| !in_code(&code, t.range.start))
        .map(|token| {
            let line_start = markdown[..token.range.start]
                .rfind('\n')
                .map_or(0, |at| at + 1);
            let line_end = markdown[token.range.end..]
                .find('\n')
                .map_or(markdown.len(), |at| token.range.end + at);
            let before = markdown[line_start..token.range.start].trim_start();
            let after = &markdown[token.range.end..line_end];
            let alone = after.trim().is_empty() && block_prefix(before).trim().is_empty();
            (token.name, alone)
        })
        .collect()
}

/// What is left of a line start after quote markers and one list marker.
fn block_prefix(mut line: &str) -> &str {
    while let Some(rest) = line.strip_prefix('>') {
        line = rest.trim_start();
    }
    if let Some(rest) = line
        .strip_prefix("- ")
        .or_else(|| line.strip_prefix("* "))
        .or_else(|| line.strip_prefix("+ "))
    {
        return rest;
    }
    let digits = line.bytes().take_while(u8::is_ascii_digit).count();
    if digits > 0 {
        if let Some(rest) = line[digits..]
            .strip_prefix(". ")
            .or_else(|| line[digits..].strip_prefix(") "))
        {
            return rest;
        }
    }
    line
}

/// Where an unfinished token at the very end of `text` starts (`:`, `:em`, `:emo[ha`), so
/// a streaming view can hide it until it completes. None when the end is not such a prefix
/// or it sits in code.
pub fn incomplete_tail(text: &str) -> Option<usize> {
    let window = text.len().saturating_sub(PREFIX.len() + MAX_NAME + 1);
    let window = (window..=text.len()).find(|i| text.is_char_boundary(*i))?;
    let start = text[window..]
        .match_indices(':')
        .map(|(i, _)| window + i)
        .find(|&start| {
            let tail = &text[start..];
            match tail.strip_prefix(PREFIX) {
                Some(rest) => rest.len() <= MAX_NAME && !rest.contains([']', '[', '\n', '\r']),
                None => PREFIX.starts_with(tail),
            }
        })?;
    let code = code_ranges(text, true);
    (!in_code(&code, start)).then_some(start)
}

/// A stable seed for a token: the same session, message row and token index always give
/// the same value, on every run and platform (FNV-1a 64).
pub fn seed(session_id: &str, message_row: usize, index: usize) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut feed = |bytes: &[u8]| {
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    feed(session_id.as_bytes());
    feed(&[0xff]);
    feed(&(message_row as u64).to_le_bytes());
    feed(&(index as u64).to_le_bytes());
    hash
}

/// The emoji a token shows: one of the emotion's candidates, chosen by `seed`. None when
/// the feature is off, the name is empty or unknown, or the emotion has no picture.
pub fn pick<'a>(library: &'a Library, name: &str, seed: u64) -> Option<(&'a Pack, &'a Emoji)> {
    if !library.enabled || name.trim().is_empty() {
        return None;
    }
    let candidates = library.candidates(name);
    if candidates.is_empty() {
        return None;
    }
    candidates
        .get((seed % candidates.len() as u64) as usize)
        .copied()
}
