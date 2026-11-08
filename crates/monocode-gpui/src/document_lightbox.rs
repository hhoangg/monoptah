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
    sync::Arc,
};

/// A native web view draws above every GPUI overlay, so it is only shown while
/// nothing with a higher priority than the lightbox is up.
fn shows_web_view(kind: Option<Kind>, covered: bool) -> bool {
    kind == Some(Kind::Html) && !covered
}

/// Files above this size are not read; the user is offered the default app.
pub(crate) const MAX_BYTES: u64 = 5 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Markdown,
    Html,
    /// Source code or plain text, shown as a code block. Only files of a cloud workspace
    /// are opened as this kind (by [`CloudDoc`]); local notes keep Markdown and HTML.
    Code,
}

/// A file of a cloud workspace shown by the lightbox. `kind` replaces the extension lookup.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CloudDoc {
    pub kind: Kind,
    /// The whole file when it was read into memory. `Open::path` then only names it (its
    /// extension picks the code language) and nothing is read from disk.
    pub text: Option<Arc<str>>,
    /// For HTML opened from disk: the folder the page may read its assets from.
    pub web_root: Option<PathBuf>,
}

/// The fence that shows `text` as code, longer than any run of backticks inside it.
fn code_block(text: &str, path: &Path) -> String {
    let mut longest = 0;
    let mut run = 0;
    for ch in text.chars() {
        run = if ch == '`' { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    let fence = "`".repeat((longest + 1).max(3));
    let language = path
        .extension()
        .and_then(|ext| ext.to_str())
        .filter(|ext| ext.chars().all(|c| c.is_ascii_alphanumeric()))
        .unwrap_or("");
    format!("{fence}{language}\n{text}\n{fence}")
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
    /// Set for a file of a cloud workspace; `None` for every local file.
    pub cloud: Option<CloudDoc>,
}

actions!(document_lightbox, [Close]);

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
    /// The file exists on disk, so "Open with default app" can open it.
    on_disk: bool,
    body: Body,
    error: Option<String>,
    /// A higher-priority overlay is up, so a native web view must not show.
    covered: bool,
    focus: FocusHandle,
    restore: Option<FocusHandle>,
}
impl EventEmitter<Closed> for DocumentLightbox {}

impl DocumentLightbox {
    pub(crate) fn new(open: Open, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let restore = window.focused(cx);
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let cloud = open.cloud;
        let kind = cloud
            .as_ref()
            .map(|doc| doc.kind)
            .or_else(|| kind_of(&open.path));
        let in_memory = cloud.as_ref().is_some_and(|doc| doc.text.is_some());
        let mut this = Self {
            path: open.path.clone(),
            title: open.title,
            kind,
            text: None,
            on_disk: !in_memory,
            body: Body::Loading,
            error: None,
            covered: false,
            focus,
            restore,
        };
        if kind.is_none() {
            this.body = Body::Message("This file type can't be previewed here.".into());
            return this;
        }
        if let Some(CloudDoc {
            text: Some(text), ..
        }) = &cloud
        {
            this.loaded(Ok(Loaded::Text(text.to_string())), cx);
            return this;
        }
        #[cfg(target_os = "macos")]
        if let Some(CloudDoc {
            kind: Kind::Html,
            web_root: Some(root),
            ..
        }) = &cloud
        {
            // A downloaded page loads from disk with its assets; there is no size limit.
            let (path, root) = (open.path.clone(), root.clone());
            this.body = Body::Html(cx.new(|_| {
                let mut html = Html::from_file(path, root);
                html.set_interactive(true);
                html
            }));
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
                    Some(Kind::Code) => {
                        let shown = code_block(&text, &self.path);
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
                    None => Body::Message("This file type can't be previewed here.".into()),
                };
                self.text = Some(text);
                body
            }
        };
    }

    pub(crate) fn set_covered(&mut self, covered: bool) {
        self.covered = covered;
    }

    #[cfg(test)]
    pub(crate) fn covered(&self) -> bool {
        self.covered
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
            Body::Html(view) if !shows_web_view(self.kind, self.covered) => {
                view.read(cx).hide();
                div().size_full().into_any_element()
            }
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
                .when(self.on_disk, |d| {
                    d.child(Self::button(
                        "document-lightbox-message-open",
                        "Open with default app",
                        cx,
                        |this, _, cx| this.open_external(cx),
                    ))
                })
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
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.close(window, cx)),
            )
            .on_click(|_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .id("document-lightbox-panel")
                    .debug_selector(|| "document-lightbox-panel".into())
                    .w((viewport.width.min(px(960.)) - px(48.)).max(px(0.)))
                    .h((viewport.height - px(48.)).max(px(0.)))
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
                            .when(self.on_disk, |d| {
                                d.child(Self::button(
                                    "document-lightbox-open-external",
                                    "Open with default app",
                                    cx,
                                    |this, _, cx| this.open_external(cx),
                                ))
                            })
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
    fn the_web_view_shows_only_for_html_and_only_when_nothing_covers_it() {
        assert!(shows_web_view(Some(Kind::Html), false));
        assert!(!shows_web_view(Some(Kind::Html), true));
        assert!(!shows_web_view(Some(Kind::Markdown), false));
        assert!(!shows_web_view(None, false));
    }

    #[test]
    fn code_is_fenced_longer_than_any_backtick_run_it_contains() {
        assert_eq!(
            code_block("fn a() {}", Path::new("src/a.rs")),
            "```rs\nfn a() {}\n```"
        );
        let shown = code_block("x ```` y", Path::new("notes"));
        assert!(shown.starts_with("`````\nx ```` y"), "{shown}");
        assert!(shown.ends_with("\n`````"), "{shown}");
        // An odd extension never reaches the info string.
        assert!(code_block("a", Path::new("a.r s!")).starts_with("```\n"));
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
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
