//! A file chip in a chat is a button: a left click previews the file, a right click keeps the
//! link menu.
use super::*;
use crate::file_preview::PreviewPath;
use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext, WindowHandle, point};
use std::{cell::RefCell, rc::Rc};

const TEXT: &str = "Open `report.html` or `~/Documents/x/uiux-findings.md:12` or `plain-word`.";

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
            |_, cx| {
                cx.new(|cx| TranscriptView::document_in(TEXT.into(), Some("/project".into()), cx))
            },
        )
        .unwrap()
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    window
}

#[gpui::test]
fn a_left_click_on_a_file_chip_previews_the_file(cx: &mut TestAppContext) {
    let seen = Rc::new(RefCell::new(Vec::<String>::new()));
    let log = seen.clone();
    cx.update(|cx| {
        cx.on_action(move |preview: &PreviewPath, _| log.borrow_mut().push(preview.0.clone()));
    });
    let window = document(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    let chip = visual
        .debug_bounds("file-chip:report.html")
        .expect("the chip has a selector");
    visual.simulate_click(chip.center(), Modifiers::default());
    assert_eq!(*seen.borrow(), ["/project/report.html"]);

    let deep = visual
        .debug_bounds("file-chip:~/Documents/x/uiux-findings.md:12")
        .expect("a chip outside any project has one too");
    visual.simulate_click(deep.center(), Modifiers::default());
    let seen = seen.borrow();
    assert_eq!(seen.len(), 2);
    assert!(
        seen[1].ends_with("/Documents/x/uiux-findings.md:12:1"),
        "the chip carries its path and line: {}",
        seen[1]
    );
    assert!(!seen[1].starts_with('~'), "the home folder is expanded");
}

#[gpui::test]
fn a_word_in_code_is_not_a_file_chip(cx: &mut TestAppContext) {
    let window = document(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    assert!(visual.debug_bounds("file-chip:plain-word").is_none());
}

#[gpui::test]
fn a_right_click_on_a_file_chip_does_not_preview_it(cx: &mut TestAppContext) {
    let seen = Rc::new(RefCell::new(Vec::<String>::new()));
    let log = seen.clone();
    cx.update(|cx| {
        cx.on_action(move |preview: &PreviewPath, _| log.borrow_mut().push(preview.0.clone()));
    });
    let window = document(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    let chip = visual.debug_bounds("file-chip:report.html").unwrap();
    visual.simulate_mouse_down(chip.center(), MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(chip.center(), MouseButton::Right, Modifiers::default());
    assert!(seen.borrow().is_empty());
}
