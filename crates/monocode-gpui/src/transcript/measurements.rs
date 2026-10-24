//! Retain unchanged text-row heights and share probes within each frame.
use super::*;

#[derive(PartialEq)]
struct LayoutStyle {
    text: gpui::TextStyle,
    rem: Pixels,
    scale: f32,
    reduced_motion: bool,
}

#[derive(Default)]
pub(super) struct Measurements {
    style: Option<LayoutStyle>,
    viewport: gpui::Size<Pixels>,
    row_count: usize,
    rows: HashMap<usize, Pixels>,
    retained: HashMap<(usize, usize), Pixels>,
    #[cfg(test)]
    pub(super) fresh: usize,
}

impl Measurements {
    pub(super) fn clear(&mut self) {
        self.rows.clear();
        self.retained.clear();
    }
}

impl TranscriptView {
    pub(super) fn begin_measurements(&mut self, window: &Window, cx: &Context<Self>) {
        let style = LayoutStyle {
            text: window.text_style(),
            rem: window.rem_size(),
            scale: window.scale_factor(),
            reduced_motion: cx.reduce_motion(),
        };
        if self.measurements.style.as_ref() != Some(&style) {
            // GPUI retains measured item heights independently of text styling.
            self.list.remeasure_items(0..self.list.item_count());
            self.phase_layouts.clear();
            self.measurements.style = Some(style);
            self.measurements.clear();
        }
        if self.measurements.viewport != self.viewport.size
            || self.measurements.row_count != self.rows.len()
        {
            // Removing closing rows can also change their neighbours' spacing.
            self.measurements.clear();
            self.measurements.viewport = self.viewport.size;
            self.measurements.row_count = self.rows.len();
        }
        self.measurements.rows.clear();
        let motions = &self.motions;
        self.measurements
            .retained
            .retain(|key, _| !motions.contains_key(key));
    }

    pub(super) fn measure_row_height(
        &mut self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Pixels {
        // The viewport probe can discover a resize after render's follow pass.
        if self.measurements.viewport != self.viewport.size {
            self.measurements.clear();
            self.measurements.viewport = self.viewport.size;
        }
        if let Some(height) = self.measurements.rows.get(&ix) {
            return *height;
        }
        let key = self.rows[ix].key;
        let stable = stable_height(&self.rows[ix], &self.messages, &self.motions);
        if stable && let Some(&height) = self.measurements.retained.get(&key) {
            self.measurements.rows.insert(ix, height);
            return height;
        }
        let mut row = self.row(ix, window, cx);
        let height = row
            .layout_as_root(
                size(
                    AvailableSpace::Definite(self.viewport.size.width),
                    AvailableSpace::MinContent,
                ),
                window,
                cx,
            )
            .height;
        #[cfg(test)]
        {
            self.measurements.fresh += 1;
        }
        self.measurements.rows.insert(ix, height);
        if stable {
            self.measurements.retained.insert(key, height);
        }
        height
    }
}

fn stable_height(
    row: &UiRow,
    messages: &[Message],
    motions: &HashMap<(usize, usize), Motion>,
) -> bool {
    // Independent child views, attachments and disclosures can resize without
    // a source patch. Retain only text geometry that rebuild invalidates.
    matches!(row.body, Body::Plain(_) | Body::Markdown(_))
        && !row.in_phase()
        && !motions.contains_key(&row.key)
        && messages[row.message].attachments.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Entity, Render, TestAppContext, WindowBounds, WindowOptions, point};
    use std::{cell::RefCell, rc::Rc, time::Duration};

    type Probe = Box<dyn FnOnce(&mut TranscriptView, &mut Window, &mut Context<TranscriptView>)>;

    struct Fixture {
        transcript: Entity<TranscriptView>,
        probe: Option<Probe>,
    }
    impl Render for Fixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let transcript = self.transcript.clone();
            let probe = self.probe.take();
            div()
                .relative()
                .size_full()
                .child(self.transcript.clone())
                .child(
                    canvas(
                        move |_, window, cx| {
                            if let Some(probe) = probe {
                                transcript.update(cx, |view, cx| probe(view, window, cx));
                            }
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full(),
                )
        }
    }

    fn row(role: &str, text: &str) -> Row {
        Row {
            role: role.into(),
            text: text.into(),
            first: true,
            sent_at: None,
        }
    }

    fn chat(cx: &mut TestAppContext) -> gpui::WindowHandle<Fixture> {
        let window = cx.update(|cx| {
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        point(px(0.), px(0.)),
                        size(px(900.), px(600.)),
                    ))),
                    ..Default::default()
                },
                |_, cx| {
                    let transcript = cx.new(|cx| {
                        let mut view = TranscriptView::new();
                        view.visible = false;
                        view.streaming = true;
                        view.smooth_reveal = false;
                        view.patch(
                            0,
                            vec![row("user", "Question"), row("assistant", "A short answer.")],
                            cx,
                        );
                        view.visible = true;
                        view
                    });
                    cx.new(|_| Fixture {
                        transcript,
                        probe: None,
                    })
                },
            )
            .unwrap()
        });
        render(window, cx);
        render(window, cx);
        window
    }

    fn render(window: gpui::WindowHandle<Fixture>, cx: &mut TestAppContext) {
        window.update(cx, |_, _, cx| cx.notify()).unwrap();
        cx.refresh().unwrap();
        cx.run_until_parked();
    }

    // GPUI layout must run in a real draw phase, rather than an update handler.
    fn probe<R: 'static>(
        window: gpui::WindowHandle<Fixture>,
        cx: &mut TestAppContext,
        inspect: impl FnOnce(&mut TranscriptView, &mut Window, &mut Context<TranscriptView>) -> R
        + 'static,
    ) -> R {
        let result = Rc::new(RefCell::new(None));
        let output = result.clone();
        window
            .update(cx, |fixture, _, cx| {
                fixture.probe = Some(Box::new(move |view, window, cx| {
                    *output.borrow_mut() = Some(inspect(view, window, cx));
                }));
                cx.notify();
            })
            .unwrap();
        render(window, cx);

        result.borrow_mut().take().expect("the layout probe ran")
    }

    fn answer(view: &TranscriptView) -> usize {
        view.rows
            .iter()
            .position(|row| matches!(row.body, Body::Markdown(_)))
            .unwrap()
    }

    #[gpui::test]
    fn retained_row_measurements_skip_unchanged_layout_and_match_fresh_heights(
        cx: &mut TestAppContext,
    ) {
        let window = chat(cx);
        probe(window, cx, |view, window, cx| {
            let ix = answer(view);
            let scroll = view.list.logical_scroll_top();
            view.begin_measurements(window, cx);
            let before = view.measurements.fresh;
            let retained = view.measure_row_height(ix, window, cx);
            assert_eq!(view.measurements.fresh, before);
            view.measurements.clear();
            let fresh = view.measure_row_height(ix, window, cx);
            assert!((retained - fresh).abs() < px(0.01));
            assert_eq!(view.measurements.fresh, before + 1);
            assert_eq!(view.list.logical_scroll_top().item_ix, scroll.item_ix);
            assert_eq!(
                view.list.logical_scroll_top().offset_in_item,
                scroll.offset_in_item
            );
            view.begin_measurements(window, cx);
            view.update_follow(window, cx);
            let after_follow = view.measurements.fresh;
            let first_footer = view.footer_height(window, cx);
            assert_eq!(view.footer_height(window, cx), first_footer);
            assert_eq!(view.measurements.fresh, after_follow);
        });
    }

    #[gpui::test]
    fn retained_row_measurements_refresh_after_text_resize_and_font_changes(
        cx: &mut TestAppContext,
    ) {
        let window = chat(cx);
        let short = probe(window, cx, |view, window, cx| {
            let short = view.measure_row_height(answer(view), window, cx);
            view.patch(
                1,
                vec![row("assistant", &"More words in the answer. ".repeat(40))],
                cx,
            );
            view.motions.clear();
            let ix = answer(view);
            view.begin_measurements(window, cx);
            let before = view.measurements.fresh;
            let grown = view.measure_row_height(ix, window, cx);
            assert!(grown > short);
            assert_eq!(view.measurements.fresh, before + 1);
            short
        });
        let wide = probe(window, cx, |view, window, cx| {
            view.measure_row_height(answer(view), window, cx)
        });
        cx.simulate_window_resize(window.into(), size(px(400.), px(600.)));
        render(window, cx);
        render(window, cx);
        probe(window, cx, move |view, window, cx| {
            let ix = answer(view);
            let narrow = view.measure_row_height(ix, window, cx);
            assert!(narrow > wide);
            assert!(wide > short);
            window.set_rem_size(px(22.));
            view.begin_measurements(window, cx);
            let before = view.measurements.fresh;
            let _ = view.measure_row_height(ix, window, cx);
            assert_eq!(view.measurements.fresh, before + 1);
        });
    }

    #[gpui::test]
    fn retained_row_measurements_keep_height_animation_live(cx: &mut TestAppContext) {
        let window = chat(cx);
        probe(window, cx, |view, window, cx| {
            let ix = answer(view);
            let full = view.measure_row_height(ix, window, cx);
            let now = Instant::now();
            view.motions
                .insert(view.rows[ix].key, Motion::new(true, false, now));
            view.frame_now = now + Duration::from_millis(30);
            view.begin_measurements(window, cx);
            let opening = view.measure_row_height(ix, window, cx);
            view.frame_now = now + Duration::from_millis(150);
            view.begin_measurements(window, cx);
            let later = view.measure_row_height(ix, window, cx);
            assert!(opening < later);
            assert!(later < full);
        });
    }

    #[gpui::test]
    fn retained_row_measurements_refresh_when_closing_rows_leave(cx: &mut TestAppContext) {
        let window = chat(cx);
        probe(window, cx, |view, window, cx| {
            view.patch(
                1,
                vec![row(
                    "assistant",
                    &format!("Short.\n\n{}", "A wrapped second paragraph. ".repeat(25)),
                )],
                cx,
            );
            view.motions.clear();
            view.begin_measurements(window, cx);
            let indices: Vec<_> = view
                .rows
                .iter()
                .enumerate()
                .filter(|(_, row)| matches!(row.body, Body::Markdown(_)))
                .map(|(ix, _)| ix)
                .collect();
            assert_eq!(indices.len(), 2);
            let short = view.measure_row_height(indices[0], window, cx);
            let long = view.measure_row_height(indices[1], window, cx);
            assert!(long > short);
            view.motions.insert(
                view.rows[indices[0]].key,
                Motion::new(false, false, Instant::now() - Duration::from_secs(2)),
            );
            view.animate_frame(cx);
            view.begin_measurements(window, cx);
            let before = view.measurements.fresh;
            assert_eq!(view.measure_row_height(indices[0], window, cx), long);
            assert_eq!(view.measurements.fresh, before + 1);
        });
    }
}
