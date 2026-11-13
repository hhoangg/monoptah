//! Mono emoji in a Mono transcript. After markdown parsing, plain runs of an assistant
//! message are scanned for `:emo[<emotion>]`. Each token becomes an empty-text run that
//! carries the chosen picture, so selection, search and copy see nothing, or is removed when
//! no picture can be shown. See `docs/native/mono-emoji.md`, "Mono usage".

use crate::markdown::{MarkdownRow, Part};
use crate::markdown_model::{EmojiPick, InlineRun, InlineStyle};
use monocode_core::mono::emoji::{Library, image_path, token};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

const PREFIX: &str = ":emo[";

/// A Mono's emoji library as the transcript needs it.
#[derive(Clone)]
pub(crate) struct MonoEmoji {
    pub library: Arc<Library>,
    /// The Mono's emoji folder (`mono::emoji::dir`).
    pub dir: PathBuf,
    /// The seed key. `Conversation::set_mono_emoji` fills it with the session id, so a
    /// caller can leave it empty.
    pub session: String,
    /// Whether each picked file exists, found out once per library push so laying out a
    /// message does not touch the disk for every token.
    exists: Arc<Mutex<HashMap<PathBuf, bool>>>,
}

impl MonoEmoji {
    pub(crate) fn new(library: Arc<Library>, dir: PathBuf) -> Self {
        Self {
            library,
            dir,
            session: String::new(),
            exists: Arc::default(),
        }
    }

    fn file_exists(&self, path: &Path) -> bool {
        let Ok(mut known) = self.exists.lock() else {
            return path.is_file();
        };
        *known
            .entry(path.to_path_buf())
            .or_insert_with(|| path.is_file())
    }
}

/// The tokens of one assistant message while its blocks are turned into rows.
pub(super) struct MessageTokens<'a> {
    emoji: Option<&'a MonoEmoji>,
    /// The raw message source. The pick seed hashes the part before a token: unlike a view
    /// row or a store row, it is the same after a restart that loads another history
    /// window, and it does not change while the reply streams in after the token.
    text: &'a str,
    /// Start offset in `text` of every token outside code, in source order.
    starts: Vec<usize>,
    /// Name and "alone on its line" of every token outside code, in source order.
    lines: Vec<(String, bool)>,
    /// How many tokens earlier blocks of this message held. It is the seed index.
    pub cursor: usize,
    /// While the message streams: the index of a token that ends the shown text. Nothing
    /// says yet whether it will stay alone on its line, so it is drawn inline.
    open_tail: Option<usize>,
}

impl<'a> MessageTokens<'a> {
    pub fn new(emoji: Option<&'a MonoEmoji>, text: &'a str, streaming: bool) -> Self {
        let found = token::outside_code(text);
        let open_tail = (streaming && found.last().is_some_and(|t| t.range.end == text.len()))
            .then(|| found.len() - 1);
        Self {
            emoji,
            text,
            starts: found.iter().map(|t| t.range.start).collect(),
            lines: token::standalone(text),
            cursor: 0,
            open_tail,
        }
    }

    /// Whether the open tail token is one of the next `count` tokens.
    pub fn tail_within(&self, count: usize) -> bool {
        self.open_tail_in(self.cursor..self.cursor + count)
    }

    /// Whether the open tail token has an index in `range`.
    pub fn open_tail_in(&self, range: std::ops::Range<usize>) -> bool {
        self.open_tail.is_some_and(|tail| range.contains(&tail))
    }

    /// Turn the tokens of a block's rows into pictures, or remove them. A row left with
    /// nothing after a removal is dropped.
    pub fn apply(&mut self, rows: Vec<MarkdownRow>) -> Vec<MarkdownRow> {
        let mut out = Vec::with_capacity(rows.len());
        for mut row in rows {
            match &mut row.part {
                Part::Text { runs, heading } if has_token(runs) => {
                    let whole =
                        *heading == 0 && only_tokens(runs) && !self.tail_within(token_count(runs));
                    let converted = self.convert(runs, whole, *heading == 0);
                    if converted.is_empty() {
                        continue;
                    }
                    *runs = converted;
                }
                Part::Table { cells, .. } => {
                    for runs in cells.iter_mut().flatten() {
                        if has_token(runs) {
                            *runs = self.convert(runs, false, false);
                        }
                    }
                }
                _ => {}
            }
            out.push(row);
        }
        out
    }

    fn convert(&mut self, runs: &[InlineRun], whole: bool, allow_large: bool) -> Vec<InlineRun> {
        let mut out = Vec::with_capacity(runs.len() + 2);
        let mut at = 0;
        while at < runs.len() {
            let mut end = at + 1;
            if plain(&runs[at].style) {
                while end < runs.len() && runs[end].style == runs[at].style {
                    end += 1;
                }
            }
            let group = &runs[at..end];
            at = end;
            let text: String = group.iter().map(|r| r.text.as_str()).collect();
            let tokens = if plain(&group[0].style) {
                token::find(&text)
            } else {
                Vec::new()
            };
            if tokens.is_empty() {
                out.extend_from_slice(group);
                continue;
            }
            let style = &group[0].style;
            let mut buf = String::new();
            let mut cursor = 0;
            let mut trim_next = false;
            let flush = |buf: &mut String, out: &mut Vec<InlineRun>| {
                if !buf.is_empty() {
                    out.push(InlineRun {
                        text: std::mem::take(buf),
                        style: style.clone(),
                    });
                }
            };
            for found in tokens {
                let mut piece = &text[cursor..found.range.start];
                if trim_next {
                    piece = piece.trim_start();
                    trim_next = piece.is_empty();
                }
                buf.push_str(piece);
                cursor = found.range.end;
                let index = self.cursor;
                self.cursor += 1;
                let large = allow_large
                    && self.open_tail != Some(index)
                    && (whole
                        || self
                            .lines
                            .get(index)
                            .is_some_and(|(name, alone)| *alone && *name == found.name));
                match self.pick(&found.name, index, large) {
                    Some(pick) => {
                        if !large && buf.ends_with(' ') && text[cursor..].starts_with(' ') {
                            // The picture stands in for the gap, so selected text keeps
                            // one space instead of two.
                            cursor += 1;
                        }
                        if large {
                            buf.truncate(buf.trim_end().len());
                            trim_next = true;
                        }
                        flush(&mut buf, &mut out);
                        out.push(InlineRun {
                            text: String::new(),
                            style: InlineStyle {
                                emoji: Some(Arc::new(pick)),
                                ..InlineStyle::default()
                            },
                        });
                    }
                    None => {
                        // Take one neighbouring space along so no double space is left.
                        if buf.ends_with(' ') && text[cursor..].starts_with(' ') {
                            cursor += 1;
                        } else if buf.ends_with(' ') && cursor == text.len() {
                            buf.pop();
                        }
                    }
                }
            }
            let mut rest = &text[cursor..];
            if trim_next {
                rest = rest.trim_start();
            }
            buf.push_str(rest);
            flush(&mut buf, &mut out);
        }
        out
    }

    fn pick(&self, name: &str, index: usize, large: bool) -> Option<EmojiPick> {
        let emoji = self.emoji?;
        let start = self.starts.get(index).copied().unwrap_or(self.text.len());
        let seed = token::seed_for(&emoji.session, &self.text[..start], index);
        let (pack, picked) = token::pick(&emoji.library, name, seed)?;
        let path = image_path(&emoji.dir, pack, picked);
        // A file that is gone draws nothing and takes no room.
        if !emoji.file_exists(&path) {
            return None;
        }
        let emotion = emoji
            .library
            .emotions
            .iter()
            .find(|e| e.name.eq_ignore_ascii_case(name))
            .map_or_else(|| name.to_owned(), |e| e.name.clone());
        Some(EmojiPick {
            path,
            name: picked.name.clone(),
            emotion,
            format: picked.format.label(),
            large,
        })
    }
}

fn plain(style: &InlineStyle) -> bool {
    !style.code
        && !style.math
        && !style.pending_link
        && style.link.is_none()
        && style.task.is_none()
        && style.emoji.is_none()
}

fn has_token(runs: &[InlineRun]) -> bool {
    // The parser may cut `:emo[happy]` into several text events at the brackets.
    runs.chunk_by(|a, b| a.style == b.style)
        .filter(|group| plain(&group[0].style))
        .any(|group| {
            let text: String = group.iter().map(|r| r.text.as_str()).collect();
            text.contains(PREFIX)
        })
}

/// How many tokens the plain text of `runs` holds.
fn token_count(runs: &[InlineRun]) -> usize {
    runs.chunk_by(|a, b| a.style == b.style)
        .filter(|group| plain(&group[0].style))
        .map(|group| {
            let text: String = group.iter().map(|r| r.text.as_str()).collect();
            token::find(&text).len()
        })
        .sum()
}

/// Whether a paragraph holds tokens and nothing else but white space.
fn only_tokens(runs: &[InlineRun]) -> bool {
    let mut any = false;
    for group in runs.chunk_by(|a, b| a.style == b.style) {
        let text: String = group.iter().map(|r| r.text.as_str()).collect();
        if !plain(&group[0].style) {
            if !text.trim().is_empty() {
                return false;
            }
            continue;
        }
        let tokens = token::find(&text);
        any |= !tokens.is_empty();
        let mut at = 0;
        for found in tokens {
            if !text[at..found.range.start].trim().is_empty() {
                return false;
            }
            at = found.range.end;
        }
        if !text[at..].trim().is_empty() {
            return false;
        }
    }
    any
}

#[cfg(test)]
#[path = "emoji_tests.rs"]
mod tests;
