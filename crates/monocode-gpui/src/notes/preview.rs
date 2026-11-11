//! Notes use shared Markdown rendering with their desktop image asset paths.
use crate::transcript::selection::{Selection, TextSelection};
use crate::{
    markdown::{self, MarkdownRow, Part, Tone},
    markdown_model,
    runtime::{Database, locked},
    theme::*,
};
use gpui::{
    AnyElement, ClipboardItem, Context, FocusHandle, ImageSource, MouseButton, Render, Role,
    Window, div, prelude::*, px,
};
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use std::{path::Path, time::Instant};

enum Row {
    Markdown(MarkdownRow),
    Image { source: ImageSource, alt: String },
    Missing(String),
}
pub(super) struct Preview {
    rows: Vec<Row>,
    /// Mouse selection of the rendered text, shared with the transcript.
    /// Row keys are `(index in rows, 0)`; there is one response, 0.
    selection: Selection,
    focus: FocusHandle,
}
impl Preview {
    pub(super) fn new(text: &str, cwd: &str, store: &Database, cx: &mut Context<Self>) -> Self {
        let mut spans = Vec::new();
        let mut open = None;
        for (event, range) in Parser::new_ext(text, Options::all()).into_offset_iter() {
            match event {
                Event::Start(Tag::Image { dest_url, .. }) => {
                    open = Some((range.start, dest_url.to_string(), String::new()))
                }
                Event::Text(value) if open.is_some() => open.as_mut().unwrap().2.push_str(&value),
                Event::End(TagEnd::Image) => {
                    if let Some((start, url, alt)) = open.take() {
                        spans.push((start..range.end, url, alt));
                    }
                }
                _ => {}
            }
        }
        let mut rows = Vec::new();
        let mut at = 0;
        for (span, url, alt) in spans {
            if span.start < at || span.end > text.len() {
                continue;
            }
            append_markdown(&mut rows, &text[at..span.start], cwd);
            let source = if url.starts_with("/note-assets/") {
                locked(store)
                    .and_then(|store| store.note_image_path(&url))
                    .map(ImageSource::from)
            } else if url.starts_with("https://")
                || url.starts_with("http://")
                || url.starts_with("data:image/")
            {
                Ok(ImageSource::from(url))
            } else {
                let path = Path::new(&url);
                Ok(ImageSource::from(if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    Path::new(cwd).join(path)
                }))
            };
            rows.push(match source {
                Ok(source) => Row::Image { source, alt },
                Err(error) => Row::Missing(error),
            });
            at = span.end;
        }
        append_markdown(&mut rows, &text[at..], cwd);
        Self {
            rows,
            selection: Selection::default(),
            focus: cx.focus_handle(),
        }
    }
    fn selectable_rows(&self) -> impl Iterator<Item = ((usize, usize), usize, String)> + '_ {
        self.rows
            .iter()
            .enumerate()
            .filter_map(|(ix, row)| match row {
                Row::Markdown(row) => Some(((ix, 0), 0, markdown::selection_text(row))),
                _ => None,
            })
    }
    pub(super) fn selected_text(&self) -> Option<String> {
        self.selection.text_of(self.selectable_rows())
    }
    fn copy(&mut self, _: &crate::input::Copy, _: &mut Window, cx: &mut Context<Self>) {
        match self.selected_text() {
            Some(text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
            None => cx.propagate(),
        }
    }
    fn select_all(&mut self, _: &crate::input::SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        let rows: Vec<_> = self.selectable_rows().collect();
        if let (Some(first), Some(last)) = (rows.first(), rows.last()) {
            self.selection.select_rows(0, first.0, last.0, last.2.len());
            cx.notify();
        }
    }
}
fn append_markdown(rows: &mut Vec<Row>, text: &str, cwd: &str) {
    for block in &markdown_model::parse_full_hard_breaks(text).blocks {
        for mut row in markdown::rows(&block.block) {
            markdown::resolve_file_links(&mut row, cwd);
            rows.push(Row::Markdown(row));
        }
    }
}
impl Render for Preview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.selection.begin_frame();
        let view = cx.weak_entity();
        let selection = |ix: usize| {
            let view = view.clone();
            TextSelection::new(&self.selection, (ix, 0), move |window, cx| {
                let _ = view.update(cx, |this, cx| {
                    window.focus(&this.focus, cx);
                    cx.notify();
                });
            })
        };
        div()
            .id("note-preview-body")
            .key_context("TranscriptSelection")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::select_all))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    // A text run's press starts a drag before this bubbles.
                    if !this.selection.dragging() {
                        this.selection.clear();
                        cx.notify();
                    }
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _, cx| {
                let pressed = event.pressed_button == Some(MouseButton::Left);
                if this.selection.drag_to(event.position, pressed) {
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.selection.finish_drag() {
                        cx.notify();
                    }
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.selection.finish_drag() {
                        cx.notify();
                    }
                }),
            )
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" && this.selected_text().is_some() {
                    this.selection.clear();
                    cx.notify();
                    cx.stop_propagation()
                }
            }))
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .when(self.rows.is_empty(), |d| {
                d.child(
                    div()
                        .text_size(px(13.))
                        .text_color(content(0.45))
                        .child("No description"),
                )
            })
            .children(self.rows.iter().enumerate().map(|(ix, row)| {
                match row {
                    Row::Image { source, alt } => div()
                        .mt_3()
                        .mb_3()
                        .w_full()
                        .min_w_0()
                        .child(
                            crate::image_memory::image(source.clone())
                                .id(("note-image", ix))
                                .role(Role::Image)
                                .aria_label(alt.clone())
                                .max_w_full()
                                .max_h(px(640.))
                                .rounded_md()
                                .object_fit(gpui::ObjectFit::Contain),
                        )
                        .into_any_element(),
                    Row::Missing(error) => div()
                        .py_2()
                        .text_size(px(12.))
                        .text_color(content(0.45))
                        .child(error.clone())
                        .into_any_element(),
                    Row::Markdown(row) => {
                        let id: gpui::SharedString = format!("note-markdown-{ix}").into();
                        let body: AnyElement = match &row.part {
                            Part::Code { fence, lines } => div()
                                .id(id.clone())
                                .overflow_x_scroll()
                                .rounded_lg()
                                .border_1()
                                .border_color(content(0.1))
                                .bg(content(0.03))
                                .py_2()
                                .child(markdown::code_lines_selected(
                                    fence,
                                    lines.clone(),
                                    None,
                                    "",
                                    false,
                                    Some(selection(ix)),
                                ))
                                .into_any_element(),
                            _ => markdown::body_selected(
                                row,
                                id,
                                Tone::Normal,
                                None,
                                Instant::now(),
                                Some(selection(ix)),
                                None,
                            ),
                        };
                        div()
                            .pt(px(row.gap))
                            .flex()
                            .min_w_0()
                            .gap_2()
                            .pl(px(row.indent as f32 * 16.))
                            .when(row.quote > 0, |d| {
                                d.border_l_2().border_color(content(0.15)).pl_3()
                            })
                            .when_some(row.marker.clone(), |d, marker| {
                                d.child(
                                    div()
                                        .w_4()
                                        .flex_shrink_0()
                                        .text_color(content(0.7))
                                        .text_size(px(14.))
                                        .line_height(px(24.))
                                        .child(marker),
                                )
                            })
                            .child(div().flex_1().min_w_0().child(body))
                            .into_any_element()
                    }
                }
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Modifiers, TestAppContext, VisualTestContext, WindowHandle, point, size};
    use std::sync::{Arc, Mutex};

    const MODIFIER: &str = if cfg!(target_os = "macos") {
        "cmd"
    } else {
        "ctrl"
    };

    fn preview(text: &'static str, cx: &mut TestAppContext) -> WindowHandle<Preview> {
        cx.update(crate::input::bind);
        let store: Database = Arc::new(Mutex::new(
            monocode_core::store::Store::open_monocode(Path::new(":memory:")).unwrap(),
        ));
        let window = cx.update(|cx| {
            cx.open_window(
                gpui::WindowOptions {
                    window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::new(
                        point(px(0.), px(0.)),
                        size(px(700.), px(500.)),
                    ))),
                    ..Default::default()
                },
                |_, cx| cx.new(|cx| Preview::new(text, "/tmp", &store, cx)),
            )
            .unwrap()
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        window
    }

    fn at(
        window: WindowHandle<Preview>,
        row: usize,
        byte: usize,
        cx: &mut TestAppContext,
    ) -> gpui::Point<gpui::Pixels> {
        window
            .read_with(cx, |view, _| view.selection.point_for_test((row, 0), byte))
            .unwrap()
    }

    fn selected(window: WindowHandle<Preview>, cx: &mut TestAppContext) -> Option<String> {
        window
            .read_with(cx, |view, _| view.selected_text())
            .unwrap()
    }

    #[gpui::test]
    fn dragging_in_a_note_preview_selects_text_and_copy_writes_it(cx: &mut TestAppContext) {
        let window = preview("First note line.\n\n```\nlet x = 1;\n```", cx);
        let (from, to) = (at(window, 0, 6, cx), at(window, 1, 5, cx));
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
        assert_eq!(selected(window, cx).as_deref(), Some("note line.\nlet x"));
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        visual.simulate_keystrokes(&format!("{MODIFIER}-c"));
        assert_eq!(
            cx.read_from_clipboard()
                .and_then(|item| item.text())
                .as_deref(),
            Some("note line.\nlet x")
        );
        // A plain click drops it; Select All takes the whole note.
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        visual.simulate_click(from, Modifiers::default());
        assert_eq!(selected(window, cx), None);
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        visual.simulate_keystrokes(&format!("{MODIFIER}-a"));
        assert_eq!(
            selected(window, cx).as_deref(),
            Some("First note line.\nlet x = 1;")
        );
    }

    #[gpui::test]
    fn a_link_in_a_note_preview_still_opens_on_click(cx: &mut TestAppContext) {
        let url = "https://example.com/note";
        let window = preview("See [the page](https://example.com/note).", cx);
        let point = crate::markdown::rendered_link_point(url).expect("link drawn");
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        visual.simulate_click(point, Modifiers::default());
        assert_eq!(cx.opened_url().as_deref(), Some(url));
        assert_eq!(selected(window, cx), None);
    }
}
