//! The files attached to a note, drawn inside its bubble: images inline (one
//! at its own shape, several as a grid of square thumbnails) and other files
//! as rows. Everything opens the stored copy with the system default app, as
//! if it were opened from Finder; there is no in-app viewer. A right-click (or
//! the row's "..." button) offers Open, Show in Finder and Copy.
use super::Today;
use crate::{
    file_icons::file_type_icon,
    popover::{Anchor, Menu, MenuAction, MenuEvent, MenuItem, menu_popover},
    theme::*,
    transcript::attachments::{Origin, Photo},
};
use gpui::{
    AnyElement, App, ClipboardEntry, ClipboardItem, Context, ElementId, Entity, ExternalPaths,
    MouseButton, MouseDownEvent, ObjectFit, Pixels, Point, Subscription, Task, Window, div,
    prelude::*, px,
};
use monocode_core::{
    files,
    today::{Activity, Note, NoteAttachment},
};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, Instant},
};

/// A square thumbnail's side in the grid.
const TILE: f32 = 92.;
/// The tallest a single inline image grows.
const SINGLE_MAX_HEIGHT: f32 = 240.;
/// Tiles shown before the rest fold into a "+N" overlay.
const MAX_TILES: usize = 6;
/// How long a "is the stored copy there" answer is trusted.
const PRESENT_FOR: Duration = Duration::from_secs(2);

/// What the system is asked to do with a stored copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AttachmentAction {
    Open,
    Reveal,
}

/// Runs an action for a stored copy. Blocking work goes to a background task,
/// whose result is the error to show, if any. Tests swap it for a recorder.
pub(crate) type Actions =
    Rc<dyn Fn(AttachmentAction, PathBuf, &mut App) -> Task<Result<(), String>>>;

fn system_actions() -> Actions {
    Rc::new(|action, path, cx| {
        cx.background_executor().spawn(async move {
            match action {
                AttachmentAction::Open => files::open_path_with_default_app(&path),
                AttachmentAction::Reveal => files::reveal_path(&path),
            }
        })
    })
}

/// A choice in the attachment menu.
#[derive(Clone, Copy)]
enum Act {
    Open,
    Reveal,
    Copy,
}

struct OpenMenu {
    menu: Entity<Menu>,
    /// The note the menu was opened on; only that note draws it.
    note: String,
    at: Point<Pixels>,
    _events: Subscription,
}

/// Today's state for note attachments.
pub(crate) struct NoteFiles {
    pub(crate) actions: Actions,
    /// Decoded thumbnails by stored path. Dropping one frees its GPU sprite.
    photos: RefCell<HashMap<PathBuf, Entity<Photo>>>,
    /// Whether a stored copy exists, and when that was looked up.
    present: RefCell<HashMap<PathBuf, (Instant, bool)>>,
    menu: Option<OpenMenu>,
}

impl Default for NoteFiles {
    fn default() -> Self {
        Self {
            actions: system_actions(),
            photos: RefCell::default(),
            present: RefCell::default(),
            menu: None,
        }
    }
}

impl NoteFiles {
    fn exists(&self, path: &Path) -> bool {
        let mut present = self.present.borrow_mut();
        if let Some((at, found)) = present.get(path)
            && at.elapsed() < PRESENT_FOR
        {
            return *found;
        }
        let found = path.is_file();
        present.insert(path.to_path_buf(), (Instant::now(), found));
        found
    }

    /// The thumbnail of `path`, decoded in the background the first time.
    fn photo(&self, att: &NoteAttachment, path: &Path, whole: bool, cx: &mut App) -> Entity<Photo> {
        self.photos
            .borrow_mut()
            .entry(path.to_path_buf())
            .or_insert_with(|| {
                let origin = Origin::Path(path.to_path_buf());
                let mime = att
                    .mime
                    .clone()
                    .or_else(|| monocode_core::composer::image_mime(path).map(str::to_string))
                    .unwrap_or_else(|| "image/png".into());
                cx.new(|cx| {
                    if whole {
                        Photo::whole(origin, mime.into(), cx)
                    } else {
                        Photo::new(origin, mime.into(), cx)
                    }
                })
            })
            .clone()
    }

    /// Drops every thumbnail that no shown tile needs any more.
    fn prune(&self, activity: &[Activity]) {
        let wanted: HashSet<PathBuf> = activity
            .iter()
            .filter_map(|item| match item {
                Activity::Note(note) => Some(note),
                Activity::Event(_) => None,
            })
            .flat_map(|note| {
                tiles(note)
                    .into_iter()
                    .filter_map(|(_, att)| att.path.clone())
            })
            .map(PathBuf::from)
            .collect();
        self.photos
            .borrow_mut()
            .retain(|path, _| wanted.contains(path));
        self.present
            .borrow_mut()
            .retain(|path, _| wanted.contains(path));
    }

    #[cfg(test)]
    pub(super) fn photo_count(&self) -> usize {
        self.photos.borrow().len()
    }
}

/// The images of a note that are drawn: all of them for one, the first
/// `MAX_TILES` for a grid. Paired with their index among the note's files.
fn tiles(note: &Note) -> Vec<(usize, &NoteAttachment)> {
    note.attachments
        .iter()
        .enumerate()
        .filter(|(_, att)| att.is_image())
        .take(MAX_TILES)
        .collect()
}

/// The overlay on the last tile when `hidden` more images are not drawn.
fn more_label(hidden: usize) -> String {
    format!("+{hidden}")
}

/// A single image's size in points: its own shape, at most `SINGLE_MAX_HEIGHT`
/// tall. `None` while the shape is unknown.
fn single_size(shape: Option<(u32, u32)>) -> Option<(f32, f32)> {
    let (w, h) = shape.filter(|(w, h)| *w > 0 && *h > 0)?;
    let (w, h) = (w as f32, h as f32);
    let scale = (SINGLE_MAX_HEIGHT / h).min(1.);
    Some(((w * scale).round().max(1.), (h * scale).round().max(1.)))
}

impl Today {
    /// The attachments of one note, or `None` when it has none.
    pub(super) fn note_attachments_view(
        &self,
        note: &Note,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        self.note_files.prune(&self.activity);
        if note.attachments.is_empty() {
            return None;
        }
        let images: Vec<_> = note
            .attachments
            .iter()
            .enumerate()
            .filter(|(_, att)| att.is_image())
            .collect();
        let others: Vec<_> = note
            .attachments
            .iter()
            .enumerate()
            .filter(|(_, att)| !att.is_image())
            .collect();
        let single = images.len() == 1;
        let hidden = images.len().saturating_sub(MAX_TILES);
        let mut column = div()
            .debug_selector(|| "today-note-attachments".into())
            .mt(px(6.))
            .flex()
            .flex_col()
            .gap(px(6.));
        if !images.is_empty() {
            let shown = images.len().min(MAX_TILES);
            let grid = div().flex().flex_wrap().gap(px(4.)).children(
                images
                    .iter()
                    .take(shown)
                    .enumerate()
                    .map(|(pos, (ix, att))| {
                        let more = (hidden > 0 && pos + 1 == shown).then_some(hidden);
                        self.image_tile(note, *ix, pos, att, single, more, cx)
                    })
                    .collect::<Vec<_>>(),
            );
            column = column.child(grid);
        }
        for (pos, (ix, att)) in others.iter().enumerate() {
            column = column.child(self.file_row(note, *ix, pos, att, cx));
        }
        let menu = self
            .note_files
            .menu
            .as_ref()
            .filter(|open| open.note == note.id)
            .map(|open| menu_popover(&open.menu, Anchor::Point(open.at), cx).into_any_element());
        Some(column.children(menu).into_any_element())
    }

    #[allow(clippy::too_many_arguments)]
    fn image_tile(
        &self,
        note: &Note,
        ix: usize,
        pos: usize,
        att: &NoteAttachment,
        single: bool,
        more: Option<usize>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let path = att.path.as_deref().map(PathBuf::from);
        let present = path.as_deref().is_some_and(|p| self.note_files.exists(p));
        let key = format!("{}-{ix}", note.id);
        let mut tile = div()
            .id(ElementId::Name(format!("today-note-image-{key}").into()))
            .debug_selector(move || format!("today-note-image-{pos}"))
            .relative()
            .overflow_hidden()
            .rounded(px(8.))
            .flex_shrink_0()
            .bg(content(0.06))
            .aria_label(format!("Open {}", att.name));
        if let Some(path) = path.clone() {
            tile = tile.cursor_pointer().on_click(cx.listener({
                let path = path.clone();
                move |this, _, _, cx| this.attachment_act(Act::Open, path.clone(), cx)
            }));
            let note_id = note.id.clone();
            tile = tile.on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    this.open_attachment_menu(
                        note_id.clone(),
                        path.clone(),
                        event.position,
                        window,
                        cx,
                    )
                }),
            );
        }
        if !present {
            return tile
                .size(px(TILE))
                .child(missing_tile(format!("today-note-attachment-missing-{ix}")))
                .into_any_element();
        }
        let path = path.expect("a present file has a path");
        let photo = self.note_files.photo(att, &path, single, cx);
        let (source, shape) = {
            let photo = photo.read(cx);
            (photo.source(), photo.shape())
        };
        let tile = if single {
            let size = single_size(att.width.zip(att.height).or(shape));
            match size {
                Some((w, h)) => tile.w(px(w)).max_w_full().aspect_ratio(w / h),
                None => tile.size(px(TILE * 1.5)),
            }
        } else {
            tile.size(px(TILE))
        };
        let tile = match source {
            Some(source) => tile.child(gpui::img(source).size_full().object_fit(if single {
                ObjectFit::Fill
            } else {
                ObjectFit::Cover
            })),
            None => tile,
        };
        tile.children(more.map(|count| {
            div()
                .debug_selector(|| "today-note-image-more".into())
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::black().opacity(0.5))
                .text_color(gpui::white())
                .text_size(px(16.))
                .font_weight(gpui::FontWeight::MEDIUM)
                .child(more_label(count))
        }))
        .into_any_element()
    }

    fn file_row(
        &self,
        note: &Note,
        ix: usize,
        pos: usize,
        att: &NoteAttachment,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let path = att.path.as_deref().map(PathBuf::from);
        let present = path.as_deref().is_some_and(|p| self.note_files.exists(p));
        let key = format!("{}-{ix}", note.id);
        let mut row = div()
            .id(ElementId::Name(format!("today-note-file-{key}").into()))
            .debug_selector(move || format!("today-note-file-{pos}"))
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(8.))
            .py(px(6.))
            .rounded(px(6.))
            .bg(content(0.06))
            .aria_label(format!("Open {}", att.name))
            .child(file_type_icon(&att.name, false, false, 16.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(content(if present { 0.8 } else { 0.45 }))
                    .child(att.name.clone()),
            );
        row = if present {
            row.child(
                div()
                    .flex_shrink_0()
                    .text_size(px(11.))
                    .text_color(content(0.45))
                    .child(crate::file_editor::media::file_size(att.size)),
            )
        } else {
            row.child(
                div()
                    .debug_selector(move || format!("today-note-attachment-missing-{ix}"))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .text_size(px(11.))
                    .text_color(content(0.45))
                    .child(icon("circle-alert", 12.))
                    .child("File missing"),
            )
        };
        if let Some(path) = path {
            let note_id = note.id.clone();
            row = row
                .cursor_pointer()
                .hover(|s| s.bg(content(0.09)))
                .on_click(cx.listener({
                    let path = path.clone();
                    move |this, _, _, cx| this.attachment_act(Act::Open, path.clone(), cx)
                }))
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener({
                        let (note_id, path) = (note_id.clone(), path.clone());
                        move |this, event: &MouseDownEvent, window, cx| {
                            this.open_attachment_menu(
                                note_id.clone(),
                                path.clone(),
                                event.position,
                                window,
                                cx,
                            )
                        }
                    }),
                )
                .child(
                    div()
                        .id(ElementId::Name(
                            format!("today-note-file-more-{key}").into(),
                        ))
                        .flex_shrink_0()
                        .cursor_pointer()
                        .rounded(px(4.))
                        .p(px(2.))
                        .hover(|s| s.bg(content(0.1)))
                        .aria_label(format!("Actions for {}", att.name))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(
                            cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                                cx.stop_propagation();
                                this.open_attachment_menu(
                                    note_id.clone(),
                                    path.clone(),
                                    event.position(),
                                    window,
                                    cx,
                                )
                            }),
                        )
                        .child(icon("more-horizontal", 14.)),
                );
        }
        row.into_any_element()
    }

    fn open_attachment_menu(
        &mut self,
        note: String,
        path: PathBuf,
        at: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let items = vec![
            MenuItem::from(MenuAction::new("today-attachment-open", "Open")),
            MenuItem::from(MenuAction::new("today-attachment-reveal", "Show in Finder")),
            MenuItem::from(MenuAction::new("today-attachment-copy", "Copy")),
        ];
        let menu = cx.new(|cx| Menu::new("Attachment actions", items, window, cx));
        let events = cx.subscribe_in(&menu, window, move |this, _, event: &MenuEvent, _, cx| {
            this.note_files.menu = None;
            if let MenuEvent::Picked(id, _) = event {
                let act = match id.as_ref() {
                    "today-attachment-open" => Some(Act::Open),
                    "today-attachment-reveal" => Some(Act::Reveal),
                    "today-attachment-copy" => Some(Act::Copy),
                    _ => None,
                };
                if let Some(act) = act {
                    this.attachment_act(act, path.clone(), cx);
                }
            }
            cx.notify();
        });
        self.note_files.menu = Some(OpenMenu {
            menu,
            note,
            at,
            _events: events,
        });
        cx.notify();
    }

    /// Does `act` to a stored copy, or says why it cannot.
    fn attachment_act(&mut self, act: Act, path: PathBuf, cx: &mut Context<Self>) {
        // A fresh look: the copy may have gone since the last frame.
        if !path.is_file() {
            self.note_files.present.borrow_mut().remove(&path);
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            self.action_error = Some(format!("{name} is missing from this machine."));
            cx.notify();
            return;
        }
        let action = match act {
            Act::Copy => {
                cx.write_to_clipboard(ClipboardItem {
                    entries: vec![ClipboardEntry::ExternalPaths(ExternalPaths(
                        std::iter::once(path).collect(),
                    ))],
                });
                return;
            }
            Act::Open => AttachmentAction::Open,
            Act::Reveal => AttachmentAction::Reveal,
        };
        let task = (self.note_files.actions)(action, path, cx);
        cx.spawn(async move |this, cx| {
            if let Err(error) = task.await {
                let _ = this.update(cx, |this, cx| {
                    this.action_error = Some(error);
                    cx.notify();
                });
            }
        })
        .detach();
    }
}

/// What a tile shows in place of a copy that is gone.
fn missing_tile(selector: String) -> AnyElement {
    div()
        .debug_selector(move || selector.clone())
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(4.))
        .text_size(px(10.))
        .text_color(content(0.45))
        .child(icon("image-add", 18.))
        .child("File missing")
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::super::list::testkit::*;
    use super::*;
    use gpui::{Modifiers, TestAppContext, VisualTestContext};
    use monocode_core::today::{AttachmentKind, DayMode, Group};
    use std::cell::RefCell;

    type Calls = Rc<RefCell<Vec<(AttachmentAction, PathBuf)>>>;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "monoptah-note-files-{}-{tag}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
    fn image(dir: &Path, name: &str, width: u32, height: u32) -> NoteAttachment {
        let path = dir.join(name);
        image::RgbaImage::new(width, height)
            .save_with_format(&path, image::ImageFormat::Png)
            .unwrap();
        NoteAttachment {
            name: name.into(),
            file: format!("today-attachments/t/n/{name}"),
            size: std::fs::metadata(&path).unwrap().len(),
            kind: AttachmentKind::Image,
            mime: Some("image/png".into()),
            width: Some(width),
            height: Some(height),
            path: Some(path.to_string_lossy().into_owned()),
        }
    }
    fn file(dir: &Path, name: &str) -> NoteAttachment {
        let path = dir.join(name);
        std::fs::write(&path, vec![b'x'; 2048]).unwrap();
        NoteAttachment {
            name: name.into(),
            file: format!("today-attachments/t/n/{name}"),
            size: 2048,
            kind: AttachmentKind::File,
            mime: None,
            width: None,
            height: None,
            path: Some(path.to_string_lossy().into_owned()),
        }
    }
    fn missing_file(dir: &Path, name: &str) -> NoteAttachment {
        let mut att = file(dir, name);
        std::fs::remove_file(dir.join(name)).unwrap();
        att.size = 10;
        att
    }
    fn note(id: &str, body: &str, attachments: Vec<NoteAttachment>) -> Note {
        Note {
            id: id.into(),
            task_id: "id-a".into(),
            day: "2026-11-01".into(),
            at: 0,
            mono_id: None,
            body: body.into(),
            attachments,
            edited_at: None,
            session_id: None,
        }
    }
    /// A selected task with these notes, and a recorder in place of the system.
    fn show(mode: DayMode, notes: Vec<Note>, calls: &Calls) -> impl FnOnce(&mut Today) + 'static {
        let calls = calls.clone();
        move |today| {
            let day = if mode == DayMode::Past {
                "2026-10-31"
            } else {
                "2026-11-01"
            };
            today.view = Some(view(
                mode,
                day,
                vec![row(Group::Today, task("a", "Review it"), 0)],
            ));
            today.selected = Some("id-a".into());
            today.activity = notes.into_iter().map(Activity::Note).collect();
            today.note_files.actions = Rc::new(move |action, path, _| {
                calls.borrow_mut().push((action, path));
                Task::ready(Ok(()))
            });
        }
    }
    fn click(visual: &mut VisualTestContext, selector: &'static str) {
        visual.refresh().unwrap();
        let bounds = visual
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} is drawn"));
        visual.simulate_click(bounds.center(), Default::default());
        visual.refresh().unwrap();
    }
    fn right_click(visual: &mut VisualTestContext, selector: &'static str) {
        visual.refresh().unwrap();
        let bounds = visual
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} is drawn"));
        visual.simulate_mouse_down(
            bounds.center(),
            gpui::MouseButton::Right,
            Modifiers::default(),
        );
        visual.refresh().unwrap();
    }
    fn stored(att: &NoteAttachment) -> PathBuf {
        PathBuf::from(att.path.clone().unwrap())
    }

    #[test]
    fn the_overflow_overlay_counts_the_hidden_images() {
        assert_eq!(more_label(2), "+2");
    }
    #[test]
    fn a_single_image_keeps_its_shape_and_stops_at_the_height_cap() {
        assert_eq!(single_size(Some((100, 50))), Some((100., 50.)));
        assert_eq!(single_size(Some((960, 480))), Some((480., 240.)));
        assert_eq!(single_size(Some((0, 10))), None);
        assert_eq!(single_size(None), None);
    }

    #[gpui::test]
    fn one_image_renders_one_tile(cx: &mut TestAppContext) {
        let dir = scratch("one");
        let notes = vec![note("n1", "", vec![image(&dir, "a.png", 40, 20)])];
        let (_, mut visual) = page(
            cx,
            Show::Detail,
            show(DayMode::Current, notes, &Calls::default()),
        );
        assert!(has(&mut visual, "today-note-attachments"));
        assert!(has(&mut visual, "today-note-image-0"));
        assert!(!has(&mut visual, "today-note-image-1"));
        assert!(!has(&mut visual, "today-note-image-more"));
        assert!(!has(&mut visual, "today-note-attachment-missing-0"));
    }
    #[gpui::test]
    fn three_images_render_a_grid(cx: &mut TestAppContext) {
        let dir = scratch("three");
        let images = (0..3)
            .map(|i| image(&dir, &format!("{i}.png"), 30, 60))
            .collect();
        let notes = vec![note("n1", "look", images)];
        let (_, mut visual) = page(
            cx,
            Show::Detail,
            show(DayMode::Current, notes, &Calls::default()),
        );
        for tile in [
            "today-note-image-0",
            "today-note-image-1",
            "today-note-image-2",
        ] {
            assert!(has(&mut visual, tile));
        }
        assert!(!has(&mut visual, "today-note-image-3"));
        assert!(!has(&mut visual, "today-note-image-more"));
        // Grid tiles are squares; a single image is not.
        let b = |v: &mut VisualTestContext, s: &'static str| v.debug_bounds(s).unwrap().size;
        let tile = b(&mut visual, "today-note-image-0");
        assert_eq!(tile.width, tile.height);
    }
    #[gpui::test]
    fn eight_images_show_six_tiles_and_the_overflow(cx: &mut TestAppContext) {
        let dir = scratch("eight");
        let images = (0..8)
            .map(|i| image(&dir, &format!("{i}.png"), 16, 16))
            .collect();
        let notes = vec![note("n1", "", images)];
        let (_, mut visual) = page(
            cx,
            Show::Detail,
            show(DayMode::Current, notes, &Calls::default()),
        );
        assert!(has(&mut visual, "today-note-image-5"));
        assert!(!has(&mut visual, "today-note-image-6"));
        assert!(has(&mut visual, "today-note-image-more"));
    }
    #[gpui::test]
    fn exactly_six_images_have_no_overflow(cx: &mut TestAppContext) {
        let dir = scratch("six");
        let images = (0..6)
            .map(|i| image(&dir, &format!("{i}.png"), 16, 16))
            .collect();
        let notes = vec![note("n1", "", images)];
        let (_, mut visual) = page(
            cx,
            Show::Detail,
            show(DayMode::Current, notes, &Calls::default()),
        );
        assert!(has(&mut visual, "today-note-image-5"));
        assert!(!has(&mut visual, "today-note-image-more"));
    }
    #[gpui::test]
    fn a_file_renders_a_row(cx: &mut TestAppContext) {
        let dir = scratch("row");
        let notes = vec![note("n1", "", vec![file(&dir, "report.pdf")])];
        let (_, mut visual) = page(
            cx,
            Show::Detail,
            show(DayMode::Current, notes, &Calls::default()),
        );
        assert!(has(&mut visual, "today-note-file-0"));
        assert!(!has(&mut visual, "today-note-image-0"));
        assert!(!has(&mut visual, "today-note-attachment-missing-0"));
    }
    #[gpui::test]
    fn clicking_an_image_or_a_file_opens_the_stored_copy(cx: &mut TestAppContext) {
        let dir = scratch("open");
        let (picture, report) = (image(&dir, "a.png", 8, 8), file(&dir, "report.pdf"));
        let expected = [stored(&picture), stored(&report)];
        let notes = vec![note("n1", "", vec![picture, report])];
        let calls = Calls::default();
        let (_, mut visual) = page(cx, Show::Detail, show(DayMode::Current, notes, &calls));
        click(&mut visual, "today-note-image-0");
        click(&mut visual, "today-note-file-0");
        assert_eq!(
            *calls.borrow(),
            vec![
                (AttachmentAction::Open, expected[0].clone()),
                (AttachmentAction::Open, expected[1].clone())
            ]
        );
    }
    #[gpui::test]
    fn the_menu_opens_reveals_and_copies(cx: &mut TestAppContext) {
        let dir = scratch("menu");
        let picture = image(&dir, "a.png", 8, 8);
        let path = stored(&picture);
        let notes = vec![note("n1", "", vec![picture])];
        let calls = Calls::default();
        let (_, mut visual) = page(cx, Show::Detail, show(DayMode::Current, notes, &calls));

        right_click(&mut visual, "today-note-image-0");
        click(&mut visual, "menu-action-today-attachment-reveal");
        assert_eq!(
            *calls.borrow(),
            vec![(AttachmentAction::Reveal, path.clone())]
        );

        right_click(&mut visual, "today-note-image-0");
        click(&mut visual, "menu-action-today-attachment-open");
        assert_eq!(calls.borrow()[1], (AttachmentAction::Open, path.clone()));

        right_click(&mut visual, "today-note-image-0");
        click(&mut visual, "menu-action-today-attachment-copy");
        assert_eq!(calls.borrow().len(), 2, "copy goes to the pasteboard only");
        let copied = visual.read_from_clipboard().expect("something was copied");
        match copied.entries.as_slice() {
            [ClipboardEntry::ExternalPaths(paths)] => assert_eq!(paths.paths(), [path]),
            other => panic!("expected the file itself, got {other:?}"),
        }
    }
    #[gpui::test]
    fn right_clicking_a_file_row_opens_the_menu(cx: &mut TestAppContext) {
        let dir = scratch("more");
        let notes = vec![note("n1", "", vec![file(&dir, "report.pdf")])];
        let (_, mut visual) = page(
            cx,
            Show::Detail,
            show(DayMode::Current, notes, &Calls::default()),
        );
        right_click(&mut visual, "today-note-file-0");
        assert!(has(&mut visual, "menu-action-today-attachment-open"));
    }
    #[gpui::test]
    fn a_missing_copy_shows_it_and_opens_nothing(cx: &mut TestAppContext) {
        let dir = scratch("gone");
        let gone = missing_file(&dir, "old.zip");
        let notes = vec![note("n1", "", vec![gone])];
        let calls = Calls::default();
        let (window, mut visual) = page(cx, Show::Detail, show(DayMode::Current, notes, &calls));
        assert!(has(&mut visual, "today-note-attachment-missing-0"));
        click(&mut visual, "today-note-file-0");
        right_click(&mut visual, "today-note-file-0");
        click(&mut visual, "menu-action-today-attachment-reveal");
        right_click(&mut visual, "today-note-file-0");
        click(&mut visual, "menu-action-today-attachment-copy");
        assert!(calls.borrow().is_empty());
        assert!(visual.read_from_clipboard().is_none());
        let error = window
            .read_with(&visual, |page, cx| page.today.read(cx).action_error.clone())
            .unwrap();
        assert!(error.is_some_and(|text| text.contains("old.zip")));
    }
    #[gpui::test]
    fn a_missing_image_shows_the_missing_tile(cx: &mut TestAppContext) {
        let dir = scratch("gone-image");
        let picture = image(&dir, "a.png", 8, 8);
        std::fs::remove_file(stored(&picture)).unwrap();
        let notes = vec![note("n1", "", vec![picture])];
        let calls = Calls::default();
        let (_, mut visual) = page(cx, Show::Detail, show(DayMode::Current, notes, &calls));
        assert!(has(&mut visual, "today-note-attachment-missing-0"));
        click(&mut visual, "today-note-image-0");
        assert!(calls.borrow().is_empty());
    }
    #[gpui::test]
    fn a_past_day_still_shows_and_opens_attachments(cx: &mut TestAppContext) {
        let dir = scratch("past");
        let picture = image(&dir, "a.png", 8, 8);
        let path = stored(&picture);
        let notes = vec![note("n1", "done", vec![picture])];
        let calls = Calls::default();
        let (_, mut visual) = page(cx, Show::Detail, show(DayMode::Past, notes, &calls));
        assert!(has(&mut visual, "today-note-image-0"));
        click(&mut visual, "today-note-image-0");
        assert_eq!(*calls.borrow(), vec![(AttachmentAction::Open, path)]);
    }
    #[gpui::test]
    fn a_note_with_only_attachments_has_no_empty_body(cx: &mut TestAppContext) {
        let dir = scratch("nobody");
        let notes = vec![
            note("n1", "  ", vec![image(&dir, "a.png", 8, 8)]),
            note("n2", "words", Vec::new()),
        ];
        let (_, mut visual) = page(
            cx,
            Show::Detail,
            show(DayMode::Current, notes, &Calls::default()),
        );
        assert!(has(&mut visual, "today-note-body"));
        assert!(has(&mut visual, "today-note-image-0"));
    }
    #[gpui::test]
    fn an_attachment_only_note_draws_no_body(cx: &mut TestAppContext) {
        let dir = scratch("nobody-only");
        let notes = vec![note("n1", "", vec![file(&dir, "a.txt")])];
        let (_, mut visual) = page(
            cx,
            Show::Detail,
            show(DayMode::Current, notes, &Calls::default()),
        );
        assert!(!has(&mut visual, "today-note-body"));
        assert!(has(&mut visual, "today-note-file-0"));
    }
    #[gpui::test]
    fn thumbnails_are_kept_only_for_shown_tiles(cx: &mut TestAppContext) {
        let dir = scratch("prune");
        let images = (0..8)
            .map(|i| image(&dir, &format!("{i}.png"), 16, 16))
            .collect();
        let notes = vec![note("n1", "", images)];
        let (window, mut visual) = page(
            cx,
            Show::Detail,
            show(DayMode::Current, notes, &Calls::default()),
        );
        visual.refresh().unwrap();
        let count = |visual: &VisualTestContext| {
            window
                .read_with(visual, |page, cx| {
                    page.today.read(cx).note_files.photo_count()
                })
                .unwrap()
        };
        assert_eq!(count(&visual), 6);
        // Another task's notes replace them: the old thumbnails go.
        window
            .update(&mut visual, |page, _, cx| {
                page.today.update(cx, |today, cx| {
                    today.activity = vec![Activity::Note(note("n2", "x", Vec::new()))];
                    cx.notify();
                })
            })
            .unwrap();
        visual.refresh().unwrap();
        assert_eq!(count(&visual), 0);
    }
}
