//! The "Import emoji packs" dialog: unpack each zip, let a model suggest an emotion for every
//! image, review the suggestion, then save. Re-sorting a pack reuses the same dialog. The model
//! is chosen in the dialog with the shared model picker.
//!
//! Unpacking and sorting run on the background executor and report through a channel. Cancel
//! stops the whole queue: it sets the shared flags, deletes the staging folder and saves nothing
//! for the zip that was open.
use super::{EmojiLibraryView, SortModel, format_list, format_text, picture, store::READ_ONLY};
use crate::{
    cloud_mark::CloudTip,
    modal,
    pickers::{ModelList, ModelPicked, cached_model_name},
    popover::{Align, Anchor, Menu, MenuEvent, MenuItem, Popover, menu_popover, popover},
    runtime::{Database, locked},
    theme::*,
};
use gpui::{
    AnyElement, App, Context, FontWeight, Role, SharedString, Subscription, Window, div,
    prelude::*, px,
};
use monocode_core::{
    agent::{AccessMode, Settings},
    mono::emoji::{
        self, Format,
        import::{self, Limits, Staged},
        sort::{self, EmotionRef, Progress, Sorted, Source},
    },
    provider::{self, Kind, Request},
};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

const PRIORITY: usize = 85;
/// Above the dialog, so the model list opens over it.
const PICKER_PRIORITY: usize = 90;
const NOTE: &str = "Files are copied into Monoptah's data folder; the zip can be deleted.";

/// What a sorting run needs to talk to the chosen model.
pub(crate) struct SortParts {
    pub kind: Kind,
    pub model: Option<String>,
    pub cwd: String,
    pub settings: Settings,
    pub store: Database,
}

/// Makes the runner that asks the model, from the chosen provider, model and account. It runs
/// on a background thread. `Ok(None)` means there is no model to ask, so emoji are sorted by
/// their file names. Tests replace it; under `cfg(test)` the default never starts a real model.
pub(crate) type SorterFactory =
    Arc<dyn Fn(SortParts) -> Result<Option<Arc<dyn sort::Runner>>, String> + Send + Sync>;

pub(super) fn default_sorter() -> SorterFactory {
    Arc::new(|parts| {
        if cfg!(test) || !sort::supports_images(parts.kind) {
            return Ok(None);
        }
        let SortParts {
            kind,
            model,
            cwd,
            mut settings,
            store,
        } = parts;
        if settings.account.id != monocode_core::accounts::DEFAULT {
            let store = locked(&store)?;
            if !store
                .provider_accounts()?
                .exists(kind, &settings.account.id)
            {
                return Err(
                    "This provider account was removed. Choose another account first.".into(),
                );
            }
            settings.account.root = store.data_dir().map(std::path::Path::to_path_buf);
        }
        let binary = provider::resolve(kind).ok_or_else(|| kind.unavailable_hint())?;
        let request = Request {
            kind,
            binary,
            cwd: cwd.into(),
            prompt: String::new(),
            provider_id: None,
            model,
            settings,
        };
        Ok(Some(Arc::new(sort::ProviderRunner { request })))
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ItemState {
    Waiting,
    Unpacking,
    Sorting,
    Review,
    Saving,
    Saved,
    Skipped(String),
    Failed(String),
}

impl ItemState {
    fn label(&self) -> String {
        match self {
            Self::Waiting => "waiting".into(),
            Self::Unpacking => "unpacking".into(),
            Self::Sorting => "sorting".into(),
            Self::Review => "review".into(),
            Self::Saving => "saving".into(),
            Self::Saved => "saved".into(),
            Self::Skipped(why) => format!("skipped: {why}"),
            Self::Failed(why) => format!("failed: {why}"),
        }
    }
}

pub(crate) struct QueueItem {
    pub path: PathBuf,
    pub name: String,
    pub size: u64,
    pub state: ItemState,
}

#[derive(Clone)]
pub(crate) enum Mode {
    Import,
    Resort { pack: String, name: String },
}

/// What the unpacking kept, for the "Kept N images" line.
pub(crate) struct Kept {
    count: usize,
    formats: Vec<(Format, usize)>,
    skipped: String,
}

pub(crate) struct Review {
    pub suggestion: HashMap<String, Option<String>>,
    pub source: Source,
}

/// The zip being worked on.
#[derive(Default)]
pub(crate) struct Step {
    staged: Option<Staged>,
    entries: Option<usize>,
    kept: Option<Kept>,
    items: Vec<sort::Item>,
    progress: (usize, usize),
    pub review: Option<Review>,
}

pub(crate) struct ImportDialog {
    model: SortModel,
    model_label: String,
    mode: Mode,
    pub queue: Vec<QueueItem>,
    pub current: usize,
    pub step: Option<Step>,
    /// Stops unpacking, and the dialog as a whole.
    cancel: Arc<AtomicBool>,
    /// Stops the sorting run that is going; a new run (another model) gets a new flag.
    sort_cancel: Arc<AtomicBool>,
    /// The number of the sorting run whose events count.
    run: u64,
    token: u64,
    focus: gpui::FocusHandle,
    /// The queue is finished but something was skipped or failed, so the dialog stays to say so.
    pub done: bool,
}

impl ImportDialog {
    fn saving(&self) -> bool {
        self.queue.iter().any(|i| i.state == ItemState::Saving)
    }

    /// Whether the model can be changed now: a zip is being sorted or waits for review.
    fn model_changeable(&self) -> bool {
        !self.done
            && !self.saving()
            && self.step.as_ref().is_some_and(|s| !s.items.is_empty())
            && matches!(
                self.queue.get(self.current).map(|i| &i.state),
                Some(ItemState::Sorting | ItemState::Review)
            )
    }

    /// Tells every background job of this dialog to stop.
    pub(super) fn stop(&self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.sort_cancel.store(true, Ordering::Relaxed);
    }

    #[cfg(test)]
    pub(crate) fn model(&self) -> &SortModel {
        &self.model
    }
}

/// The open model list.
pub(crate) struct ModelPicker {
    menu: gpui::Entity<Menu>,
    #[cfg(test)]
    pub(super) list: gpui::Entity<ModelList>,
    _events: Vec<Subscription>,
}

enum Event {
    Staged {
        staged: Staged,
        items: Vec<sort::Item>,
        kept: Kept,
        entries: usize,
    },
    Progress(Progress),
    Sorted(Sorted),
    Failed(String),
}

fn size_text(bytes: u64) -> String {
    const KB: f64 = 1024.;
    let b = bytes as f64;
    if b >= KB * KB {
        format!("{:.1} MB", b / (KB * KB))
    } else if b >= KB {
        format!("{:.0} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

fn skipped_text(staged: &Staged) -> String {
    let mut counts: Vec<(&'static str, usize)> = Vec::new();
    for skipped in &staged.skipped {
        let label = skipped.reason.label();
        match counts.iter_mut().find(|(l, _)| *l == label) {
            Some((_, n)) => *n += 1,
            None => counts.push((label, 1)),
        }
    }
    if counts.is_empty() {
        "none".into()
    } else {
        counts
            .iter()
            .map(|(label, n)| format!("{n} {label}"))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// The sentence for a names fallback: the reason may already say what happened.
pub(crate) fn names_notice(reason: &str) -> String {
    if reason.contains("suggested from their file names") {
        reason.to_owned()
    } else {
        format!("{reason}, so emoji were suggested from their file names.")
    }
}

/// The sentences of the dialog that depend on the queue and the model.
pub(crate) mod dialog_texts {
    /// "3 files · into your emoji library", or the pack name of a re-sort.
    pub(crate) fn subtitle(resort: bool, pack: &str, files: usize) -> String {
        if resort {
            format!("{pack} \u{b7} your emoji library")
        } else if files == 1 {
            "1 file \u{b7} into your emoji library".to_string()
        } else {
            format!("{files} files \u{b7} into your emoji library")
        }
    }

    pub(crate) fn sorting_line(model: &str, done: usize, total: usize) -> String {
        format!("Sorting into emotions with {model}... {done} of {total}")
    }
}

/// The label of a model, as the dialog shows it.
fn model_label(model: &SortModel) -> String {
    cached_model_name(model.kind, model.model.as_deref()).to_string()
}

impl EmojiLibraryView {
    /// The folder a sorting runs in: the app data folder.
    fn work_dir(&self, cx: &App) -> String {
        let store = self.store.read(cx);
        store
            .dir()
            .parent()
            .unwrap_or(store.dir())
            .to_string_lossy()
            .into_owned()
    }

    /// The model a dialog starts with: the one of the Mono whose settings were opened last,
    /// else the app's default provider and model.
    fn default_model(&self, cx: &App) -> SortModel {
        if let Some(model) = self.store.read(cx).last_model() {
            return model.clone();
        }
        let cwd = self.work_dir(cx);
        let prefs = cx
            .try_global::<crate::settings::Preferences>()
            .cloned()
            .unwrap_or_default();
        let choice = crate::shell::new_chat_choice::resolve(&prefs, &cwd);
        let account = crate::accounts::get(cx)
            .map(|accounts| accounts.read(cx).selected(choice.kind, &cwd))
            .unwrap_or_default();
        SortModel {
            kind: choice.kind,
            model: choice.model,
            account,
        }
    }

    /// What a sorting run needs: the way to reach the chosen model, the model's label and the
    /// emotions of the library.
    fn sort_inputs(&self, cx: &App) -> (SortParts, String, Vec<EmotionRef>) {
        let store = self.store.read(cx);
        let emotions = store
            .library()
            .emotions
            .iter()
            .map(|e| EmotionRef {
                name: e.name.clone(),
                description: e.description.clone(),
            })
            .collect();
        let model = self
            .import
            .as_ref()
            .map(|d| d.model.clone())
            .unwrap_or_else(|| self.default_model(cx));
        let parts = SortParts {
            kind: model.kind,
            model: model.model.clone(),
            cwd: self.work_dir(cx),
            settings: Settings {
                account: model.account.clone(),
                text_only: true,
                disable_claude_hooks: true,
                access: AccessMode::ReadOnly,
                ..Default::default()
            },
            store: store.database(),
        };
        (parts, model_label(&model), emotions)
    }

    /// "Import zip...": the native picker, several files at once.
    pub(super) fn pick_zips(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.import.is_some() {
            return;
        }
        let picked = crate::attachment_tray::pick_files(cx);
        cx.spawn_in(window, async move |this, cx| {
            if let Some(paths) = picked.await {
                let _ = this.update_in(cx, |this, window, cx| this.import_paths(paths, window, cx));
            }
        })
        .detach();
    }

    /// Opens the dialog for the zips among `paths`; anything else is listed as skipped.
    pub(super) fn import_paths(
        &mut self,
        paths: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.import.is_some() || paths.is_empty() {
            return;
        }
        let (loaded, read_only) = {
            let store = self.store.read(cx);
            (store.loaded(), store.read_only())
        };
        if read_only {
            self.error = Some(READ_ONLY.into());
            cx.notify();
            return;
        }
        if !loaded {
            self.error = Some("The emoji library is still loading. Try again in a moment.".into());
            cx.notify();
            return;
        }
        let queue = paths
            .into_iter()
            .map(|path| {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string());
                let is_zip = path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("zip"));
                QueueItem {
                    size: 0,
                    state: if is_zip {
                        ItemState::Waiting
                    } else {
                        ItemState::Skipped("not a .zip file".into())
                    },
                    path,
                    name,
                }
            })
            .collect();
        self.error = None;
        self.open_dialog(Mode::Import, queue, window, cx);
        self.load_sizes(cx);
    }

    /// Looks up the size of every picked file off the main thread.
    fn load_sizes(&mut self, cx: &mut Context<Self>) {
        let Some(d) = self.import.as_ref() else {
            return;
        };
        let token = d.token;
        let paths: Vec<PathBuf> = d.queue.iter().map(|i| i.path.clone()).collect();
        let work = cx.background_executor().spawn(async move {
            paths
                .iter()
                .map(|p| std::fs::metadata(p).map_or(0, |m| m.len()))
                .collect::<Vec<u64>>()
        });
        cx.spawn(async move |this, cx| {
            let sizes = work.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(d) = this.import.as_mut().filter(|d| d.token == token) {
                    for (item, size) in d.queue.iter_mut().zip(sizes) {
                        item.size = size;
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Deletes a staging folder off the main thread.
    fn discard(&self, staged: Staged, cx: &mut Context<Self>) {
        cx.background_executor()
            .spawn(async move { import::discard(staged) })
            .detach();
    }

    fn open_dialog(
        &mut self,
        mode: Mode,
        queue: Vec<QueueItem>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let model = self.default_model(cx);
        self.next_token += 1;
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        self.import_flag.store(true, Ordering::Relaxed);
        self.import = Some(ImportDialog {
            model_label: model_label(&model),
            model,
            mode,
            queue,
            current: 0,
            step: None,
            cancel: Arc::new(AtomicBool::new(false)),
            sort_cancel: Arc::new(AtomicBool::new(false)),
            run: 0,
            token: self.next_token,
            focus,
            done: false,
        });
        self.import_next(window, cx);
        cx.notify();
    }

    /// Re-sorts an existing pack with the same dialog, without the unpack steps.
    pub(super) fn resort(&mut self, pack_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let (dir, library, read_only) = {
            let store = self.store.read(cx);
            (
                store.dir().to_path_buf(),
                store.library().clone(),
                store.read_only(),
            )
        };
        if self.import.is_some() || read_only {
            return;
        }
        let Some(pack) = library.packs.iter().find(|p| p.id == pack_id) else {
            return;
        };
        let items: Vec<sort::Item> = pack
            .emoji
            .iter()
            .map(|e| sort::Item {
                key: e.file.clone(),
                name: e.name.clone(),
                path: emoji::image_path(&dir, pack, e),
                format: e.format,
            })
            .collect();
        let (pack_name, size) = (pack.name.clone(), pack.emoji.len() as u64);
        let queue = vec![QueueItem {
            path: PathBuf::new(),
            name: pack_name.clone(),
            size,
            state: ItemState::Waiting,
        }];
        let mode = Mode::Resort {
            pack: pack_id.into(),
            name: pack_name,
        };
        self.open_dialog(mode, queue, window, cx);
        if let Some(step) = self.import.as_mut().and_then(|d| d.step.as_mut()) {
            step.progress = (0, items.len());
            step.items = items;
        }
        self.start_sort(window, cx);
    }

    /// Starts a sorting run of the open step, and stops the one that was going.
    fn start_sort(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.import.as_mut() else {
            return;
        };
        d.sort_cancel.store(true, Ordering::Relaxed);
        d.sort_cancel = Arc::new(AtomicBool::new(false));
        d.run += 1;
        let (token, run, cancel) = (d.token, d.run, d.sort_cancel.clone());
        let Some(step) = d.step.as_mut() else {
            return;
        };
        step.review = None;
        step.progress = (0, step.items.len());
        let items = step.items.clone();
        if let Some(item) = d.queue.get_mut(d.current) {
            item.state = ItemState::Sorting;
        }
        let (parts, label, emotions) = self.sort_inputs(cx);
        let factory = self.sorter.clone();
        let dir = self.store.read(cx).dir().to_path_buf();
        let (tx, rx) = async_channel::unbounded();
        cx.background_executor()
            .spawn(async move {
                sort_job(items, emotions, parts, factory, label, dir, cancel, tx);
            })
            .detach();
        self.watch(token, run, rx, window, cx);
        cx.notify();
    }

    fn watch(
        &mut self,
        token: u64,
        run: u64,
        rx: async_channel::Receiver<Event>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.spawn_in(window, async move |this, cx| {
            while let Ok(event) = rx.recv().await {
                let last = matches!(event, Event::Sorted(_) | Event::Failed(_));
                let alive = this.update_in(cx, |this, window, cx| {
                    this.import_event(token, run, event, window, cx)
                });
                if alive.is_err() || last {
                    break;
                }
            }
        })
        .detach();
    }

    /// Starts the next waiting zip, or finishes when none is left.
    fn import_next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.import.as_mut() else {
            return;
        };
        let Some(index) = d.queue.iter().position(|i| i.state == ItemState::Waiting) else {
            if d.queue
                .iter()
                .any(|i| matches!(i.state, ItemState::Failed(_) | ItemState::Skipped(_)))
            {
                d.done = true;
                d.step = None;
                cx.notify();
            } else {
                self.import_close(window, cx);
            }
            return;
        };
        d.current = index;
        d.step = Some(Step::default());
        if matches!(d.mode, Mode::Resort { .. }) {
            d.queue[index].state = ItemState::Sorting;
            return;
        }
        d.queue[index].state = ItemState::Unpacking;
        d.sort_cancel.store(true, Ordering::Relaxed);
        d.sort_cancel = Arc::new(AtomicBool::new(false));
        d.run += 1;
        let (token, run, cancel, sort_cancel) =
            (d.token, d.run, d.cancel.clone(), d.sort_cancel.clone());
        let zip = d.queue[index].path.clone();
        let (dir, existing) = {
            let store = self.store.read(cx);
            (store.dir().to_path_buf(), store.library().known_hashes())
        };
        let (parts, label, emotions) = self.sort_inputs(cx);
        let factory = self.sorter.clone();
        let (tx, rx) = async_channel::unbounded();
        cx.background_executor()
            .spawn(async move {
                let staged = match import::stage(
                    &zip,
                    &dir,
                    &existing,
                    &Limits::default(),
                    &cancel,
                    |_, _| {},
                ) {
                    Ok(staged) => staged,
                    Err(error) => {
                        let _ = tx.send_blocking(Event::Failed(error));
                        return;
                    }
                };
                if staged.images.is_empty() {
                    let skipped = skipped_text(&staged);
                    import::discard(staged);
                    let _ = tx.send_blocking(Event::Failed(format!(
                        "no usable images found (skipped: {skipped})"
                    )));
                    return;
                }
                let items: Vec<sort::Item> = staged
                    .images
                    .iter()
                    .map(|image| sort::Item {
                        key: image.file.clone(),
                        name: image.name.clone(),
                        path: staged.dir.join(&image.file),
                        format: image.format,
                    })
                    .collect();
                let kept = Kept {
                    count: items.len(),
                    formats: format_list(items.iter().map(|i| i.format)),
                    skipped: skipped_text(&staged),
                };
                let entries = staged.entries;
                let sorting = items.clone();
                let _ = tx.send_blocking(Event::Staged {
                    staged,
                    items,
                    kept,
                    entries,
                });
                sort_job(
                    sorting,
                    emotions,
                    parts,
                    factory,
                    label,
                    dir,
                    sort_cancel,
                    tx,
                );
            })
            .detach();
        self.watch(token, run, rx, window, cx);
        cx.notify();
    }

    fn import_event(
        &mut self,
        token: u64,
        run: u64,
        event: Event,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(d) = self
            .import
            .as_mut()
            .filter(|d| d.token == token && d.run == run)
        else {
            // The dialog was cancelled, or a newer sorting replaced this one: nothing may stay
            // behind.
            if let Event::Staged { staged, .. } = event {
                self.discard(staged, cx);
            }
            return;
        };
        let index = d.current;
        match event {
            Event::Staged {
                staged,
                items,
                kept,
                entries,
            } => {
                let step = d.step.get_or_insert_with(Step::default);
                step.progress = (0, items.len());
                step.items = items;
                step.kept = Some(kept);
                step.entries = Some(entries);
                step.staged = Some(staged);
                d.queue[index].state = ItemState::Sorting;
            }
            Event::Progress(p) => {
                if let Some(step) = d.step.as_mut() {
                    step.progress = (p.done, p.total);
                }
            }
            Event::Sorted(sorted) => {
                if let Some(step) = d.step.as_mut() {
                    step.progress = (step.items.len(), step.items.len());
                    step.review = Some(Review {
                        suggestion: sorted.suggestion,
                        source: sorted.source,
                    });
                }
                d.queue[index].state = ItemState::Review;
                window.focus(&d.focus, cx);
            }
            Event::Failed(error) => {
                let staged = d.step.take().and_then(|s| s.staged);
                d.queue[index].state = ItemState::Failed(error);
                if let Some(staged) = staged {
                    self.discard(staged, cx);
                }
                self.import_next(window, cx);
            }
        }
        cx.notify();
    }

    /// "Use this sorting" (`sorted`) or "Import without sorting": saves the open zip.
    pub(super) fn accept(&mut self, sorted: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.import.as_mut() else {
            return;
        };
        let index = d.current;
        if d.queue[index].state != ItemState::Review {
            return;
        }
        let token = d.token;
        let Some(mut step) = d.step.take() else {
            return;
        };
        let Some(review) = step.review.take() else {
            return;
        };
        let sorted_by = match (&review.source, sorted) {
            (Source::Model { label }, true) => Some(label.clone()),
            _ => None,
        };
        match d.mode.clone() {
            Mode::Resort { pack, .. } => {
                let result = self.edit(cx, |l| {
                    l.apply_sorting(&pack, &review.suggestion, sorted_by);
                    Ok(())
                });
                self.finish_item(result, window, cx);
            }
            Mode::Import => {
                let Some(staged) = step.staged.take() else {
                    return;
                };
                let (dir, mut scratch) = {
                    let store = self.store.read(cx);
                    (store.dir().to_path_buf(), (**store.library()).clone())
                };
                let emoji_list = staged
                    .images
                    .iter()
                    .map(|i| emoji::Emoji {
                        file: i.file.clone(),
                        hash: i.hash.clone(),
                        name: i.name.clone(),
                        format: i.format,
                        emotion: None,
                    })
                    .collect();
                let pack_id = scratch.add_pack(&staged.source_name, sorted_by.clone(), emoji_list);
                if sorted {
                    scratch.apply_sorting(&pack_id, &review.suggestion, sorted_by);
                }
                let Some(pack) = scratch.packs.iter().find(|p| p.id == pack_id).cloned() else {
                    self.discard(staged, cx);
                    return;
                };
                if let Some(d) = self.import.as_mut() {
                    d.queue[index].state = ItemState::Saving;
                }
                // The pack joins the library only after its folder is in place.
                let work = cx.background_executor().spawn(async move {
                    let result = import::commit(&staged, &dir, &pack_id);
                    if result.is_err() {
                        import::discard(staged);
                    }
                    result.map(|_| ())
                });
                cx.spawn_in(window, async move |this, cx| {
                    let result = work.await;
                    let _ = this.update_in(cx, |this, window, cx| {
                        if this.import.as_ref().is_none_or(|d| d.token != token) {
                            return;
                        }
                        let result = match result {
                            Ok(()) => this
                                .store
                                .clone()
                                .update(cx, |store, cx| store.add_pack(pack, cx)),
                            Err(error) => Err(error),
                        };
                        this.finish_item(result, window, cx);
                    });
                })
                .detach();
            }
        }
        cx.notify();
    }

    fn finish_item(
        &mut self,
        result: Result<(), String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(d) = self.import.as_mut() {
            d.queue[d.current].state = match result {
                Ok(()) => ItemState::Saved,
                Err(error) => ItemState::Failed(error),
            };
        }
        self.import_next(window, cx);
        cx.notify();
    }

    /// Cancel, Escape, a press outside, or Close: stops everything and keeps nothing.
    pub(super) fn import_cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.import.as_ref().is_some_and(|d| d.saving()) {
            return;
        }
        self.import_close(window, cx);
    }

    fn import_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.import.take() else {
            return;
        };
        d.stop();
        self.picker = None;
        self.import_flag.store(false, Ordering::Relaxed);
        if let Some(staged) = d.step.and_then(|s| s.staged) {
            self.discard(staged, cx);
        }
        window.focus(&self.focus, cx);
        // A read that was refused while the import was open can go ahead.
        self.store
            .clone()
            .update(cx, |store, cx| store.retry_load(cx));
        cx.notify();
    }

    /// Opens the shared model list under the dialog's model button.
    fn open_model_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.picker.take().is_some() {
            cx.notify();
            return;
        }
        let Some(d) = self.import.as_ref().filter(|d| d.model_changeable()) else {
            return;
        };
        let (kind, model, account) = (d.model.kind, d.model.model.clone(), d.model.account.clone());
        let project = self.work_dir(cx);
        let list = cx.new(|cx| ModelList::new_for(kind, model, &project, account, cx));
        #[cfg(test)]
        let list_for_test = list.clone();
        let focus = list.read(cx).search_focus(cx);
        let picked = cx.subscribe_in(
            &list,
            window,
            |this, _, picked: &ModelPicked, window, cx| {
                this.picker = None;
                this.pick_model(picked.0, picked.1.clone(), window, cx);
            },
        );
        let menu = cx.new(|cx| {
            Menu::new(
                "Model",
                vec![MenuItem::Custom(list.into(), focus)],
                window,
                cx,
            )
            .width(400.)
        });
        let closed = cx.subscribe(&menu, |this, _, _: &MenuEvent, cx| {
            this.picker = None;
            cx.notify();
        });
        self.picker = Some(ModelPicker {
            menu,
            #[cfg(test)]
            list: list_for_test,
            _events: vec![picked, closed],
        });
        cx.notify();
    }

    /// A model was picked: the zip under review is sorted again with it.
    fn pick_model(
        &mut self,
        kind: Kind,
        model: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cwd = self.work_dir(cx);
        let Some(d) = self.import.as_mut() else {
            return;
        };
        if !d.model_changeable() {
            return;
        }
        let account = if d.model.kind == kind {
            d.model.account.clone()
        } else {
            crate::accounts::get(cx)
                .map(|accounts| accounts.read(cx).selected(kind, &cwd))
                .unwrap_or_default()
        };
        d.model = SortModel {
            kind,
            model,
            account,
        };
        d.model_label = model_label(&d.model);
        window.focus(&d.focus, cx);
        self.start_sort(window, cx);
    }

    pub(super) fn model_popover(&self, cx: &App) -> Option<Popover> {
        let picker = self.picker.as_ref()?;
        Some(
            menu_popover(&picker.menu, Anchor::Slot(self.model_anchor.clone()), cx)
                .align(Align::Start)
                .without_animation()
                .priority(PICKER_PRIORITY)
                .width(400.),
        )
    }

    pub(super) fn import_modal(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let d = self.import.as_ref()?;
        let library = self.store.read(cx).library().clone();
        let size = window.viewport_size();
        let width = 760f32.min(size.width.as_f32() - 24.);
        let max_body = (size.height.as_f32() - 260.).max(160.);
        let weak = cx.weak_entity();
        let resort = matches!(d.mode, Mode::Resort { .. });
        let review_ready = d.queue[d.current].state == ItemState::Review && !d.done;
        let saving = d.saving();
        let heading = match &d.mode {
            Mode::Import => "Import emoji packs".to_string(),
            Mode::Resort { name, .. } => format!("Re-sort {name}"),
        };
        let subtitle = match &d.mode {
            Mode::Import => dialog_texts::subtitle(false, "", d.queue.len()),
            Mode::Resort { name, .. } => dialog_texts::subtitle(true, name, 1),
        };
        let queue = (!resort).then(|| {
            div()
                .flex()
                .flex_col()
                .gap_1p5()
                .children(d.queue.iter().enumerate().map(|(i, item)| {
                    let current = i == d.current && !d.done;
                    div()
                        .id(SharedString::from(format!("emoji-lib-import-queue-{i}")))
                        .debug_selector(move || format!("emoji-lib-import-queue-{i}"))
                        .px_2p5()
                        .py_1p5()
                        .rounded_lg()
                        .border_1()
                        .border_color(if current {
                            accent_color().opacity(0.45)
                        } else {
                            content(0.08)
                        })
                        .bg(content(0.04))
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_size(px(12.))
                        .text_color(content(if current { 0.9 } else { 0.6 }))
                        .child(div().flex_1().min_w_0().truncate().child(item.name.clone()))
                        .child(
                            div()
                                .flex_shrink_0()
                                .max_w(px(320.))
                                .truncate()
                                .text_color(content(0.45))
                                .child(if item.size == 0 {
                                    item.state.label()
                                } else {
                                    format!(
                                        "{} \u{b7} {}",
                                        size_text(item.size),
                                        item.state.label()
                                    )
                                }),
                        )
                }))
        });
        let body = div()
            .id("emoji-lib-import-body")
            .max_h(px(max_body))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_3()
            .children(queue)
            .children(d.step.as_ref().map(|step| self.steps(d, step)))
            .children(d.step.as_ref().and_then(|step| {
                step.review
                    .as_ref()
                    .map(|r| self.review(&library, d, step, r))
            }));
        let changeable = d.model_changeable();
        let model_row = div()
            .flex()
            .items_center()
            .gap_2()
            .text_size(px(12.))
            .text_color(content(0.55))
            .child("Sort with")
            .child(
                modal::disabled(
                    div()
                        .id("emoji-lib-import-model")
                        .debug_selector(|| "emoji-lib-import-model".into())
                        .role(Role::Button)
                        .aria_label(format!("Model: {}", d.model_label))
                        .relative()
                        .h(px(26.))
                        .max_w(px(320.))
                        .px_2p5()
                        .flex()
                        .items_center()
                        .gap_2()
                        .rounded_md()
                        .border_1()
                        .border_color(content(0.1))
                        .text_color(content(0.85))
                        .child(self.model_anchor.probe())
                        .child(div().min_w_0().truncate().child(d.model_label.clone()))
                        .child(icon("chevron-down", 12.).text_color(content(0.5))),
                    !changeable,
                )
                .when(changeable, |b| {
                    b.cursor_pointer().hover(|s| s.bg(content(0.06))).on_click(
                        cx.listener(|this, _, window, cx| this.open_model_picker(window, cx)),
                    )
                }),
            );
        let cancel = modal::disabled(
            modal::ghost_button("emoji-lib-import-cancel", "Cancel")
                .debug_selector(|| "emoji-lib-import-cancel".into()),
            saving,
        )
        .when(!saving, |b| {
            b.on_click(cx.listener(|this, _, window, cx| this.import_cancel(window, cx)))
        });
        let buttons = if d.done {
            div().child(
                modal::primary_button("emoji-lib-import-close", "Close")
                    .debug_selector(|| "emoji-lib-import-close".into())
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, window, cx| this.import_cancel(window, cx))),
            )
        } else {
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(cancel)
                .children((!resort).then(|| {
                    modal::disabled(
                        modal::ghost_button("emoji-lib-import-skip", "Import without sorting")
                            .debug_selector(|| "emoji-lib-import-skip".into()),
                        !review_ready,
                    )
                    .when(review_ready, |b| {
                        b.on_click(
                            cx.listener(|this, _, window, cx| this.accept(false, window, cx)),
                        )
                    })
                }))
                .child(
                    modal::disabled(
                        modal::primary_button("emoji-lib-import-use", "Use this sorting")
                            .debug_selector(|| "emoji-lib-import-use".into()),
                        !review_ready,
                    )
                    .when(review_ready, |b| {
                        b.on_click(cx.listener(|this, _, window, cx| this.accept(true, window, cx)))
                    }),
                )
        };
        let picker_surface = self
            .picker
            .as_ref()
            .map(|p| p.menu.read(cx).surface_region());
        Some(
            div()
                .child(modal::barrier("emoji-lib-import-scrim", PRIORITY, window))
                .child(
                    popover("emoji-lib-import-modal", Anchor::Center)
                        .modal_surface(true)
                        .width(width)
                        .gap(0.)
                        .priority(PRIORITY)
                        .map(|p| match picker_surface {
                            // A press in the open model list is not a press outside.
                            Some(surface) => p.ignore(surface),
                            None => p,
                        })
                        .on_dismiss(move |window, cx| {
                            let _ = weak.update(cx, |this, cx| this.import_cancel(window, cx));
                        })
                        .child(
                            div()
                                .id("emoji-lib-import-dialog")
                                .debug_selector(|| "emoji-lib-import-dialog".into())
                                .role(Role::Dialog)
                                .aria_label(heading.clone())
                                .track_focus(&d.focus)
                                .key_context("EmojiImport")
                                .p_5()
                                .flex()
                                .flex_col()
                                .gap_3()
                                .child(
                                    div().child(modal::title(heading)).child(
                                        modal::body(subtitle)
                                            .id("emoji-lib-import-subtitle")
                                            .debug_selector(|| "emoji-lib-import-subtitle".into()),
                                    ),
                                )
                                .child(model_row)
                                .child(body)
                                .child(
                                    div()
                                        .pt_3()
                                        .border_t_1()
                                        .border_color(content(0.08))
                                        .flex()
                                        .items_center()
                                        .gap_3()
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .text_size(px(12.))
                                                .line_height(px(17.))
                                                .text_color(content(0.45))
                                                .child(NOTE),
                                        )
                                        .child(buttons),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }

    fn steps(&self, d: &ImportDialog, step: &Step) -> AnyElement {
        let row = |id: &'static str, done: bool, text: String| {
            div()
                .id(id)
                .debug_selector(move || id.into())
                .flex()
                .items_center()
                .gap_2()
                .text_size(px(13.))
                .line_height(px(19.))
                .text_color(content(if done { 0.8 } else { 0.6 }))
                .child(div().w(px(16.)).flex_shrink_0().child(
                    icon(if done { "check" } else { "refresh-cw" }, 12.).text_color(if done {
                        content(0.7)
                    } else {
                        accent_color()
                    }),
                ))
                .child(div().min_w_0().child(text))
        };
        let resort = matches!(d.mode, Mode::Resort { .. });
        let mut rows = div().flex().flex_col().gap_2();
        if !resort {
            rows = rows.child(match step.entries {
                Some(n) => row(
                    "emoji-lib-step-unpacked",
                    true,
                    format!("Unpacked {n} files"),
                ),
                None => row("emoji-lib-step-unpacked", false, "Unpacking...".into()),
            });
        }
        if let Some(kept) = &step.kept {
            rows = rows.child(row(
                "emoji-lib-step-kept",
                true,
                format!(
                    "Kept {} images: {} (skipped: {})",
                    kept.count,
                    format_text(&kept.formats),
                    kept.skipped
                ),
            ));
        }
        let (done, total) = step.progress;
        if step.kept.is_some() || resort {
            rows = match &step.review {
                Some(review) => rows.child(row(
                    "emoji-lib-step-sorting",
                    true,
                    match &review.source {
                        Source::Model { .. } => {
                            format!("Sorted into emotions with {}", d.model_label)
                        }
                        Source::Names { .. } => "Suggested a sorting from the file names".into(),
                    },
                )),
                None => rows
                    .child(row(
                        "emoji-lib-step-sorting",
                        false,
                        dialog_texts::sorting_line(&d.model_label, done, total),
                    ))
                    .child(
                        div()
                            .id("emoji-lib-progress")
                            .debug_selector(|| "emoji-lib-progress".into())
                            .ml(px(24.))
                            .w(px(300.))
                            .h(px(6.))
                            .rounded_full()
                            .overflow_hidden()
                            .bg(content(0.08))
                            .child(
                                div()
                                    .h_full()
                                    .w(gpui::relative(if total == 0 {
                                        0.
                                    } else {
                                        done as f32 / total as f32
                                    }))
                                    .bg(accent_color()),
                            ),
                    ),
            };
        }
        rows.into_any_element()
    }

    fn review(
        &self,
        library: &emoji::Library,
        d: &ImportDialog,
        step: &Step,
        review: &Review,
    ) -> AnyElement {
        let mut groups: Vec<(String, Vec<&sort::Item>)> = library
            .emotions
            .iter()
            .map(|e| (e.name.clone(), Vec::new()))
            .collect();
        let mut unsorted: Vec<&sort::Item> = Vec::new();
        for item in &step.items {
            let emotion = review
                .suggestion
                .get(&item.key)
                .cloned()
                .flatten()
                .and_then(|name| library.emotion_id(&name).map(str::to_owned))
                .and_then(|id| library.emotions.iter().position(|e| e.id == id));
            match emotion {
                Some(index) => groups[index].1.push(item),
                None => unsorted.push(item),
            }
        }
        let went = unsorted.len();
        groups.push(("Unsorted".into(), unsorted));
        let boxes: Vec<AnyElement> = groups
            .into_iter()
            .map(|(name, items)| {
                let key = name.to_lowercase().replace(' ', "-");
                div()
                    .id(SharedString::from(format!("emoji-lib-review-{key}")))
                    .debug_selector({
                        let key = key.clone();
                        move || format!("emoji-lib-review-{key}")
                    })
                    .flex_1()
                    .min_w_0()
                    .p_2()
                    .rounded_lg()
                    .border_1()
                    .border_color(content(0.08))
                    .bg(content(0.03))
                    .flex()
                    .flex_col()
                    .gap_1p5()
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .text_size(px(12.))
                            .child(div().font_weight(FontWeight::MEDIUM).child(name))
                            .child(
                                div()
                                    .text_color(content(0.4))
                                    .child(items.len().to_string()),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_1()
                            .children(items.into_iter().map(review_chip)),
                    )
                    .into_any_element()
            })
            .collect();
        let mut rows = Vec::new();
        let mut boxes = boxes.into_iter();
        loop {
            let row: Vec<AnyElement> = boxes.by_ref().take(4).collect();
            if row.is_empty() {
                break;
            }
            let fill = 4 - row.len();
            rows.push(
                div()
                    .flex()
                    .gap_2()
                    .children(row)
                    .children((0..fill).map(|_| div().flex_1().min_w_0())),
            );
        }
        div()
            .id("emoji-lib-review")
            .debug_selector(|| "emoji-lib-review".into())
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .text_color(content(0.55))
                    .child(if matches!(d.mode, Mode::Resort { .. }) {
                        "Suggested sorting. Nothing changes until you press Use this sorting; you can change everything afterwards."
                    } else {
                        "Suggested sorting. Nothing is saved until you press Use this sorting; you can change everything afterwards."
                    }),
            )
            .children(match &review.source {
                Source::Names { reason } => Some(
                    div()
                        .id("emoji-lib-import-notice")
                        .debug_selector(|| "emoji-lib-import-notice".into())
                        .role(Role::Status)
                        .rounded_lg()
                        .bg(content(0.05))
                        .px_3()
                        .py_2()
                        .text_size(px(12.))
                        .line_height(px(18.))
                        .text_color(content(0.75))
                        .child(names_notice(reason)),
                ),
                Source::Model { .. } => None,
            })
            .child(div().flex().flex_col().gap_2().children(rows))
            .children((went > 0).then(|| {
                div()
                    .id("emoji-lib-import-unsorted")
                    .debug_selector(|| "emoji-lib-import-unsorted".into())
                    .text_size(px(12.))
                    .text_color(content(0.55))
                    .child(format!("{went} emoji went to Unsorted."))
            }))
            .into_any_element()
    }
}

/// A 36 point chip in the review grid.
fn review_chip(item: &sort::Item) -> AnyElement {
    let tip: SharedString = format!("{} \u{b7} {}", item.name, item.format.label()).into();
    let id = format!("emoji-lib-review-chip-{}", item.key);
    let selector = id.clone();
    div()
        .id(SharedString::from(id.clone()))
        .debug_selector(move || selector.clone())
        .relative()
        .size(px(36.))
        .flex_shrink_0()
        .rounded_lg()
        .border_1()
        .border_color(content(0.08))
        .bg(content(0.03))
        .flex()
        .items_center()
        .justify_center()
        .child(picture(&item.path, 28., None))
        .when(item.format == Format::Gif, |d| {
            d.child(
                div()
                    .absolute()
                    .right(px(-4.))
                    .bottom(px(-4.))
                    .px(px(3.))
                    .rounded(px(4.))
                    .bg(content(0.2))
                    .text_size(px(9.))
                    .line_height(px(12.))
                    .text_color(content(0.9))
                    .child("GIF"),
            )
        })
        .tooltip(move |_, cx| cx.new(|_| CloudTip(tip.clone())).into())
        .into_any_element()
}

/// Suggests an emotion for every item and reports progress and the result.
#[allow(clippy::too_many_arguments)]
fn sort_job(
    items: Vec<sort::Item>,
    emotions: Vec<EmotionRef>,
    parts: SortParts,
    factory: SorterFactory,
    label: String,
    scratch: PathBuf,
    cancel: Arc<AtomicBool>,
    tx: async_channel::Sender<Event>,
) {
    // A model that cannot be reached is not an error: the names fallback says why. A provider
    // that cannot read images keeps the wording `sort` gives.
    let (runner, why) = match factory(parts) {
        Ok(runner) => (runner, None),
        Err(error) => (
            None,
            Some(format!(
                "{label} is not available: {}",
                error.trim_end_matches('.')
            )),
        ),
    };
    let mut result = sort::sort(
        &items,
        &emotions,
        runner.as_deref(),
        &label,
        &scratch,
        &cancel,
        |p| {
            let _ = tx.send_blocking(Event::Progress(p));
        },
    );
    if let (Some(why), Ok(sorted)) = (why, result.as_mut())
        && let Source::Names { reason } = &mut sorted.source
    {
        *reason = format!("{why}, so emoji were suggested from their file names.");
    }
    let _ = tx.send_blocking(match result {
        Ok(sorted) => Event::Sorted(sorted),
        Err(error) => Event::Failed(error),
    });
}
