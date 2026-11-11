//! Undo and redo for plain (non-code) text fields such as the composer.
//!
//! The code editor keeps its own whole-buffer history in `code.rs`. This one
//! groups typing the way native text fields do: consecutive character inserts
//! (or consecutive deletes) at the caret are one step, and a pause, a caret
//! jump, a change of kind or the start of a new word ends the group. Pastes,
//! cuts, selection replacements and programmatic changes are always a step of
//! their own. Each step restores the text together with its selection.
use gpui::SharedString;
use std::ops::Range;
use std::time::{Duration, Instant};

/// Most steps kept; older ones are dropped first.
const MAX_STEPS: usize = 200;
/// Upper bound on the bytes the snapshots of one field may hold.
const MAX_BYTES: usize = 4 * 1024 * 1024;
/// A pause this long between keystrokes starts a new undo step.
const PAUSE: Duration = Duration::from_millis(1000);

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Snapshot {
    pub content: SharedString,
    pub selection: Range<usize>,
    pub reversed: bool,
}

/// What a single edit did, which decides whether it extends the open group.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum EditKind {
    /// Characters typed at a collapsed caret.
    Insert { whitespace: bool },
    /// One grapheme removed by Backspace or Delete at a collapsed caret.
    Delete,
    /// Anything else: paste, cut, deleting a selection, recall, completion.
    Other,
}

#[derive(Clone, Copy, Debug)]
struct Group {
    kind: EditKind,
    /// Where the caret was left, which the next edit must start from.
    caret: usize,
    at: Instant,
}

#[derive(Default)]
pub(crate) struct EditHistory {
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    group: Option<Group>,
}

impl EditHistory {
    /// Classify an edit replacing `range` with `text`.
    /// `char_delete` is set by Backspace and Delete at a collapsed caret.
    pub(crate) fn kind(
        range: &Range<usize>,
        text: &str,
        forced: bool,
        char_delete: bool,
    ) -> EditKind {
        if forced {
            EditKind::Other
        } else if range.is_empty() && !text.is_empty() && text.chars().count() == 1 {
            EditKind::Insert {
                whitespace: text.chars().all(char::is_whitespace),
            }
        } else if char_delete && text.is_empty() {
            EditKind::Delete
        } else {
            EditKind::Other
        }
    }

    /// Call before applying an edit of `range`, with the state as it is now.
    /// `caret_after` is where the caret lands once the edit is applied.
    pub(crate) fn record(
        &mut self,
        before: Snapshot,
        range: &Range<usize>,
        kind: EditKind,
        caret_after: usize,
        now: Instant,
    ) {
        // `kind` already tells a selection replacement (Other) from typing.
        let continues = self.group.is_some_and(|group| {
            let adjacent = match kind {
                EditKind::Insert { .. } => range.start == group.caret,
                // Backspace ends where the last edit left the caret, and
                // forward delete starts there.
                EditKind::Delete => range.end == group.caret || range.start == group.caret,
                EditKind::Other => false,
            };
            let same_kind = match (group.kind, kind) {
                // "hello " is one step and "world" the next.
                (EditKind::Insert { whitespace: was }, EditKind::Insert { whitespace: is }) => {
                    !(was && !is)
                }
                (EditKind::Delete, EditKind::Delete) => true,
                _ => false,
            };
            adjacent && same_kind && now.duration_since(group.at) < PAUSE
        });
        if !continues {
            self.undo.push(before);
            self.trim();
        }
        self.redo.clear();
        self.group = (kind != EditKind::Other).then_some(Group {
            kind,
            caret: caret_after,
            at: now,
        });
    }

    pub(crate) fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.group = None;
    }

    /// A composition that ended leaving `current` as the text it started
    /// from (cancelled IME) drops the step recorded when it began.
    pub(crate) fn discard_noop(&mut self, current: &SharedString) {
        if self.undo.last().is_some_and(|top| top.content == *current) {
            self.undo.pop();
            self.group = None;
        }
    }

    pub(crate) fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub(crate) fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Returns the state to restore, keeping `current` for redo.
    pub(crate) fn undo(&mut self, current: Snapshot) -> Option<Snapshot> {
        let previous = self.undo.pop()?;
        self.redo.push(current);
        self.group = None;
        Some(previous)
    }

    /// Returns the state to restore, keeping `current` for undo.
    pub(crate) fn redo(&mut self, current: Snapshot) -> Option<Snapshot> {
        let next = self.redo.pop()?;
        self.undo.push(current);
        self.group = None;
        Some(next)
    }

    fn trim(&mut self) {
        let mut bytes = 0;
        let mut keep = 0;
        for snapshot in self.undo.iter().rev().take(MAX_STEPS) {
            bytes += snapshot.content.len();
            if bytes > MAX_BYTES {
                break;
            }
            keep += 1;
        }
        let discard = self.undo.len().saturating_sub(keep.max(1));
        self.undo.drain(..discard);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(text: &str, selection: Range<usize>) -> Snapshot {
        Snapshot {
            content: text.to_owned().into(),
            selection,
            reversed: false,
        }
    }

    /// Types `text` one char at a time from the end of `start`.
    fn type_into(
        history: &mut EditHistory,
        start: &str,
        text: &str,
        now: Instant,
        step: Duration,
    ) -> String {
        let mut content = start.to_owned();
        let mut now = now;
        for ch in text.chars() {
            let at = content.len();
            let s = ch.to_string();
            let kind = EditHistory::kind(&(at..at), &s, false, false);
            history.record(snap(&content, at..at), &(at..at), kind, at + s.len(), now);
            content.push(ch);
            now += step;
        }
        content
    }

    #[test]
    fn consecutive_typing_coalesces_by_word() {
        let mut history = EditHistory::default();
        let now = Instant::now();
        let content = type_into(
            &mut history,
            "",
            "hello world",
            now,
            Duration::from_millis(50),
        );
        assert_eq!(content, "hello world");
        // "hello " then "world".
        assert_eq!(history.undo.len(), 2);
        let back = history.undo(snap(&content, 11..11)).unwrap();
        assert_eq!(back.content.as_ref(), "hello ");
        let back = history.undo(back).unwrap();
        assert_eq!(back.content.as_ref(), "");
        assert!(!history.can_undo());
    }

    #[test]
    fn a_pause_or_caret_jump_breaks_the_group() {
        let mut history = EditHistory::default();
        let now = Instant::now();
        type_into(&mut history, "", "ab", now, Duration::from_millis(1500));
        assert_eq!(history.undo.len(), 2, "a pause starts a new step");

        let mut history = EditHistory::default();
        let kind = EditHistory::kind(&(0..0), "a", false, false);
        history.record(snap("xyz", 3..3), &(3..3), kind, 4, now);
        // The caret moved to 0 before the next keystroke.
        history.record(snap("xyza", 0..0), &(0..0), kind, 1, now);
        assert_eq!(history.undo.len(), 2);
    }

    #[test]
    fn backspaces_coalesce_and_selection_deletes_stand_alone() {
        let mut history = EditHistory::default();
        let now = Instant::now();
        let kind = EditHistory::kind(&(4..5), "", false, true);
        assert_eq!(kind, EditKind::Delete);
        history.record(snap("hello", 5..5), &(4..5), kind, 4, now);
        history.record(snap("hell", 4..4), &(3..4), kind, 3, now);
        assert_eq!(history.undo.len(), 1);
        let kind = EditHistory::kind(&(0..3), "", false, false);
        history.record(snap("hel", 0..3), &(0..3), kind, 0, now);
        assert_eq!(history.undo.len(), 2);
    }

    #[test]
    fn forced_edits_and_new_edits_after_undo() {
        let mut history = EditHistory::default();
        let now = Instant::now();
        let content = type_into(&mut history, "", "ab", now, Duration::ZERO);
        let paste = EditHistory::kind(&(2..2), "c", true, false);
        history.record(snap(&content, 2..2), &(2..2), paste, 3, now);
        assert_eq!(history.undo.len(), 2, "a paste is its own step");
        let back = history.undo(snap("abc", 3..3)).unwrap();
        assert!(history.can_redo());
        let kind = EditHistory::kind(&(2..2), "x", false, false);
        history.record(back, &(2..2), kind, 3, now);
        assert!(!history.can_redo(), "a new edit clears redo");
    }

    #[test]
    fn discard_noop_drops_only_an_unchanged_step() {
        let mut history = EditHistory::default();
        let now = Instant::now();
        history.record(snap("ab", 2..2), &(2..2), EditKind::Other, 3, now);
        history.discard_noop(&"abc".into());
        assert_eq!(history.undo.len(), 1, "a changed text keeps its step");
        history.discard_noop(&"ab".into());
        assert!(
            !history.can_undo(),
            "a cancelled composition leaves no step"
        );
    }

    #[test]
    fn history_is_capped() {
        let mut history = EditHistory::default();
        let now = Instant::now();
        for i in 0..(MAX_STEPS + 50) {
            history.record(snap(&i.to_string(), 0..0), &(0..0), EditKind::Other, 0, now);
        }
        assert_eq!(history.undo.len(), MAX_STEPS);
        assert_eq!(history.undo[0].content.as_ref(), "50");
    }
}
