//! A file chip or link clicked in a chat previews the file (`PreviewPath`), wherever the chat is.
use super::*;
use crate::{document_lightbox::Kind, file_preview::PreviewPath};
use gpui::{TestAppContext, VisualTestContext};
use std::path::PathBuf;

/// A temp folder with files in it, removed when the test ends, even if it fails.
struct Scratch(PathBuf);
impl Scratch {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "monoptah-preview-path-{tag}-{}",
            monocode_core::mono::new_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
        path
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const SOURCE: &str = "export const a = 1;\n";

fn source_lines() -> String {
    (1..=30)
        .map(|n| format!("const line{n} = {n};\n"))
        .collect()
}

fn start(cx: &mut TestAppContext) -> (gpui::WindowHandle<Shell>, VisualTestContext) {
    cx.update(|cx| {
        crate::input::bind(cx);
        crate::document_lightbox::bind(cx);
        crate::image_lightbox::bind(cx);
        super::bind_keys(cx);
        cx.set_reduce_motion(true);
    });
    let (window, _, _) = super::tests::window(cx);
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    window
        .update(&mut visual, |shell, w, cx| shell.focus_active(w, cx))
        .unwrap();
    visual.run_until_parked();
    (window, visual)
}

/// What a click on a chat's file chip sends.
fn preview(window: gpui::WindowHandle<Shell>, visual: &mut VisualTestContext, target: &str) {
    let target = target.to_owned();
    window
        .update(visual, |_, w, cx| {
            w.dispatch_action(Box::new(PreviewPath(target)), cx)
        })
        .unwrap();
    visual.run_until_parked();
    visual.refresh().unwrap();
}

fn text(path: &std::path::Path) -> String {
    path.to_string_lossy().into_owned()
}

#[gpui::test]
fn an_html_file_opens_as_a_page_from_its_own_folder_with_scripts_off(cx: &mut TestAppContext) {
    let dir = Scratch::new("html");
    let page = dir.file("report.html", b"<img src=img/a.png><script>x()</script>");
    dir.file("img/a.png", b"png");
    let (window, mut visual) = start(cx);
    preview(window, &mut visual, &text(&page));
    window
        .update(&mut visual, |shell, _, cx| {
            let (view, _) = shell.document_lightbox.as_ref().expect("the page opened");
            let view = view.read(cx);
            assert_eq!(view.kind(), Some(Kind::Html));
            assert_eq!(view.path(), page);
            assert!(shell.image_lightbox.is_none() && shell.cannot_preview.is_none());
            // A web view exists on macOS only, and there the page runs no script.
            #[cfg(target_os = "macos")]
            assert_eq!(view.html_runs_scripts(cx), Some(false));
        })
        .unwrap();
    assert!(visual.debug_bounds("document-lightbox").is_some());
}

#[gpui::test]
fn a_markdown_file_opens_rendered_and_draws_its_local_pictures(cx: &mut TestAppContext) {
    let dir = Scratch::new("markdown");
    let doc = dir.file(
        "docs/notes.md",
        b"# Notes\n\n![chart](img/chart.png)\n\n![web](https://example.com/a.png)\n\nafter\n",
    );
    dir.file("docs/img/chart.png", b"png");
    let (window, mut visual) = start(cx);
    preview(window, &mut visual, &text(&doc));
    window
        .update(&mut visual, |shell, _, cx| {
            let (view, _) = shell
                .document_lightbox
                .as_ref()
                .expect("the document opened");
            let view = view.read(cx);
            assert_eq!(view.kind(), Some(Kind::Markdown));
            assert_eq!(view.path(), doc);
            assert_eq!(view.base(), doc.parent());
        })
        .unwrap();
    assert!(visual.debug_bounds("document-lightbox-markdown").is_some());
    let drawn = (0..8).filter(|ix| {
        visual
            .debug_bounds(Box::leak(format!("document-image-{ix}").into_boxed_str()))
            .is_some()
    });
    assert_eq!(
        drawn.count(),
        1,
        "the local picture is drawn and the remote one is not"
    );
}

#[gpui::test]
fn an_mdx_file_opens_as_a_markdown_document(cx: &mut TestAppContext) {
    let dir = Scratch::new("mdx");
    let doc = dir.file("page.mdx", b"# Page\n");
    let (window, mut visual) = start(cx);
    preview(window, &mut visual, &text(&doc));
    window
        .update(&mut visual, |shell, _, cx| {
            let (view, _) = shell
                .document_lightbox
                .as_ref()
                .expect("the document opened");
            assert_eq!(view.read(cx).kind(), Some(Kind::Markdown));
        })
        .unwrap();
}

#[gpui::test]
fn a_picture_opens_in_the_image_lightbox(cx: &mut TestAppContext) {
    let dir = Scratch::new("image");
    let picture = dir.file("shot.png", b"png");
    let svg = dir.file("logo.svg", b"<svg xmlns='http://www.w3.org/2000/svg'/>");
    let (window, mut visual) = start(cx);
    for file in [&picture, &svg] {
        preview(window, &mut visual, &text(file));
        window
            .update(&mut visual, |shell, _, _| {
                assert!(shell.image_lightbox.is_some(), "{file:?}");
                assert!(shell.document_lightbox.is_none());
                shell.image_lightbox = None;
            })
            .unwrap();
    }
}

#[gpui::test]
fn source_with_a_line_suffix_opens_in_the_editor_at_that_line(cx: &mut TestAppContext) {
    let dir = Scratch::new("source");
    let lines = source_lines();
    let file = dir.file("src/app.ts", lines.as_bytes());
    let (window, mut visual) = start(cx);
    for suffix in [":12", ":12:1", "#L12"] {
        preview(window, &mut visual, &format!("{}{suffix}", text(&file)));
        window
            .update(&mut visual, |shell, _, cx| {
                assert!(shell.document_lightbox.is_none() && shell.image_lightbox.is_none());
                let review = shell.active_review(cx).expect("the file pane is open");
                let editor = review.read(cx).active_editor().expect("the editor is open");
                assert_eq!(editor.read(cx).path, text(&file), "{suffix}");
                let offset: usize = lines.lines().take(11).map(|l| l.len() + 1).sum();
                assert_eq!(
                    editor.read(cx).input.read(cx).selection_range(),
                    offset..offset,
                    "{suffix}"
                );
            })
            .unwrap();
    }
}

#[gpui::test]
fn a_page_or_document_with_a_line_suffix_opens_as_source(cx: &mut TestAppContext) {
    let dir = Scratch::new("suffix");
    let page = dir.file("report.html", b"<p>one</p>\n<p>two</p>\n<p>three</p>\n");
    let doc = dir.file("notes.md", b"one\ntwo\nthree\n");
    let (window, mut visual) = start(cx);
    for file in [&page, &doc] {
        preview(window, &mut visual, &format!("{}:2", text(file)));
        window
            .update(&mut visual, |shell, _, cx| {
                assert!(shell.document_lightbox.is_none(), "{file:?}");
                let review = shell.active_review(cx).expect("the file pane is open");
                let editor = review.read(cx).active_editor().expect("the editor is open");
                assert_eq!(editor.read(cx).path, text(file));
            })
            .unwrap();
    }
}

#[gpui::test]
fn a_binary_file_says_it_cannot_be_previewed_and_offers_two_ways_out(cx: &mut TestAppContext) {
    let dir = Scratch::new("binary");
    let blob = dir.file("archive.bin", &[0, 1, 2, 3, 0xff, 0xfe]);
    let (window, mut visual) = start(cx);
    preview(window, &mut visual, &text(&blob));
    window
        .update(&mut visual, |shell, _, cx| {
            let (view, _) = shell.cannot_preview.as_ref().expect("the notice opened");
            let view = view.read(cx);
            assert_eq!(view.path(), blob);
            assert_eq!(view.size(), 6);
            assert!(shell.document_lightbox.is_none() && shell.image_lightbox.is_none());
        })
        .unwrap();
    for selector in [
        "cannot-preview",
        "cannot-preview-size",
        "cannot-preview-path",
        "cannot-preview-reveal",
        "cannot-preview-open",
    ] {
        assert!(visual.debug_bounds(selector).is_some(), "{selector}");
    }
    let close = visual.debug_bounds("cannot-preview-close").unwrap();
    visual.simulate_click(close.center(), Default::default());
    visual.run_until_parked();
    window
        .update(&mut visual, |shell, _, _| {
            assert!(shell.cannot_preview.is_none())
        })
        .unwrap();
}

#[gpui::test]
fn a_missing_file_is_reported_not_ignored(cx: &mut TestAppContext) {
    let dir = Scratch::new("missing");
    let gone = dir.0.join("gone.html");
    let (window, mut visual) = start(cx);
    preview(window, &mut visual, &text(&gone));
    window
        .update(&mut visual, |shell, _, _| {
            assert_eq!(
                shell.error.as_deref(),
                Some(format!("File not found: {}", text(&gone)).as_str())
            );
            assert!(shell.document_lightbox.is_none() && shell.cannot_preview.is_none());
        })
        .unwrap();
}

#[gpui::test]
fn a_file_opens_when_no_session_pane_is_active(cx: &mut TestAppContext) {
    let dir = Scratch::new("no-pane");
    let file = dir.file("main.rs", SOURCE.as_bytes());
    let (window, mut visual) = start(cx);
    window
        .update(&mut visual, |shell, _, _| shell.workspace.active = None)
        .unwrap();
    preview(window, &mut visual, &text(&file));
    window
        .update(&mut visual, |shell, _, cx| {
            assert!(shell.workspace.active.is_some());
            let review = shell.active_review(cx).expect("the file pane is open");
            let editor = review.read(cx).active_editor().expect("the editor is open");
            assert_eq!(editor.read(cx).path, text(&file));
        })
        .unwrap();
}

#[gpui::test]
fn a_file_opens_from_a_mono_tab_that_has_no_active_pane(cx: &mut TestAppContext) {
    let dir = Scratch::new("mono");
    let file = dir.file("main.rs", SOURCE.as_bytes());
    let (window, mut visual) = start(cx);
    window
        .update(&mut visual, |shell, w, cx| {
            shell
                .mono_manager()
                .update(cx, |m, cx| m.create(Vec::new(), w, cx));
        })
        .unwrap();
    visual.run_until_parked();
    // A Mono tab of the tab strip is the active tab with no session pane behind it. A click on
    // a file chip used to stop at `open_review`, which has nothing to attach the file to.
    window
        .update(&mut visual, |shell, _, cx| {
            assert!(shell.mono_active(cx));
            shell.workspace.active = None;
        })
        .unwrap();
    preview(window, &mut visual, &text(&file));
    window
        .update(&mut visual, |shell, _, cx| {
            assert!(!shell.mono_active(cx), "the file takes the window over");
            let pane = shell.workspace.active.expect("the file has a pane");
            assert!(pane < 0, "an independent file pane");
            let review = shell.active_review(cx).expect("the file pane is open");
            let editor = review.read(cx).active_editor().expect("the editor is open");
            assert_eq!(editor.read(cx).path, text(&file));
        })
        .unwrap();
}

#[gpui::test]
fn open_path_from_a_mono_tab_without_an_active_pane_opens_the_file_too(cx: &mut TestAppContext) {
    let dir = Scratch::new("mono-open");
    let file = dir.file("main.rs", SOURCE.as_bytes());
    let (window, mut visual) = start(cx);
    window
        .update(&mut visual, |shell, w, cx| {
            shell
                .mono_manager()
                .update(cx, |m, cx| m.create(Vec::new(), w, cx));
        })
        .unwrap();
    visual.run_until_parked();
    window
        .update(&mut visual, |shell, w, cx| {
            shell.workspace.active = None;
            w.dispatch_action(Box::new(crate::file_editor::OpenPath(text(&file))), cx);
        })
        .unwrap();
    visual.run_until_parked();
    window
        .update(&mut visual, |shell, _, cx| {
            assert!(shell.workspace.active.is_some());
            let review = shell.active_review(cx).expect("the file pane is open");
            let editor = review.read(cx).active_editor().expect("the editor is open");
            assert_eq!(editor.read(cx).path, text(&file));
        })
        .unwrap();
}

#[gpui::test]
fn a_new_preview_replaces_the_one_before(cx: &mut TestAppContext) {
    let dir = Scratch::new("replace");
    let picture = dir.file("shot.png", b"png");
    let doc = dir.file("notes.md", b"# Notes\n");
    let (window, mut visual) = start(cx);
    preview(window, &mut visual, &text(&picture));
    preview(window, &mut visual, &text(&doc));
    window
        .update(&mut visual, |shell, _, cx| {
            assert!(shell.image_lightbox.is_none());
            assert_eq!(shell.document_preview_path(cx), Some(doc.clone()));
        })
        .unwrap();
}

#[gpui::test]
fn a_forwarded_link_never_goes_to_the_active_cloud_session(cx: &mut TestAppContext) {
    use super::cloud::CloudMark;
    use monocode_core::cloud_sessions::CloudBackendKind;
    let (window, mut visual) = start(cx);
    window
        .update(&mut visual, |shell, w, cx| {
            let id = shell.workspace.active.unwrap_or(7);
            shell.workspace.active = Some(id);
            shell.cloud_marks.insert(
                id,
                CloudMark {
                    local_project: "/work/monocode".into(),
                    codespace: "octo-abc".into(),
                    kind: CloudBackendKind::Codespaces,
                },
            );
            // A click in the cloud session's own transcript is read from the cloud workspace.
            shell.preview_path("/workspaces/widgets/a.md", w, cx);
            assert!(shell.cloud_view.is_some());
            shell.dismiss_cloud_view(cx);
            // A link a floating Mono forwarded names a file of this machine: no cloud read.
            shell.preview_local_path("/workspaces/widgets/a.md", w, cx);
            assert!(shell.cloud_view.is_none());
        })
        .unwrap();
}
