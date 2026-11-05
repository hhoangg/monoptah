//! The pending-attachment row and drop highlight shared by the chat composer
//! and the Today note composer: image thumbnails, file chips, the remove
//! button, the "Attaching..." and error lines, the drop target and overlay.
use crate::explorer::DraggedFile;
use crate::theme::*;
use crate::transcript::attachments::{Origin, Photo};
use gpui::{
    AnyElement, App, Context, CursorStyle, Div, Entity, ExternalPaths, MouseButton, Role, Stateful,
    Window, div, prelude::*, px,
};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    rc::Rc,
};

pub(crate) type Photos = HashMap<PathBuf, Entity<Photo>>;

/// Drops the thumbnails of removed attachments (their sprites are released
/// with the entity) and creates one for every new image.
pub(crate) fn sync_photos(paths: &[PathBuf], photos: &mut Photos, cx: &mut App) {
    photos.retain(|path, _| paths.contains(path));
    for path in paths {
        if let Some(mime) = monocode_core::composer::image_mime(path)
            && !photos.contains_key(path)
        {
            let photo = cx.new(|cx| Photo::new(Origin::Path(path.clone()), mime.into(), cx));
            photos.insert(path.clone(), photo);
        }
    }
}

/// Debug selectors of one tray: the row, `<image>-{ix}` for image tiles and
/// `<file>-{ix}` for file chips. `remove` is the remove button's element id.
#[derive(Clone, Copy)]
pub(crate) struct Ids {
    pub tray: &'static str,
    pub image: &'static str,
    pub file: &'static str,
    pub remove: &'static str,
}

pub(crate) struct Tray<'a> {
    pub paths: &'a [PathBuf],
    pub photos: &'a Photos,
    pub attaching: bool,
    pub error: Option<&'a str>,
    pub ids: Ids,
    /// The image tile's accessible label for a file name.
    pub open_label: fn(&str) -> String,
}

pub(crate) type OnRemove = Rc<dyn Fn(usize, &mut Window, &mut App)>;
pub(crate) type OnOpen = Rc<dyn Fn(&Path, &mut Window, &mut App)>;

/// The row, or `None` when there is nothing to show. `on_open` runs when an
/// image tile is clicked.
pub(crate) fn tray(spec: Tray<'_>, on_remove: OnRemove, on_open: OnOpen) -> Option<AnyElement> {
    if spec.paths.is_empty() && spec.error.is_none() && !spec.attaching {
        return None;
    }
    let ids = spec.ids;
    Some(
        div()
            .debug_selector(move || ids.tray.into())
            .px_3()
            .pt_3()
            .flex()
            .flex_wrap()
            .gap_1p5()
            .children(spec.paths.iter().enumerate().map(|(ix, path)| {
                let label = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                let photo = spec.photos.get(path).cloned();
                let image = monocode_core::composer::image_mime(path).is_some();
                let on_remove = on_remove.clone();
                let remove = div()
                    .id((ids.remove, ix))
                    .debug_selector(move || format!("{}-{ix}", ids.remove))
                    .role(Role::Button)
                    .aria_label(format!("Remove {label}"))
                    .cursor_pointer()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(image, |d| {
                        d.absolute()
                            .top(px(-4.))
                            .right(px(-4.))
                            .size(px(20.))
                            .rounded_full()
                            .bg(content(0.2))
                            .text_color(content(0.7))
                            .hover(|s| s.bg(content(0.3)).text_color(content(1.)))
                    })
                    .when(!image, |d| {
                        d.h(px(26.))
                            .px(px(6.))
                            .rounded_md()
                            .bg(content(0.1))
                            .text_color(content(1.))
                            .hover(|s| s.bg(content(0.15)))
                    })
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(icon("close", if image { 12. } else { 10. }))
                    .on_click(move |_, window, cx| {
                        cx.stop_propagation();
                        on_remove(ix, window, cx);
                    });
                if image {
                    let on_open = on_open.clone();
                    let path = path.clone();
                    return div()
                        .id(("attachment", ix))
                        .relative()
                        .size(px(36.))
                        .flex_shrink_0()
                        .role(Role::Button)
                        .focusable()
                        .debug_selector(move || format!("{}-{ix}", ids.image))
                        .aria_label((spec.open_label)(&label))
                        .cursor(CursorStyle::PointingHand)
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(move |_, window, cx| {
                            cx.stop_propagation();
                            on_open(&path, window, cx);
                        })
                        .children(photo)
                        .child(remove)
                        .into_any_element();
                }
                div()
                    .id(("attachment", ix))
                    .debug_selector(move || format!("{}-{ix}", ids.file))
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(content(0.06))
                    .text_size(px(11.))
                    .child(crate::file_icons::file_type_icon(&label, false, false, 14.))
                    .child(div().max_w(px(190.)).truncate().child(label))
                    .child(remove)
                    .into_any_element()
            }))
            .when(spec.attaching, |d| {
                d.child(
                    div()
                        .text_size(px(11.))
                        .text_color(content(0.5))
                        .child("Attaching…"),
                )
            })
            .when_some(spec.error.map(str::to_owned), |d, error| {
                d.child(
                    div()
                        .max_w_full()
                        .min_w_0()
                        .text_size(px(11.))
                        .text_color(warning_color().opacity(0.9))
                        .child(error),
                )
            })
            .into_any_element(),
    )
}

/// "Drop files to attach" over a composer while a file drag is over it.
pub(crate) fn drop_overlay(over: bool, radius: f32) -> Option<AnyElement> {
    over.then(|| {
        div()
            .absolute()
            .inset_0()
            .rounded(px(radius))
            .bg(accent_color().opacity(0.08))
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(12.))
            .text_color(content(0.7))
            .child("Drop files to attach")
            .into_any_element()
    })
}

/// How a drop target talks to its entity. `drop` gets an empty list when a
/// drag carried no file.
pub(crate) struct DropHooks<T> {
    pub over: fn(&T) -> bool,
    pub set_over: fn(&mut T, bool, &mut Context<T>),
    pub drop: fn(&mut T, Vec<PathBuf>, &mut Window, &mut Context<T>),
}

impl<T> Clone for DropHooks<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for DropHooks<T> {}

/// Wires external and explorer file drags onto `target`: tracks whether one is
/// over it (`set_over`), cancels with Escape and hands dropped paths to `drop`.
pub(crate) fn bind_drop<T: 'static>(
    target: Stateful<Div>,
    hooks: DropHooks<T>,
    cx: &mut Context<T>,
) -> Stateful<Div> {
    let set = hooks.set_over;
    target
        .on_drag_move::<ExternalPaths>(cx.listener(
            move |this, event: &gpui::DragMoveEvent<ExternalPaths>, _, cx| {
                set(this, event.bounds.contains(&event.event.position), cx);
            },
        ))
        .on_drag_move::<DraggedFile>(cx.listener(
            move |this, event: &gpui::DragMoveEvent<DraggedFile>, _, cx| {
                set(this, event.bounds.contains(&event.event.position), cx);
            },
        ))
        .on_file_drop_exit(cx.listener(move |this, _, _, cx| set(this, false, cx)))
        .on_mouse_exit(cx.listener(move |this, _, _, cx| set(this, false, cx)))
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| set(this, false, cx)),
        )
        .capture_key_down(
            cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" && (hooks.over)(this) {
                    cx.stop_active_drag(window);
                    set(this, false, cx);
                    cx.stop_propagation();
                }
            }),
        )
        .on_drop(cx.listener(move |this, paths: &ExternalPaths, window, cx| {
            set(this, false, cx);
            (hooks.drop)(this, paths.paths().to_vec(), window, cx);
        }))
        .on_drop(cx.listener(move |this, file: &DraggedFile, window, cx| {
            set(this, false, cx);
            (hooks.drop)(this, vec![file.0.clone()], window, cx);
        }))
}

/// Shown when a drag carried no file (an image dragged out of a web page).
pub(crate) const NO_FILE_DRAGGED: &str =
    "This drag did not provide a file. Save the image, then drag the saved file here.";

/// What a paste brought: copied files, or (when there are none) images.
pub(crate) enum Pasted {
    Paths(Vec<PathBuf>),
    Images(Vec<gpui::Image>),
}

/// The clipboard's files, else its images; `None` when it holds neither.
pub(crate) fn pasted(cx: &App) -> Option<Pasted> {
    let clipboard = cx.read_from_clipboard()?;
    let paths: Vec<_> = clipboard
        .entries()
        .iter()
        .filter_map(|entry| match entry {
            gpui::ClipboardEntry::ExternalPaths(paths) => Some(paths.paths().to_vec()),
            _ => None,
        })
        .flatten()
        .collect();
    if !paths.is_empty() {
        return Some(Pasted::Paths(paths));
    }
    let images: Vec<_> = clipboard
        .entries()
        .iter()
        .filter_map(|entry| match entry {
            gpui::ClipboardEntry::Image(image) => Some(image.clone()),
            _ => None,
        })
        .collect();
    (!images.is_empty()).then_some(Pasted::Images(images))
}

/// The multi select file picker; `None` when cancelled or it failed.
pub(crate) fn pick_files(
    cx: &mut App,
) -> impl std::future::Future<Output = Option<Vec<PathBuf>>> + use<> {
    let paths = cx.prompt_for_paths(gpui::PathPromptOptions {
        files: true,
        directories: false,
        multiple: true,
        prompt: Some("Attach files".into()),
    });
    async move { paths.await.ok().and_then(Result::ok).flatten() }
}
