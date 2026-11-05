//! A modal that shows a Markdown or HTML file inside the app, opened from a
//! note attachment. It follows `image_lightbox`: an `Open` action, a dark
//! backdrop, and Close on Escape or a backdrop click.
use crate::{
    monos::html::Html,
    theme::{black, content, shell_fill, white},
    transcript::TranscriptView,
};
use gpui::{
    App, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, KeyBinding, MouseButton, Role,
    SharedString, Window, actions, div, prelude::*, px,
};
use monocode_core::files;
use std::{
    io::Read,
    path::{Path, PathBuf},
};

/// Files above this size are not read; the user is offered the default app.
pub(crate) const MAX_BYTES: u64 = 5 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Markdown,
    Html,
}

/// What the lightbox can show for a file, by extension.
pub(crate) fn kind_of(path: &Path) -> Option<Kind> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "md" | "markdown" => Some(Kind::Markdown),
        "html" | "htm" => Some(Kind::Html),
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(namespace = document_lightbox, no_json)]
pub(crate) struct Open {
    pub path: PathBuf,
    pub title: SharedString,
}

actions!(document_lightbox, [Close, CopyText, OpenExternal]);

pub(crate) fn bind(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("escape", Close, Some("DocumentLightbox"))]);
}

#[derive(Debug, PartialEq)]
enum Loaded {
    Text(String),
    TooLarge(u64),
}

/// Reads the file as text, refusing anything over `MAX_BYTES`.
fn read_text(path: &Path) -> Result<Loaded, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let len = file.metadata().map_err(|e| e.to_string())?.len();
    if len > MAX_BYTES {
        return Ok(Loaded::TooLarge(len));
    }
    let mut bytes = Vec::new();
    // Bounded again in case the file grows between the check and the read.
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_BYTES {
        return Ok(Loaded::TooLarge(bytes.len() as u64));
    }
    Ok(Loaded::Text(String::from_utf8_lossy(&bytes).into_owned()))
}

enum Body {
    Loading,
    Markdown(Entity<TranscriptView>),
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Html(Entity<Html>),
    /// The message to show instead of the document.
    Message(String),
}

pub(crate) struct Closed;
pub(crate) struct DocumentLightbox {
    path: PathBuf,
    title: SharedString,
    kind: Option<Kind>,
    text: Option<String>,
    body: Body,
    error: Option<String>,
    focus: FocusHandle,
    restore: Option<FocusHandle>,
}
impl EventEmitter<Closed> for DocumentLightbox {}

impl DocumentLightbox {
    pub(crate) fn new(open: Open, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let restore = window.focused(cx);
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let kind = kind_of(&open.path);
        let mut this = Self {
            path: open.path.clone(),
            title: open.title,
            kind,
            text: None,
            body: Body::Loading,
            error: None,
            focus,
            restore,
        };
        if kind.is_none() {
            this.body = Body::Message("This file type can't be previewed here.".into());
            return this;
        }
        let path = open.path;
        let work = cx
            .background_executor()
            .spawn(async move { read_text(&path) });
        cx.spawn(async move |this, cx| {
            let loaded = work.await;
            let _ = this.update(cx, |this, cx| {
                this.loaded(loaded, cx);
                cx.notify();
            });
        })
        .detach();
        this
    }

    fn loaded(&mut self, loaded: Result<Loaded, String>, cx: &mut Context<Self>) {
        self.body = match loaded {
            Err(error) => Body::Message(format!("Couldn't read this file: {error}")),
            Ok(Loaded::TooLarge(len)) => Body::Message(format!(
                "This file is {:.1} MB. Files over {} MB aren't previewed here.",
                len as f64 / (1024. * 1024.),
                MAX_BYTES / (1024 * 1024)
            )),
            Ok(Loaded::Text(text)) => {
                let body = match self.kind {
                    Some(Kind::Markdown) => {
                        let shown = text.clone();
                        Body::Markdown(cx.new(|cx| TranscriptView::document(shown, cx)))
                    }
                    #[cfg(target_os = "macos")]
                    Some(Kind::Html) => {
                        let shown = text.clone();
                        Body::Html(cx.new(|_| {
                            let mut html = Html::new(shown);
                            html.set_interactive(true);
                            html
                        }))
                    }
                    #[cfg(not(target_os = "macos"))]
                    Some(Kind::Html) => Body::Message(
                        "HTML preview needs a web view, which this platform doesn't have here."
                            .into(),
                    ),
                    None => Body::Loading,
                };
                self.text = Some(text);
                body
            }
        };
    }

    pub(crate) fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(restore) = self.restore.take() {
            window.focus(&restore, cx);
        }
        cx.emit(Closed);
        cx.stop_propagation();
    }

    fn copy_text(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = &self.text {
            cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
        }
    }

    fn open_external(&mut self, cx: &mut Context<Self>) {
        let path = self.path.clone();
        let work = cx
            .background_executor()
            .spawn(async move { files::open_path_with_default_app(&path) });
        cx.spawn(async move |this, cx| {
            if let Err(error) = work.await {
                let _ = this.update(cx, |this, cx| {
                    this.error = Some(error);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn button(
        id: &'static str,
        label: &'static str,
        cx: &mut Context<Self>,
        run: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> impl IntoElement {
        div()
            .id(id)
            .debug_selector(move || id.into())
            .role(Role::Button)
            .aria_label(label)
            .px(px(10.))
            .py(px(4.))
            .rounded(px(6.))
            .text_size(px(12.))
            .text_color(content(0.8))
            .bg(content(0.06))
            .cursor_pointer()
            .hover(|s| s.bg(content(0.12)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                run(this, window, cx)
            }))
            .child(label)
    }
}

impl Render for DocumentLightbox {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = window.viewport_size();
        let has_text = self.text.is_some();
        let message = match &self.body {
            Body::Message(message) => Some(message.clone()),
            _ => None,
        };
        let body = match &self.body {
            Body::Loading => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(content(0.5))
                .child("Loading...")
                .into_any_element(),
            Body::Markdown(view) => div()
                .id("document-lightbox-markdown")
                .debug_selector(|| "document-lightbox-markdown".into())
                .size_full()
                .child(view.clone())
                .into_any_element(),
            Body::Html(view) => div()
                .id("document-lightbox-html")
                .debug_selector(|| "document-lightbox-html".into())
                .size_full()
                .child(view.clone())
                .into_any_element(),
            Body::Message(_) => div()
                .id("document-lightbox-message")
                .debug_selector(|| "document-lightbox-message".into())
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(10.))
                .text_color(content(0.7))
                .child(message.unwrap_or_default())
                .child(Self::button(
                    "document-lightbox-message-open",
                    "Open with default app",
                    cx,
                    |this, _, cx| this.open_external(cx),
                ))
                .into_any_element(),
        };
        div()
            .id("document-lightbox")
            .debug_selector(|| "document-lightbox".into())
            .role(Role::Dialog)
            .aria_label(format!("Document preview: {}", self.title))
            .key_context("DocumentLightbox")
            .track_focus(&self.focus)
            .absolute()
            .top_0()
            .left_0()
            .w(viewport.width)
            .h(viewport.height)
            .overflow_hidden()
            .flex()
            .items_center()
            .justify_center()
            .bg(black(0.7))
            .on_action(cx.listener(|this, _: &Close, window, cx| this.close(window, cx)))
            .on_action(cx.listener(|this, _: &CopyText, _, cx| this.copy_text(cx)))
            .on_action(cx.listener(|this, _: &OpenExternal, _, cx| this.open_external(cx)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.close(window, cx)),
            )
            .on_click(|_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .id("document-lightbox-panel")
                    .debug_selector(|| "document-lightbox-panel".into())
                    .w(viewport.width.min(px(960.)) - px(48.))
                    .h(viewport.height - px(48.))
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .rounded(px(12.))
                    .border_1()
                    .border_color(white(0.12))
                    .bg(shell_fill())
                    .text_color(content(1.))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .h(px(44.))
                            .flex_shrink_0()
                            .px(px(12.))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .border_b_1()
                            .border_color(content(0.1))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(13.))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .child(self.title.clone()),
                            )
                            .when(self.error.is_some(), |d| {
                                d.child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(content(0.6))
                                        .child(self.error.clone().unwrap_or_default()),
                                )
                            })
                            .when(has_text, |d| {
                                d.child(Self::button(
                                    "document-lightbox-copy",
                                    "Copy",
                                    cx,
                                    |this, _, cx| this.copy_text(cx),
                                ))
                            })
                            .child(Self::button(
                                "document-lightbox-open-external",
                                "Open with default app",
                                cx,
                                |this, _, cx| this.open_external(cx),
                            ))
                            .child(Self::button(
                                "document-lightbox-close",
                                "Close",
                                cx,
                                |this, window, cx| this.close(window, cx),
                            )),
                    )
                    .child(div().flex_1().min_h_0().child(body)),
            )
    }
}

pub(crate) fn overlay(view: &Entity<DocumentLightbox>) -> impl IntoElement {
    gpui::deferred(
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .child(view.clone()),
    )
    .with_priority(100)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_follows_the_extension_case_insensitively() {
        for name in ["a.md", "a.MD", "a.markdown", "dir/Notes.Markdown"] {
            assert_eq!(kind_of(Path::new(name)), Some(Kind::Markdown), "{name}");
        }
        for name in ["a.html", "a.HTM", "b.htm", "x/y.Html"] {
            assert_eq!(kind_of(Path::new(name)), Some(Kind::Html), "{name}");
        }
        for name in ["a.png", "a.pdf", "a.mdx", "md", "a.", "a.txt", ".md"] {
            assert_eq!(kind_of(Path::new(name)), None, "{name}");
        }
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "monoptah-doc-lightbox-{}-{tag}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn small_files_are_read_and_oversized_ones_are_refused() {
        let dir = scratch("read");
        let small = dir.join("a.md");
        std::fs::write(&small, "# Hi").unwrap();
        assert_eq!(read_text(&small), Ok(Loaded::Text("# Hi".into())));
        let big = dir.join("big.md");
        std::fs::write(&big, vec![b'x'; MAX_BYTES as usize + 1]).unwrap();
        assert_eq!(read_text(&big), Ok(Loaded::TooLarge(MAX_BYTES + 1)));
        let edge = dir.join("edge.md");
        std::fs::write(&edge, vec![b'x'; MAX_BYTES as usize]).unwrap();
        assert!(matches!(read_text(&edge), Ok(Loaded::Text(_))));
        assert!(read_text(&dir.join("missing.md")).is_err());
    }
}
