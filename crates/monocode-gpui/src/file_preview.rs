//! Previewing a file a chat mentions. A click on a file chip or a markdown file link dispatches
//! [`PreviewPath`]; the shell (and the floating Mono window, which forwards it to the shell)
//! decides by what the file is: a web page, a markdown document or a picture opens in its
//! lightbox, source and other text opens in the code editor (at the line a `:12` or `#L12`
//! suffix names), and a file that cannot be shown says so in [`CannotPreview`].
use crate::{modal, theme::content};
use gpui::{
    App, Context, EventEmitter, FocusHandle, Focusable, IntoElement, KeyDownEvent, Role,
    SharedString, Window, div, prelude::*, px,
};
use monocode_core::files;
use std::{
    io::Read,
    path::{Path, PathBuf},
};

/// A file link was clicked in a chat. The string is the link as bound by the chat: an absolute
/// path with an optional `:line:col`, or a link that still has to be resolved.
#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(namespace = file_preview, no_json)]
pub(crate) struct PreviewPath(pub String);

/// Pictures the image lightbox draws. SVG is rasterized by the image loader, never run.
const IMAGE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "svg", "avif", "bmp", "ico",
];

/// How much of a file is read to tell text from binary.
const SNIFF_BYTES: usize = 8 * 1024;

/// What a file is, and so where it opens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Route {
    /// A web page, shown by the document lightbox with its folder readable and scripts off.
    Html,
    /// A markdown document, rendered by the document lightbox. `mdx` is not known to the
    /// lightbox's own extension lookup, so it is opened with an explicit kind.
    Markdown { mdx: bool },
    /// A picture, shown by the image lightbox.
    Image,
    /// Source or other text, shown by the code editor.
    Editor,
    /// A binary file, a folder, or anything else that cannot be shown.
    Unsupported { size: u64, folder: bool },
    /// Nothing exists at the path.
    Missing,
    /// The path exists but could not be read (permissions and the like).
    Unreadable(String),
}

fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// Whether `bytes` (the head of a file) look like text: no NUL and valid UTF-8. A character cut
/// in half by the end of the sample does not count against it.
fn looks_like_text(bytes: &[u8]) -> bool {
    if bytes.contains(&0) {
        return false;
    }
    match std::str::from_utf8(bytes) {
        Ok(_) => true,
        Err(error) => error.error_len().is_none(),
    }
}

/// Decides where `path` opens. Reads at most [`SNIFF_BYTES`] of the file, and only when its
/// extension does not already say what it is. Blocking, and cheap enough for a click.
pub(crate) fn classify(path: &Path) -> Route {
    let meta = match std::fs::metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Route::Missing,
        Err(error) => return Route::Unreadable(error.to_string()),
    };
    if meta.is_dir() {
        return Route::Unsupported {
            size: 0,
            folder: true,
        };
    }
    match extension(path).as_str() {
        "html" | "htm" => return Route::Html,
        "md" | "markdown" => return Route::Markdown { mdx: false },
        "mdx" => return Route::Markdown { mdx: true },
        ext if IMAGE_EXTENSIONS.contains(&ext) => return Route::Image,
        _ => {}
    }
    let mut head = Vec::with_capacity(SNIFF_BYTES);
    match std::fs::File::open(path)
        .and_then(|file| file.take(SNIFF_BYTES as u64).read_to_end(&mut head))
    {
        Ok(_) if looks_like_text(&head) => Route::Editor,
        Ok(_) => Route::Unsupported {
            size: meta.len(),
            folder: false,
        },
        Err(error) => Route::Unreadable(error.to_string()),
    }
}

/// Where a click opens `path`. A line suffix means the reader wants that line, so a page or a
/// document (whose rendering has no lines) opens as source in the editor; a picture ignores it.
pub(crate) fn destination(route: Route, location: Option<(usize, usize)>) -> Route {
    match route {
        Route::Html | Route::Markdown { .. } if location.is_some() => Route::Editor,
        other => other,
    }
}

/// "12.3 KB" for the size line of [`CannotPreview`].
pub(crate) fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024. && unit + 1 < UNITS.len() {
        value /= 1024.;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

pub(crate) fn file_name(path: &Path) -> SharedString {
    path.file_name()
        .map_or_else(
            || path.to_string_lossy().into_owned(),
            |name| name.to_string_lossy().into_owned(),
        )
        .into()
}

/// The label of the reveal button, as the link menu words it.
pub(crate) fn reveal_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "Reveal in Finder"
    } else if cfg!(target_os = "windows") {
        "Reveal in File Explorer"
    } else {
        "Open Containing Folder"
    }
}

pub(crate) struct Closed;

/// The notice for a file that cannot be previewed: what it is, how big, and two ways out.
pub(crate) struct CannotPreview {
    path: PathBuf,
    size: u64,
    folder: bool,
    error: Option<String>,
    focus: FocusHandle,
    restore: Option<FocusHandle>,
}
impl EventEmitter<Closed> for CannotPreview {}
impl Focusable for CannotPreview {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl CannotPreview {
    pub(crate) fn new(
        path: PathBuf,
        size: u64,
        folder: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let restore = window.focused(cx);
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        Self {
            path,
            size,
            folder,
            error: None,
            focus,
            restore,
        }
    }

    #[cfg(test)]
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    #[cfg(test)]
    pub(crate) fn size(&self) -> u64 {
        self.size
    }

    pub(crate) fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(restore) = self.restore.take() {
            window.focus(&restore, cx);
        }
        cx.emit(Closed);
        cx.stop_propagation();
    }

    /// Reveals or opens the file off the UI thread; a failure is shown in the card.
    fn run(&mut self, reveal: bool, cx: &mut Context<Self>) {
        let path = self.path.clone();
        let work = cx.background_executor().spawn(async move {
            if reveal {
                files::reveal_path(&path)
            } else {
                files::open_path_with_default_app(&path)
            }
        });
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

    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.key == "escape" {
            self.close(window, cx);
        }
    }
}

impl Render for CannotPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity();
        let what = if self.folder { "folder" } else { "file" };
        let detail = if self.folder {
            "This is a folder.".to_owned()
        } else {
            format!("Size: {}", human_size(self.size))
        };
        let card = modal::card().child(
            modal::card_body()
                .gap_2()
                .child(modal::title(format!("Cannot preview this {what}")))
                .child(modal::body(detail).debug_selector(|| "cannot-preview-size".into()))
                .child(
                    div()
                        .id("cannot-preview-path")
                        .debug_selector(|| "cannot-preview-path".into())
                        .text_size(px(12.))
                        .line_height(px(17.))
                        .text_color(content(0.8))
                        .font_family(crate::fonts::mono())
                        .whitespace_normal()
                        .child(self.path.to_string_lossy().into_owned()),
                )
                .when_some(self.error.clone(), |d, error| {
                    d.child(
                        div()
                            .text_size(px(12.))
                            .text_color(content(0.6))
                            .child(error),
                    )
                })
                .child(
                    modal::actions()
                        .flex_wrap()
                        .child(
                            modal::ghost_button("cannot-preview-close", "Close")
                                .debug_selector(|| "cannot-preview-close".into())
                                .cursor_pointer()
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.close(window, cx)),
                                ),
                        )
                        .child(
                            modal::ghost_button("cannot-preview-reveal", reveal_label())
                                .debug_selector(|| "cannot-preview-reveal".into())
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, _, cx| this.run(true, cx))),
                        )
                        .child(
                            modal::primary_button("cannot-preview-open", "Open with Default App")
                                .debug_selector(|| "cannot-preview-open".into())
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, _, cx| this.run(false, cx))),
                        ),
                ),
        );
        div()
            .id("cannot-preview")
            .debug_selector(|| "cannot-preview".into())
            .role(Role::Dialog)
            .aria_label("Cannot preview this file")
            .key_context("CannotPreview")
            .track_focus(&self.focus)
            .absolute()
            .inset_0()
            .on_key_down(cx.listener(Self::key))
            .child(modal::overlay("cannot-preview", card, move |window, cx| {
                this.update(cx, |this, cx| this.close(window, cx))
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "monoptah-file-preview-{}-{tag}",
            monocode_core::mono::new_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn files_route_by_what_they_are() {
        let dir = scratch("route");
        let write = |name: &str, bytes: &[u8]| {
            let path = dir.join(name);
            std::fs::write(&path, bytes).unwrap();
            path
        };
        assert_eq!(classify(&write("a.html", b"<p>")), Route::Html);
        assert_eq!(classify(&write("a.HTM", b"<p>")), Route::Html);
        assert_eq!(
            classify(&write("a.md", b"# x")),
            Route::Markdown { mdx: false }
        );
        assert_eq!(
            classify(&write("a.mdx", b"# x")),
            Route::Markdown { mdx: true }
        );
        for name in [
            "a.png", "a.JPG", "a.jpeg", "a.gif", "a.webp", "a.svg", "a.avif", "a.bmp", "a.ico",
        ] {
            assert_eq!(classify(&write(name, b"x")), Route::Image, "{name}");
        }
        for name in ["a.ts", "a.rs", "a.json", "notes.txt", "run.log", "Makefile"] {
            assert_eq!(
                classify(&write(name, b"let a = 1;\n")),
                Route::Editor,
                "{name}"
            );
        }
        // Text is recognized by content, whatever the extension says.
        assert_eq!(
            classify(&write("a.weird", "héllo\n".as_bytes())),
            Route::Editor
        );
        assert_eq!(classify(&write("empty.dat", b"")), Route::Editor);
        assert_eq!(
            classify(&write("a.bin", &[0x7f, b'E', b'L', b'F', 0, 1, 2])),
            Route::Unsupported {
                size: 7,
                folder: false
            }
        );
        assert_eq!(
            classify(&write("a.pdf", &[0xff, 0xfe, 0xfd, 0xfc])),
            Route::Unsupported {
                size: 4,
                folder: false
            }
        );
        assert_eq!(
            classify(&dir),
            Route::Unsupported {
                size: 0,
                folder: true
            }
        );
        assert_eq!(classify(&dir.join("gone.md")), Route::Missing);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_character_cut_by_the_sample_end_is_still_text() {
        let text = "é".repeat(5000);
        let bytes = text.as_bytes();
        // 8 KiB lands in the middle of a two-byte character.
        assert!(!bytes.is_empty() && bytes.len() > SNIFF_BYTES);
        assert!(looks_like_text(&bytes[..SNIFF_BYTES + 1]));
        assert!(!looks_like_text(&[b'a', 0xff, b'b']));
    }

    #[test]
    fn a_line_suffix_sends_pages_and_documents_to_the_editor() {
        let at = Some((12, 1));
        assert_eq!(destination(Route::Html, at), Route::Editor);
        assert_eq!(
            destination(Route::Markdown { mdx: false }, at),
            Route::Editor
        );
        assert_eq!(destination(Route::Html, None), Route::Html);
        assert_eq!(destination(Route::Image, at), Route::Image);
        assert_eq!(destination(Route::Missing, at), Route::Missing);
    }

    #[test]
    fn sizes_read_in_the_largest_whole_unit() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(1023), "1023 B");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(human_size(5 * 1024 * 1024), "5.0 MB");
    }
}
