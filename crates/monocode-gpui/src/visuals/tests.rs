use super::*;
use gpui::TestAppContext;

fn document(source: &str) -> VisualDocument {
    VisualDocument {
        id: "native-test".into(),
        title: "Native visual".into(),
        source: source.into(),
    }
}
fn first_text(node: &Option<Node>) -> &str {
    match node.as_ref().unwrap() {
        Node::Column { children, .. } => match &children[0] {
            Node::Text { text, .. } => text,
            _ => panic!("Expected text"),
        },
        _ => panic!("Expected column"),
    }
}

#[gpui::test]
fn native_button_preserves_state_across_frames(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| {
                VisualView::new(
                    document(
                        r#"
        const state = visual.state({count: 0});
        return () => visual.column([
            visual.text(String(state.count)),
            visual.button('Increase', () => { state.count++; })
        ]);
    "#,
                    ),
                    cx,
                )
            })
        })
        .unwrap()
    });
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    assert!(visual.debug_bounds("visual-expand").is_none());
    for expected in ["1", "2"] {
        let bounds = visual
            .debug_bounds("visual-button-root/1")
            .expect("Native GPUI button must be present");
        visual.simulate_click(bounds.center(), Default::default());
        visual.run_until_parked();
        window
            .update(visual, |view, _, _| {
                assert_eq!(first_text(&view.tree), expected)
            })
            .unwrap();
    }
}

#[gpui::test]
fn native_tabs_render_and_rebind_selection_across_clicks(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| {
                VisualView::new(
                    document(
                        r#"
        const state = visual.state({tab: 0});
        return () => visual.column([
            visual.text(String(state.tab)),
            visual.tabs(['First', 'Second', 'Third'], state.tab, value => { state.tab = value; })
        ]);
    "#,
                    ),
                    cx,
                )
            })
        })
        .unwrap()
    });
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    for (selected, selector) in [
        (1, "visual-tab-root/1-1"),
        (2, "visual-tab-root/1-2"),
        (0, "visual-tab-root/1-0"),
    ] {
        let bounds = visual.debug_bounds(selector).unwrap();
        visual.simulate_click(bounds.center(), Default::default());
        visual.run_until_parked();
        window
            .update(visual, |view, _, _| {
                assert!(view.error.is_none(), "{:?}", view.error);
                assert_eq!(first_text(&view.tree), selected.to_string());
                let Some(Node::Column { children, .. }) = &view.tree else {
                    panic!("Expected column")
                };
                let Node::Tabs {
                    selected: actual, ..
                } = &children[1]
                else {
                    panic!("Expected tabs")
                };
                assert_eq!(*actual, selected);
            })
            .unwrap();
    }
}

#[gpui::test]
fn slider_drag_rebinds_callbacks_after_each_native_update(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| {
                VisualView::new(
                    document(
                        r#"
        const state = visual.state({amount: 0});
        return () => visual.column([
            visual.text(String(state.amount)),
            visual.slider('Amount', state.amount, 0, 100, value => { state.amount = value; })
        ]);
    "#,
                    ),
                    cx,
                )
            })
        })
        .unwrap()
    });
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    let track = visual.debug_bounds("visual-slider-root/1").unwrap();
    visual.simulate_event(gpui::MouseDownEvent {
        position: point(track.left() + track.size.width * 0.25, track.center().y),
        button: MouseButton::Left,
        modifiers: Default::default(),
        click_count: 1,
        first_mouse: false,
    });
    visual.run_until_parked();
    visual.simulate_event(gpui::MouseMoveEvent {
        position: point(track.left() + track.size.width * 0.75, track.center().y),
        pressed_button: Some(MouseButton::Left),
        modifiers: Default::default(),
    });
    visual.run_until_parked();
    window
        .update(visual, |view, _, _| {
            assert!(view.error.is_none(), "{:?}", view.error);
            assert_eq!(first_text(&view.tree), "75");
        })
        .unwrap();
}

#[gpui::test]
fn failed_handler_is_contained(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| {
                VisualView::new(
                    document(
                        r#"
        return () => visual.column([
            visual.text('Still here'),
            visual.button('Fail', () => { throw new Error('Expected failure'); })
        ]);
    "#,
                    ),
                    cx,
                )
            })
        })
        .unwrap()
    });
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    let fail = visual.debug_bounds("visual-button-root/1").unwrap();
    visual.simulate_click(fail.center(), Default::default());
    visual.run_until_parked();
    window
        .update(visual, |view, _, _| {
            assert!(view.error.as_deref().unwrap().contains("Expected failure"));
            assert_eq!(first_text(&view.tree), "Still here");
        })
        .unwrap();
}

#[gpui::test]
fn native_input_and_selection_keep_working_after_state_updates(cx: &mut TestAppContext) {
    cx.update(crate::input::bind);
    let window=cx.update(|cx|cx.open_window(Default::default(),|_,cx|cx.new(|cx|VisualView::new(document(r#"
        const state = visual.state({query:'',selected:0});
        return () => visual.column([
            visual.text(state.query + ':' + state.selected),
            visual.input('Filter',state.query,value=>{state.query=value;},{placeholder:'Type a filter'}),
            visual.select('Mode',['First','Second'],state.selected,value=>{state.selected=value;})
        ]);
    "#),cx))).unwrap());
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    let field = visual.debug_bounds("visual-input-root/1").unwrap();
    visual.simulate_click(field.center(), Default::default());
    visual.simulate_input("a");
    visual.run_until_parked();
    window
        .update(visual, |view, _, _| {
            assert!(view.error.is_none(), "{:?}", view.error);
            assert_eq!(first_text(&view.tree), "a:0");
        })
        .unwrap();
    visual.simulate_input("b");
    visual.run_until_parked();
    window
        .update(visual, |view, _, _| {
            assert!(view.error.is_none(), "{:?}", view.error);
            assert_eq!(first_text(&view.tree), "ab:0");
        })
        .unwrap();
    let option = visual.debug_bounds("visual-option-root/2-1").unwrap();
    visual.simulate_click(option.center(), Default::default());
    visual.run_until_parked();
    window
        .update(visual, |view, _, _| {
            assert_eq!(first_text(&view.tree), "ab:1")
        })
        .unwrap();
    let field = visual.debug_bounds("visual-input-root/1").unwrap();
    visual.simulate_click(field.center(), Default::default());
    visual.simulate_input("c");
    visual.run_until_parked();
    window
        .update(visual, |view, _, _| {
            assert!(view.error.is_none(), "{:?}", view.error);
            assert_eq!(first_text(&view.tree), "abc:1");
        })
        .unwrap();
}

#[gpui::test]
fn native_scene_orbits_and_reuses_static_projection(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| {
                VisualView::new(
                    document(
                        r#"
        return () => visual.scene([visual.object({kind:'box'})],{height:240});
    "#,
                    ),
                    cx,
                )
            })
        })
        .unwrap()
    });
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    let first = window
        .update(visual, |view, _, _| {
            view.scene_cache["root"]
                .borrow()
                .as_ref()
                .unwrap()
                .projection
                .clone()
        })
        .unwrap();
    visual.refresh().unwrap();
    visual.run_until_parked();
    let second = window
        .update(visual, |view, _, _| {
            view.scene_cache["root"]
                .borrow()
                .as_ref()
                .unwrap()
                .projection
                .clone()
        })
        .unwrap();
    assert!(
        Rc::ptr_eq(&first, &second),
        "Static repaints should reuse projected geometry"
    );
    let bounds = visual.debug_bounds("visual-scene-root").unwrap();
    visual.simulate_event(gpui::MouseDownEvent {
        position: bounds.center(),
        button: MouseButton::Left,
        modifiers: Default::default(),
        click_count: 1,
        first_mouse: false,
    });
    visual.simulate_event(gpui::MouseMoveEvent {
        position: bounds.center() + point(px(60.), px(25.)),
        pressed_button: Some(MouseButton::Left),
        modifiers: Default::default(),
    });
    visual.run_until_parked();
    window
        .update(visual, |view, _, _| {
            let orbit = view.orbits["root"];
            assert!((orbit.yaw - 0.6).abs() < 0.001);
            assert!((orbit.pitch - 0.25).abs() < 0.001);
            let after = view.scene_cache["root"]
                .borrow()
                .as_ref()
                .unwrap()
                .projection
                .clone();
            assert!(!Rc::ptr_eq(&first, &after));
        })
        .unwrap();
}

#[gpui::test]
fn tall_visual_uses_full_height_and_wheel_scrolls_the_conversation(cx: &mut TestAppContext) {
    struct Conversation {
        view: gpui::Entity<VisualView>,
        scroll: gpui::ScrollHandle,
    }
    impl Render for Conversation {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .id("visual-test-conversation")
                .debug_selector(|| "visual-test-conversation".into())
                .size_full()
                .overflow_y_scroll()
                .track_scroll(&self.scroll)
                .child(self.view.clone())
        }
    }
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            let view = cx.new(|cx| {
                VisualView::new(
                    document(
                        r#"
                return () => visual.column(Array.from({length: 40}, (_, i) =>
                    visual.button('Row ' + i, () => {})), {gap: 12});
            "#,
                    ),
                    cx,
                )
            });
            cx.new(|_| Conversation {
                view,
                scroll: gpui::ScrollHandle::new(),
            })
        })
        .unwrap()
    });
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.simulate_window_resize(window.into(), size(px(800.), px(500.)));
    visual.run_until_parked();
    let content = visual.debug_bounds("visual-content").unwrap();
    assert!(
        content.size.height > px(1400.),
        "All forty rows must contribute to the visual's height: {content:?}"
    );
    window
        .update(visual, |conversation, _, _| {
            assert!(conversation.scroll.max_offset().y > px(900.));
            assert_eq!(conversation.scroll.offset().y, px(0.));
        })
        .unwrap();
    let viewport = visual.debug_bounds("visual-test-conversation").unwrap();
    visual.simulate_event(gpui::ScrollWheelEvent {
        position: viewport.center(),
        delta: gpui::ScrollDelta::Pixels(point(px(0.), px(-180.))),
        modifiers: Default::default(),
        touch_phase: gpui::TouchPhase::Moved,
    });
    visual.run_until_parked();
    window
        .update(visual, |conversation, _, _| {
            assert!(
                conversation.scroll.offset().y < px(-100.),
                "Scrolling over a tall visual must move the conversation"
            );
        })
        .unwrap();
    assert_eq!(
        visual.debug_bounds("visual-content").unwrap().size.height,
        content.size.height
    );
}

#[gpui::test]
fn width_changes_emit_row_remeasure_events(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| VisualView::new(document("return () => visual.text('Ready');"), cx))
        })
        .unwrap()
    });
    let view = window.update(cx, |_, _, cx| cx.entity()).unwrap();
    let count = Rc::new(Cell::new(0));
    let event_count = count.clone();
    let _subscription = cx.update(|cx| {
        cx.subscribe(&view, move |_, _: &VisualChanged, _| {
            event_count.set(event_count.get() + 1)
        })
    });
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    count.set(0);
    visual.simulate_window_resize(window.into(), size(px(300.), px(700.)));
    visual.refresh().unwrap();
    visual.run_until_parked();
    assert!(
        count.get() > 0,
        "Responsive width changes must also remeasure the transcript row"
    );
}

mod cartesian {
    use super::super::{charts::*, dither::CELL};
    use monocode_core::visual::ChartKind;

    /// The columns every bar of every bucket takes, from the layout's own start rule.
    fn spans(band: f32, n: usize, buckets: usize) -> Vec<(i32, i32)> {
        let layout = bar_layout(band, n);
        let mut spans = vec![];
        for i in 0..buckets {
            let start =
                ((band * (i as f32 + 0.5)) / CELL - layout.group() as f32 / 2.).round() as i32;
            for slot in 0..layout.shown as i32 {
                let first = start + slot * (layout.wide + layout.gap);
                spans.push((first, first + layout.wide));
            }
        }
        spans
    }

    #[test]
    fn a_group_that_fits_keeps_its_share_of_the_band() {
        // A wide band: the old rule, 70% of the band with a one cell gap.
        let layout = bar_layout(120., 3);
        assert_eq!((layout.wide, layout.gap, layout.shown), (9, 1, 3));
        // One series fills 70%, capped at the widest bar.
        let layout = bar_layout(300., 1);
        assert_eq!((layout.wide, layout.gap, layout.shown), (12, 1, 1));
    }

    #[test]
    fn a_crowded_group_narrows_then_loses_its_gap_then_shows_what_fits() {
        // 6 series in a 60px band (20 cells): the preferred 2 cell bars with
        // gaps take 17 cells, and still fit.
        assert_eq!(bar_layout(60., 6).shown, 6);
        // 6 series in a 36px band (12 cells): 1 cell bars with gaps (11) fit.
        let layout = bar_layout(36., 6);
        assert_eq!((layout.wide, layout.gap, layout.shown), (1, 1, 6));
        // 24px (8 cells): the gaps go, 1 cell bars touch.
        let layout = bar_layout(24., 6);
        assert_eq!((layout.wide, layout.gap, layout.shown), (1, 0, 6));
        // 15px (5 cells): only four 1 cell bars fit, one cell left clear.
        let layout = bar_layout(15., 6);
        assert_eq!((layout.wide, layout.gap, layout.shown), (1, 0, 4));
        // Narrower than a cell still draws one bar.
        assert_eq!(bar_layout(2., 6).shown, 1);
    }

    #[test]
    fn groups_never_overlap_their_neighbours_at_any_width() {
        for n in 1..=7 {
            for band_cells in 1..=60 {
                let band = band_cells as f32 * CELL + 0.7;
                let spans = spans(band, n, 12);
                // Each bucket's bars are drawn left to right; none starts
                // before the one drawn just before it ends.
                for pair in spans.windows(2) {
                    assert!(
                        pair[0].1 <= pair[1].0,
                        "n={n} band={band}: {:?} overlaps {:?}",
                        pair[0],
                        pair[1]
                    );
                }
            }
        }
    }

    #[test]
    fn the_series_that_get_a_bar_are_the_lit_ones_first() {
        let drawn = |dimmed: bool| Drawn {
            label: String::new(),
            color: gpui::hsla(0., 0., 1., 1.),
            values: vec![],
            dimmed,
        };
        let series = [drawn(true), drawn(false), drawn(false)];
        assert_eq!(bar_series(&series, 3), [0, 1, 2]);
        assert_eq!(bar_series(&series, 2), [1, 2]);
        assert_eq!(bar_series(&series, 1), [1]);
    }

    #[test]
    fn lines_run_edge_to_edge_while_bars_keep_their_bands() {
        for kind in [ChartKind::Line, ChartKind::Area] {
            assert_eq!(x_at(kind, 0, 3, 100.), 0.);
            assert_eq!(x_at(kind, 1, 3, 100.), 50.);
            assert_eq!(x_at(kind, 2, 3, 100.), 100.);
            assert_eq!(x_at(kind, 0, 1, 100.), 50.);
        }
        assert_eq!(x_at(ChartKind::Bar, 0, 4, 100.), 12.5);
        assert_eq!(x_at(ChartKind::Bar, 3, 4, 100.), 87.5);
        assert_eq!(x_at(ChartKind::Bar, 0, 1, 100.), 50.);
    }

    #[test]
    fn the_pointer_picks_the_band_or_the_nearest_point() {
        assert_eq!(bucket_at(ChartKind::Bar, 0., 100., 4), Some(0));
        assert_eq!(bucket_at(ChartKind::Bar, 24.9, 100., 4), Some(0));
        assert_eq!(bucket_at(ChartKind::Bar, 25., 100., 4), Some(1));
        assert_eq!(bucket_at(ChartKind::Bar, 100., 100., 4), Some(3));
        assert_eq!(bucket_at(ChartKind::Bar, 100.1, 100., 4), None);
        assert_eq!(bucket_at(ChartKind::Line, 24., 100., 3), Some(0));
        assert_eq!(bucket_at(ChartKind::Line, 26., 100., 3), Some(1));
        assert_eq!(bucket_at(ChartKind::Line, -1., 100., 3), None);
        assert_eq!(bucket_at(ChartKind::Line, 5., 100., 1), Some(0));
        assert_eq!(bucket_at(ChartKind::Line, 5., 100., 0), None);
        assert_eq!(bucket_at(ChartKind::Line, 5., 0., 3), None);
    }

    #[test]
    fn x_labels_are_thinned_to_stay_72px_apart() {
        assert_eq!(label_stride(7, 600.), 1);
        assert_eq!(label_stride(15, 360.), 3);
        assert_eq!(label_stride(48, 720.), 5);
        assert_eq!(label_stride(1, 10.), 1);
        assert_eq!(label_stride(0, 0.), 1);
    }

    #[test]
    fn resting_samples_split_into_runs_that_share_their_joints() {
        let samples = [[0., 10.], [1., 10.], [2., 5.], [3., 2.], [4., 10.]];
        let out = runs(&samples, |y| y >= 10.);
        let shape: Vec<(bool, usize)> = out.iter().map(|(f, r)| (*f, r.len())).collect();
        // Each run takes the first point of the next, so the line stays whole.
        assert_eq!(shape, [(true, 3), (false, 3), (true, 1)]);
        assert_eq!(out[0].1[2], [2., 5.]);
        assert!(runs(&[], |_| true).is_empty());
    }

    #[test]
    fn the_plot_leaves_room_for_the_y_labels_and_a_bars_value() {
        use super::super::dither::Scale;
        let scale = Scale {
            min: 0.,
            max: 1200.,
            step: 400.,
        };
        // "1.2K" is the longest label: 4 chars.
        let lines = geometry(ChartKind::Line, &scale, 400., 200.);
        assert_eq!((lines.left, lines.top), (35., 8.));
        assert_eq!((lines.width, lines.height), (359., 170.));
        let bars = geometry(ChartKind::Bar, &scale, 400., 200.);
        assert_eq!((bars.top, bars.width, bars.height), (16., 365., 162.));
    }
}
