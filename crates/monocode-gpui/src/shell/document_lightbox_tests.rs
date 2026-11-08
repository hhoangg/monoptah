use super::*;
use gpui::{TestAppContext, VisualTestContext};

fn temp_file(name: &str, text: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "monoptah-doc-lightbox-shell-{}",
        monocode_core::mono::new_id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, text).unwrap();
    path
}

/// Removes the temp directory when the test ends, even if it fails.
struct Scratch(std::path::PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(self.0.parent().unwrap_or(&self.0));
    }
}

fn open(
    cx: &mut TestAppContext,
    name: &str,
    text: &str,
) -> (gpui::WindowHandle<Shell>, VisualTestContext, Scratch) {
    cx.update(|cx| {
        crate::input::bind(cx);
        crate::document_lightbox::bind(cx);
        super::bind_keys(cx);
        cx.set_reduce_motion(true);
    });
    let (window, _, _) = super::tests::window(cx);
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    let path = temp_file(name, text);
    let scratch = Scratch(path.clone());
    window
        .update(&mut visual, |shell, w, cx| {
            shell.focus_active(w, cx);
            w.dispatch_action(
                Box::new(crate::document_lightbox::Open {
                    path,
                    title: name.to_string().into(),
                    cloud: None,
                }),
                cx,
            )
        })
        .unwrap();
    visual.run_until_parked();
    visual.refresh().unwrap();
    (window, visual, scratch)
}

fn is_open(window: gpui::WindowHandle<Shell>, visual: &mut VisualTestContext) -> bool {
    window
        .update(visual, |shell, _, _| shell.document_lightbox.is_some())
        .unwrap()
}

#[gpui::test]
fn markdown_attachment_opens_in_the_lightbox_and_close_action_closes_it(cx: &mut TestAppContext) {
    let (window, mut visual, _scratch) = open(cx, "report.md", "# Report\n\nBody text");
    assert!(is_open(window, &mut visual));
    assert!(visual.debug_bounds("document-lightbox-markdown").is_some());
    assert!(visual.debug_bounds("document-lightbox").is_some());
    assert!(visual.debug_bounds("document-lightbox-panel").is_some());
    assert!(visual.debug_bounds("document-lightbox-copy").is_some());
    assert!(
        visual
            .debug_bounds("document-lightbox-open-external")
            .is_some()
    );
    window
        .update(&mut visual, |_, w, cx| {
            w.dispatch_action(Box::new(crate::document_lightbox::Close), cx)
        })
        .unwrap();
    visual.run_until_parked();
    assert!(!is_open(window, &mut visual));
}

#[gpui::test]
fn escape_closes_the_document_lightbox(cx: &mut TestAppContext) {
    let (window, mut visual, _scratch) = open(cx, "report.md", "text");
    assert!(is_open(window, &mut visual));
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    assert!(!is_open(window, &mut visual));
}

#[gpui::test]
fn backdrop_click_closes_but_a_panel_click_does_not(cx: &mut TestAppContext) {
    let (window, mut visual, _scratch) = open(cx, "report.md", "text");
    let panel = visual.debug_bounds("document-lightbox-panel").unwrap();
    visual.simulate_click(panel.center(), Default::default());
    assert!(is_open(window, &mut visual));
    visual.simulate_click(gpui::point(gpui::px(2.), gpui::px(2.)), Default::default());
    visual.run_until_parked();
    assert!(!is_open(window, &mut visual));
}

#[gpui::test]
fn oversized_files_show_a_message_and_offer_the_default_app(cx: &mut TestAppContext) {
    let big = "x".repeat(crate::document_lightbox::MAX_BYTES as usize + 1);
    let (window, mut visual, _scratch) = open(cx, "huge.md", &big);
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(is_open(window, &mut visual));
    assert!(visual.debug_bounds("document-lightbox-message").is_some());
    assert!(
        visual
            .debug_bounds("document-lightbox-message-open")
            .is_some()
    );
    assert!(visual.debug_bounds("document-lightbox-copy").is_none());
}

#[gpui::test]
fn the_lightbox_learns_when_a_toast_sits_above_it(cx: &mut TestAppContext) {
    let (window, mut visual, _scratch) = open(cx, "page.html", "<p>hi</p>");
    let covered = |window: gpui::WindowHandle<Shell>, visual: &mut VisualTestContext| {
        window
            .update(visual, |shell, _, cx| {
                shell
                    .document_lightbox
                    .as_ref()
                    .unwrap()
                    .0
                    .read(cx)
                    .covered()
            })
            .unwrap()
    };
    assert!(!covered(window, &mut visual));
    window
        .update(&mut visual, |shell, _, cx| {
            shell.error = Some("Something failed".into());
            cx.notify();
        })
        .unwrap();
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(covered(window, &mut visual));
}

#[gpui::test]
fn a_cloud_file_in_memory_opens_without_reading_disk_or_offering_the_default_app(
    cx: &mut TestAppContext,
) {
    cx.update(|cx| {
        crate::input::bind(cx);
        crate::document_lightbox::bind(cx);
        super::bind_keys(cx);
        cx.set_reduce_motion(true);
    });
    let (window, _, _) = super::tests::window(cx);
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    window
        .update(&mut visual, |shell, w, cx| {
            shell.focus_active(w, cx);
            w.dispatch_action(
                Box::new(crate::document_lightbox::Open {
                    // Only names the file: it does not exist locally.
                    path: "/nonexistent/cloud/src/main.rs".into(),
                    title: "main.rs".into(),
                    cloud: Some(crate::document_lightbox::CloudDoc {
                        kind: crate::document_lightbox::Kind::Code,
                        text: Some("fn main() {}".into()),
                        web_root: None,
                    }),
                }),
                cx,
            )
        })
        .unwrap();
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(is_open(window, &mut visual));
    assert!(visual.debug_bounds("document-lightbox-markdown").is_some());
    assert!(visual.debug_bounds("document-lightbox-copy").is_some());
    assert!(
        visual
            .debug_bounds("document-lightbox-open-external")
            .is_none()
    );
}
