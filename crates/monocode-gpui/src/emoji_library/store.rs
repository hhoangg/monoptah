//! The one emoji library of the app: its packs and emotions as last loaded or edited, and the
//! serialized, debounced writer that keeps `<data>/emoji/emoji.json` current.
//!
//! The library is an `Arc<Library>` edited in place with `Arc::make_mut`, so a chat that holds
//! an older `Arc` keeps a stable snapshot. Every edit bumps a generation and marks the store
//! dirty; one background write runs at a time and the latest state is always written last.
//! Views observe the store; `Monos` subscribes to its events to push the library to its chats
//! and to drop a removed pack from every Mono's selection.
use crate::runtime::Database;
use gpui::{App, AppContext, Context, Entity, EventEmitter, Global};
use monocode_core::{
    accounts::Profile,
    mono::emoji::{self, Library},
    provider::Kind,
};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

/// How long a text field rests before what was typed is written.
pub(crate) const SAVE_DELAY: Duration = Duration::from_millis(400);

pub(crate) const READ_ONLY: &str = "This emoji library was saved by a newer version of Monoptah. It is shown here but cannot be changed.";

/// What the library tells its listeners.
pub(crate) enum EmojiEvent {
    /// The library was loaded or edited (in memory; the write may still be pending).
    Changed,
    /// A pack was removed from the library and its folder deleted.
    PackRemoved(String),
}

/// The provider, model and account a sorting asks. Remembered from the Mono whose settings were
/// opened last, so the import dialog starts on a model the user already chose.
#[derive(Clone, Debug)]
pub(crate) struct SortModel {
    pub kind: Kind,
    pub model: Option<String>,
    pub account: Profile,
}

pub(crate) struct EmojiStore {
    /// The app database, which a sorting needs to find its provider account.
    database: Database,
    dir: PathBuf,
    library: Arc<Library>,
    loaded: bool,
    read_only: bool,
    error: Option<String>,
    loading: bool,
    /// The library changed since it was last written.
    dirty: bool,
    saving: bool,
    /// Typed text is waiting for its write, by the number of the timer.
    debounce: Option<u64>,
    debounce_next: u64,
    /// How many edits the library has had: a read that began before an edit is stale.
    edit_gen: u64,
    /// Stale scratch folders were removed since the app started.
    cleaned: bool,
    /// A read was refused because an edit, a write or an import was under way; whether forced.
    wanted: Option<bool>,
    /// Set by the library view while its import dialog is open.
    import_open: Arc<AtomicBool>,
    /// Packs a finished import added and no write has covered yet.
    fresh: Vec<String>,
    /// The model of the Mono whose settings were opened last.
    last_model: Option<SortModel>,
}

impl EventEmitter<EmojiEvent> for EmojiStore {}

struct Shared(Entity<EmojiStore>);
impl Global for Shared {}

/// The app's library, once installed.
pub(crate) fn get(cx: &App) -> Option<Entity<EmojiStore>> {
    cx.try_global::<Shared>().map(|s| s.0.clone())
}

/// Install the library kept under `data` (the app data folder) unless one is installed already,
/// and start reading it.
pub(crate) fn install(database: Database, data: &Path, cx: &mut App) -> Entity<EmojiStore> {
    if let Some(store) = get(cx) {
        return store;
    }
    let dir = emoji::dir(data);
    let store = cx.new(|cx| {
        let mut store = EmojiStore::new(database, dir);
        store.load(false, cx);
        store
    });
    cx.set_global(Shared(store.clone()));
    store
}

impl EmojiStore {
    fn new(database: Database, dir: PathBuf) -> Self {
        Self {
            database,
            dir,
            library: Arc::new(Library::default()),
            loaded: false,
            read_only: false,
            error: None,
            loading: false,
            dirty: false,
            saving: false,
            debounce: None,
            debounce_next: 0,
            edit_gen: 0,
            cleaned: false,
            wanted: None,
            import_open: Arc::new(AtomicBool::new(false)),
            fresh: Vec::new(),
            last_model: None,
        }
    }

    pub(crate) fn database(&self) -> Database {
        self.database.clone()
    }

    /// The `<data>/emoji` folder.
    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }

    pub(crate) fn library(&self) -> &Arc<Library> {
        &self.library
    }

    pub(crate) fn loaded(&self) -> bool {
        self.loaded
    }

    pub(crate) fn read_only(&self) -> bool {
        self.read_only
    }

    pub(crate) fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Shared with the library view: true while its import dialog is open, so a read does not
    /// run over the packs it is about to add.
    pub(crate) fn import_flag(&self) -> Arc<AtomicBool> {
        self.import_open.clone()
    }

    /// The model of the Mono whose settings were opened last.
    pub(crate) fn last_model(&self) -> Option<&SortModel> {
        self.last_model.as_ref()
    }

    pub(crate) fn remember_model(&mut self, model: SortModel) {
        self.last_model = Some(model);
    }

    /// Whether an edit, a write or an import is under way that a read must not run over.
    fn busy(&self) -> bool {
        self.dirty
            || self.debounce.is_some()
            || self.saving
            || self.import_open.load(Ordering::Relaxed)
    }

    /// Reads the library in the background. `force` reads again even when one is loaded. A read
    /// refused because the store is busy is tried again when it is not.
    pub(crate) fn load(&mut self, force: bool, cx: &mut Context<Self>) {
        if self.loading || (!force && self.loaded) {
            return;
        }
        if self.busy() {
            *self.wanted.get_or_insert(false) |= force;
            return;
        }
        self.loading = true;
        // The first read since the app started also removes what an earlier run left behind.
        // No import can be open yet: the dialog needs the library loaded.
        let clean = !std::mem::replace(&mut self.cleaned, true);
        let generation = self.edit_gen;
        let dir = self.dir.clone();
        let work = cx.background_executor().spawn(async move {
            if clean {
                emoji::clean_stale(&dir);
            }
            emoji::load(&dir)
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                let stale = this.edit_gen != generation || this.busy();
                match result {
                    // An edit, a write or an import that began while this was reading wins.
                    Ok(_) if stale => {}
                    Ok(loaded) => {
                        this.library = Arc::new(loaded.library);
                        this.loaded = true;
                        this.read_only = loaded.read_only;
                        this.error = None;
                        cx.emit(EmojiEvent::Changed);
                    }
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Starts the read that was refused while the store was busy, when it is free now.
    pub(crate) fn retry_load(&mut self, cx: &mut Context<Self>) {
        if let Some(force) = self.wanted.take() {
            self.load(force, cx);
        }
    }

    /// Changes the library in place and queues it for writing. An `Err` leaves the library as
    /// it was and writes nothing. Nothing is edited while the file is read-only.
    pub(crate) fn edit<R>(
        &mut self,
        cx: &mut Context<Self>,
        edit: impl FnOnce(&mut Library) -> Result<R, String>,
    ) -> Result<R, String> {
        self.edit_with(cx, false, edit)
    }

    /// Like `edit` for text typed into a field: the write waits until typing pauses.
    pub(crate) fn edit_typed<R>(
        &mut self,
        cx: &mut Context<Self>,
        edit: impl FnOnce(&mut Library) -> Result<R, String>,
    ) -> Result<R, String> {
        self.edit_with(cx, true, edit)
    }

    fn edit_with<R>(
        &mut self,
        cx: &mut Context<Self>,
        typed: bool,
        edit: impl FnOnce(&mut Library) -> Result<R, String>,
    ) -> Result<R, String> {
        if self.read_only {
            return Err(READ_ONLY.into());
        }
        if !self.loaded {
            return Err("The emoji library is not loaded yet".into());
        }
        let value = edit(Arc::make_mut(&mut self.library))?;
        self.edit_gen += 1;
        if typed {
            self.wait_to_save(cx);
        } else {
            // This write carries the typed text too.
            self.debounce = None;
            self.dirty = true;
            self.pump(cx);
        }
        cx.emit(EmojiEvent::Changed);
        cx.notify();
        Ok(value)
    }

    /// Writes the library `SAVE_DELAY` after the last typed change; the last state wins.
    fn wait_to_save(&mut self, cx: &mut Context<Self>) {
        self.debounce_next += 1;
        let turn = self.debounce_next;
        self.debounce = Some(turn);
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SAVE_DELAY).await;
            let _ = this.update(cx, |this, cx| {
                if this.debounce == Some(turn) {
                    this.flush(cx);
                }
            });
        })
        .detach();
    }

    /// Queues the write of typed text now, for example when its field is left.
    pub(crate) fn flush(&mut self, cx: &mut Context<Self>) {
        if self.debounce.take().is_some() {
            self.dirty = true;
            self.pump(cx);
        }
    }

    /// Writes the changed library, one write at a time. A change made during a write marks the
    /// store dirty again, so the latest state is always written last.
    fn pump(&mut self, cx: &mut Context<Self>) {
        if self.saving || !self.dirty {
            return;
        }
        self.dirty = false;
        self.saving = true;
        let snapshot = self.library.clone();
        // The packs this write covers: if it fails they are not kept.
        let fresh = std::mem::take(&mut self.fresh);
        let dir = self.dir.clone();
        let work = cx
            .background_executor()
            .spawn(async move { emoji::save(&dir, &snapshot) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.saving = false;
                match result {
                    Ok(()) => this.error = None,
                    Err(error) => {
                        let kept = this.drop_unsaved_packs(&fresh, cx);
                        let note = if kept {
                            ""
                        } else {
                            " The imported pack was not kept."
                        };
                        this.error =
                            Some(format!("Could not save the emoji library. {error}{note}"));
                    }
                }
                this.pump(cx);
                this.retry_load(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// A write that failed after an import: take the new packs out of memory and delete their
    /// folders, so nothing is left on disk that no saved file knows about. True when there was
    /// nothing to take out.
    fn drop_unsaved_packs(&mut self, packs: &[String], cx: &mut Context<Self>) -> bool {
        if packs.is_empty() {
            return true;
        }
        Arc::make_mut(&mut self.library)
            .packs
            .retain(|p| !packs.contains(&p.id));
        self.discard_pack_folders(packs.to_vec(), cx);
        cx.emit(EmojiEvent::Changed);
        false
    }

    /// Deletes `<dir>/packs/<id>` for each id, off the main thread.
    pub(crate) fn discard_pack_folders(&self, packs: Vec<String>, cx: &mut Context<Self>) {
        let dir = self.dir.clone();
        cx.background_executor()
            .spawn(async move {
                for id in packs {
                    let _ = std::fs::remove_dir_all(emoji::pack_dir(&dir, &id));
                }
            })
            .detach();
    }

    /// Adds a pack whose folder is already in place. When the library cannot take it, the
    /// folder is deleted again.
    pub(crate) fn add_pack(
        &mut self,
        pack: emoji::Pack,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let id = pack.id.clone();
        // Noted first: the write starts inside `edit`.
        self.fresh.push(id.clone());
        let added = self.edit(cx, move |l| {
            l.packs.push(pack);
            Ok(())
        });
        if added.is_err() {
            // The library cannot take it: no file will ever know it.
            self.fresh.retain(|p| *p != id);
            self.discard_pack_folders(vec![id], cx);
        }
        added
    }

    /// Removes a pack and its folder. The folder goes first, off the main thread; the library
    /// follows once it is gone. Listeners hear `PackRemoved` so no Mono keeps the id.
    pub(crate) fn remove_pack(&mut self, pack_id: &str, cx: &mut Context<Self>) {
        if self.read_only || !self.loaded {
            return;
        }
        let (dir, mut copy) = (self.dir.clone(), (*self.library).clone());
        let pack_id = pack_id.to_owned();
        let work = {
            let pack_id = pack_id.clone();
            cx.background_executor()
                .spawn(async move { copy.remove_pack(&dir, &pack_id) })
        };
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        let _ = this.edit(cx, |l| {
                            l.packs.retain(|p| p.id != pack_id);
                            Ok(())
                        });
                        cx.emit(EmojiEvent::PackRemoved(pack_id));
                    }
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }

    #[cfg(test)]
    pub(crate) fn forget_model(&mut self) {
        self.last_model = None;
    }

    #[cfg(test)]
    pub(crate) fn pending_read(&self) -> Option<bool> {
        self.wanted
    }

    #[cfg(test)]
    pub(crate) fn forget_cleaned(&mut self) {
        self.cleaned = false;
    }
}
