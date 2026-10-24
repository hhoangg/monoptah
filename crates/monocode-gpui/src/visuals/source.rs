//! Immutable JavaScript source, highlighted once and drawn a viewport at a time.
use crate::{
    markdown::MONO,
    syntax::{HighlightRequest, HighlightedDocument},
    theme::content,
};
use gpui::{
    Context, HighlightStyle, IntoElement, ListHorizontalSizingBehavior, Render, SharedString,
    StyledText, Task, UniformListScrollHandle, Window, div, prelude::*, px, rgb, uniform_list,
};
use std::{ops::Range, sync::Arc};

const LINE_HEIGHT: f32 = 20.;
const MAX_HEIGHT: f32 = 420.;

struct Document {
    source: SharedString,
    lines: Vec<Range<usize>>,
    longest_line: usize,
    content_width: f32,
    gutter_width: f32,
}

impl Document {
    fn new(source: String) -> Self {
        let mut lines = Vec::new();
        let mut offset = 0;
        let mut longest_line = 0;
        let mut longest_width = 0;
        for line in source.split_inclusive('\n') {
            let ending = if line.ends_with("\r\n") {
                2
            } else {
                usize::from(line.ends_with('\n'))
            };
            let end = offset + line.len() - ending;
            let text = &source[offset..end];
            // Bytes conservatively cover fallback wide glyphs; extra tab space
            // prevents an offscreen tabbed line from losing horizontal travel.
            let width = text.len() + text.bytes().filter(|byte| *byte == b'\t').count() * 3;
            if width > longest_width {
                longest_width = width;
                longest_line = lines.len();
            }
            lines.push(offset..end);
            offset += line.len();
        }
        if lines.is_empty() || source.ends_with('\n') {
            lines.push(source.len()..source.len());
        }
        let gutter_width = lines.len().to_string().len() as f32 * 6. + 12.;
        Self {
            source: source.into(),
            lines,
            longest_line,
            content_width: longest_width as f32 * 8. + gutter_width + 16.,
            gutter_width,
        }
    }

    fn styles(
        &self,
        index: usize,
        highlighted: Option<&HighlightedDocument>,
    ) -> Vec<(Range<usize>, HighlightStyle)> {
        highlighted
            .and_then(|document| document.lines.get(index))
            .into_iter()
            .flatten()
            .map(|span| {
                (
                    span.range.clone(),
                    HighlightStyle {
                        color: Some(rgb(span.color).into()),
                        ..Default::default()
                    },
                )
            })
            .collect()
    }
}

pub(super) struct SourceView {
    document: Arc<Document>,
    highlighted: Option<Arc<HighlightedDocument>>,
    scroll: UniformListScrollHandle,
    _highlight: Task<()>,
}

impl SourceView {
    pub(super) fn new(source: String, cx: &mut Context<Self>) -> Self {
        let document = Arc::new(Document::new(source));
        let code = document.source.clone();
        let work = cx.background_executor().spawn(async move {
            crate::syntax::highlight(HighlightRequest {
                source: code.as_ref(),
                path: None,
                fence_tag: Some("javascript"),
            })
            .ok()
            .map(Arc::new)
        });
        let highlight = cx.spawn(async move |this, cx| {
            let highlighted = work.await;
            let _ = this.update(cx, |this, cx| {
                this.highlighted = highlighted;
                cx.notify();
            });
        });
        Self {
            document,
            highlighted: None,
            scroll: UniformListScrollHandle::new(),
            _highlight: highlight,
        }
    }
}

impl Render for SourceView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let document = self.document.clone();
        let highlighted = self.highlighted.clone();
        let count = document.lines.len();
        let longest_line = document.longest_line;
        div()
            .id("visual-source-pane")
            .debug_selector(|| "visual-source-pane".into())
            .w_full()
            .min_w_0()
            .h(px((count as f32 * LINE_HEIGHT).min(MAX_HEIGHT)))
            .overflow_hidden()
            .child(
                uniform_list("visual-source-lines", count, move |range, _, _| {
                    range
                        .map(|index| {
                            let line = &document.source[document.lines[index].clone()];
                            let text: SharedString = if line.is_empty() {
                                " ".into()
                            } else {
                                line.to_owned().into()
                            };
                            div()
                                .id(index)
                                .debug_selector(move || format!("visual-source-line-{index}"))
                                .flex()
                                .items_center()
                                .h(px(LINE_HEIGHT))
                                .w(px(document.content_width))
                                .min_w_full()
                                .text_size(px(12.))
                                .line_height(px(LINE_HEIGHT))
                                .font_family(MONO)
                                .text_color(content(0.85))
                                .child(
                                    div()
                                        .w(px(document.gutter_width))
                                        .mr_2()
                                        .flex_shrink_0()
                                        .text_right()
                                        .text_size(px(10.))
                                        .text_color(content(0.35))
                                        .child((index + 1).to_string()),
                                )
                                .child(StyledText::new(text).with_highlights(
                                    document.styles(index, highlighted.as_deref()),
                                ))
                        })
                        .collect::<Vec<_>>()
                })
                .with_width_from_item(Some(longest_line))
                .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
                .track_scroll(&self.scroll)
                .w_full()
                .h_full(),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{ScrollStrategy, TestAppContext};

    #[gpui::test]
    fn javascript_source_preserves_unicode_colors_and_virtualizes_lines(cx: &mut TestAppContext) {
        let first = "const greeting = 'Здравей 🌍'; // Unicode stays intact\r\n";
        let source = format!("{first}{}", "return visual.text(greeting);\n".repeat(1000));
        let exact = source.clone();
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|cx| SourceView::new(source, cx))
            })
            .unwrap()
        });
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        window
            .update(visual, |view, _, _| {
                assert_eq!(view.document.source.as_ref(), exact);
                let document = view
                    .highlighted
                    .as_deref()
                    .expect("JavaScript is highlighted");
                let spans = &document.lines[0];
                let rendered = view.document.styles(0, Some(document));
                assert_eq!(rendered.len(), spans.len());
                let colors: std::collections::HashSet<_> =
                    spans.iter().map(|span| span.color).collect();
                assert!(
                    colors.len() >= 3,
                    "Keywords, strings and comments keep their syntax colors"
                );
                let line = &view.document.source[view.document.lines[0].clone()];
                assert_eq!(line, first.trim_end_matches(['\r', '\n']));
                for (span, (range, style)) in spans.iter().zip(&rendered) {
                    assert!(line.is_char_boundary(range.start) && line.is_char_boundary(range.end));
                    assert_eq!(range, &span.range);
                    assert_eq!(style.color, Some(rgb(span.color).into()));
                }
            })
            .unwrap();
        assert!(visual.debug_bounds("visual-source-line-0").is_some());
        assert!(visual.debug_bounds("visual-source-line-1000").is_none());
        window
            .update(visual, |view, _, cx| {
                view.scroll.scroll_to_item(1000, ScrollStrategy::Nearest);
                cx.notify();
            })
            .unwrap();
        visual.run_until_parked();
        assert!(visual.debug_bounds("visual-source-line-1000").is_some());
        assert!(visual.debug_bounds("visual-source-line-0").is_none());
    }
}
