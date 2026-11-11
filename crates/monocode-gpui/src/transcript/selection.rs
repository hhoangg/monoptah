//! Select the rendered text, retaining byte anchors instead of shaped history.
use super::*;
use gpui::{
    AnyElement, App, Element, ElementId, GlobalElementId, InspectorElementId, LayoutId,
    MouseButton, Point, TextLayout,
};
use std::{cell::RefCell, ops::Range, rc::Rc};
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Position {
    row: (usize, usize),
    byte: usize,
}
#[derive(Clone, Copy)]
enum Granularity {
    Character,
    Word,
    Paragraph,
}
struct Drag {
    origin: Point<Pixels>,
    anchor: Range<Position>,
    granularity: Granularity,
    moved: bool,
    /// Pressed outside any text run: the anchor is resolved to the nearest
    /// text position once the pointer starts dragging, so a plain click on
    /// a margin, gap or control selects nothing.
    pending: bool,
}
#[derive(Default)]
struct State {
    anchor: Option<Position>,
    head: Option<Position>,
    response: usize,
    quote: bool,
    drag: Option<Drag>,
    menu: Option<Point<Pixels>>,
    layouts: HashMap<((usize, usize), usize, usize), Fragment>,
}
struct Fragment {
    row: (usize, usize),
    response: usize,
    offset: usize,
    quote: bool,
    text: SharedString,
    row_text: SharedString,
    layout: TextLayout,
}
/// The text position closest to `point`, as a browser resolves a press
/// beside the text: the nearest run vertically, then the line and character
/// under the point (a line's start or end when the point lies left or right
/// of it, the run's start or end when the point lies above or below it).
fn nearest(
    layouts: &HashMap<((usize, usize), usize, usize), Fragment>,
    response: Option<usize>,
    point: Point<Pixels>,
) -> Option<(&Fragment, usize)> {
    let fragment = layouts
        .values()
        .filter(|f| response.is_none_or(|r| f.response == r))
        .min_by(|a, b| {
            let distance = |f: &Fragment| {
                let bounds = f.layout.bounds();
                let dx = (bounds.left() - point.x)
                    .max(px(0.))
                    .max(point.x - bounds.right());
                let dy = (bounds.top() - point.y)
                    .max(px(0.))
                    .max(point.y - bounds.bottom());
                f32::from(dy) * 10000. + f32::from(dx)
            };
            distance(a)
                .total_cmp(&distance(b))
                // Deterministic among equals: the earlier text wins.
                .then((a.row, a.offset).cmp(&(b.row, b.offset)))
        })?;
    // Inside the run's vertical span this resolves the line under the
    // point; above or below it, the run's start or end. So a drag that
    // leaves the text vertically keeps boundary semantics (through the end
    // of the last line, or back to the start of the first), and a point in
    // the gap between two runs falls to the nearer run's edge.
    let byte = fragment
        .layout
        .index_for_position(point)
        .unwrap_or_else(|ix| ix);
    Some((fragment, byte))
}
/// The selection state of one selectable surface: the transcript, or any
/// other view that renders Markdown rows (the notes preview).
#[derive(Clone, Default)]
pub(crate) struct Selection(Rc<RefCell<State>>);
/// Runs after a press on a text run: focuses the owning view so Copy and
/// Escape reach it, and asks it to redraw.
type Pressed = Rc<dyn Fn(&mut Window, &mut App)>;
#[derive(Clone)]
pub(crate) struct TextSelection {
    state: Selection,
    pressed: Pressed,
    row: (usize, usize),
    response: usize,
    offset: usize,
    quote: bool,
    row_text: Option<SharedString>,
    cursor: gpui::CursorStyle,
}
impl Selection {
    pub(super) fn hide_menu(&self) {
        let mut state = self.0.borrow_mut();
        state.menu = None;
        state.drag = None;
    }
    /// A left press that no text run claimed (a margin, the gap between
    /// lines or blocks, beside a short line). Arms a pending drag; the
    /// selection starts at the nearest text once the pointer moves.
    pub(super) fn press_outside(&self, event: &gpui::MouseDownEvent) {
        if self.dragging() {
            // A text run already started a selection for this press.
            return;
        }
        self.clear();
        if event.click_count != 1 || event.modifiers.shift {
            return;
        }
        let origin = Position {
            row: (0, 0),
            byte: 0,
        };
        self.0.borrow_mut().drag = Some(Drag {
            origin: event.position,
            anchor: origin..origin,
            granularity: Granularity::Character,
            moved: false,
            pending: true,
        });
    }
    /// A margin press is waiting for the pointer to move.
    pub(super) fn pending(&self) -> bool {
        self.0
            .borrow()
            .drag
            .as_ref()
            .is_some_and(|drag| drag.pending)
    }
    pub(super) fn active(&self) -> bool {
        let state = self.0.borrow();
        state.drag.is_some() || state.menu.is_some() || state.anchor != state.head
    }
    #[cfg(test)]
    pub(super) fn mark_for_test(&self) {
        let mut state = self.0.borrow_mut();
        state.anchor = Some(Position {
            row: (0, 0),
            byte: 0,
        });
        state.head = Some(Position {
            row: (0, 0),
            byte: 3,
        });
    }
    pub(crate) fn clear(&self) {
        let mut state = self.0.borrow_mut();
        state.anchor = None;
        state.head = None;
        state.drag = None;
        state.menu = None;
        state.layouts.clear();
    }
    pub(crate) fn begin_frame(&self) {
        self.0.borrow_mut().layouts.clear();
    }
    pub(crate) fn dragging(&self) -> bool {
        self.0.borrow().drag.is_some()
    }
    /// Select from the start of `first` to `end` bytes into `last`, within
    /// one response (a document has a single one).
    pub(crate) fn select_rows(
        &self,
        response: usize,
        first: (usize, usize),
        last: (usize, usize),
        end: usize,
    ) {
        let mut state = self.0.borrow_mut();
        state.response = response;
        state.anchor = Some(Position {
            row: first,
            byte: 0,
        });
        state.head = Some(Position {
            row: last,
            byte: end,
        });
        state.drag = None;
        state.menu = None;
    }
    /// The selected text of `rows` (key, response, selectable text), in order.
    pub(crate) fn text_of(
        &self,
        rows: impl IntoIterator<Item = ((usize, usize), usize, String)>,
    ) -> Option<String> {
        let state = self.0.borrow();
        let (a, b) = (state.anchor?, state.head?);
        if a == b {
            return None;
        }
        let (a, b) = if a < b { (a, b) } else { (b, a) };
        let mut parts = Vec::new();
        for (key, response, text) in rows {
            if key < a.row || key > b.row || response != state.response {
                continue;
            }
            let start = if key == a.row { snap(&text, a.byte) } else { 0 };
            let end = if key == b.row {
                snap(&text, b.byte)
            } else {
                text.len()
            };
            if start <= end {
                parts.push(text[start..end].to_owned())
            }
        }
        let text = parts.join("\n").trim().to_owned();
        (!text.is_empty()).then_some(text)
    }
    /// Extend a drag to `point`, as the transcript's `move_selection` does.
    /// True when the selection changed and the view should redraw.
    pub(crate) fn drag_to(&self, point: Point<Pixels>, pressed: bool) -> bool {
        let mut state = self.0.borrow_mut();
        let Some(drag) = state.drag.as_mut() else {
            return false;
        };
        if !pressed {
            state.drag = None;
            return false;
        }
        if !drag.moved {
            let delta = point - drag.origin;
            if delta.x.abs().max(delta.y.abs()) < px(3.) {
                return false;
            }
            drag.moved = true;
        }
        let started = drag.pending;
        if started {
            let origin = drag.origin;
            let Some((fragment, byte)) = nearest(&state.layouts, None, origin) else {
                return false;
            };
            let at = Position {
                row: fragment.row,
                byte: snap(&fragment.row_text, fragment.offset + byte),
            };
            let (response, quote) = (fragment.response, fragment.quote);
            let drag = state.drag.as_mut().unwrap();
            drag.pending = false;
            drag.anchor = at..at;
            state.response = response;
            state.quote = quote;
            state.anchor = Some(at);
            state.head = Some(at);
            state.menu = None;
        }
        let drag = state.drag.as_ref().unwrap();
        let (anchor, granularity, response) =
            (drag.anchor.clone(), drag.granularity, state.response);
        let Some((fragment, byte)) = nearest(&state.layouts, Some(response), point) else {
            return false;
        };
        let range = unit(&fragment.row_text, fragment.offset + byte, granularity);
        let row = fragment.row;
        let (start, end) = (
            Position {
                row,
                byte: range.start,
            },
            Position {
                row,
                byte: range.end,
            },
        );
        let (anchor, head) = if start < anchor.start {
            (anchor.end, start)
        } else {
            (anchor.start, end)
        };
        if state.anchor == Some(anchor) && state.head == Some(head) {
            return started;
        }
        state.anchor = Some(anchor);
        state.head = Some(head);
        true
    }
    /// End a drag. True when one was in progress.
    pub(crate) fn finish_drag(&self) -> bool {
        self.0.borrow_mut().drag.take().is_some()
    }
    fn range(
        &self,
        row: (usize, usize),
        response: usize,
        offset: usize,
        len: usize,
    ) -> Range<usize> {
        let s = self.0.borrow();
        let (Some(a), Some(b)) = (s.anchor, s.head) else {
            return 0..0;
        };
        if response != s.response {
            return 0..0;
        }
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        if row < a.row || row > b.row {
            return 0..0;
        }
        let start = if row == a.row {
            a.byte.saturating_sub(offset).min(len)
        } else {
            0
        };
        let end = if row == b.row {
            b.byte.saturating_sub(offset).min(len)
        } else {
            len
        };
        start.min(end)..end
    }
}
const MENU_DELAY: std::time::Duration = std::time::Duration::from_millis(300);
/// The top centre of the selection's first line, as React anchors to the
/// range's first client rect rather than the pointer.
fn menu_anchor(state: &State) -> Option<Point<Pixels>> {
    let (a, b) = (state.anchor?, state.head?);
    let (start, end) = if a < b { (a, b) } else { (b, a) };
    let fragment = state.layouts.values().find(|f| {
        f.response == state.response
            && f.row == start.row
            && f.offset <= start.byte
            && start.byte < f.offset + f.text.len()
    })?;
    let top = fragment
        .layout
        .position_for_index(start.byte - fragment.offset)?;
    let right = (end.row == start.row && end.byte <= fragment.offset + fragment.text.len())
        .then(|| {
            fragment
                .layout
                .position_for_index(end.byte - fragment.offset)
        })
        .flatten()
        .filter(|p| p.y == top.y)
        .map_or(fragment.layout.bounds().right(), |p| p.x);
    Some(gpui::point((top.x + right) / 2., top.y))
}
fn snap(text: &str, byte: usize) -> usize {
    if byte >= text.len() {
        return text.len();
    }
    text.grapheme_indices(true)
        .map(|(ix, _)| ix)
        .take_while(|ix| *ix <= byte)
        .last()
        .unwrap_or(0)
}
fn word(text: &str, byte: usize) -> Range<usize> {
    let at = if byte >= text.len() {
        text.grapheme_indices(true)
            .next_back()
            .map_or(0, |(ix, _)| ix)
    } else {
        snap(text, byte)
    };
    text.split_word_bound_indices()
        .find(|(ix, s)| *ix <= at && at < *ix + s.len())
        .map(|(ix, s)| ix..ix + s.len())
        .unwrap_or(at..at)
}
fn unit(text: &str, byte: usize, granularity: Granularity) -> Range<usize> {
    let byte = snap(text, byte);
    match granularity {
        Granularity::Character => byte..byte,
        Granularity::Word => word(text, byte),
        Granularity::Paragraph => {
            let start = text[..byte].rfind(['\n', '\t']).map_or(0, |ix| ix + 1);
            let end = text[byte..]
                .find(['\n', '\t'])
                .map_or(text.len(), |ix| byte + ix);
            start..end
        }
    }
}
impl TextSelection {
    /// A run of row `row` in a single-response surface outside the
    /// transcript. `pressed` focuses and redraws the owning view.
    pub(crate) fn new(
        state: &Selection,
        row: (usize, usize),
        pressed: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            state: state.clone(),
            pressed: Rc::new(pressed),
            row,
            response: 0,
            offset: 0,
            quote: false,
            row_text: None,
            cursor: gpui::CursorStyle::IBeam,
        }
    }
    pub(crate) fn with_text(mut self, text: SharedString) -> Self {
        self.row_text = Some(text);
        self
    }
    pub(crate) fn with_cursor(mut self, cursor: gpui::CursorStyle) -> Self {
        self.cursor = cursor;
        self
    }
    pub(crate) fn offset(&self, offset: usize) -> Self {
        let mut next = self.clone();
        next.offset += offset;
        next
    }
    pub(crate) fn has_selection(&self) -> bool {
        let state = self.state.0.borrow();
        state.anchor != state.head || state.drag.as_ref().is_some_and(|drag| drag.moved)
    }
    pub(crate) fn wrap(
        &self,
        text: SharedString,
        layout: TextLayout,
        child: AnyElement,
    ) -> AnyElement {
        let selection = self.clone();
        let click_layout = layout.clone();
        let click_text = text.clone();
        let row_text = self.row_text.clone().unwrap_or_else(|| text.clone());
        div()
            .relative()
            .cursor(self.cursor)
            .on_mouse_down(MouseButton::Left, move |event, window, cx| {
                let byte = click_layout
                    .index_for_position(event.position)
                    .unwrap_or_else(|ix| ix);
                let byte = snap(&click_text, byte);
                let granularity = if event.click_count >= 3 {
                    Granularity::Paragraph
                } else if event.click_count == 2 {
                    Granularity::Word
                } else {
                    Granularity::Character
                };
                let range = unit(&row_text, selection.offset + byte, granularity);
                {
                    let mut state = selection.state.0.borrow_mut();
                    let start = Position {
                        row: selection.row,
                        byte: range.start,
                    };
                    let end = Position {
                        row: selection.row,
                        byte: range.end,
                    };
                    let extend = event.modifiers.shift
                        && state.anchor.is_some()
                        && state.response == selection.response;
                    let anchor = if extend {
                        let anchor = state.anchor.unwrap();
                        anchor..anchor
                    } else {
                        start..end
                    };
                    state.anchor = Some(anchor.start);
                    state.head = Some(if start < anchor.start { start } else { end });
                    state.response = selection.response;
                    state.quote = selection.quote;
                    state.drag = Some(Drag {
                        origin: event.position,
                        anchor,
                        granularity,
                        moved: false,
                        pending: false,
                    });
                    state.menu = None;
                    drop(state);
                    (selection.pressed)(window, cx);
                }
            })
            .child(SelectedText {
                context: self.clone(),
                text,
                layout,
                child,
            })
            .into_any_element()
    }
}
struct SelectedText {
    context: TextSelection,
    text: SharedString,
    layout: TextLayout,
    child: AnyElement,
}
impl IntoElement for SelectedText {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for SelectedText {
    type RequestLayoutState = ();
    type PrepaintState = ();
    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.prepaint(window, cx);
        self.context.state.0.borrow_mut().layouts.insert(
            (self.context.row, self.context.response, self.context.offset),
            Fragment {
                row: self.context.row,
                response: self.context.response,
                offset: self.context.offset,
                quote: self.context.quote,
                text: self.text.clone(),
                row_text: self
                    .context
                    .row_text
                    .clone()
                    .unwrap_or_else(|| self.text.clone()),
                layout: self.layout.clone(),
            },
        );
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let selected = self.context.state.range(
            self.context.row,
            self.context.response,
            self.context.offset,
            self.text.len(),
        );
        if !selected.is_empty() {
            let bounds = self.layout.bounds();
            let height = self.layout.line_height();
            if let (Some(a), Some(b)) = (
                self.layout.position_for_index(selected.start),
                self.layout.position_for_index(selected.end),
            ) {
                let mut y = a.y;
                while y <= b.y {
                    let left = if y == a.y { a.x } else { bounds.left() };
                    let right = if y == b.y { b.x } else { bounds.right() };
                    if right > left {
                        window.paint_quad(gpui::fill(
                            Bounds::new(gpui::point(left, y), size(right - left, height)),
                            crate::theme::text_selection(),
                        ))
                    }
                    y += height;
                }
            }
        }
        self.child.paint(window, cx);
    }
}
impl TranscriptView {
    pub(super) fn selection_turn(&self) -> Option<usize> {
        let state = self.selection.0.borrow();
        state.menu?;
        self.messages
            .iter()
            .find(|m| m.start == state.response)
            .map(|m| m.turn)
    }
    pub(super) fn text_selection(
        &self,
        row: (usize, usize),
        response: usize,
        quote: bool,
        cx: &Context<Self>,
    ) -> Option<TextSelection> {
        let view = cx.weak_entity();
        Some(TextSelection {
            state: self.selection.clone(),
            pressed: Rc::new(move |window, cx| {
                let _ = view.update(cx, |view, cx| {
                    if let Some(focus) = &view.selection_focus {
                        window.focus(focus, cx)
                    }
                    cx.notify();
                });
            }),
            row,
            response,
            offset: 0,
            // A document has no chat or note to add the selection to; its
            // menu offers Copy only.
            quote: quote && !self.document,
            row_text: None,
            cursor: gpui::CursorStyle::IBeam,
        })
    }
    pub(super) fn move_selection(
        &mut self,
        event: &gpui::MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pending = self.selection.pending();
        let pressed = event.pressed_button == Some(MouseButton::Left);
        let changed = self.selection.drag_to(event.position, pressed);
        // A margin press became a selection: take focus so Copy and Escape
        // reach this view, as a press on a text run does.
        if pending
            && !self.selection.pending()
            && self.selection.dragging()
            && let Some(focus) = &self.selection_focus
        {
            window.focus(focus, cx)
        }
        if changed {
            cx.notify()
        }
    }
    pub(super) fn finish_selection(
        &mut self,
        event: &gpui::MouseUpEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut state = self.selection.0.borrow_mut();
        if state.drag.take().is_none() {
            return;
        }
        state.menu = None;
        let (anchor, head) = (state.anchor, state.head);
        drop(state);
        cx.notify();
        if anchor == head {
            return;
        }
        // Wait out a quick select-and-go before offering the menu, and drop
        // it if the selection moved on in the meantime.
        let fallback = event.position;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(MENU_DELAY).await;
            let _ = this.update(cx, |this, cx| {
                let mut state = this.selection.0.borrow_mut();
                if state.drag.is_some() || state.anchor != anchor || state.head != head {
                    return;
                }
                state.menu = Some(menu_anchor(&state).unwrap_or(fallback));
                drop(state);
                cx.notify();
            });
        })
        .detach();
    }
    fn selectable_rows(&self) -> impl Iterator<Item = ((usize, usize), usize, String)> + '_ {
        self.rows.iter().filter_map(|row| {
            let text = match &row.body {
                Body::Markdown(md) => crate::markdown::selection_text(md),
                Body::Plain(text) => text.to_string(),
                Body::Interjection(note) => note.text.clone(),
                _ => return None,
            };
            Some((row.key, self.messages[row.message].start, text))
        })
    }
    pub(super) fn selected_text(&self) -> Option<String> {
        self.selection.text_of(self.selectable_rows())
    }
    /// Select All in a document view selects the whole document. A chat
    /// keeps it unhandled: its selection never crosses responses.
    pub(super) fn select_all(
        &mut self,
        _: &crate::input::SelectAll,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rows: Vec<_> = self.selectable_rows().collect();
        let (Some(first), Some(last)) = (rows.first(), rows.last()) else {
            cx.propagate();
            return;
        };
        if !self.document {
            cx.propagate();
            return;
        }
        let (first, response, last, end) = (first.0, first.1, last.0, last.2.len());
        self.selection.select_rows(response, first, last, end);
        cx.notify();
    }
    pub(super) fn copy_selection(
        &mut self,
        _: &crate::input::Copy,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(text) = self.selected_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            cx.stop_propagation()
        }
    }
    pub(super) fn selection_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let state = self.selection.0.borrow();
        let position = state.menu?;
        let quote = state.quote;
        let response = state.response;
        drop(state);
        let text = self.selected_text()?;
        let copy = text.clone();
        let note = text.clone();
        let turn = self
            .messages
            .iter()
            .find(|m| m.start == response)
            .map_or(response, |m| m.turn);
        let button = |id: &'static str, label: &'static str, glyph: &'static str| {
            div()
                .id(id)
                .role(Role::Button)
                .aria_label(label)
                .flex()
                .items_center()
                .gap_2()
                .h(px(32.))
                .px_2p5()
                .rounded(px(8.))
                .text_size(px(13.))
                .hover(|s| s.bg(content(0.05)))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(icon(glyph, 14.))
                .child(label)
        };
        let panel = div()
            .id("transcript-selection-actions")
            .role(Role::Toolbar)
            .aria_label("Selected text actions")
            .p_1()
            .min_w(px(144.))
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(
                button("selection-copy", "Copy", "copy").on_click(cx.listener(
                    move |_, _, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()))
                    },
                )),
            )
            .when(quote, |d| {
                d.child(
                    button("selection-chat", "Add to chat", "comment-add").on_click(cx.listener(
                        move |this, _, window, cx| {
                            if this.mono {
                                cx.emit(MonoResponseAction::Quote {
                                    text: text.clone(),
                                    window: window.window_handle(),
                                })
                            } else {
                                cx.emit(ResponseRequest::QuoteSelection { text: text.clone() })
                            }
                            this.selection.clear();
                            cx.notify();
                        },
                    )),
                )
            })
            .when(quote, |d| {
                d.child(
                    button("selection-note", "Add to notes", "response-note").on_click(
                        cx.listener(move |this, _, _, cx| {
                            if this.note_pending.insert(turn) {
                                cx.emit(ResponseRequest::SaveNote {
                                    turn,
                                    text: note.clone(),
                                    selection: true,
                                });
                                cx.notify()
                            }
                        }),
                    ),
                )
            })
            .when_some(
                self.note_feedback
                    .get(&turn)
                    .and_then(|result| result.as_ref().err()),
                |d, error| {
                    d.child(
                        div()
                            .id("selection-note-error")
                            .role(Role::Alert)
                            .max_w(px(280.))
                            .px_2p5()
                            .py_1()
                            .text_size(px(11.))
                            .text_color(rgb(0xf87171))
                            .child(format!("Could not save note: {error}")),
                    )
                },
            );
        let weak = cx.weak_entity();
        Some(
            popover("transcript-selection", Anchor::Point(position))
                .allow_outside_press()
                .side(Side::Top)
                .align(crate::popover::Align::Center)
                .child(panel)
                .on_dismiss(move |_, cx| {
                    let _ = weak.update(cx, |this, cx| {
                        this.selection.0.borrow_mut().menu = None;
                        cx.notify()
                    });
                })
                .into_any_element(),
        )
    }
}

#[cfg(test)]
impl Selection {
    /// Whether the selection menu is open, and whether it offers quoting.
    pub(crate) fn menu_for_test(&self) -> (bool, bool) {
        let state = self.0.borrow();
        (state.menu.is_some(), state.quote)
    }
    /// The window point of `byte` in the drawn run of `row`, nudged into the
    /// line so a press there resolves to that byte.
    pub(crate) fn point_for_test(&self, row: (usize, usize), byte: usize) -> Point<Pixels> {
        let state = self.0.borrow();
        let fragment = state
            .layouts
            .values()
            .filter(|f| f.row == row && f.offset <= byte && byte <= f.offset + f.text.len())
            .min_by_key(|f| f.offset)
            .unwrap_or_else(|| panic!("row {row:?} byte {byte} is not drawn"));
        let at = fragment
            .layout
            .position_for_index(byte - fragment.offset)
            .unwrap();
        gpui::point(at.x + px(1.), at.y + fragment.layout.line_height() / 2.)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;
    use monocode_core::tool::{ToolCall, ToolState};
    #[gpui::test]
    fn rendered_selection_crosses_markdown_and_code_without_chrome_or_other_responses(
        cx: &mut TestAppContext,
    ) {
        let view = cx.new(|_| TranscriptView::new());
        view.update(cx,|view,cx|{
            view.patch(0,vec![Row{role:"assistant".into(),text:"Hello **café**.\n\n```rs\nlet x = 1;\n```\n\nTail".into(),first:true,sent_at:None},Row{role:"user".into(),text:"Next".into(),first:true,sent_at:None},Row{role:"assistant".into(),text:"Other response".into(),first:true,sent_at:None}],cx);
            let first=view.rows.iter().find(|row|matches!(&row.body,Body::Markdown(md) if crate::markdown::selection_text(md)=="Hello café.")).unwrap().key;
            let code=view.rows.iter().find(|row|matches!(&row.body,Body::Markdown(md) if matches!(md.part,Part::Code{..}))).unwrap().key;
            {let mut selection=view.selection.0.borrow_mut();selection.response=0;selection.quote=true;selection.anchor=Some(Position{row:first,byte:6});selection.head=Some(Position{row:code,byte:10});}
            assert_eq!(view.selected_text().as_deref(),Some("café.\nlet x = 1;"));
            {let mut selection=view.selection.0.borrow_mut();let anchor=selection.anchor;selection.anchor=selection.head;selection.head=anchor;}
            assert_eq!(view.selected_text().as_deref(),Some("café.\nlet x = 1;"));
            view.selection.clear();assert!(view.selected_text().is_none());
        });
    }
    #[test]
    fn selection_ranges_are_direction_independent_and_response_scoped() {
        let selection = Selection::default();
        {
            let mut state = selection.0.borrow_mut();
            state.response = 5;
            state.anchor = Some(Position {
                row: (5, 2),
                byte: 8,
            });
            state.head = Some(Position {
                row: (5, 0),
                byte: 3,
            });
        }
        assert_eq!(selection.range((5, 0), 5, 0, 10), 3..10);
        assert_eq!(selection.range((5, 1), 5, 0, 20), 0..20);
        assert_eq!(selection.range((5, 2), 5, 4, 20), 0..4);
        assert_eq!(selection.range((5, 1), 6, 0, 20), 0..0);
    }
    #[test]
    fn unicode_anchors_do_not_split_composed_graphemes() {
        assert_eq!(snap("e\u{301}🙂", 2), 0);
        assert_eq!(snap("e\u{301}🙂", 4), 3);
        assert_eq!(word("hello café", 7), 6..11);
    }
    fn open(cx: &mut TestAppContext, rows: Vec<Row>) -> gpui::WindowHandle<TranscriptView> {
        cx.update(|cx| cx.set_reduce_motion(true));
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|_| TranscriptView::new())
            })
            .unwrap()
        });
        window
            .update(cx, |view, _, cx| view.patch(0, rows, cx))
            .unwrap();
        window
    }
    /// Bounds of the rendered text runs, in reading order.
    fn runs(view: &TranscriptView) -> Vec<(String, Bounds<Pixels>)> {
        let state = view.selection.0.borrow();
        let mut runs: Vec<_> = state
            .layouts
            .values()
            .map(|f| ((f.row, f.offset), f.text.to_string(), f.layout.bounds()))
            .collect();
        runs.sort_by_key(|(key, ..)| *key);
        runs.into_iter().map(|(_, t, b)| (t, b)).collect()
    }
    fn drag(visual: &mut gpui::VisualTestContext, from: Point<Pixels>, to: Point<Pixels>) {
        let mid = gpui::point((from.x + to.x) / 2., (from.y + to.y) / 2.);
        visual.simulate_mouse_down(from, MouseButton::Left, Default::default());
        visual.simulate_mouse_move(mid, MouseButton::Left, Default::default());
        visual.simulate_mouse_move(to, MouseButton::Left, Default::default());
        visual.simulate_mouse_up(to, MouseButton::Left, Default::default());
        visual.run_until_parked();
    }
    fn paragraphs() -> Vec<Row> {
        vec![Row {
            role: "assistant".into(),
            text: "First paragraph here.\n\nSecond one.".into(),
            first: true,
            sent_at: None,
        }]
    }

    #[gpui::test]
    fn dragging_from_the_left_margin_selects_from_the_nearest_line_start(cx: &mut TestAppContext) {
        let window = open(cx, paragraphs());
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        let runs = window.update(visual, |view, _, _| runs(view)).unwrap();
        let first = runs
            .iter()
            .find(|(t, _)| t == "First paragraph here.")
            .unwrap()
            .1;
        let second = runs.iter().find(|(t, _)| t == "Second one.").unwrap().1;
        assert!(first.left() > px(4.), "the message has a left margin");
        // From the margin beside the first line to past the end of the second.
        let from = gpui::point(px(2.), first.center().y);
        let to = gpui::point(second.right() + px(40.), second.center().y);
        drag(visual, from, to);
        window
            .update(visual, |view, _, _| {
                assert_eq!(
                    view.selected_text().as_deref(),
                    Some("First paragraph here.\nSecond one.")
                );
            })
            .unwrap();
        // From the gap between the paragraphs, just above the second one,
        // back up into the first: the selection starts at the second line's
        // start, not mid-way through either paragraph.
        let gap = gpui::point(px(2.), second.top() - px(1.));
        let start = window
            .update(visual, |view, _, _| {
                view.selection
                    .0
                    .borrow()
                    .layouts
                    .values()
                    .find(|f| f.text.as_ref() == "First paragraph here.")
                    .unwrap()
                    .layout
                    .position_for_index(6)
                    .unwrap()
            })
            .unwrap();
        drag(visual, gap, start + gpui::point(px(1.), px(2.)));
        window
            .update(visual, |view, _, _| {
                assert_eq!(view.selected_text().as_deref(), Some("paragraph here."));
            })
            .unwrap();
    }

    #[gpui::test]
    fn a_plain_click_beside_the_text_selects_nothing_and_controls_still_click(
        cx: &mut TestAppContext,
    ) {
        let mut call = ToolCall::new("chain", "Bash");
        call.input = serde_json::json!({
            "command": "cd /repo && git status --short && cat package.json && sed -n '1,260p' scripts/build-native.mjs && sed -n '1,260p' scripts/dev-native.mjs",
        })
        .to_string();
        call.state = ToolState::Completed;
        let window = open(
            cx,
            vec![
                Row {
                    role: "user".into(),
                    text: "Find model settings".into(),
                    first: true,
                    sent_at: None,
                },
                Row {
                    role: "tool".into(),
                    text: serde_json::to_string(&call).unwrap(),
                    first: true,
                    sent_at: None,
                },
            ],
        );
        window
            .update(cx, |view, _, cx| {
                view.work_folds.insert(1, true);
                view.phases.insert(1, true);
                view.rebuild(0, cx);
            })
            .unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.simulate_resize(gpui::size(px(900.), px(600.)));
        visual.run_until_parked();
        // A click in the margin neither selects nor arms a lasting drag.
        visual.simulate_click(gpui::point(px(2.), px(2.)), Default::default());
        visual.run_until_parked();
        window
            .update(visual, |view, _, _| assert!(!view.has_selection()))
            .unwrap();
        assert!(visual.debug_bounds("tool-steps-1").is_none());
        let row = visual.debug_bounds("tool-1").unwrap();
        visual.simulate_click(
            gpui::point(row.right() - px(8.), row.center().y),
            Default::default(),
        );
        visual.run_until_parked();
        assert!(
            visual.debug_bounds("tool-steps-1").is_some(),
            "the tool row still expands"
        );
        window
            .update(visual, |view, _, _| assert!(!view.has_selection()))
            .unwrap();
    }

    /// Bounds of the two paragraphs.
    fn paragraph_bounds(
        visual: &mut gpui::VisualTestContext,
        window: gpui::WindowHandle<TranscriptView>,
    ) -> (Bounds<Pixels>, Bounds<Pixels>) {
        let runs = window.update(visual, |view, _, _| runs(view)).unwrap();
        let first = runs
            .iter()
            .find(|(t, _)| t == "First paragraph here.")
            .unwrap()
            .1;
        let second = runs.iter().find(|(t, _)| t == "Second one.").unwrap().1;
        (first, second)
    }
    fn selected(
        visual: &mut gpui::VisualTestContext,
        window: gpui::WindowHandle<TranscriptView>,
    ) -> Option<String> {
        window
            .update(visual, |view, _, _| view.selected_text())
            .unwrap()
    }

    #[gpui::test]
    fn dragging_past_the_text_vertically_selects_through_its_boundary(cx: &mut TestAppContext) {
        let window = open(cx, paragraphs());
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        let (first, second) = paragraph_bounds(visual, window);
        // Down from the first character to below the last line, near the
        // left text edge: through the end of the response, not that column.
        let start = gpui::point(first.left() + px(1.), first.center().y);
        drag(
            visual,
            start,
            gpui::point(second.left() + px(2.), second.bottom() + px(30.)),
        );
        assert_eq!(
            selected(visual, window).as_deref(),
            Some("First paragraph here.\nSecond one.")
        );
        // Up from past the last character to above the first line: back to
        // the start of the response.
        let end = gpui::point(second.right() - px(1.), second.center().y);
        let above = gpui::point(first.right() - px(40.), first.top() - px(1.));
        drag(visual, end, above);
        assert_eq!(
            selected(visual, window).as_deref(),
            Some("First paragraph here.\nSecond one.")
        );
        // Into the gap just under the first paragraph, near the left edge:
        // the whole first paragraph, not its first character.
        drag(
            visual,
            start,
            gpui::point(first.left() + px(2.), first.bottom() + px(1.)),
        );
        assert_eq!(
            selected(visual, window).as_deref(),
            Some("First paragraph here.")
        );
    }
}
