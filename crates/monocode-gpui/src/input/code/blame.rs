//! Git blame in the editor: a column before the line numbers, an annotation
//! after the cursor line, a hover card and click-to-open. The state and the
//! wording live in `file_editor::blame`; this paints and hit-tests them.
use super::*;
use crate::file_editor::blame::{self as pure, State, TipRow};
use monocode_core::scm::BlameHunk;

/// 22ch of the editor font, as in the React column.
pub(super) const BLAME_COLUMN: f32 = 165.;
const FONT: f32 = 11.;
const CHAR: f32 = FONT * 0.6;
/// Space between the end of the line and its annotation (4ch).
const INLINE_GAP: f32 = 30.;
const TIP_WIDTH: f32 = 420.;

/// Where blame was painted last frame, for hover and click.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct BlameHit {
    pub bounds: Bounds<Pixels>,
    pub hunk: Option<usize>,
    pub inline: bool,
}

/// One painted column cell.
pub(super) struct ColumnRow {
    top: f32,
    height: Pixels,
    hunk: Option<usize>,
    tint: Hsla,
    label: Option<(String, Option<String>)>,
}

pub(super) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

impl Code {
    /// The column holds its width while blame loads and collapses once the
    /// file is known to have none. The diff view has its own gutter.
    pub(super) fn blame_width(&self) -> f32 {
        if self.blame_column && !self.diff && self.blame.reserves_column() {
            BLAME_COLUMN
        } else {
            0.
        }
    }
}

/// The cells of the visible rows.
pub(super) fn column_rows(
    code: &Code,
    lines: &[(usize, WrappedLine)],
    tops: &[f32],
    lh: Pixels,
) -> Vec<ColumnRow> {
    if code.blame_width() == 0. {
        return Vec::new();
    }
    lines
        .iter()
        .zip(tops)
        .filter_map(|((row, line), top)| {
            let cell = code.blame.cell(row + 1)?;
            let tint = match cell.hunk.and_then(|i| code.blame.hunk_at(i)) {
                Some(hunk) => gpui::hsla(pure::hunk_hue(&hunk.sha) as f32 / 360., 0.55, 0.55, 0.55),
                None => crate::theme::content(0.18),
            };
            Some(ColumnRow {
                top: *top,
                height: line.size(lh).height,
                hunk: cell.hunk,
                tint,
                label: cell.label,
            })
        })
        .collect()
}

pub(super) fn paint_column(
    rows: &[ColumnRow],
    bounds: Bounds<Pixels>,
    scroll: Pixels,
    lh: Pixels,
    w: &mut Window,
    cx: &mut App,
) -> Vec<BlameHit> {
    let mut hits = Vec::new();
    for row in rows {
        let cell = Bounds::new(
            point(bounds.left(), bounds.top() + px(row.top) - scroll),
            size(px(BLAME_COLUMN), row.height),
        );
        w.paint_quad(fill(
            Bounds::new(cell.origin, size(px(2.), cell.size.height)),
            row.tint,
        ));
        if let Some((author, date)) = &row.label {
            let muted = crate::theme::content(0.4);
            let right = cell.right() - px(6.);
            let mut room = BLAME_COLUMN - 2. - 6. - 6.;
            if let Some(date) = date {
                let line = plain_line(date, None, FONT, w, muted);
                let _ = line.paint(
                    point(right - line.width(), cell.top()),
                    lh,
                    TextAlign::Left,
                    None,
                    w,
                    cx,
                );
                room -= line.width().as_f32() + CHAR;
            }
            let author = pure::fit(author, (room / CHAR).floor().max(0.) as usize);
            if !author.is_empty() {
                let line = plain_line(&author, None, FONT, w, muted);
                let _ = line.paint(
                    point(cell.left() + px(8.), cell.top()),
                    lh,
                    TextAlign::Left,
                    None,
                    w,
                    cx,
                );
            }
        }
        let clipped = cell.intersect(&bounds);
        if clipped.size.height > px(0.) {
            hits.push(BlameHit {
                bounds: clipped,
                hunk: row.hunk,
                inline: false,
            });
        }
    }
    hits
}

/// Paints the annotation after a line's content; the hit is where it landed.
pub(super) fn paint_inline(
    (text, hunk): &(String, Option<usize>),
    line: &WrappedLine,
    origin: Point<Pixels>,
    clip: Bounds<Pixels>,
    lh: Pixels,
    w: &mut Window,
    cx: &mut App,
) -> Option<BlameHit> {
    let end = line.position_for_index(line.len(), lh)?;
    let shaped = plain_line(text, None, FONT, w, crate::theme::content(0.4));
    let at = origin + end + point(px(INLINE_GAP), px(0.));
    let _ = shaped.paint(at, lh, TextAlign::Left, None, w, cx);
    let bounds = Bounds::new(at, size(shaped.width(), lh)).intersect(&clip);
    (bounds.size.width > px(0.)).then_some(BlameHit {
        bounds,
        hunk: *hunk,
        inline: true,
    })
}

impl Input {
    #[cfg(test)]
    pub(crate) fn editor_blame_state(&self) -> &State {
        &self.code.as_ref().unwrap().blame
    }

    #[cfg(test)]
    pub(crate) fn editor_blame_options(&self) -> (bool, bool) {
        let code = self.code.as_ref().unwrap();
        (code.blame_inline, code.blame_column)
    }

    /// Blame for the file as saved: `None` while it is not loaded, an empty
    /// slice when the file has none (untracked, binary, oversized).
    pub(crate) fn set_editor_blame(&mut self, hunks: Option<&[BlameHunk]>, cx: &mut Context<Self>) {
        if let Some(code) = &mut self.code {
            code.blame = hunks.map_or(State::Loading, |h| State::from_hunks(h, &self.content));
            code.blame_hits.clear();
            code.hovered_blame = None;
            code.layout_width = None;
        }
        cx.notify();
    }

    pub(crate) fn set_editor_blame_options(
        &mut self,
        inline: bool,
        column: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some(code) = &mut self.code
            && (code.blame_inline, code.blame_column) != (inline, column)
        {
            code.blame_inline = inline;
            code.blame_column = column;
            code.blame_hits.clear();
            code.hovered_blame = None;
            code.layout_width = None;
            cx.notify();
        }
    }

    /// The annotation for the cursor line: its text and hunk. Shown only for
    /// a caret, with the editor focused and blame loaded.
    pub(super) fn blame_inline(
        &self,
        focused: bool,
        now_ms: i64,
    ) -> Option<(String, Option<usize>)> {
        let code = self.code.as_ref()?;
        if !code.blame_inline || code.diff || !focused || !self.selection.is_empty() {
            return None;
        }
        code.blame.inline(code.row(self.cursor()) + 1, now_ms)
    }

    pub(in crate::input) fn blame_hover(&mut self, e: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(code) = &mut self.code else { return };
        let hovered = code
            .blame_hits
            .iter()
            .rposition(|hit| hit.bounds.contains(&e.position));
        if code.hovered_blame != hovered {
            code.hovered_blame = hovered;
            cx.notify();
        }
    }

    pub(in crate::input) fn blame_leave(&mut self, cx: &mut Context<Self>) {
        if let Some(code) = &mut self.code
            && code.hovered_blame.take().is_some()
        {
            cx.notify();
        }
    }

    /// A click on a committed cell or annotation opens its commit. Cells never
    /// move the caret; an uncommitted annotation behaves like the text under it.
    pub(in crate::input) fn blame_down(
        &mut self,
        e: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(code) = &self.code else {
            return false;
        };
        let Some(hit) = code
            .blame_hits
            .iter()
            .rfind(|hit| hit.bounds.contains(&e.position))
        else {
            return false;
        };
        let commit = hit
            .hunk
            .and_then(|i| code.blame.hunk_at(i))
            .filter(|hunk| !hunk.uncommitted)
            .map(|hunk| BlameCommit {
                sha: hunk.sha.clone(),
                short_sha: hunk.short_sha.clone(),
                subject: hunk.summary.clone(),
            });
        if commit.is_none() && hit.inline {
            return false;
        }
        cx.stop_propagation();
        if let Some(commit) = commit {
            cx.emit(CodeAction::OpenCommit(commit));
        }
        true
    }

    pub(super) fn blame_tooltip(&self) -> Option<AnyElement> {
        let code = self.code.as_ref()?;
        let hit = code.blame_hits.get(code.hovered_blame?)?;
        let frame = code.frame?;
        let rows = pure::tooltip_rows(hit.hunk.and_then(|i| code.blame.hunk_at(i)), true);
        let height = rows.len() as f32 * 18. + 14.;
        let left = (hit.bounds.left() - frame.left())
            .min(frame.size.width - px(TIP_WIDTH + 8.))
            .max(px(8.));
        let below = hit.bounds.bottom() - frame.top() + px(4.);
        let top = if below + px(height) > frame.size.height - px(8.) {
            (hit.bounds.top() - frame.top() - px(height + 4.)).max(px(8.))
        } else {
            below
        };
        Some(
            div()
                .absolute()
                .left(left)
                .top(top)
                .max_w(px(TIP_WIDTH))
                .px_2()
                .py_1()
                .flex()
                .flex_col()
                .rounded_md()
                .bg(crate::theme::pane_fill())
                .border_1()
                .border_color(crate::theme::content(0.14))
                .font_family(crate::fonts::mono())
                .text_size(px(12.))
                .text_color(crate::theme::content(1.))
                .children(rows.into_iter().map(|(kind, text)| {
                    div()
                        .when(kind != TipRow::Summary, |d| {
                            d.text_color(crate::theme::content(0.4))
                        })
                        .when(kind == TipRow::Hint, |d| d.text_size(px(11.)))
                        .when(kind == TipRow::Summary, |d| d.my_1())
                        .child(text)
                }))
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, WindowBounds, WindowHandle, WindowOptions};
    use std::{cell::RefCell, rc::Rc};

    const NOW: i64 = 1_781_524_800_000;
    const DOC: &str = "one\ntwo\nthree\nfour\nfive\nsix";

    fn hunk(sha: &str, start: usize, count: usize, uncommitted: bool) -> BlameHunk {
        BlameHunk {
            sha: format!("{sha:0<40}"),
            short_sha: sha.chars().take(7).collect(),
            author: format!("author-{sha}"),
            author_time: NOW / 1000 - 2 * 86400,
            summary: format!("summary {sha}"),
            uncommitted,
            start_line: start,
            line_count: count,
        }
    }
    fn hunks() -> Vec<BlameHunk> {
        vec![
            hunk("aaa1111", 1, 2, false),
            hunk("bbb2222", 3, 3, false),
            hunk("0", 6, 1, true),
        ]
    }
    fn editor(cx: &mut TestAppContext) -> WindowHandle<Input> {
        let window = cx.update(|cx| {
            bind(cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        point(px(0.), px(0.)),
                        size(px(600.), px(300.)),
                    ))),
                    ..Default::default()
                },
                |_, cx| cx.new(Input::editor),
            )
            .unwrap()
        });
        window
            .update(cx, |i, w, cx| {
                w.focus(&i.focus, cx);
                i.set_text(DOC.into(), cx);
                i.move_to(0, cx);
            })
            .unwrap();
        window
    }
    fn on_line(i: &mut Input, line: usize, cx: &mut Context<Input>) {
        let at = DOC.split('\n').take(line - 1).map(|l| l.len() + 1).sum();
        i.select_range(at..at, cx);
    }

    #[gpui::test]
    fn the_column_widens_the_gutter_only_while_it_is_on(cx: &mut TestAppContext) {
        let window = editor(cx);
        window
            .update(cx, |i, _, cx| {
                let plain = i.code.as_ref().unwrap().gutter();
                // Still loading: the column holds its width.
                i.set_editor_blame_options(true, true, cx);
                assert_eq!(i.code.as_ref().unwrap().gutter(), plain + BLAME_COLUMN);
                i.set_editor_blame(Some(&hunks()), cx);
                assert_eq!(i.code.as_ref().unwrap().gutter(), plain + BLAME_COLUMN);
                // Known to have none: it collapses.
                i.set_editor_blame(Some(&[]), cx);
                assert_eq!(i.code.as_ref().unwrap().gutter(), plain);
                // Back on, but the column itself is off.
                i.set_editor_blame(Some(&hunks()), cx);
                i.set_editor_blame_options(true, false, cx);
                assert_eq!(i.code.as_ref().unwrap().gutter(), plain);
                // The diff view has its own gutter.
                i.set_editor_blame_options(true, true, cx);
                i.set_editor_diff(true, false);
                let diff = i.code.as_ref().unwrap().gutter();
                i.set_editor_blame_options(true, false, cx);
                assert_eq!(i.code.as_ref().unwrap().gutter(), diff);
            })
            .unwrap();
    }

    #[gpui::test]
    fn inline_text_shows_on_the_cursor_line(cx: &mut TestAppContext) {
        let window = editor(cx);
        window
            .update(cx, |i, _, cx| {
                i.set_editor_blame_options(true, false, cx);
                // Not loaded yet.
                assert_eq!(i.blame_inline(true, NOW), None);
                i.set_editor_blame(Some(&hunks()), cx);
                on_line(i, 4, cx);
                assert_eq!(
                    i.blame_inline(true, NOW).unwrap().0,
                    "author-bbb2222, 2 days ago \u{2022} summary bbb2222"
                );
                on_line(i, 6, cx);
                assert_eq!(i.blame_inline(true, NOW).unwrap().0, "Not committed yet");
                // Hidden while a range is selected, without focus, or when off.
                i.select_range(0..5, cx);
                assert_eq!(i.blame_inline(true, NOW), None);
                on_line(i, 4, cx);
                assert_eq!(i.blame_inline(false, NOW), None);
                i.set_editor_blame_options(false, true, cx);
                assert_eq!(i.blame_inline(true, NOW), None);
            })
            .unwrap();
    }

    #[gpui::test]
    fn an_untracked_file_shows_no_blame(cx: &mut TestAppContext) {
        let window = editor(cx);
        window
            .update(cx, |i, _, cx| {
                i.set_editor_blame_options(true, true, cx);
                let plain = {
                    i.set_editor_blame_options(true, false, cx);
                    i.code.as_ref().unwrap().gutter()
                };
                i.set_editor_blame_options(true, true, cx);
                // The core returns no hunks for untracked, binary and oversized files.
                i.set_editor_blame(Some(&[]), cx);
                on_line(i, 2, cx);
                assert_eq!(i.blame_inline(true, NOW), None);
                assert_eq!(i.code.as_ref().unwrap().gutter(), plain);
            })
            .unwrap();
    }

    #[gpui::test]
    fn edits_keep_authorship_on_the_right_lines(cx: &mut TestAppContext) {
        let window = editor(cx);
        window
            .update(cx, |i, _, cx| {
                i.set_editor_blame_options(true, true, cx);
                i.set_editor_blame(Some(&hunks()), cx);
                // Enter at the start of line 3 leaves a blank unblamed line.
                let at = "one\ntwo\n".len();
                i.replace_bytes(at..at, "\n", 0, cx);
                let who = |i: &Input, line| {
                    i.code
                        .as_ref()
                        .unwrap()
                        .blame
                        .hunk(line)
                        .map(|h| h.author.clone())
                };
                assert_eq!(who(i, 2).as_deref(), Some("author-aaa1111"));
                assert_eq!(who(i, 3), None);
                assert_eq!(who(i, 4).as_deref(), Some("author-bbb2222"));
            })
            .unwrap();
    }

    #[gpui::test]
    fn painted_cells_hover_and_open_the_commit(cx: &mut TestAppContext) {
        let window = editor(cx);
        let opened = Rc::new(RefCell::new(Vec::<String>::new()));
        let input = window.root(cx).unwrap();
        let log = opened.clone();
        cx.update(|cx| {
            cx.subscribe(&input, move |_, event: &CodeAction, _| {
                if let CodeAction::OpenCommit(commit) = event {
                    log.borrow_mut().push(commit.sha[..7].to_owned());
                }
            })
            .detach();
        });
        window
            .update(cx, |i, _, cx| {
                i.set_editor_blame_options(true, true, cx);
                i.set_editor_blame(Some(&hunks()), cx);
                on_line(i, 4, cx);
            })
            .unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        let (first, uncommitted, inline) = window
            .update(visual, |i, _, _| {
                let hits = &i.code.as_ref().unwrap().blame_hits;
                let column: Vec<_> = hits.iter().filter(|h| !h.inline).collect();
                assert_eq!(column.len(), 6, "one cell per line");
                let inline = hits.iter().find(|h| h.inline).expect("cursor line text");
                assert_eq!(inline.hunk, Some(1));
                (
                    column[0].bounds.center(),
                    column[5].bounds.center(),
                    inline.bounds.center(),
                )
            })
            .unwrap();
        // Hovering a cell shows the card; leaving it hides the card.
        visual.simulate_mouse_move(first, None, Default::default());
        window
            .update(visual, |i, _, _| {
                assert!(i.code.as_ref().unwrap().hovered_blame.is_some());
                assert!(i.blame_tooltip().is_some());
            })
            .unwrap();
        visual.simulate_mouse_move(point(px(300.), px(200.)), None, Default::default());
        window
            .update(visual, |i, _, _| {
                assert!(i.code.as_ref().unwrap().hovered_blame.is_none());
            })
            .unwrap();
        // A committed cell and the inline text open their commit; an
        // uncommitted cell does nothing and the caret stays where it was.
        let caret = window.update(visual, |i, _, _| i.cursor()).unwrap();
        visual.simulate_click(first, Default::default());
        visual.simulate_click(inline, Default::default());
        visual.simulate_click(uncommitted, Default::default());
        assert_eq!(*opened.borrow(), ["aaa1111", "bbb2222"]);
        window
            .update(visual, |i, _, _| assert_eq!(i.cursor(), caret))
            .unwrap();
    }

    #[gpui::test]
    fn the_card_goes_away_when_what_is_under_the_pointer_is_replaced(cx: &mut TestAppContext) {
        let window = editor(cx);
        window
            .update(cx, |i, _, cx| {
                i.set_editor_blame_options(true, true, cx);
                i.set_editor_blame(Some(&hunks()), cx);
                on_line(i, 4, cx);
            })
            .unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        // Fresh blame, an edit, or the cursor moving off the annotated line.
        for change in 0..3 {
            let cell = window
                .update(visual, |i, _, _| {
                    i.code.as_ref().unwrap().blame_hits[0].bounds.center()
                })
                .unwrap();
            visual.simulate_mouse_move(cell, None, Default::default());
            window
                .update(visual, |i, _, cx| {
                    assert!(i.blame_tooltip().is_some(), "case {change}");
                    match change {
                        0 => i.set_editor_blame(Some(&hunks()), cx),
                        1 => i.replace_bytes(0..0, "x", 1, cx),
                        _ => on_line(i, 5, cx),
                    }
                })
                .unwrap();
            visual.run_until_parked();
            window
                .update(visual, |i, _, _| {
                    assert!(i.blame_tooltip().is_none(), "case {change}")
                })
                .unwrap();
        }
    }

    const CODE: &str = "fn a() {\n    x();\n    y();\n}";

    /// One hunk per line, authors a, b, c, ...
    fn per_line(text: &str) -> Vec<BlameHunk> {
        text.split('\n')
            .enumerate()
            .map(|(i, _)| {
                let name = char::from(b'a' + i as u8).to_string();
                BlameHunk {
                    author: format!("author-{name}"),
                    ..hunk(&name, i + 1, 1, false)
                }
            })
            .collect()
    }
    fn blamed_editor(cx: &mut TestAppContext, text: &str) -> WindowHandle<Input> {
        let window = editor(cx);
        window
            .update(cx, |i, _, cx| {
                i.set_text(text.into(), cx);
                i.set_editor_blame_options(true, true, cx);
                i.set_editor_blame(Some(&per_line(text)), cx);
            })
            .unwrap();
        window
    }
    /// The author letter of every line, `-` for a line with none.
    fn authors(i: &Input) -> String {
        let code = i.code.as_ref().unwrap();
        (1..=i.editor_line_count())
            .map(|n| {
                code.blame.hunk(n).map_or('-', |h| {
                    h.author.chars().last().unwrap().to_ascii_uppercase()
                })
            })
            .collect()
    }
    fn offset_of(text: &str, needle: &str) -> usize {
        text.find(needle).unwrap()
    }

    #[gpui::test]
    fn enter_at_the_end_of_an_indented_line_leaves_the_new_line_unblamed(cx: &mut TestAppContext) {
        let window = blamed_editor(cx, CODE);
        window
            .update(cx, |i, _, cx| {
                let at = offset_of(CODE, "x();") + 4;
                i.select_range(at..at, cx);
            })
            .unwrap();
        cx.simulate_keystrokes(window.into(), "enter");
        window
            .update(cx, |i, _, _| {
                assert_eq!(i.content.as_ref(), "fn a() {\n    x();\n    \n    y();\n}");
                assert_eq!(authors(i), "AB-CD");
            })
            .unwrap();
        // Undo goes through the diff and takes the blank line out again.
        let m = if cfg!(target_os = "macos") {
            "cmd"
        } else {
            "ctrl"
        };
        cx.simulate_keystrokes(window.into(), &format!("{m}-z"));
        window
            .update(cx, |i, _, _| {
                assert_eq!(i.content.as_ref(), CODE);
                assert_eq!(authors(i), "ABCD");
            })
            .unwrap();
    }

    #[gpui::test]
    fn deleting_a_whole_line_leaves_the_next_line_its_own_author(cx: &mut TestAppContext) {
        use gpui::EntityInputHandler;
        let window = blamed_editor(cx, CODE);
        window
            .update(cx, |i, w, cx| {
                let from = offset_of(CODE, "    x();");
                i.select_range(from..from + "    x();\n".len(), cx);
                i.replace_text_in_range(None, "", w, cx);
                assert_eq!(i.content.as_ref(), "fn a() {\n    y();\n}");
                assert_eq!(authors(i), "ACD");
            })
            .unwrap();
        // Without indentation: "three" keeps its own hunk, not "two"'s.
        let plain = "one\ntwo\nthree";
        let window = blamed_editor(cx, plain);
        window
            .update(cx, |i, w, cx| {
                let from = offset_of(plain, "two");
                i.select_range(from..from + 4, cx);
                i.replace_text_in_range(None, "", w, cx);
                assert_eq!(i.content.as_ref(), "one\nthree");
                assert_eq!(authors(i), "AC");
            })
            .unwrap();
    }

    #[gpui::test]
    fn a_pasted_line_that_starts_like_the_next_one_is_unblamed(cx: &mut TestAppContext) {
        use gpui::EntityInputHandler;
        let window = blamed_editor(cx, CODE);
        window
            .update(cx, |i, w, cx| {
                let at = offset_of(CODE, "    y();");
                i.select_range(at..at, cx);
                i.replace_text_in_range(None, "    z();\n", w, cx);
                assert_eq!(
                    i.content.as_ref(),
                    "fn a() {\n    x();\n    z();\n    y();\n}"
                );
                assert_eq!(authors(i), "AB-CD");
            })
            .unwrap();
    }

    #[gpui::test]
    fn rewriting_the_buffer_maps_each_changed_place_on_its_own(cx: &mut TestAppContext) {
        // Replace all, or Format without saving, rewrites the whole buffer.
        let text = "foo\nb\nc\nfoo\nd";
        let window = blamed_editor(cx, text);
        window
            .update(cx, |i, _, cx| {
                i.replace_bytes(0..text.len(), "foo\nbar\nb\nc\nfoo\nbar\nd", 0, cx);
                assert_eq!(authors(i), "A-BCD-E");
            })
            .unwrap();
    }

    #[gpui::test]
    fn indenting_and_outdenting_a_selection_keeps_every_lines_author(cx: &mut TestAppContext) {
        let text = "one\ntwo\nthree\nfour\nfive\nsix";
        let window = blamed_editor(cx, text);
        window
            .update(cx, |i, _, cx| {
                i.select_range(offset_of(text, "two")..offset_of(text, "five") + 4, cx);
            })
            .unwrap();
        cx.simulate_keystrokes(window.into(), "tab");
        window
            .update(cx, |i, _, _| {
                assert_eq!(
                    i.content.as_ref(),
                    "one\n    two\n    three\n    four\n    five\nsix"
                );
                assert_eq!(authors(i), "ABCDEF");
            })
            .unwrap();
        cx.simulate_keystrokes(window.into(), "shift-tab");
        window
            .update(cx, |i, _, _| {
                assert_eq!(i.content.as_ref(), text);
                assert_eq!(authors(i), "ABCDEF");
            })
            .unwrap();
    }

    /// A line that becomes exactly another old line's text once indented (a
    /// closing brace) must still keep its own author.
    #[gpui::test]
    fn indenting_lines_that_become_copies_of_other_lines_keeps_each_author(
        cx: &mut TestAppContext,
    ) {
        let text = "    if a {\n        b();\n    }\n}";
        let window = blamed_editor(cx, text);
        window
            .update(cx, |i, _, cx| i.select_range(0..text.len(), cx))
            .unwrap();
        // The last brace becomes a copy of the old third line.
        cx.simulate_keystrokes(window.into(), "tab");
        window
            .update(cx, |i, _, _| {
                assert_eq!(
                    i.content.as_ref(),
                    "        if a {\n            b();\n        }\n    }"
                );
                assert_eq!(authors(i), "ABCD");
            })
            .unwrap();
        cx.simulate_keystrokes(window.into(), "tab");
        window
            .update(cx, |i, _, _| {
                assert_eq!(
                    i.content.as_ref(),
                    "            if a {\n                b();\n            }\n        }"
                );
                assert_eq!(authors(i), "ABCD");
            })
            .unwrap();
    }

    #[gpui::test]
    fn outdenting_lines_that_become_copies_of_other_lines_keeps_each_author(
        cx: &mut TestAppContext,
    ) {
        let text = "        x\n    x";
        let window = blamed_editor(cx, text);
        window
            .update(cx, |i, _, cx| i.select_range(0..text.len(), cx))
            .unwrap();
        cx.simulate_keystrokes(window.into(), "shift-tab");
        window
            .update(cx, |i, _, _| {
                assert_eq!(i.content.as_ref(), "    x\nx");
                assert_eq!(authors(i), "AB");
            })
            .unwrap();
        // Outdenting a line with nothing to remove leaves its author alone.
        cx.simulate_keystrokes(window.into(), "shift-tab");
        window
            .update(cx, |i, _, _| {
                assert_eq!(i.content.as_ref(), "x\nx");
                assert_eq!(authors(i), "AB");
            })
            .unwrap();
    }
}
