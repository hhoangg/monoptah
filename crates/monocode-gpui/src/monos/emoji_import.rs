//! The "Import emoji packs" dialog: unpack each zip, let the Mono's model suggest an emotion for
//! every image, review the suggestion, then save. Re-sorting a pack reuses the same dialog.
//!
//! Unpacking and sorting run on the background executor and report through a channel. Cancel
//! stops the whole queue: it sets one shared flag, deletes the staging folder and saves nothing
//! for the zip that was open.
use super::emoji_ui::{format_list, format_text, picture};
use super::*;
use crate::{cloud_mark::CloudTip, modal};
use gpui::FontWeight;
use monocode_core::{
    agent::AccessMode,
    mono::emoji::{
        self, Format,
        import::{self, Limits, Staged},
        sort::{self, EmotionRef, Progress, Sorted, Source},
    },
    provider::{self, Request},
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

const PRIORITY: usize = 85;
const NOTE: &str = "Files are copied into Monoptah's data folder; the zip can be deleted.";

/// What a sorting run needs from the Mono's chat to talk to its model.
pub(super) struct SortParts {
    pub kind: Kind,
    pub model: Option<String>,
    pub cwd: String,
    pub settings: Settings,
    pub store: Database,
}

/// Makes the runner that asks the model, from the Mono's chat settings. It runs on a background
/// thread. `Ok(None)` means there is no model to ask, so emoji are sorted by their file names.
/// Tests replace it; under `cfg(test)` the default never starts a real model.
pub(super) type SorterFactory =
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
pub(super) enum ItemState {
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

pub(super) struct QueueItem {
    pub path: PathBuf,
    pub name: String,
    pub size: u64,
    pub state: ItemState,
}

#[derive(Clone)]
pub(super) enum Mode {
    Import,
    Resort { pack: String, name: String },
}

/// What the unpacking kept, for the "Kept N images" line.
pub(super) struct Kept {
    count: usize,
    formats: Vec<(Format, usize)>,
    skipped: String,
}

pub(super) struct Review {
    pub suggestion: HashMap<String, Option<String>>,
    pub source: Source,
}

/// The zip being worked on.
#[derive(Default)]
pub(super) struct Step {
    staged: Option<Staged>,
    entries: Option<usize>,
    kept: Option<Kept>,
    items: Vec<sort::Item>,
    progress: (usize, usize),
    pub review: Option<Review>,
}

pub(super) struct ImportDialog {
    pub mono: String,
    mono_name: String,
    model_label: String,
    mode: Mode,
    pub queue: Vec<QueueItem>,
    current: usize,
    pub step: Option<Step>,
    cancel: Arc<AtomicBool>,
    token: u64,
    focus: gpui::FocusHandle,
    /// The queue is finished but something was skipped or failed, so the dialog stays to say so.
    pub done: bool,
}

impl ImportDialog {
    fn saving(&self) -> bool {
        self.queue.iter().any(|i| i.state == ItemState::Saving)
    }
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
pub(super) fn names_notice(reason: &str) -> String {
    if reason.contains("suggested from their file names") {
        reason.to_owned()
    } else {
        format!("{reason}, so emoji were suggested from their file names.")
    }
}

impl Monos {
    /// "Import zip...": the native picker, several files at once.
    pub(super) fn emoji_pick_zips(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.emoji_ui.import.is_some() {
            return;
        }
        let picked = crate::attachment_tray::pick_files(cx);
        cx.spawn_in(window, async move |this, cx| {
            if let Some(paths) = picked.await {
                let _ = this.update_in(cx, |this, window, cx| {
                    this.emoji_import_paths(paths, window, cx)
                });
            }
        })
        .detach();
    }

    /// What a sorting run needs: the way to reach the Mono's model, or why there is none, the
    /// model's label and the emotions.
    fn emoji_sort_inputs(
        &self,
        mono: &str,
        cx: &App,
    ) -> (Result<SortParts, String>, String, Vec<EmotionRef>) {
        let emotions = self
            .emoji
            .get(mono)
            .map(|(_, library)| {
                library
                    .emotions
                    .iter()
                    .map(|e| EmotionRef {
                        name: e.name.clone(),
                        description: e.description.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let Some(chat) = self.chats.get(mono) else {
            let who = self.roster.get(mono).map(Mono::label).unwrap_or_default();
            let reason = format!("Open {who}'s chat once so its model can sort by image");
            return (Err(reason), "This Mono's model".into(), emotions);
        };
        let chat = chat.read(cx);
        let parts = SortParts {
            kind: Kind::from_id(&chat.summary.provider),
            model: chat.mono_model(),
            cwd: chat.summary.cwd.clone(),
            settings: Settings {
                account: chat.settings.account.clone(),
                environment: chat.settings.environment.clone(),
                effort: chat.settings.effort.clone(),
                model_settings: chat.settings.model_settings.clone(),
                text_only: true,
                disable_claude_hooks: true,
                access: AccessMode::ReadOnly,
                ..Default::default()
            },
            store: self.store.clone(),
        };
        (Ok(parts), chat.mono_model_label().to_string(), emotions)
    }

    /// Opens the dialog for the zips among `paths`; anything else is listed as skipped.
    pub(super) fn emoji_import_paths(
        &mut self,
        paths: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(mono) = self.active.clone() else {
            return;
        };
        if self.emoji_ui.import.is_some() || paths.is_empty() {
            return;
        }
        if self.emoji_ui.read_only.contains(&mono) {
            self.emoji_ui
                .errors
                .insert(mono, super::emoji_ui::READ_ONLY.into());
            cx.notify();
            return;
        }
        if !self.emoji.contains_key(&mono) {
            self.emoji_ui.errors.insert(
                mono,
                "The emoji settings are still loading. Try again in a moment.".into(),
            );
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
        self.emoji_open_dialog(&mono, Mode::Import, queue, window, cx);
        self.emoji_load_sizes(cx);
    }

    /// Looks up the size of every picked file off the main thread.
    fn emoji_load_sizes(&mut self, cx: &mut Context<Self>) {
        let Some(d) = self.emoji_ui.import.as_ref() else {
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
                if let Some(d) = this.emoji_ui.import.as_mut().filter(|d| d.token == token) {
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
    fn emoji_discard(&self, staged: Staged, cx: &mut Context<Self>) {
        cx.background_executor()
            .spawn(async move { import::discard(staged) })
            .detach();
    }

    fn emoji_open_dialog(
        &mut self,
        mono: &str,
        mode: Mode,
        queue: Vec<QueueItem>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (_, model_label, _) = self.emoji_sort_inputs(mono, cx);
        let mono_name = self.roster.get(mono).map(Mono::label).unwrap_or_default();
        self.emoji_ui.next_token += 1;
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        self.emoji_ui.import = Some(ImportDialog {
            mono: mono.into(),
            mono_name,
            model_label,
            mode,
            queue,
            current: 0,
            step: None,
            cancel: Arc::new(AtomicBool::new(false)),
            token: self.emoji_ui.next_token,
            focus,
            done: false,
        });
        self.emoji_import_next(window, cx);
        cx.notify();
    }

    /// Re-sorts an existing pack with the same dialog, without the unpack steps.
    pub(super) fn emoji_resort(
        &mut self,
        mono: &str,
        pack_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.emoji_ui.import.is_some() || self.emoji_ui.read_only.contains(mono) {
            return;
        }
        let Some((dir, library)) = self.emoji.get(mono) else {
            return;
        };
        let Some(pack) = library.packs.iter().find(|p| p.id == pack_id) else {
            return;
        };
        let items: Vec<sort::Item> = pack
            .emoji
            .iter()
            .map(|e| sort::Item {
                key: e.file.clone(),
                name: e.name.clone(),
                path: emoji::image_path(dir, pack, e),
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
        self.emoji_open_dialog(mono, mode, queue, window, cx);
        if let Some(step) = self.emoji_ui.import.as_mut().and_then(|d| d.step.as_mut()) {
            step.progress = (0, items.len());
            step.items = items;
        }
        self.emoji_start_resort(window, cx);
    }

    fn emoji_start_resort(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.emoji_ui.import.as_ref() else {
            return;
        };
        let (mono, token, cancel) = (d.mono.clone(), d.token, d.cancel.clone());
        let items = d.step.as_ref().map(|s| s.items.clone()).unwrap_or_default();
        let (parts, label, emotions) = self.emoji_sort_inputs(&mono, cx);
        let factory = self.emoji_ui.sorter.clone();
        let Ok(dir) = emoji::dir(&self.root, &mono) else {
            return;
        };
        let (tx, rx) = async_channel::unbounded();
        cx.background_executor()
            .spawn(async move {
                sort_job(items, emotions, parts, factory, label, dir, cancel, tx);
            })
            .detach();
        self.emoji_watch(token, rx, window, cx);
    }

    fn emoji_watch(
        &mut self,
        token: u64,
        rx: async_channel::Receiver<Event>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.spawn_in(window, async move |this, cx| {
            while let Ok(event) = rx.recv().await {
                let last = matches!(event, Event::Sorted(_) | Event::Failed(_));
                let alive = this.update_in(cx, |this, window, cx| {
                    this.emoji_import_event(token, event, window, cx)
                });
                if alive.is_err() || last {
                    break;
                }
            }
        })
        .detach();
    }

    /// Starts the next waiting zip, or finishes when none is left.
    fn emoji_import_next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.emoji_ui.import.as_mut() else {
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
                self.emoji_import_close(window, cx);
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
        let (mono, token, cancel) = (d.mono.clone(), d.token, d.cancel.clone());
        let zip = d.queue[index].path.clone();
        let Ok(dir) = emoji::dir(&self.root, &mono) else {
            return;
        };
        let existing = self
            .emoji
            .get(&mono)
            .map(|(_, l)| l.known_hashes())
            .unwrap_or_default();
        let (parts, label, emotions) = self.emoji_sort_inputs(&mono, cx);
        let factory = self.emoji_ui.sorter.clone();
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
                sort_job(sorting, emotions, parts, factory, label, dir, cancel, tx);
            })
            .detach();
        self.emoji_watch(token, rx, window, cx);
        cx.notify();
    }

    fn emoji_import_event(
        &mut self,
        token: u64,
        event: Event,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(d) = self.emoji_ui.import.as_mut().filter(|d| d.token == token) else {
            // The dialog was cancelled while this was working: nothing may stay behind.
            if let Event::Staged { staged, .. } = event {
                self.emoji_discard(staged, cx);
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
                    self.emoji_discard(staged, cx);
                }
                self.emoji_import_next(window, cx);
            }
        }
        cx.notify();
    }

    /// "Use this sorting" (`sorted`) or "Import without sorting": saves the open zip.
    pub(super) fn emoji_import_accept(
        &mut self,
        sorted: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(d) = self.emoji_ui.import.as_mut() else {
            return;
        };
        let index = d.current;
        if d.queue[index].state != ItemState::Review {
            return;
        }
        let mono = d.mono.clone();
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
                let result = self.emoji_edit(&mono, cx, |l| {
                    l.apply_sorting(&pack, &review.suggestion, sorted_by);
                    Ok(())
                });
                self.emoji_finish_item(result, window, cx);
            }
            Mode::Import => {
                let Some(staged) = step.staged.take() else {
                    return;
                };
                let Some((dir, library)) = self.emoji.get(&mono) else {
                    self.emoji_discard(staged, cx);
                    return;
                };
                let (dir, mut scratch) = (dir.clone(), (**library).clone());
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
                    self.emoji_discard(staged, cx);
                    return;
                };
                if let Some(d) = self.emoji_ui.import.as_mut() {
                    d.queue[index].state = ItemState::Saving;
                }
                // The pack joins the library only after its folder is in place.
                let (folder_dir, new_pack) = (dir.clone(), pack_id.clone());
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
                        if this
                            .emoji_ui
                            .import
                            .as_ref()
                            .is_none_or(|d| d.token != token)
                        {
                            return;
                        }
                        let result = match result {
                            Ok(()) => {
                                // Noted first: the write starts inside `emoji_edit`.
                                this.emoji_ui
                                    .fresh
                                    .entry(mono.clone())
                                    .or_default()
                                    .push(new_pack.clone());
                                let added = this.emoji_edit(&mono, cx, move |l| {
                                    l.packs.push(pack);
                                    Ok(())
                                });
                                if added.is_err() {
                                    // The library cannot take it: no file will ever know it.
                                    if let Some(fresh) = this.emoji_ui.fresh.get_mut(&mono) {
                                        fresh.retain(|id| *id != new_pack);
                                    }
                                    this.emoji_discard_pack_folders(
                                        folder_dir.clone(),
                                        vec![new_pack.clone()],
                                        cx,
                                    );
                                }
                                added
                            }
                            Err(error) => Err(error),
                        };
                        this.emoji_finish_item(result, window, cx);
                    });
                })
                .detach();
            }
        }
        cx.notify();
    }

    fn emoji_finish_item(
        &mut self,
        result: Result<(), String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(d) = self.emoji_ui.import.as_mut() {
            d.queue[d.current].state = match result {
                Ok(()) => ItemState::Saved,
                Err(error) => ItemState::Failed(error),
            };
        }
        self.emoji_import_next(window, cx);
        cx.notify();
    }

    /// Cancel, Escape, a press outside, or Close: stops everything and keeps nothing.
    pub(super) fn emoji_import_cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.emoji_ui.import.as_ref().is_some_and(|d| d.saving()) {
            return;
        }
        self.emoji_import_close(window, cx);
    }

    fn emoji_import_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.emoji_ui.import.take() else {
            return;
        };
        d.cancel.store(true, Ordering::Relaxed);
        if let Some(staged) = d.step.and_then(|s| s.staged) {
            self.emoji_discard(staged, cx);
        }
        window.focus(&self.settings_focus, cx);
        // A read that was refused while the import was open can go ahead.
        self.emoji_retry_loads(cx);
        cx.notify();
    }

    pub(super) fn emoji_import_modal(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let d = self.emoji_ui.import.as_ref()?;
        let size = window.viewport_size();
        let width = 760f32.min(size.width.as_f32() - 24.);
        let max_body = (size.height.as_f32() - 220.).max(160.);
        let weak = cx.weak_entity();
        let resort = matches!(d.mode, Mode::Resort { .. });
        let review_ready = d.queue[d.current].state == ItemState::Review && !d.done;
        let saving = d.saving();
        let heading = match &d.mode {
            Mode::Import => "Import emoji packs".to_string(),
            Mode::Resort { name, .. } => format!("Re-sort {name}"),
        };
        let subtitle = match &d.mode {
            Mode::Import => format!(
                "{} \u{b7} into {}'s emoji",
                if d.queue.len() == 1 {
                    "1 file".to_string()
                } else {
                    format!("{} files", d.queue.len())
                },
                d.mono_name
            ),
            Mode::Resort { name, .. } => format!("{name} \u{b7} {}'s emoji", d.mono_name),
        };
        let queue = (!resort).then(|| {
            div()
                .flex()
                .flex_col()
                .gap_1p5()
                .children(d.queue.iter().enumerate().map(|(i, item)| {
                    let current = i == d.current && !d.done;
                    div()
                        .id(SharedString::from(format!("mono-emoji-import-queue-{i}")))
                        .debug_selector(move || format!("mono-emoji-import-queue-{i}"))
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
            .id("mono-emoji-import-body")
            .max_h(px(max_body))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_3()
            .children(queue)
            .children(d.step.as_ref().map(|step| self.emoji_steps(d, step)))
            .children(
                d.step
                    .as_ref()
                    .and_then(|step| step.review.as_ref().map(|r| self.emoji_review(d, step, r))),
            );
        let cancel = modal::disabled(
            modal::ghost_button("mono-emoji-import-cancel", "Cancel")
                .debug_selector(|| "mono-emoji-import-cancel".into()),
            saving,
        )
        .when(!saving, |b| {
            b.on_click(cx.listener(|this, _, window, cx| this.emoji_import_cancel(window, cx)))
        });
        let buttons = if d.done {
            div().child(
                modal::primary_button("mono-emoji-import-close", "Close")
                    .debug_selector(|| "mono-emoji-import-close".into())
                    .cursor_pointer()
                    .on_click(
                        cx.listener(|this, _, window, cx| this.emoji_import_cancel(window, cx)),
                    ),
            )
        } else {
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(cancel)
                .children((!resort).then(|| {
                    modal::disabled(
                        modal::ghost_button("mono-emoji-import-skip", "Import without sorting")
                            .debug_selector(|| "mono-emoji-import-skip".into()),
                        !review_ready,
                    )
                    .when(review_ready, |b| {
                        b.on_click(cx.listener(|this, _, window, cx| {
                            this.emoji_import_accept(false, window, cx)
                        }))
                    })
                }))
                .child(
                    modal::disabled(
                        modal::primary_button("mono-emoji-import-use", "Use this sorting")
                            .debug_selector(|| "mono-emoji-import-use".into()),
                        !review_ready,
                    )
                    .when(review_ready, |b| {
                        b.on_click(cx.listener(|this, _, window, cx| {
                            this.emoji_import_accept(true, window, cx)
                        }))
                    }),
                )
        };
        Some(
            div()
                .absolute()
                .size_full()
                .child(modal::barrier("mono-emoji-import-scrim", PRIORITY, window))
                .child(
                    popover("mono-emoji-import-modal", Anchor::Center)
                        .modal_surface(true)
                        .width(width)
                        .gap(0.)
                        .priority(PRIORITY)
                        .on_dismiss(move |window, cx| {
                            let _ =
                                weak.update(cx, |this, cx| this.emoji_import_cancel(window, cx));
                        })
                        .child(
                            div()
                                .id("mono-emoji-import-dialog")
                                .debug_selector(|| "mono-emoji-import-dialog".into())
                                .role(Role::Dialog)
                                .aria_label(heading.clone())
                                .track_focus(&d.focus)
                                .key_context("MonoEmojiImport")
                                .p_5()
                                .flex()
                                .flex_col()
                                .gap_3()
                                .child(
                                    div().child(modal::title(heading)).child(
                                        modal::body(subtitle)
                                            .id("mono-emoji-import-subtitle")
                                            .debug_selector(|| "mono-emoji-import-subtitle".into()),
                                    ),
                                )
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

    fn emoji_steps(&self, d: &ImportDialog, step: &Step) -> AnyElement {
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
                    "mono-emoji-step-unpacked",
                    true,
                    format!("Unpacked {n} files"),
                ),
                None => row("mono-emoji-step-unpacked", false, "Unpacking...".into()),
            });
        }
        if let Some(kept) = &step.kept {
            rows = rows.child(row(
                "mono-emoji-step-kept",
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
            let sorting = format!(
                "Sorting into emotions with {}'s model ({})...",
                d.mono_name, d.model_label
            );
            rows = match &step.review {
                Some(review) => rows.child(row(
                    "mono-emoji-step-sorting",
                    true,
                    match &review.source {
                        Source::Model { .. } => format!(
                            "Sorted into emotions with {}'s model ({})",
                            d.mono_name, d.model_label
                        ),
                        Source::Names { .. } => "Suggested a sorting from the file names".into(),
                    },
                )),
                None => rows
                    .child(row(
                        "mono-emoji-step-sorting",
                        false,
                        format!("{sorting} {done} of {total}"),
                    ))
                    .child(
                        div()
                            .id("mono-emoji-progress")
                            .debug_selector(|| "mono-emoji-progress".into())
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

    fn emoji_review(&self, d: &ImportDialog, step: &Step, review: &Review) -> AnyElement {
        let Some((_, library)) = self.emoji.get(&d.mono) else {
            return div().into_any_element();
        };
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
                    .id(SharedString::from(format!("mono-emoji-review-{key}")))
                    .debug_selector({
                        let key = key.clone();
                        move || format!("mono-emoji-review-{key}")
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
            .id("mono-emoji-review")
            .debug_selector(|| "mono-emoji-review".into())
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
                        .id("mono-emoji-import-notice")
                        .debug_selector(|| "mono-emoji-import-notice".into())
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
                    .id("mono-emoji-import-unsorted")
                    .debug_selector(|| "mono-emoji-import-unsorted".into())
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
    let id = format!("mono-emoji-review-chip-{}", item.key);
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
    parts: Result<SortParts, String>,
    factory: SorterFactory,
    label: String,
    scratch: PathBuf,
    cancel: Arc<AtomicBool>,
    tx: async_channel::Sender<Event>,
) {
    // A model that cannot be reached is not an error: the names fallback says why. A provider
    // that cannot read images keeps the wording `sort` gives.
    let (runner, why) = match parts {
        Ok(parts) => match factory(parts) {
            Ok(runner) => (runner, None),
            Err(error) => (
                None,
                Some(format!(
                    "{label} is not available: {}",
                    error.trim_end_matches('.')
                )),
            ),
        },
        Err(reason) => (None, Some(reason)),
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
