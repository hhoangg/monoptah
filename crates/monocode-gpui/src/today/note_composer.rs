//! Attachments on the note composer: pending files above the field, the
//! paperclip, paste and drop, validation off the UI thread, and sending a note
//! with its files. The row, the drop target, the drop overlay, the clipboard
//! split and the picker are the chat composer's own (`attachment_tray`).
use super::{Today, locked, now_ms};
use crate::attachment_tray::{
    DropHooks, Ids, NO_FILE_DRAGGED, OnOpen, OnRemove, Pasted, Photos, Tray, bind_drop,
    drop_overlay, pasted, pick_files, sync_photos, tray,
};
use gpui::{AnyElement, Context, Div, Focusable, Stateful, Window, div, prelude::*};
use monocode_core::today::{
    Actor, DayMode,
    attachments::{MAX_FILE_BYTES, MAX_PER_NOTE, check_file, display_name},
};
use std::{
    path::{Path, PathBuf},
    rc::Rc,
};

/// The note composer's pending files and drag state.
pub(crate) struct Attach {
    pub(super) paths: Vec<PathBuf>,
    photos: Photos,
    /// Files added while a check or a send runs; checked afterwards.
    queue: Vec<PathBuf>,
    /// The batch a check is running on, and the files a send is copying.
    checking: Vec<PathBuf>,
    sent: Vec<PathBuf>,
    /// Pasted images, written to temp files that nothing else keeps.
    pasted: Vec<PathBuf>,
    /// Checks and paste writes running. Send waits until none is.
    busy: usize,
    sending: bool,
    pub(super) error: Option<String>,
    pub(super) over: bool,
    /// Bumped when the draft is dropped, so a late result is ignored.
    epoch: u64,
    /// Where pasted images are written, one private folder each.
    pub(super) paste_root: PathBuf,
}

impl Default for Attach {
    fn default() -> Self {
        Self {
            paths: Vec::new(),
            photos: Photos::default(),
            queue: Vec::new(),
            checking: Vec::new(),
            sent: Vec::new(),
            pasted: Vec::new(),
            busy: 0,
            sending: false,
            error: None,
            over: false,
            epoch: 0,
            paste_root: monocode_core::attachment::attachment_dir(),
        }
    }
}

impl Attach {
    /// Forgets the draft's files (another task was picked).
    fn clear(&mut self) {
        self.paths.clear();
        self.photos.clear();
        self.queue.clear();
        self.error = None;
        self.over = false;
        self.epoch += 1;
    }
    fn held(&self, path: &PathBuf) -> bool {
        [&self.paths, &self.queue, &self.checking, &self.sent]
            .iter()
            .any(|list| list.contains(path))
    }
    /// The pasted temp files nothing holds any more.
    fn take_unused(&mut self) -> Vec<PathBuf> {
        let pasted = std::mem::take(&mut self.pasted);
        let (keep, unused): (Vec<_>, Vec<_>) = pasted.into_iter().partition(|p| self.held(p));
        self.pasted = keep;
        unused
    }
}

const ATTACH_IDS: Ids = Ids {
    tray: "today-note-attachments",
    image: "today-note-image",
    file: "today-note-file",
    remove: "today-note-remove",
};

/// One pasted image in its own owner-only folder under `root`, under a plain
/// name, so the note shows `pasted-image.png`.
fn write_pasted_image(root: &Path, image: &gpui::Image) -> Result<PathBuf, String> {
    let dir = root.join(format!("paste-{}", monocode_core::mono::new_id()));
    std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(&dir).map_err(|e| e.to_string())?;
    let name = format!("pasted-image.{}", image.format().extension());
    let written = monocode_core::attachment::write_attachment_limited(
        &dir,
        &name,
        image.bytes(),
        MAX_FILE_BYTES,
    )
    .and_then(|temp| {
        let plain = dir.join(&name);
        std::fs::rename(&temp, &plain)
            .map(|_| plain)
            .map_err(|e| e.to_string())
    });
    if written.is_err() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    written
}

/// Deletes the folder a pasted image lives in; anything else is left alone.
fn remove_pasted(root: &Path, file: &Path) {
    if let Some(dir) = file.parent()
        && dir.parent() == Some(root)
        && dir
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("paste-"))
    {
        let _ = std::fs::remove_dir_all(dir);
    }
}

impl Today {
    /// The composer exists only on a current day with a task selected.
    fn note_editable(&self) -> bool {
        self.selected.is_some() && self.view.as_ref().is_some_and(|v| v.mode != DayMode::Past)
    }
    /// Send is ready with text or at least one pending file.
    pub(super) fn note_has_attachments(&self) -> bool {
        !self.attach.paths.is_empty()
    }
    /// A check, a paste or a send is running: Send waits.
    pub(super) fn note_busy(&self) -> bool {
        self.attach.busy > 0 || self.attach.sending
    }

    /// Another task was picked: forget the draft's files and delete its pasted
    /// temp files (those a running send still copies are deleted afterwards).
    pub(super) fn reset_note_files(&mut self, cx: &mut Context<Self>) {
        self.attach.clear();
        self.sweep_note_files(cx);
    }
    /// Deletes, off the UI thread, pasted temp files nothing holds any more.
    fn sweep_note_files(&mut self, cx: &mut Context<Self>) {
        let unused = self.attach.take_unused();
        if unused.is_empty() {
            return;
        }
        let root = self.attach.paste_root.clone();
        cx.background_executor()
            .spawn(async move {
                for file in unused {
                    remove_pasted(&root, &file);
                }
            })
            .detach();
    }

    /// Adds files (picker, paste, drop): each is checked off the UI thread, and
    /// one that is too large, unreadable or over the limit is reported and left
    /// out while the others are added.
    pub(super) fn add_note_files(&mut self, files: Vec<PathBuf>, cx: &mut Context<Self>) {
        if !self.note_editable() {
            self.attach.queue.clear();
            self.sweep_note_files(cx);
            return;
        }
        let mut added = false;
        for file in files {
            if !self.attach.held(&file) {
                self.attach.queue.push(file);
                added = true;
            }
        }
        if added {
            self.attach.error = None;
        }
        self.pump_note_files(cx);
    }
    /// Starts the next check when nothing else is running.
    fn pump_note_files(&mut self, cx: &mut Context<Self>) {
        if self.attach.busy > 0 || self.attach.sending || self.attach.queue.is_empty() {
            cx.notify();
            return;
        }
        let batch = std::mem::take(&mut self.attach.queue);
        let epoch = self.attach.epoch;
        self.attach.checking = batch.clone();
        self.attach.busy += 1;
        let work = cx.background_executor().spawn(async move {
            batch
                .into_iter()
                .map(|path| {
                    let checked = check_file(&path).map(|_| ());
                    (path, checked)
                })
                .collect::<Vec<_>>()
        });
        cx.spawn(async move |this, cx| {
            let checked = work.await;
            let _ = this.update(cx, |this, cx| this.finish_note_check(epoch, checked, cx));
        })
        .detach();
        cx.notify();
    }
    fn finish_note_check(
        &mut self,
        epoch: u64,
        checked: Vec<(PathBuf, Result<(), String>)>,
        cx: &mut Context<Self>,
    ) {
        self.attach.busy = self.attach.busy.saturating_sub(1);
        self.attach.checking.clear();
        if epoch == self.attach.epoch {
            let mut errors = Vec::new();
            for (path, result) in checked {
                match result {
                    // Dropped twice while the check ran.
                    Ok(()) if self.attach.paths.contains(&path) => {}
                    Err(error) => errors.push(error),
                    Ok(()) if self.attach.paths.len() >= MAX_PER_NOTE => errors.push(format!(
                        "{} was not added: a note holds at most {MAX_PER_NOTE} attachments.",
                        display_name(&path)
                    )),
                    Ok(()) => self.attach.paths.push(path),
                }
            }
            if !errors.is_empty() {
                self.attach.error = Some(errors.join("\n"));
            }
            sync_photos(&self.attach.paths, &mut self.attach.photos, cx);
        }
        self.sweep_note_files(cx);
        self.pump_note_files(cx);
    }
    fn show_note_error(&mut self, error: String, cx: &mut Context<Self>) {
        self.attach.error = Some(error);
        cx.notify();
    }

    pub(super) fn choose_note_files(&mut self, cx: &mut Context<Self>) {
        let picked = pick_files(cx);
        let epoch = self.attach.epoch;
        cx.spawn(async move |this, cx| {
            if let Some(paths) = picked.await {
                let _ = this.update(cx, |this, cx| this.finish_note_pick(epoch, paths, cx));
            }
        })
        .detach();
    }
    /// The picker's result. The panel is not modal, so it may arrive after
    /// another task was selected: then it is dropped.
    pub(super) fn finish_note_pick(
        &mut self,
        epoch: u64,
        paths: Vec<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        if epoch == self.attach.epoch {
            self.add_note_files(paths, cx);
        }
    }

    /// Cmd V in the note field: copied files and images become attachments;
    /// returns false for anything else so the field pastes its text.
    pub(super) fn paste_note_files(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.note_editable() {
            return false;
        }
        let images = match pasted(cx) {
            None => return false,
            Some(Pasted::Paths(paths)) => {
                self.add_note_files(paths, cx);
                return true;
            }
            Some(Pasted::Images(images)) => images,
        };
        // Bytes go to temp files first; the store copies them into the data dir.
        let epoch = self.attach.epoch;
        let root = self.attach.paste_root.clone();
        self.attach.busy += 1;
        let work = cx.background_executor().spawn({
            let root = root.clone();
            async move {
                images
                    .iter()
                    .map(|image| write_pasted_image(&root, image))
                    .collect::<Vec<_>>()
            }
        });
        cx.spawn(async move |this, cx| {
            let written = work.await;
            let _ = this.update(cx, |this, cx| {
                this.attach.busy = this.attach.busy.saturating_sub(1);
                let mut files = Vec::new();
                let mut errors = Vec::new();
                for result in written {
                    match result {
                        Ok(path) => files.push(path),
                        Err(error) => errors.push(error),
                    }
                }
                if epoch == this.attach.epoch {
                    this.attach.pasted.extend(files.iter().cloned());
                    this.add_note_files(files, cx);
                    if !errors.is_empty() {
                        this.show_note_error(errors.join("\n"), cx);
                    }
                } else {
                    cx.background_executor()
                        .spawn(async move {
                            for file in files {
                                remove_pasted(&root, &file);
                            }
                        })
                        .detach();
                }
                this.sweep_note_files(cx);
                this.pump_note_files(cx);
            });
        })
        .detach();
        cx.notify();
        true
    }

    /// Pending files above the field, or `None` when there is nothing to show.
    pub(super) fn note_attachment_tray(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let this = cx.entity().downgrade();
        let remove: OnRemove = Rc::new(move |ix, _, cx| {
            let _ = this.update(cx, |this, cx| {
                // The files being sent are already on their way.
                if this.attach.sending || ix >= this.attach.paths.len() {
                    return;
                }
                this.attach.paths.remove(ix);
                this.attach.error = None;
                sync_photos(&this.attach.paths, &mut this.attach.photos, cx);
                this.sweep_note_files(cx);
                cx.notify();
            });
        });
        // The stored note opens its images with the system app, not the lightbox.
        let open: OnOpen = Rc::new(|path, _, cx| cx.open_with_system(path));
        tray(
            Tray {
                paths: &self.attach.paths,
                photos: &self.attach.photos,
                attaching: self.note_busy(),
                error: self.attach.error.as_deref(),
                ids: ATTACH_IDS,
                open_label: |label| format!("Open {label}"),
            },
            remove,
            open,
        )
    }

    /// The paperclip beside the field: the chat's own tool button.
    pub(super) fn note_paperclip(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        crate::composer_box::attach_tool("today-note-attach")
            .on_click(cx.listener(|this, _, _, cx| this.choose_note_files(cx)))
    }

    /// The composer's drop region; a past day never builds it.
    pub(super) fn note_drop_target(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        bind_drop(
            div()
                .id("today-note-drop")
                .debug_selector(|| "today-note-drop".into()),
            DropHooks {
                over: |this: &Self| this.attach.over,
                set_over: |this, over, cx| {
                    if this.attach.over != over {
                        this.attach.over = over;
                        cx.notify();
                    }
                },
                drop: |this, paths, window: &mut Window, cx| {
                    if paths.is_empty() {
                        this.show_note_error(NO_FILE_DRAGGED.into(), cx);
                    } else {
                        this.add_note_files(paths, cx);
                    }
                    window.focus(&this.note.focus_handle(cx), cx);
                },
            },
            cx,
        )
    }

    /// A file drag is over the composer. A drag that ended elsewhere (its
    /// release was not seen) does not count, as in the chat.
    pub(super) fn note_drag_over(&self, cx: &Context<Self>) -> bool {
        self.attach.over && cx.has_active_drag()
    }

    pub(super) fn note_drop_overlay(over: bool) -> Option<AnyElement> {
        drop_overlay(over, crate::composer_box::COMPOSER_RADIUS)
    }

    /// Sends the draft: its text and files. Waits for running checks, and a
    /// note with only files is fine. The draft stays until the save succeeds.
    pub(super) fn send_note(&mut self, cx: &mut Context<Self>) {
        if !self.note_editable() || self.note_busy() {
            return;
        }
        let Some(id) = self.selected.clone() else {
            return;
        };
        let (text, composing) = {
            let note = self.note.read(cx);
            (note.content.to_string(), note.is_composing())
        };
        let body = text.trim().to_string();
        let files = self.attach.paths.clone();
        if composing || (body.is_empty() && files.is_empty()) {
            return;
        }
        self.attach.sending = true;
        self.attach.sent = files.clone();
        self.attach.error = None;
        let epoch = self.attach.epoch;
        let store = self.store.clone();
        let work = cx.background_executor().spawn(async move {
            // The store is locked only to validate and to write, never while
            // the files are copied.
            let plan =
                locked(&store)?.today_note_plan(&id, &body, files.len(), &Actor::User, now_ms())?;
            let (attachments, skipped) = plan.copy(&files)?;
            locked(&store)?.today_note_commit(plan, attachments, skipped, &Actor::User, now_ms())
        });
        let sent = self.attach.sent.clone();
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.attach.sending = false;
                this.attach.sent.clear();
                match &result {
                    Ok(saved) => {
                        this.action_error = None;
                        if epoch == this.attach.epoch {
                            this.attach.paths.retain(|path| !sent.contains(path));
                            sync_photos(&this.attach.paths, &mut this.attach.photos, cx);
                            // Only what was sent goes; anything typed meanwhile stays.
                            let typed = this.note.read(cx).content.to_string();
                            if let Some(rest) = typed.strip_prefix(&text) {
                                let rest = rest.to_string();
                                this.note.update(cx, |input, cx| input.set_text(rest, cx));
                            }
                            if !saved.skipped.is_empty() {
                                this.attach.error = Some(saved.skipped.join("\n"));
                            }
                        }
                    }
                    // The text and the files stay for another try.
                    Err(error) => this.action_error = Some(error.clone()),
                }
                this.sweep_note_files(cx);
                this.pump_note_files(cx);
                this.refresh(cx);
            });
        })
        .detach();
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::super::list::testkit::*;
    use super::*;
    use base64::Engine;
    use gpui::{ClipboardItem, FileDropEvent, TestAppContext, VisualTestContext};
    use monocode_core::today::{Group, NewTask};
    use std::sync::{Arc, Mutex};

    const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=";

    /// A temp folder that is removed with the value.
    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            let dir = std::env::temp_dir()
                .join(format!("note-composer-{}", monocode_core::mono::new_id()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        fn file(&self, name: &str, bytes: usize) -> PathBuf {
            let path = self.0.join(name);
            std::fs::write(&path, vec![b'x'; bytes]).unwrap();
            path
        }
        fn png(&self, name: &str) -> PathBuf {
            let path = self.0.join(name);
            std::fs::write(&path, png_bytes()).unwrap();
            path
        }
        /// The pasted-image folder: empty when every temp file was deleted.
        fn pasted(&self) -> Vec<PathBuf> {
            std::fs::read_dir(self.0.join("pastes"))
                .map(|dir| dir.map(|e| e.unwrap().path()).collect())
                .unwrap_or_default()
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn png_bytes() -> Vec<u8> {
        base64::engine::general_purpose::STANDARD
            .decode(PNG)
            .unwrap()
    }
    fn modifier() -> &'static str {
        if cfg!(target_os = "macos") {
            "cmd"
        } else {
            "ctrl"
        }
    }
    fn clipboard_image(visual: &mut VisualTestContext) {
        visual.update(|_, cx| {
            cx.write_to_clipboard(ClipboardItem {
                entries: vec![gpui::ClipboardEntry::Image(gpui::Image::from_bytes(
                    gpui::ImageFormat::Png,
                    png_bytes(),
                ))],
            })
        });
    }
    fn open_page(
        cx: &mut TestAppContext,
        mode: DayMode,
        scratch: &Scratch,
    ) -> (
        gpui::WindowHandle<Page>,
        VisualTestContext,
        gpui::Entity<Today>,
    ) {
        cx.update(crate::input::bind);
        let (window, mut visual) = page(cx, Show::Detail, move |today| {
            let day = if mode == DayMode::Past {
                "2026-10-31"
            } else {
                "2026-11-01"
            };
            today.view = Some(view(
                mode,
                day,
                vec![row(Group::Today, task("a", "Task A"), 0)],
            ));
            today.selected = Some("id-a".into());
        });
        let today = window
            .update(&mut visual, |page, _, _| page.today.clone())
            .unwrap();
        let root = scratch.0.join("pastes");
        today.update(&mut visual, |today, _| today.attach.paste_root = root);
        (window, visual, today)
    }
    fn focus_note(
        window: &gpui::WindowHandle<Page>,
        visual: &mut VisualTestContext,
        today: &gpui::Entity<Today>,
    ) {
        let note = today.read_with(visual, |today, _| today.note.clone());
        window
            .update(visual, |_, window, cx| {
                window.focus(&note.focus_handle(cx), cx)
            })
            .unwrap();
    }
    fn add(today: &gpui::Entity<Today>, visual: &mut VisualTestContext, files: Vec<PathBuf>) {
        today.update(visual, |today, cx| today.add_note_files(files, cx));
        visual.run_until_parked();
        visual.refresh().unwrap();
    }
    fn state(
        today: &gpui::Entity<Today>,
        visual: &VisualTestContext,
    ) -> (Vec<PathBuf>, Option<String>) {
        today.read_with(visual, |today, _| {
            (today.attach.paths.clone(), today.attach.error.clone())
        })
    }
    fn text_of(today: &gpui::Entity<Today>, visual: &VisualTestContext) -> String {
        today.read_with(visual, |today, cx| today.note.read(cx).content.to_string())
    }
    fn set_text(today: &gpui::Entity<Today>, visual: &mut VisualTestContext, text: &str) {
        let note = today.read_with(visual, |today, _| today.note.clone());
        note.update(visual, |input, cx| input.set_text(text.into(), cx));
    }

    /// A Today on a file-backed store with one task, as the app has it.
    struct Fixture {
        today: gpui::Entity<Today>,
        store: crate::runtime::Database,
        task: String,
        data: Scratch,
        files: Scratch,
    }
    fn fixture(cx: &mut TestAppContext) -> (Fixture, &mut VisualTestContext) {
        cx.update(crate::input::bind);
        let data = Scratch::new();
        let files = Scratch::new();
        let store = Arc::new(Mutex::new(
            monocode_core::store::Store::open(&data.0.join("monocode.db")).unwrap(),
        ));
        let task = locked(&store)
            .unwrap()
            .today_add(
                &NewTask {
                    title: "Has files".into(),
                    ..Default::default()
                },
                &Actor::User,
                now_ms(),
            )
            .unwrap()
            .id;
        let (today, visual) = cx.add_window_view({
            let store = store.clone();
            move |_, cx| Today::new_quiet(store, cx)
        });
        today.update(visual, |today, cx| {
            today.attach.paste_root = files.0.join("pastes");
            today.refresh(cx);
        });
        visual.run_until_parked();
        today.update(visual, |today, cx| today.select(Some(task.clone()), cx));
        visual.run_until_parked();
        (
            Fixture {
                today,
                store,
                task,
                data,
                files,
            },
            visual,
        )
    }
    fn notes(f: &Fixture) -> Vec<monocode_core::today::Note> {
        locked(&f.store).unwrap().today_notes(&f.task).unwrap()
    }

    #[gpui::test]
    fn pasting_an_image_shows_a_thumbnail_and_keeps_the_typed_text(cx: &mut TestAppContext) {
        let scratch = Scratch::new();
        let (window, mut visual, today) = open_page(cx, DayMode::Current, &scratch);
        focus_note(&window, &mut visual, &today);
        set_text(&today, &mut visual, "keep me");
        clipboard_image(&mut visual);
        visual.simulate_keystrokes(&format!("{}-v", modifier()));
        visual.run_until_parked();
        assert!(has(&mut visual, "today-note-attachments"));
        assert!(has(&mut visual, "today-note-image-0"));
        let (paths, error) = state(&today, &visual);
        assert_eq!(paths.len(), 1);
        assert_eq!(error, None);
        assert_eq!(text_of(&today, &visual), "keep me");
        // A plain name in a private folder, not `<pid>-<nanos>-...`.
        assert_eq!(paths[0].file_name().unwrap(), "pasted-image.png");
        assert_eq!(scratch.pasted().len(), 1);
    }

    #[gpui::test]
    fn a_text_paste_falls_through_to_the_field(cx: &mut TestAppContext) {
        let scratch = Scratch::new();
        let (window, mut visual, today) = open_page(cx, DayMode::Current, &scratch);
        focus_note(&window, &mut visual, &today);
        visual.update(|_, cx| cx.write_to_clipboard(ClipboardItem::new_string("hello".into())));
        visual.simulate_keystrokes(&format!("{}-v", modifier()));
        visual.run_until_parked();
        assert_eq!(text_of(&today, &visual), "hello");
        assert!(state(&today, &visual).0.is_empty());
        assert!(!has(&mut visual, "today-note-attachments"));
    }

    #[gpui::test]
    fn pasted_file_paths_become_chips(cx: &mut TestAppContext) {
        let scratch = Scratch::new();
        let (window, mut visual, today) = open_page(cx, DayMode::Current, &scratch);
        focus_note(&window, &mut visual, &today);
        let file = scratch.file("notes.txt", 10);
        visual.update(|_, cx| {
            cx.write_to_clipboard(ClipboardItem {
                entries: vec![gpui::ClipboardEntry::ExternalPaths(gpui::ExternalPaths(
                    vec![file.clone()].into(),
                ))],
            })
        });
        visual.simulate_keystrokes(&format!("{}-v", modifier()));
        visual.run_until_parked();
        assert!(has(&mut visual, "today-note-file-0"));
        assert_eq!(state(&today, &visual).0, vec![file]);
        // Pasted paths are the user's own files: nothing is created or deleted.
        assert!(scratch.pasted().is_empty());
    }

    #[gpui::test]
    fn a_real_file_drop_highlights_then_adds_the_files(cx: &mut TestAppContext) {
        let scratch = Scratch::new();
        let (window, mut visual, today) = open_page(cx, DayMode::Current, &scratch);
        assert!(has(&mut visual, "today-note-drop"));
        let (a, b) = (scratch.file("a.txt", 5), scratch.png("b.png"));
        let spot = visual.debug_bounds("today-note-box").unwrap().center();
        let paths = gpui::ExternalPaths(vec![a.clone(), b.clone()].into());
        visual.simulate_event(FileDropEvent::Entered {
            position: spot,
            paths,
        });
        visual.simulate_event(FileDropEvent::Pending { position: spot });
        visual.refresh().unwrap();
        assert!(today.read_with(&visual, |today, _| today.attach.over));
        visual.simulate_event(FileDropEvent::Submit { position: spot });
        visual.run_until_parked();
        visual.refresh().unwrap();
        assert!(!today.read_with(&visual, |today, _| today.attach.over));
        assert_eq!(state(&today, &visual).0, vec![a, b]);
        assert!(has(&mut visual, "today-note-file-0"));
        assert!(has(&mut visual, "today-note-image-1"));
        let _ = window;
    }

    #[gpui::test]
    fn a_stale_drag_flag_draws_no_overlay(cx: &mut TestAppContext) {
        let scratch = Scratch::new();
        let (_, mut visual, today) = open_page(cx, DayMode::Current, &scratch);
        today.update(&mut visual, |today, _| today.attach.over = true);
        let over = today.update(&mut visual, |today, cx| today.note_drag_over(cx));
        assert!(!over, "no drag is active");
    }

    #[gpui::test]
    fn the_pickers_result_adds_files_and_another_tasks_is_dropped(cx: &mut TestAppContext) {
        let scratch = Scratch::new();
        let (_, mut visual, today) = open_page(cx, DayMode::Current, &scratch);
        let (a, b) = (scratch.file("a.txt", 5), scratch.file("b.txt", 5));
        let epoch = today.read_with(&visual, |today, _| today.attach.epoch);
        today.update(&mut visual, |today, cx| {
            today.finish_note_pick(epoch, vec![a.clone()], cx)
        });
        visual.run_until_parked();
        assert_eq!(state(&today, &visual).0, vec![a]);
        // The picker was opened on a task that is no longer selected.
        today.update(&mut visual, |today, cx| {
            today.select(Some("other".into()), cx)
        });
        today.update(&mut visual, |today, cx| {
            today.finish_note_pick(epoch, vec![b], cx)
        });
        visual.run_until_parked();
        assert!(state(&today, &visual).0.is_empty());
    }

    #[gpui::test]
    fn removing_a_chip_deletes_a_pasted_temp_file_and_a_plain_file_is_kept(
        cx: &mut TestAppContext,
    ) {
        let scratch = Scratch::new();
        let (window, mut visual, today) = open_page(cx, DayMode::Current, &scratch);
        focus_note(&window, &mut visual, &today);
        let plain = scratch.file("a.txt", 3);
        add(&today, &mut visual, vec![plain.clone()]);
        clipboard_image(&mut visual);
        visual.simulate_keystrokes(&format!("{}-v", modifier()));
        visual.run_until_parked();
        assert_eq!(state(&today, &visual).0.len(), 2);
        assert_eq!(scratch.pasted().len(), 1);
        visual.refresh().unwrap();
        let remove = visual.debug_bounds("today-note-remove-1").unwrap();
        visual.simulate_click(remove.center(), Default::default());
        visual.run_until_parked();
        assert_eq!(state(&today, &visual).0, vec![plain.clone()]);
        assert!(scratch.pasted().is_empty());
        let remove = visual.debug_bounds("today-note-remove-0").unwrap();
        visual.simulate_click(remove.center(), Default::default());
        visual.run_until_parked();
        assert!(state(&today, &visual).0.is_empty());
        assert!(plain.exists(), "the user's own file is never deleted");
    }

    #[gpui::test]
    fn the_eleventh_file_is_refused_and_the_rest_are_added(cx: &mut TestAppContext) {
        let scratch = Scratch::new();
        let (_, mut visual, today) = open_page(cx, DayMode::Current, &scratch);
        let files: Vec<_> = (0..11)
            .map(|i| scratch.file(&format!("f{i}.txt"), 3))
            .collect();
        add(&today, &mut visual, files.clone());
        let (paths, error) = state(&today, &visual);
        assert_eq!(paths, files[..10].to_vec());
        let error = error.expect("a message");
        assert!(
            error.contains("f10.txt") && error.contains("at most 10"),
            "{error}"
        );
    }

    #[gpui::test]
    fn an_oversized_or_missing_file_is_refused_and_a_later_batch_keeps_the_message(
        cx: &mut TestAppContext,
    ) {
        let scratch = Scratch::new();
        let (_, mut visual, today) = open_page(cx, DayMode::Current, &scratch);
        let big = scratch.0.join("big.bin");
        std::fs::File::create(&big)
            .unwrap()
            .set_len(MAX_FILE_BYTES + 1)
            .unwrap();
        let ok = scratch.file("ok.txt", 4);
        add(
            &today,
            &mut visual,
            vec![big, ok.clone(), scratch.0.join("gone.txt")],
        );
        let (paths, error) = state(&today, &visual);
        assert_eq!(paths, vec![ok.clone()]);
        let error = error.expect("a message");
        assert!(error.contains("big.bin is larger than 25 MB"), "{error}");
        assert!(error.contains("gone.txt could not be read"), "{error}");
        assert!(has(&mut visual, "today-note-attachments"));
        // A clean batch does not erase the message.
        let next = scratch.file("next.txt", 4);
        today.update(&mut visual, |today, cx| {
            today.attach.queue.push(next.clone());
            today.pump_note_files(cx);
        });
        visual.run_until_parked();
        let (paths, kept) = state(&today, &visual);
        assert_eq!(paths, vec![ok, next]);
        assert_eq!(kept.as_deref(), Some(error.as_str()));
    }

    #[gpui::test]
    fn the_same_file_dropped_twice_during_a_check_is_one_chip(cx: &mut TestAppContext) {
        let scratch = Scratch::new();
        let (_, mut visual, today) = open_page(cx, DayMode::Current, &scratch);
        let file = scratch.file("a.txt", 3);
        today.update(&mut visual, |today, cx| {
            today.add_note_files(vec![file.clone()], cx);
            today.add_note_files(vec![file.clone()], cx);
        });
        visual.run_until_parked();
        assert_eq!(state(&today, &visual).0, vec![file]);
    }

    #[gpui::test]
    fn a_past_day_has_no_composer_and_ignores_paste_and_files(cx: &mut TestAppContext) {
        let scratch = Scratch::new();
        let (_, mut visual, today) = open_page(cx, DayMode::Past, &scratch);
        assert!(!has(&mut visual, "today-note-box"));
        assert!(!has(&mut visual, "today-note-drop"));
        add(&today, &mut visual, vec![scratch.file("a.txt", 3)]);
        assert!(state(&today, &visual).0.is_empty());
        clipboard_image(&mut visual);
        let pasted = today.update(&mut visual, |today, cx| today.paste_note_files(cx));
        visual.run_until_parked();
        assert!(!pasted);
        assert!(state(&today, &visual).0.is_empty());
        assert!(scratch.pasted().is_empty());
    }

    #[gpui::test]
    fn overlapping_pastes_keep_send_blocked_until_all_finish(cx: &mut TestAppContext) {
        let scratch = Scratch::new();
        let (_, mut visual, today) = open_page(cx, DayMode::Current, &scratch);
        clipboard_image(&mut visual);
        today.update(&mut visual, |today, cx| {
            assert!(today.paste_note_files(cx));
            assert!(today.paste_note_files(cx));
            assert_eq!(today.attach.busy, 2);
            assert!(today.note_busy());
        });
        visual.run_until_parked();
        today.read_with(&visual, |today, _| {
            assert_eq!(today.attach.busy, 0);
            assert!(!today.note_busy());
            assert_eq!(today.attach.paths.len(), 2);
        });
        assert_eq!(scratch.pasted().len(), 2);
    }

    #[gpui::test]
    fn a_task_change_during_a_check_drops_the_files_and_pasted_temp_files(cx: &mut TestAppContext) {
        let scratch = Scratch::new();
        let (_, mut visual, today) = open_page(cx, DayMode::Current, &scratch);
        clipboard_image(&mut visual);
        let file = scratch.file("a.txt", 3);
        today.update(&mut visual, |today, cx| {
            today.add_note_files(vec![file], cx);
            assert!(today.paste_note_files(cx));
            today.select(Some("other".into()), cx);
        });
        visual.run_until_parked();
        assert!(state(&today, &visual).0.is_empty());
        assert!(scratch.pasted().is_empty());
    }

    #[gpui::test]
    fn a_note_with_only_files_is_sent_stored_and_the_temp_copy_deleted(cx: &mut TestAppContext) {
        let (f, visual) = fixture(cx);
        clipboard_image_in(visual);
        let source = f.files.file("report.txt", 12);
        f.today.update(visual, |today, cx| {
            today.add_note_files(vec![source.clone()], cx);
            assert!(today.paste_note_files(cx));
        });
        visual.run_until_parked();
        assert_eq!(f.files.pasted().len(), 1);
        assert!(
            f.today
                .read_with(visual, |today, _| today.note_has_attachments())
        );
        f.today.update(visual, |today, cx| today.add_note(cx));
        visual.run_until_parked();
        f.today.read_with(visual, |today, _| {
            assert!(today.attach.paths.is_empty());
            assert_eq!(today.action_error, None);
        });
        let notes = notes(&f);
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].body, "");
        let names: Vec<_> = notes[0]
            .attachments
            .iter()
            .map(|a| a.name.as_str())
            .collect();
        assert_eq!(names, ["report.txt", "pasted-image.png"]);
        let stored = PathBuf::from(notes[0].attachments[0].path.clone().unwrap());
        assert_ne!(stored, source);
        assert!(stored.starts_with(&f.data.0));
        assert_eq!(std::fs::read(stored).unwrap().len(), 12);
        assert!(source.exists());
        assert!(
            f.files.pasted().is_empty(),
            "the pasted temp file is deleted"
        );
    }
    fn clipboard_image_in(visual: &mut VisualTestContext) {
        clipboard_image(visual);
    }

    #[gpui::test]
    fn a_failed_send_keeps_the_text_and_the_files(cx: &mut TestAppContext) {
        let (f, visual) = fixture(cx);
        let file = f.files.file("a.txt", 3);
        add(&f.today, visual, vec![file.clone()]);
        let long = "x".repeat(6000);
        set_text(&f.today, visual, &long);
        f.today.update(visual, |today, cx| today.add_note(cx));
        visual.run_until_parked();
        assert_eq!(text_of(&f.today, visual), long);
        assert_eq!(state(&f.today, visual).0, vec![file]);
        let error = f
            .today
            .read_with(visual, |today, _| today.action_error.clone());
        assert!(error.is_some());
        assert!(notes(&f).is_empty());
        assert!(!f.today.read_with(visual, |today, _| today.note_busy()));
    }

    #[gpui::test]
    fn a_double_send_writes_one_note_and_whitespace_is_cleared(cx: &mut TestAppContext) {
        let (f, visual) = fixture(cx);
        let file = f.files.file("a.txt", 3);
        add(&f.today, visual, vec![file]);
        set_text(&f.today, visual, "   ");
        f.today.update(visual, |today, cx| {
            today.add_note(cx);
            today.add_note(cx);
        });
        visual.run_until_parked();
        assert_eq!(notes(&f).len(), 1);
        assert_eq!(text_of(&f.today, visual), "");
    }

    #[gpui::test]
    fn a_file_added_during_a_send_is_kept_and_text_typed_meanwhile_stays(cx: &mut TestAppContext) {
        let (f, visual) = fixture(cx);
        let (a, b) = (f.files.file("a.txt", 3), f.files.file("b.txt", 3));
        add(&f.today, visual, vec![a]);
        set_text(&f.today, visual, "first");
        f.today.update(visual, |today, cx| {
            today.add_note(cx);
            today.add_note_files(vec![b.clone()], cx);
        });
        set_text(&f.today, visual, "first and more");
        visual.run_until_parked();
        assert_eq!(state(&f.today, visual).0, vec![b]);
        assert_eq!(text_of(&f.today, visual), " and more");
        let notes = notes(&f);
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].attachments.len(), 1);
        assert_eq!(notes[0].attachments[0].name, "a.txt");
    }

    #[gpui::test]
    fn a_task_change_during_a_send_still_saves_and_clears_the_new_draft(cx: &mut TestAppContext) {
        let (f, visual) = fixture(cx);
        let file = f.files.file("a.txt", 3);
        add(&f.today, visual, vec![file]);
        f.today.update(visual, |today, cx| {
            today.add_note(cx);
            today.select(Some("other".into()), cx);
        });
        visual.run_until_parked();
        assert_eq!(notes(&f).len(), 1);
        assert!(state(&f.today, visual).0.is_empty());
    }

    #[gpui::test]
    fn remove_does_nothing_while_sending(cx: &mut TestAppContext) {
        let scratch = Scratch::new();
        let (_, mut visual, today) = open_page(cx, DayMode::Current, &scratch);
        add(&today, &mut visual, vec![scratch.file("a.txt", 3)]);
        today.update(&mut visual, |today, _| today.attach.sending = true);
        visual.refresh().unwrap();
        let remove = visual.debug_bounds("today-note-remove-0").unwrap();
        visual.simulate_click(remove.center(), Default::default());
        visual.run_until_parked();
        assert_eq!(state(&today, &visual).0.len(), 1);
    }

    #[gpui::test]
    fn send_is_not_ready_while_busy(cx: &mut TestAppContext) {
        let scratch = Scratch::new();
        let (_, mut visual, today) = open_page(cx, DayMode::Current, &scratch);
        add(&today, &mut visual, vec![scratch.file("a.txt", 3)]);
        today.update(&mut visual, |today, _| today.attach.busy = 1);
        assert!(today.read_with(&visual, |today, _| today.note_busy()));
        today.update(&mut visual, |today, cx| today.add_note(cx));
        visual.run_until_parked();
        assert_eq!(
            state(&today, &visual).0.len(),
            1,
            "nothing is sent while busy"
        );
    }
}
