//! Git blame for the file editor, after `editorBlame.ts`: hunks describe the
//! file as saved, so edits carry them onto the current buffer line by line.
//! Everything here is pure; `input/code` paints it and `FileEditor` fetches it.
use chrono::{Local, TimeZone};
use monocode_core::scm::BlameHunk;

pub(crate) const NOT_COMMITTED_INLINE: &str = "Not committed yet";
pub(crate) const NOT_COMMITTED_COLUMN: &str = "Uncommitted";

/// Who a buffer line belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Owner {
    /// Not part of any hunk: typed since the last save.
    Unblamed,
    /// The empty line after a final newline. Git has no such line ("a\nb\n"
    /// is two lines to it, three to an editor), so it is neither committed
    /// nor new text and gets no annotation.
    PastEnd,
    Hunk(usize),
}

/// One edit as `(from, to)` in the old text and `(from, to)` in the new, byte
/// offsets, like a CodeMirror change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Splice {
    pub from_a: usize,
    pub to_a: usize,
    pub from_b: usize,
    pub to_b: usize,
}

/// `Loading` until blame is first set (the column keeps its width so the code
/// does not jump when it arrives); `Absent` once the file is known to have
/// none (untracked, binary, oversized): annotations go away and the column
/// collapses.
#[derive(Clone, Debug, Default)]
pub(crate) enum State {
    #[default]
    Loading,
    Absent,
    Lines {
        hunks: Vec<BlameHunk>,
        owners: Vec<Owner>,
    },
}

/// A column cell: the block's hunk (committed only) and, on the first line of
/// a block, its `(author, date)` label.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Cell {
    pub hunk: Option<usize>,
    pub label: Option<(String, Option<String>)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TipRow {
    Meta,
    Summary,
    Hint,
}

impl Splice {
    /// One reported edit: `range` of the old text replaced by `inserted` bytes.
    pub(crate) fn edit(range: &std::ops::Range<usize>, inserted: usize) -> Self {
        Self {
            from_a: range.start,
            to_a: range.end,
            from_b: range.start,
            to_b: range.start + inserted,
        }
    }
}

fn line_starts(text: &str) -> Vec<usize> {
    std::iter::once(0)
        .chain(text.match_indices('\n').map(|(at, _)| at + 1))
        .collect()
}

/// The one-based line holding `offset`.
fn line_at(starts: &[usize], offset: usize) -> usize {
    starts.partition_point(|start| *start <= offset)
}

fn line_end(starts: &[usize], len: usize, line: usize) -> usize {
    starts.get(line).map_or(len, |next| next - 1)
}

fn owners_from_hunks(hunks: &[BlameHunk], text: &str) -> Vec<Owner> {
    let starts = line_starts(text);
    let lines = starts.len();
    let mut owners = vec![Owner::Unblamed; lines];
    let mut blamed = 0;
    for (index, hunk) in hunks.iter().enumerate() {
        let end = (hunk.start_line + hunk.line_count)
            .saturating_sub(1)
            .min(lines);
        for line in hunk.start_line.max(1)..=end {
            owners[line - 1] = Owner::Hunk(index);
        }
        blamed = blamed.max(end);
    }
    if lines > 1 && blamed == lines - 1 && starts[lines - 1] == text.len() {
        owners[lines - 1] = Owner::PastEnd;
    }
    owners
}

/// Carry hunks across an edit. A line keeps its hunk while it is edited, a
/// line that an edit adds has none, and removed lines drop out instead of
/// shifting authorship onto their neighbours.
///
/// Each edit is read as one splice of whole lines: the first resulting line
/// continues the first old line only if text before the edit survives on it,
/// the last resulting line continues the last old line only if text after the
/// edit survives on it, and lines in between are new. Pressing Enter at the
/// start of a line therefore leaves the blank line unblamed and keeps the
/// hunk on the line that moved down.
fn map_blame_lines(
    owners: &[Owner],
    splices: &[Splice],
    old_starts: &[usize],
    old_len: usize,
    new_starts: &[usize],
) -> Vec<Owner> {
    // Edits that touch the same old line are one splice, otherwise the second
    // would read a line the first already rewrote.
    let mut merged: Vec<Splice> = Vec::new();
    for splice in splices {
        match merged.last_mut() {
            Some(last) if line_at(old_starts, splice.from_a) <= line_at(old_starts, last.to_a) => {
                last.to_a = splice.to_a;
                last.to_b = splice.to_b;
            }
            _ => merged.push(*splice),
        }
    }
    let at = |line: usize| owners.get(line - 1).copied().unwrap_or(Owner::Unblamed);
    // A line an edit touches is new text, so the trailing line stops being
    // "past the end" as soon as anything is typed on it.
    let carry = |line: usize| match at(line) {
        Owner::PastEnd => Owner::Unblamed,
        owner => owner,
    };
    let mut out = Vec::with_capacity(new_starts.len());
    let mut next = 1;
    for splice in &merged {
        let first = line_at(old_starts, splice.from_a);
        let last = line_at(old_starts, splice.to_a);
        while next < first {
            out.push(at(next));
            next += 1;
        }
        let keeps_prefix = splice.from_a > old_starts[first - 1];
        let keeps_suffix = splice.to_a < line_end(old_starts, old_len, last)
            // Enter on an empty line: the old line moves down, the new one is above.
            || (splice.from_a == splice.to_a && splice.from_a == old_starts[first - 1]);
        let count = line_at(new_starts, splice.to_b) - line_at(new_starts, splice.from_b) + 1;
        if count == 1 {
            // With neither end surviving, the line is new text unless the edit
            // only removed text (what is left is the old last line) or
            // replaced inside one line. Replacing several lines with new text
            // keeps no hunk.
            let owner = if keeps_prefix {
                Some(first)
            } else if keeps_suffix || first == last || splice.to_b == splice.from_b {
                Some(last)
            } else {
                None
            };
            out.push(owner.map_or(Owner::Unblamed, carry));
        } else {
            out.push(if keeps_prefix {
                carry(first)
            } else {
                Owner::Unblamed
            });
            out.extend(std::iter::repeat_n(Owner::Unblamed, count - 2));
            out.push(if keeps_suffix {
                carry(last)
            } else {
                Owner::Unblamed
            });
        }
        next = last + 1;
    }
    while next <= old_starts.len() {
        out.push(at(next));
        next += 1;
    }
    out.resize(new_starts.len(), Owner::Unblamed);
    out
}

/// The splices that turn `old` into `new`, for changes the buffer did not
/// report as one range (undo, redo, reload, Format, replace all). Lines are
/// diffed, so each changed place maps on its own and an edit never slides into
/// a neighbour that happens to start with the same characters. A run of lines
/// replaced by the same number of lines is an edit in place per line, so it
/// keeps its hunks.
pub(crate) fn diff_splices(
    old: &str,
    new: &str,
    old_starts: &[usize],
    new_starts: &[usize],
) -> Vec<Splice> {
    use similar::{Algorithm, DiffOp};
    if old == new {
        return Vec::new();
    }
    let old_lines: Vec<&str> = old.split('\n').collect();
    let new_lines: Vec<&str> = new.split('\n').collect();
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(100);
    let ops = similar::capture_diff_slices_deadline(
        Algorithm::Myers,
        &old_lines,
        &new_lines,
        Some(deadline),
    );
    let at = |starts: &[usize], len: usize, line: usize| starts.get(line).copied().unwrap_or(len);
    // A line's content, without its newline.
    let content = |starts: &[usize], len: usize, line: usize| {
        (starts[line], starts.get(line + 1).map_or(len, |s| s - 1))
    };
    let (old_len, new_len) = (old.len(), new.len());
    let mut splices = Vec::new();
    for op in ops {
        match op {
            DiffOp::Equal { .. } => {}
            DiffOp::Delete {
                old_index,
                old_len: n,
                new_index,
            } => {
                if old_index + n < old_lines.len() {
                    let from_b = at(new_starts, new_len, new_index);
                    splices.push(Splice {
                        from_a: old_starts[old_index],
                        to_a: old_starts[old_index + n],
                        from_b,
                        to_b: from_b,
                    });
                } else {
                    // The last lines: the newline before them goes too.
                    let from_a = old_starts[old_index].saturating_sub(1);
                    splices.push(Splice {
                        from_a,
                        to_a: old_len,
                        from_b: new_len,
                        to_b: new_len,
                    });
                }
            }
            DiffOp::Insert {
                old_index,
                new_index,
                new_len: n,
            } => {
                if old_index < old_lines.len() {
                    let at_old = old_starts[old_index];
                    splices.push(Splice {
                        from_a: at_old,
                        to_a: at_old,
                        from_b: new_starts[new_index],
                        to_b: at(new_starts, new_len, new_index + n),
                    });
                } else {
                    splices.push(Splice {
                        from_a: old_len,
                        to_a: old_len,
                        from_b: new_starts[new_index].saturating_sub(1),
                        to_b: new_len,
                    });
                }
            }
            DiffOp::Replace {
                old_index,
                old_len: m,
                new_index,
                new_len: n,
            } if m == n => {
                for k in 0..m {
                    if old_lines[old_index + k] != new_lines[new_index + k] {
                        let (from_a, to_a) = content(old_starts, old_len, old_index + k);
                        let (from_b, to_b) = content(new_starts, new_len, new_index + k);
                        splices.push(Splice {
                            from_a,
                            to_a,
                            from_b,
                            to_b,
                        });
                    }
                }
            }
            DiffOp::Replace {
                old_index,
                old_len: m,
                new_index,
                new_len: n,
            } => {
                let (from_a, _) = content(old_starts, old_len, old_index);
                let (_, to_a) = content(old_starts, old_len, old_index + m - 1);
                let (from_b, _) = content(new_starts, new_len, new_index);
                let (_, to_b) = content(new_starts, new_len, new_index + n - 1);
                // Within the replaced lines only, so Enter in a line is still
                // a newline inserted mid-line.
                if let Some(edit) =
                    crate::editor_analysis::Edit::between(&old[from_a..to_a], &new[from_b..to_b])
                {
                    splices.push(Splice {
                        from_a: from_a + edit.start,
                        to_a: from_a + edit.old_end,
                        from_b: from_b + edit.start,
                        to_b: from_b + edit.new_end,
                    });
                }
            }
        }
    }
    splices
}

#[cfg(test)]
fn splices_between(old: &str, new: &str) -> Vec<Splice> {
    diff_splices(old, new, &line_starts(old), &line_starts(new))
}

pub(crate) fn format_blame_age(author_time_sec: i64, now_ms: i64) -> String {
    const MINUTE: i64 = 60;
    const HOUR: i64 = 3600;
    const DAY: i64 = 86400;
    // `now_ms / 1000` floors, and the author time is whole seconds, so this
    // is the floor of the fractional difference.
    let seconds = now_ms.div_euclid(1000) - author_time_sec;
    let unit = |n: i64, name: &str| format!("{n} {name}{} ago", if n == 1 { "" } else { "s" });
    if seconds < MINUTE {
        return "just now".into();
    }
    if seconds < HOUR {
        return unit(seconds / MINUTE, "minute");
    }
    if seconds < DAY {
        return unit(seconds / HOUR, "hour");
    }
    let days = seconds / DAY;
    if days < 30 {
        return unit(days, "day");
    }
    if days < 365 {
        return unit(((days as f64 / 30.4375).floor() as i64).max(1), "month");
    }
    unit(days / 365, "year")
}

/// `YYYY-MM-DD` in local time.
pub(crate) fn short_date(author_time_sec: i64) -> String {
    Local
        .timestamp_opt(author_time_sec, 0)
        .single()
        .map_or_else(String::new, |t| t.format("%Y-%m-%d").to_string())
}

/// `YYYY-MM-DD HH:MM` in local time.
fn full_date(author_time_sec: i64) -> String {
    Local
        .timestamp_opt(author_time_sec, 0)
        .single()
        .map_or_else(String::new, |t| t.format("%Y-%m-%d %H:%M").to_string())
}

fn inline_text(hunk: Option<&BlameHunk>, now_ms: i64) -> String {
    match hunk {
        Some(hunk) if !hunk.uncommitted => format!(
            "{}, {} \u{2022} {}",
            hunk.author,
            format_blame_age(hunk.author_time, now_ms),
            hunk.summary
        ),
        _ => NOT_COMMITTED_INLINE.into(),
    }
}

/// The hover card: commit and author, date, summary, and a hint when a click
/// would open the commit.
pub(crate) fn tooltip_rows(hunk: Option<&BlameHunk>, can_open: bool) -> Vec<(TipRow, String)> {
    let Some(hunk) = hunk.filter(|h| !h.uncommitted) else {
        return vec![(TipRow::Summary, NOT_COMMITTED_INLINE.into())];
    };
    let mut rows = vec![
        (TipRow::Meta, format!("{}  {}", hunk.short_sha, hunk.author)),
        (TipRow::Meta, full_date(hunk.author_time)),
        (TipRow::Summary, hunk.summary.clone()),
    ];
    if can_open {
        rows.push((TipRow::Hint, "Click to open commit".into()));
    }
    rows
}

/// A stable hue per commit so neighbouring blocks read as separate.
pub(crate) fn hunk_hue(sha: &str) -> u32 {
    sha.bytes()
        .fold(0u32, |hash, b| hash.wrapping_mul(31).wrapping_add(b as u32))
        % 360
}

/// `text` cut to `max` characters, ending in an ellipsis when it was cut.
pub(crate) fn fit(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.into();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    if max > 0 {
        out.push('\u{2026}');
    }
    out
}

impl State {
    pub(crate) fn from_hunks(hunks: &[BlameHunk], text: &str) -> Self {
        if hunks.is_empty() {
            return Self::Absent;
        }
        Self::Lines {
            hunks: hunks.to_vec(),
            owners: owners_from_hunks(hunks, text),
        }
    }

    /// Carry the hunks across an edit, given the line starts of the text
    /// before (and its length) and after.
    pub(crate) fn map_starts(
        &mut self,
        splices: &[Splice],
        old_starts: &[usize],
        old_len: usize,
        new_starts: &[usize],
    ) {
        if let Self::Lines { owners, .. } = self {
            *owners = map_blame_lines(owners, splices, old_starts, old_len, new_starts);
        }
    }

    #[cfg(test)]
    fn map(&mut self, splices: &[Splice], old: &str, new: &str) {
        self.map_starts(splices, &line_starts(old), old.len(), &line_starts(new));
    }

    /// The column holds its width until blame is known to be absent.
    pub(crate) fn reserves_column(&self) -> bool {
        !matches!(self, Self::Absent)
    }

    pub(crate) fn has_lines(&self) -> bool {
        matches!(self, Self::Lines { .. })
    }

    fn owner(&self, line: usize) -> Option<Owner> {
        match self {
            Self::Lines { owners, .. } => owners.get(line.checked_sub(1)?).copied(),
            _ => None,
        }
    }

    pub(crate) fn is_past_end(&self, line: usize) -> bool {
        self.owner(line) == Some(Owner::PastEnd)
    }

    fn hunk_index(&self, line: usize) -> Option<usize> {
        match self.owner(line)? {
            Owner::Hunk(index) => Some(index),
            _ => None,
        }
    }

    /// The hunk of a line, or `None` when it has none (not loaded, typed since
    /// the last save, or outside the blamed file).
    pub(crate) fn hunk(&self, line: usize) -> Option<&BlameHunk> {
        match self {
            Self::Lines { hunks, .. } => hunks.get(self.hunk_index(line)?),
            _ => None,
        }
    }

    pub(crate) fn hunk_at(&self, index: usize) -> Option<&BlameHunk> {
        match self {
            Self::Lines { hunks, .. } => hunks.get(index),
            _ => None,
        }
    }

    /// Hunks that count as the same block in the column; every uncommitted or
    /// unblamed line shares one block.
    fn block_of(&self, line: usize) -> Option<usize> {
        let index = self.hunk_index(line)?;
        (!self.hunk_at(index)?.uncommitted).then_some(index)
    }

    /// The annotation after the cursor line: its text and hunk. Nothing on the
    /// empty line after a final newline, or before blame has loaded.
    pub(crate) fn inline(&self, line: usize, now_ms: i64) -> Option<(String, Option<usize>)> {
        if !self.has_lines() || self.is_past_end(line) {
            return None;
        }
        Some((
            inline_text(self.hunk(line), now_ms),
            self.hunk_index(line).filter(|i| self.hunk_at(*i).is_some()),
        ))
    }

    /// The column cell of a line; none before blame loads or on the empty line
    /// after a final newline.
    pub(crate) fn cell(&self, line: usize) -> Option<Cell> {
        if !self.has_lines() || self.is_past_end(line) {
            return None;
        }
        let block = self.block_of(line);
        let first = line == 1 || self.block_of(line - 1) != block;
        let label = first.then(|| match block.and_then(|i| self.hunk_at(i)) {
            Some(hunk) => (hunk.author.clone(), Some(short_date(hunk.author_time))),
            None => (NOT_COMMITTED_COLUMN.to_owned(), None),
        });
        Some(Cell { hunk: block, label })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_781_524_800_000; // 2026-06-15 12:00:00 UTC
    const SEC: i64 = NOW / 1000;
    const DOC: &str = "one\ntwo\nthree\nfour\nfive\nsix";

    fn hunk(sha: &str, start: usize, count: usize) -> BlameHunk {
        BlameHunk {
            sha: format!("{sha:0<40}"),
            short_sha: sha.chars().take(7).collect(),
            author: format!("author-{sha}"),
            author_time: SEC - 2 * 86400,
            summary: format!("summary {sha}"),
            uncommitted: false,
            start_line: start,
            line_count: count,
        }
    }
    fn blame() -> Vec<BlameHunk> {
        let uncommitted = BlameHunk {
            author: "Not Committed Yet".into(),
            summary: "Version of x from x".into(),
            uncommitted: true,
            ..hunk("0", 6, 1)
        };
        vec![hunk("aaa1111", 1, 2), hunk("bbb2222", 3, 3), uncommitted]
    }
    fn state(text: &str) -> State {
        State::from_hunks(&blame(), text)
    }
    /// One letter per line: A, B, U for the hunks, `-` for no hunk, `$` for
    /// the empty line after a final newline.
    fn letters(state: &State, lines: usize) -> String {
        (1..=lines)
            .map(|line| {
                if state.is_past_end(line) {
                    return '$';
                }
                match state.hunk(line).map(|h| h.author.as_str()) {
                    Some("author-aaa1111") => 'A',
                    Some("author-bbb2222") => 'B',
                    Some("author-ccc3333") => 'C',
                    Some(_) => 'U',
                    None => '-',
                }
            })
            .collect()
    }
    /// The splices an editor reports for changes given in old coordinates.
    fn edit(old: &str, changes: &[(usize, usize, &str)]) -> (String, Vec<Splice>) {
        let mut text = String::new();
        let mut splices = Vec::new();
        let mut at = 0;
        for (from, to, insert) in changes {
            text.push_str(&old[at..*from]);
            let from_b = text.len();
            text.push_str(insert);
            splices.push(Splice {
                from_a: *from,
                to_a: *to,
                from_b,
                to_b: text.len(),
            });
            at = *to;
        }
        text.push_str(&old[at..]);
        (text, splices)
    }
    fn mapped(old: &str, changes: &[(usize, usize, &str)]) -> (State, String) {
        let (new, splices) = edit(old, changes);
        let mut state = state(old);
        state.map(&splices, old, &new);
        (state, new)
    }
    fn line_from(text: &str, line: usize) -> usize {
        text.split('\n').take(line - 1).map(|l| l.len() + 1).sum()
    }
    fn line_to(text: &str, line: usize) -> usize {
        line_from(text, line) + text.split('\n').nth(line - 1).unwrap().len()
    }
    fn count(text: &str) -> usize {
        text.split('\n').count()
    }
    fn local_date(sec: i64) -> String {
        chrono::Local
            .timestamp_opt(sec, 0)
            .unwrap()
            .format("%Y-%m-%d")
            .to_string()
    }

    #[test]
    fn age_says_just_now_under_a_minute_including_clock_skew() {
        let ago = |seconds: i64| format_blame_age(SEC - seconds, NOW);
        assert_eq!(ago(0), "just now");
        assert_eq!(ago(59), "just now");
        assert_eq!(ago(-300), "just now");
    }

    #[test]
    fn age_switches_units_at_the_boundaries() {
        let ago = |seconds: i64| format_blame_age(SEC - seconds, NOW);
        assert_eq!(ago(60), "1 minute ago");
        assert_eq!(ago(5 * 60), "5 minutes ago");
        assert_eq!(ago(3599), "59 minutes ago");
        assert_eq!(ago(3600), "1 hour ago");
        assert_eq!(ago(3 * 3600), "3 hours ago");
        assert_eq!(ago(86399), "23 hours ago");
        assert_eq!(ago(86400), "1 day ago");
        assert_eq!(ago(2 * 86400), "2 days ago");
        assert_eq!(ago(29 * 86400), "29 days ago");
        assert_eq!(ago(30 * 86400), "1 month ago");
        assert_eq!(ago(120 * 86400), "3 months ago");
        assert_eq!(ago(364 * 86400), "11 months ago");
        assert_eq!(ago(365 * 86400), "1 year ago");
        assert_eq!(ago(800 * 86400), "2 years ago");
    }

    #[test]
    fn age_floors_a_fractional_clock() {
        // 59.9 s is still under a minute; 60.1 s is a minute.
        assert_eq!(format_blame_age(SEC - 60, NOW - 100), "just now");
        assert_eq!(format_blame_age(SEC - 60, NOW + 100), "1 minute ago");
    }

    #[test]
    fn assigns_each_line_its_hunk() {
        assert_eq!(letters(&state(DOC), 6), "AABBBU");
    }

    #[test]
    fn no_hunks_means_no_blame_and_lines_past_the_document_are_ignored() {
        assert_eq!(letters(&state("one\ntwo"), 2), "AA");
        let absent = State::from_hunks(&[], DOC);
        assert!(matches!(absent, State::Absent));
        assert_eq!(letters(&absent, 6), "------");
    }

    #[test]
    fn gives_an_inserted_line_no_hunk_and_keeps_the_lines_after_it() {
        let at = line_to(DOC, 3);
        let (state, new) = mapped(DOC, &[(at, at, "\nnew")]);
        assert_eq!(letters(&state, count(&new)), "AAB-BBU");
    }

    #[test]
    fn keeps_the_hunk_on_the_line_that_moves_down_when_enter_splits_at_line_start() {
        let at = line_from(DOC, 3);
        let (state, new) = mapped(DOC, &[(at, at, "\n")]);
        assert_eq!(letters(&state, count(&new)), "AA-BBBU");
    }

    #[test]
    fn keeps_the_hunk_on_the_first_half_when_enter_splits_mid_line() {
        let at = line_from(DOC, 1) + 1;
        let (state, new) = mapped(DOC, &[(at, at, "\n")]);
        assert_eq!(letters(&state, count(&new)), "AAABBBU");
    }

    #[test]
    fn keeps_the_hunk_while_a_line_is_edited_in_place() {
        let at = line_to(DOC, 4);
        let (state, new) = mapped(DOC, &[(at, at, "!")]);
        assert_eq!(letters(&state, count(&new)), "AABBBU");
    }

    #[test]
    fn shrinks_a_hunk_when_its_lines_are_deleted() {
        let (from, to) = (line_from(DOC, 4), line_to(DOC, 4) + 1);
        let (state, new) = mapped(DOC, &[(from, to, "")]);
        assert_eq!(letters(&state, count(&new)), "AABBU");
    }

    #[test]
    fn drops_lines_across_a_hunk_boundary_without_shifting_authorship() {
        let (from, to) = (line_from(DOC, 2), line_from(DOC, 4));
        let (state, new) = mapped(DOC, &[(from, to, "")]);
        assert_eq!(letters(&state, count(&new)), "ABBU");
    }

    #[test]
    fn keeps_the_first_hunk_when_a_deletion_joins_two_lines_midway() {
        let (from, to) = (line_from(DOC, 2) + 1, line_from(DOC, 4) + 1);
        let (state, new) = mapped(DOC, &[(from, to, "")]);
        assert_eq!(letters(&state, count(&new)), "AABU");
    }

    #[test]
    fn handles_several_edits_on_one_line_as_a_single_splice() {
        let (from, to) = (line_from(DOC, 3), line_to(DOC, 3));
        let (state, new) = mapped(DOC, &[(from, from, "["), (to, to, "]")]);
        assert_eq!(new, "one\ntwo\n[three]\nfour\nfive\nsix");
        assert_eq!(letters(&state, count(&new)), "AABBBU");
    }

    #[test]
    fn drops_the_last_line_when_the_final_newline_is_deleted_with_it() {
        let (from, to) = (line_to(DOC, 5), DOC.len());
        let (state, new) = mapped(DOC, &[(from, to, "")]);
        assert_eq!(letters(&state, count(&new)), "AABBB");
    }

    #[test]
    fn gives_no_hunk_to_text_typed_over_several_lines() {
        let (state, new) = mapped(DOC, &[(0, line_to(DOC, 5), "z")]);
        assert_eq!(new, "z\nsix");
        assert_eq!(letters(&state, count(&new)), "-U");
    }

    #[test]
    fn keeps_the_remaining_lines_hunk_when_a_multiline_deletion_removes_text_only() {
        let (state, new) = mapped(DOC, &[(0, line_from(DOC, 6), "")]);
        assert_eq!(letters(&state, count(&new)), "U");
    }

    #[test]
    fn maps_a_bulk_replace_to_the_new_line_count_with_no_stale_hunks() {
        let (state, new) = mapped(DOC, &[(0, DOC.len(), "x\ny")]);
        assert_eq!(letters(&state, count(&new)), "--");
    }

    #[test]
    fn loading_and_absent_blame_survive_edits_unchanged() {
        let mut loading = State::default();
        loading.map(&[], DOC, "x");
        assert!(matches!(loading, State::Loading));
        let mut absent = State::from_hunks(&[], DOC);
        let (new, splices) = edit(DOC, &[(0, 0, "new\n")]);
        absent.map(&splices, DOC, &new);
        assert!(matches!(absent, State::Absent));
    }

    #[test]
    fn diff_splices_read_changes_per_line() {
        assert_eq!(splices_between("abc", "abc"), vec![]);
        // A line edited in place.
        assert_eq!(
            splices_between("one\ntwo", "one\ntwXo"),
            vec![Splice {
                from_a: 4,
                to_a: 7,
                from_b: 4,
                to_b: 8
            }]
        );
        // A line inserted, with its newline.
        assert_eq!(
            splices_between("a\nc", "a\nb\nc"),
            vec![Splice {
                from_a: 2,
                to_a: 2,
                from_b: 2,
                to_b: 4
            }]
        );
        // A line deleted, with its newline.
        assert_eq!(
            splices_between("a\nb\nc", "a\nc"),
            vec![Splice {
                from_a: 2,
                to_a: 4,
                from_b: 2,
                to_b: 2
            }]
        );
        // The last line deleted takes the newline before it.
        assert_eq!(
            splices_between("a\nb", "a"),
            vec![Splice {
                from_a: 1,
                to_a: 3,
                from_b: 1,
                to_b: 1
            }]
        );
        // Lines appended after the last one bring the newline before them.
        assert_eq!(
            splices_between("a", "a\nb"),
            vec![Splice {
                from_a: 1,
                to_a: 1,
                from_b: 1,
                to_b: 3
            }]
        );
    }

    #[test]
    fn splices_between_splits_a_same_line_count_change_per_line() {
        // Indenting three lines is one range edit natively; each line is an
        // in-place edit, as when an editor reports one change per line.
        let new = "one\n  two\n  three\n  four\nfive\nsix";
        let splices = splices_between(DOC, new);
        assert_eq!(splices.len(), 3);
        let mut state = state(DOC);
        state.map(&splices, DOC, new);
        assert_eq!(letters(&state, count(new)), "AABBBU");
    }

    #[test]
    fn mapping_through_the_diff_follows_typing_enter_and_paste() {
        let typed = "one\ntwXo\nthree\nfour\nfive\nsix";
        let mut s = state(DOC);
        s.map(&splices_between(DOC, typed), DOC, typed);
        assert_eq!(letters(&s, 6), "AABBBU");

        let enter = "one\ntw\no\nthree\nfour\nfive\nsix";
        let mut s = state(DOC);
        s.map(&splices_between(DOC, enter), DOC, enter);
        assert_eq!(letters(&s, 7), "AAABBBU");

        let pasted = "one\ntwo\nx\ny\nthree\nfour\nfive\nsix";
        let mut s = state(DOC);
        s.map(&splices_between(DOC, pasted), DOC, pasted);
        assert_eq!(letters(&s, 8), "AA--BBBU");
    }

    const CODE: &str = "fn a() {\n    x();\n    y();\n}";
    fn code_state() -> State {
        let hunks: Vec<_> = ["a", "b", "c", "d"]
            .iter()
            .enumerate()
            .map(|(i, name)| BlameHunk {
                author: format!("author-{name}"),
                ..hunk(name, i + 1, 1)
            })
            .collect();
        State::from_hunks(&hunks, CODE)
    }
    fn code_authors(state: &State, lines: usize) -> String {
        (1..=lines)
            .map(|n| {
                state.hunk(n).map_or('-', |h| {
                    h.author.chars().last().unwrap().to_ascii_uppercase()
                })
            })
            .collect()
    }

    #[test]
    fn the_diff_fallback_does_not_slide_into_a_neighbour_that_starts_alike() {
        // Enter at the end of an indented line inserts a newline and the indent.
        let enter = "fn a() {\n    x();\n    \n    y();\n}";
        let mut s = code_state();
        s.map(&splices_between(CODE, enter), CODE, enter);
        assert_eq!(code_authors(&s, 5), "AB-CD");
        // Deleting a whole line.
        let deleted = "fn a() {\n    y();\n}";
        let mut s = code_state();
        s.map(&splices_between(CODE, deleted), CODE, deleted);
        assert_eq!(code_authors(&s, 3), "ACD");
        // A pasted line that starts like the next one.
        let pasted = "fn a() {\n    x();\n    z();\n    y();\n}";
        let mut s = code_state();
        s.map(&splices_between(CODE, pasted), CODE, pasted);
        assert_eq!(code_authors(&s, 5), "AB-CD");
        // Plain text: deleting "two\n" leaves "three" with its own hunk.
        let old = "one\ntwo\nthree";
        let hunks = [
            hunk("aaa1111", 1, 1),
            hunk("bbb2222", 2, 1),
            hunk("ccc3333", 3, 1),
        ];
        let mut s = State::from_hunks(&hunks, old);
        s.map(&splices_between(old, "one\nthree"), old, "one\nthree");
        assert_eq!(letters(&s, 2), "AC");
    }

    #[test]
    fn the_diff_fallback_maps_each_changed_place_on_its_own() {
        // Replace all with a newline in two places: the lines between stay.
        let old = "foo\nb\nc\nfoo\nd";
        let new = "foo\nbar\nb\nc\nfoo\nbar\nd";
        let hunks: Vec<_> = (0..5)
            .map(|i| BlameHunk {
                author: format!(
                    "author-{}",
                    ["aaa1111", "bbb2222", "ccc3333", "ddd4444", "eee5555"][i]
                ),
                ..hunk("x", i + 1, 1)
            })
            .collect();
        let mut s = State::from_hunks(&hunks, old);
        s.map(&splices_between(old, new), old, new);
        let who: Vec<_> = (1..=7)
            .map(|n| s.hunk(n).map(|h| h.author.chars().last().unwrap()))
            .collect();
        assert_eq!(
            who,
            [
                Some('1'),
                None,
                Some('2'),
                Some('3'),
                Some('4'),
                None,
                Some('5')
            ]
        );
    }

    #[test]
    fn undoing_enter_through_the_diff_keeps_the_line() {
        let enter = "one\ntw\no\nthree\nfour\nfive\nsix";
        let mut s = state(DOC);
        s.map(&splices_between(DOC, enter), DOC, enter);
        assert_eq!(letters(&s, 7), "AAABBBU");
        s.map(&splices_between(enter, DOC), enter, DOC);
        assert_eq!(letters(&s, 6), "AABBBU");
    }

    #[test]
    fn inline_text_names_author_age_and_summary() {
        let s = state(DOC);
        let (text, hunk) = s.inline(4, NOW).unwrap();
        assert_eq!(text, "author-bbb2222, 2 days ago \u{2022} summary bbb2222");
        assert_eq!(hunk, Some(1));
        let (text, _) = s.inline(1, NOW).unwrap();
        assert_eq!(text, "author-aaa1111, 2 days ago \u{2022} summary aaa1111");
    }

    #[test]
    fn inline_says_not_committed_yet_for_uncommitted_and_unblamed_lines() {
        let s = state(DOC);
        assert_eq!(s.inline(6, NOW).unwrap().0, "Not committed yet");
        let at = line_to(DOC, 2);
        let (s, _) = mapped(DOC, &[(at, at, "\nnew")]);
        assert_eq!(s.inline(3, NOW).unwrap().0, "Not committed yet");
    }

    #[test]
    fn inline_is_absent_before_blame_loads_or_when_the_file_has_none() {
        assert_eq!(State::default().inline(1, NOW), None);
        assert_eq!(State::from_hunks(&[], DOC).inline(1, NOW), None);
    }

    #[test]
    fn tooltip_lists_commit_author_date_and_summary() {
        let h = hunk("bbb2222", 3, 3);
        let rows = tooltip_rows(Some(&h), true);
        let date = chrono::Local
            .timestamp_opt(h.author_time, 0)
            .unwrap()
            .format("%Y-%m-%d %H:%M")
            .to_string();
        assert_eq!(
            rows,
            vec![
                (TipRow::Meta, "bbb2222  author-bbb2222".to_string()),
                (TipRow::Meta, date),
                (TipRow::Summary, "summary bbb2222".to_string()),
                (TipRow::Hint, "Click to open commit".to_string()),
            ]
        );
    }

    #[test]
    fn tooltip_omits_the_click_hint_without_a_handler() {
        let h = hunk("bbb2222", 3, 3);
        assert!(
            tooltip_rows(Some(&h), false)
                .iter()
                .all(|(row, _)| *row != TipRow::Hint)
        );
    }

    #[test]
    fn tooltip_for_an_uncommitted_line_is_just_the_notice_even_with_a_handler() {
        let u = BlameHunk {
            uncommitted: true,
            ..hunk("0", 6, 1)
        };
        let only = vec![(TipRow::Summary, "Not committed yet".to_string())];
        assert_eq!(tooltip_rows(Some(&u), true), only);
        assert_eq!(tooltip_rows(None, true), only);
    }

    #[test]
    fn column_labels_the_first_line_of_each_hunk_only() {
        let s = state(DOC);
        let date = local_date(SEC - 2 * 86400);
        let cells: Vec<_> = (1..=6).map(|line| s.cell(line).unwrap().label).collect();
        assert_eq!(
            cells,
            vec![
                Some(("author-aaa1111".to_string(), Some(date.clone()))),
                None,
                Some(("author-bbb2222".to_string(), Some(date))),
                None,
                None,
                Some(("Uncommitted".to_string(), None)),
            ]
        );
    }

    #[test]
    fn column_cells_only_link_committed_hunks() {
        let s = state(DOC);
        assert_eq!(s.cell(1).unwrap().hunk, Some(0));
        assert_eq!(s.cell(6).unwrap().hunk, None);
    }

    #[test]
    fn column_starts_a_new_block_after_an_inserted_line_splits_a_hunk() {
        let at = line_to(DOC, 4);
        let (s, new) = mapped(DOC, &[(at, at, "\nnew")]);
        let authors: Vec<_> = (1..=count(&new))
            .map(|line| {
                s.cell(line)
                    .and_then(|c| c.label)
                    .map_or(String::new(), |(author, _)| author)
            })
            .collect();
        assert_eq!(
            authors,
            [
                "author-aaa1111",
                "",
                "author-bbb2222",
                "",
                "Uncommitted",
                "author-bbb2222",
                "Uncommitted"
            ]
        );
    }

    #[test]
    fn column_is_empty_until_blame_arrives_and_when_it_is_absent() {
        assert_eq!(State::default().cell(1), None);
        assert_eq!(State::from_hunks(&[], DOC).cell(1), None);
        assert!(State::default().reserves_column());
        assert!(!State::from_hunks(&[], DOC).reserves_column());
        assert!(state(DOC).reserves_column());
    }

    #[test]
    fn hue_is_stable_per_commit() {
        assert_eq!(hunk_hue("abc"), hunk_hue("abc"));
        assert!(hunk_hue("0123456789abcdef") < 360);
        assert_ne!(hunk_hue("aaa"), hunk_hue("bbb"));
    }

    #[test]
    fn fit_shortens_with_an_ellipsis() {
        assert_eq!(fit("short", 10), "short");
        assert_eq!(fit("exactly10c", 10), "exactly10c");
        assert_eq!(fit("a longer author name", 8), "a longe\u{2026}");
        assert_eq!(fit("anything", 0), "");
    }

    mod trailing_newline {
        use super::*;
        // Git sees two lines here, an editor three.
        const TEXT: &str = "one\ntwo\n";

        fn trailing() -> State {
            State::from_hunks(&[hunk("ccc3333", 1, 2)], TEXT)
        }

        #[test]
        fn the_empty_last_line_has_no_marker_cell_or_annotation() {
            let s = trailing();
            assert_eq!(letters(&s, 3), "CC$");
            assert_eq!(s.hunk(3), None);
            assert_eq!(s.cell(3), None);
            assert_eq!(s.inline(3, NOW), None);
            assert!(s.inline(2, NOW).is_some());
        }

        #[test]
        fn text_typed_on_the_trailing_line_is_not_committed_yet() {
            let mut s = trailing();
            let (new, splices) = edit(TEXT, &[(TEXT.len(), TEXT.len(), "new")]);
            s.map(&splices, TEXT, &new);
            assert_eq!(letters(&s, 3), "CC-");
            let label = s.cell(3).unwrap().label.unwrap();
            assert_eq!(label.0, "Uncommitted");
            assert_eq!(s.inline(3, NOW).unwrap().0, "Not committed yet");
        }

        #[test]
        fn a_line_added_at_the_end_is_new_leaving_the_trailing_line_blank() {
            let mut s = trailing();
            let at = line_to(TEXT, 2);
            let (new, splices) = edit(TEXT, &[(at, at, "\nthree")]);
            assert_eq!(new, "one\ntwo\nthree\n");
            s.map(&splices, TEXT, &new);
            assert_eq!(letters(&s, 4), "CC-$");
            assert_eq!(s.inline(3, NOW).unwrap().0, "Not committed yet");
            assert_eq!(s.inline(4, NOW), None);
        }

        #[test]
        fn stops_treating_the_last_line_as_past_the_end_once_enter_is_pressed_on_it() {
            let mut s = trailing();
            let (new, splices) = edit(TEXT, &[(TEXT.len(), TEXT.len(), "\n")]);
            s.map(&splices, TEXT, &new);
            assert_eq!(letters(&s, 4), "CC--");
            assert_eq!(s.inline(3, NOW).unwrap().0, "Not committed yet");
            assert_eq!(s.inline(4, NOW).unwrap().0, "Not committed yet");
        }

        #[test]
        fn a_clean_file_stays_clean_after_an_edit_elsewhere() {
            let mut s = trailing();
            let (new, splices) = edit(TEXT, &[(0, 0, "x")]);
            s.map(&splices, TEXT, &new);
            assert_eq!(letters(&s, 3), "CC$");
            assert_eq!(s.inline(3, NOW), None);
        }
    }
}
