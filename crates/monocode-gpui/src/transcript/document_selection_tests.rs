//! Mouse selection in a document view (an artifact or plan rendered as
//! Markdown), which reuses the transcript's selection.
use super::*;
use gpui::{
    Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, PlatformInput, Point, TestAppContext,
    VisualTestContext, WindowHandle, point,
};

const DOC: &str = "Alpha beta gamma.\n\nSecond [docs](https://example.com/docs) line.";
const MODIFIER: &str = if cfg!(target_os = "macos") {
    "cmd"
} else {
    "ctrl"
};

fn document(cx: &mut TestAppContext) -> WindowHandle<TranscriptView> {
    cx.update(crate::input::bind);
    let window = cx.update(|cx| {
        cx.open_window(
            gpui::WindowOptions {
                window_bounds: Some(gpui::WindowBounds::Windowed(Bounds::new(
                    point(px(0.), px(0.)),
                    size(px(700.), px(500.)),
                ))),
                ..Default::default()
            },
            |_, cx| cx.new(|cx| TranscriptView::document(DOC.into(), cx)),
        )
        .unwrap()
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    window
}

/// The keys of the document's two paragraphs.
fn paragraphs(
    window: WindowHandle<TranscriptView>,
    cx: &mut TestAppContext,
) -> Vec<(usize, usize)> {
    window
        .read_with(cx, |view, _| {
            view.rows
                .iter()
                .filter(|row| matches!(row.body, Body::Markdown(_)))
                .map(|row| row.key)
                .collect()
        })
        .unwrap()
}

fn at(
    window: WindowHandle<TranscriptView>,
    row: (usize, usize),
    byte: usize,
    cx: &mut TestAppContext,
) -> Point<Pixels> {
    window
        .read_with(cx, |view, _| view.selection.point_for_test(row, byte))
        .unwrap()
}

fn press(
    window: WindowHandle<TranscriptView>,
    position: Point<Pixels>,
    click_count: usize,
    cx: &mut TestAppContext,
) {
    for event in [
        PlatformInput::MouseDown(MouseDownEvent {
            position,
            button: MouseButton::Left,
            modifiers: Modifiers::default(),
            click_count,
            first_mouse: false,
        }),
        PlatformInput::MouseUp(MouseUpEvent {
            position,
            button: MouseButton::Left,
            modifiers: Modifiers::default(),
            click_count,
        }),
    ] {
        cx.update_window(window.into(), |_, w, cx| w.dispatch_event(event, cx))
            .unwrap();
        cx.refresh().unwrap();
    }
}

fn selected(window: WindowHandle<TranscriptView>, cx: &mut TestAppContext) -> Option<String> {
    window
        .read_with(cx, |view, _| view.selected_text())
        .unwrap()
}

#[gpui::test]
fn dragging_across_a_document_selects_its_text_and_copy_writes_it(cx: &mut TestAppContext) {
    let window = document(cx);
    let rows = paragraphs(window, cx);
    assert_eq!(rows.len(), 2);
    let (from, to) = (at(window, rows[0], 6, cx), at(window, rows[1], 6, cx));
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    assert_eq!(selected(window, cx).as_deref(), Some("beta gamma.\nSecond"));
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.simulate_keystrokes(&format!("{MODIFIER}-c"));
    assert_eq!(
        cx.read_from_clipboard()
            .and_then(|item| item.text())
            .as_deref(),
        Some("beta gamma.\nSecond")
    );
}

#[gpui::test]
fn double_and_triple_click_select_a_word_and_a_paragraph(cx: &mut TestAppContext) {
    let window = document(cx);
    let rows = paragraphs(window, cx);
    let word = at(window, rows[0], 7, cx);
    press(window, word, 2, cx);
    assert_eq!(selected(window, cx).as_deref(), Some("beta"));
    press(window, word, 3, cx);
    assert_eq!(selected(window, cx).as_deref(), Some("Alpha beta gamma."));
    // A plain click elsewhere drops the selection.
    press(window, at(window, rows[1], 2, cx), 1, cx);
    assert_eq!(selected(window, cx), None);
}

#[gpui::test]
fn select_all_selects_the_whole_document(cx: &mut TestAppContext) {
    let window = document(cx);
    let rows = paragraphs(window, cx);
    press(window, at(window, rows[0], 0, cx), 1, cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.simulate_keystrokes(&format!("{MODIFIER}-a"));
    assert_eq!(
        selected(window, cx).as_deref(),
        Some("Alpha beta gamma.\nSecond docs line.")
    );
}

#[gpui::test]
fn a_link_in_a_selectable_document_still_opens_on_click(cx: &mut TestAppContext) {
    let window = document(cx);
    let url = "https://example.com/docs";
    let point = crate::markdown::rendered_link_point(url).expect("link drawn");
    press(window, point, 1, cx);
    assert_eq!(cx.opened_url().as_deref(), Some(url));
    assert_eq!(selected(window, cx), None);
}

#[gpui::test]
fn a_document_selection_menu_offers_copy_only(cx: &mut TestAppContext) {
    let window = document(cx);
    let rows = paragraphs(window, cx);
    press(window, at(window, rows[0], 7, cx), 2, cx);
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(1));
    cx.run_until_parked();
    let (menu, quote) = window
        .read_with(cx, |view, _| view.selection.menu_for_test())
        .unwrap();
    assert!(menu, "a finished selection offers its menu");
    assert!(
        !quote,
        "a document must not offer Add to chat or Add to notes"
    );
}
