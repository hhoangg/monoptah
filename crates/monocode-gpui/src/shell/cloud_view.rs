//! Opening a file of a cloud workspace from a chat link.
//!
//! In a cloud session the agent mentions paths that exist only in the codespace or sandbox.
//! A click on one that is not a local file is read through the Host over the session's live
//! tunnel (`monocode_core::cloud_sessions::view`). The workspace idles between turns, so a
//! click (a user action) reconnects or resumes it first, off the UI thread:
//!
//! * cannot be reconnected: the card says why;
//! * previewable and up to 5 MB: opened in memory in the image or document viewer;
//! * anything else: streamed through `files.readBytes` into the session's `cloud-cache/` behind
//!   a card with a progress bar and Cancel (a confirmation first above 500 MB), then opened
//!   automatically (the viewers, or the default app for what they cannot show; programs and
//!   installers are only offered with "Reveal in Finder");
//! * a Host without `files.readBytes`: the card says the file opens after the Host update
//!   (or that the update was blocked, with the `sbx policy allow` hint). The open is queued
//!   and runs when a later connect finishes the update. There is no copy fallback and no
//!   Download button.
//!
//! Local sessions and local files never reach this module.
use super::*;
use crate::{document_lightbox::CloudDoc, theme::shell_fill};
use monocode_core::cloud_sessions::{
    CloudSessions,
    view::{CloudFile, DownloadPlan, FileKind, Opened, ViewError},
    view_cache::Downloaded,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// The shell's cloud-file card, at most one at a time.
pub(super) struct CloudView {
    id: u64,
    /// The session whose file this is.
    session: i64,
    name: String,
    /// The path as clicked, kept for a queued open.
    target: String,
    /// A queued open that runs by itself: it never reconnects and stays quiet when the Host
    /// still cannot send the file.
    from_queue: bool,
    phase: Phase,
}

enum Phase {
    /// Asking the Host for the file.
    /// `status` is the latest line of a reconnect that the click started, if any.
    Reading {
        cancel: Arc<AtomicBool>,
        status: Arc<Mutex<String>>,
    },
    /// Nothing more to do; `text` says why.
    Message(String),
    /// Over the per-file threshold: waiting for the user.
    Confirm { file: CloudFile, size: u64 },
    Downloading {
        cancel: Arc<AtomicBool>,
        progress: Arc<Mutex<(u64, u64)>>,
    },
    /// Downloaded, but the app cannot show it.
    Saved { path: PathBuf, note: String },
}

/// What a background read of a file produced, ready for the UI thread.
#[derive(Debug, PartialEq)]
pub(super) enum Outcome {
    Image {
        data: String,
        mime: &'static str,
        name: String,
    },
    Document {
        name: String,
        path: PathBuf,
        doc: CloudDoc,
    },
    Download {
        file: CloudFile,
        plan: DownloadPlan,
    },
    /// The Host lacks `files.readBytes`. `queue`: the open waits for the Host update.
    HostUpdate {
        text: String,
        queue: bool,
    },
    Message(String),
}

/// Whether a click on a file link may belong to the active cloud session's transcript.
pub(super) fn takes_cloud_link(mono_active: bool, document_lightbox_open: bool) -> bool {
    !mono_active && !document_lightbox_open
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// The path of a clicked link as the cloud workspace sees it: no `file://` prefix, escapes or
/// line suffix. Relative paths stay relative; a `remote://<environment>` link is checked
/// against the session's machine by the core.
pub(super) fn cloud_target(raw: &str) -> String {
    crate::file_editor::split_location(raw).0
}

/// Reads the file `raw` of session `session` for display. BLOCKS; run it on a worker.
/// Never wakes anything: without a live tunnel the outcome says to reconnect.
pub(super) fn read_for_view(
    cloud: &CloudSessions,
    session: i64,
    raw: &str,
    cancel: &AtomicBool,
) -> Outcome {
    match read_file(cloud, session, raw, cancel) {
        Ok(outcome) => outcome,
        Err(error) => failure(error),
    }
}

/// What a failed read shows.
fn failure(error: ViewError) -> Outcome {
    match error {
        ViewError::Cancelled => Outcome::Message(String::new()),
        ViewError::HostUpdate { ref state, .. } => Outcome::HostUpdate {
            queue: state.queues_open(),
            text: error.to_string(),
        },
        error => Outcome::Message(error.to_string()),
    }
}

/// The most opens that wait for a Host update.
pub(super) const MAX_QUEUED_OPENS: usize = 8;

/// File opens waiting for the Host update of their session. Bounded, without duplicates, and
/// cleared when another session is shown.
#[derive(Debug, Default)]
pub(super) struct OpenQueue(Vec<(i64, String)>);

impl OpenQueue {
    /// Queues `target` of `session`; a repeat keeps one entry, and the oldest goes when full.
    pub(super) fn push(&mut self, session: i64, target: String) {
        if self.0.iter().any(|(s, t)| *s == session && *t == target) {
            return;
        }
        if self.0.len() >= MAX_QUEUED_OPENS {
            self.0.remove(0);
        }
        self.0.push((session, target));
    }
    pub(super) fn has(&self, session: i64) -> bool {
        self.0.iter().any(|(s, _)| *s == session)
    }
    /// Takes every entry of `session`, oldest first.
    pub(super) fn take(&mut self, session: i64) -> Vec<String> {
        let (mine, rest): (Vec<_>, Vec<_>) = self.0.drain(..).partition(|(s, _)| *s == session);
        self.0 = rest;
        mine.into_iter().map(|(_, target)| target).collect()
    }
    /// Drops what belongs to a session other than `session` (the one now shown).
    pub(super) fn keep_only(&mut self, session: i64) {
        self.0.retain(|(s, _)| *s == session);
    }
    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.0.len()
    }
}

/// Whether a downloaded file is opened by itself with the default app: programs, scripts and
/// installers are only revealed.
pub(super) fn opens_by_itself(path: &std::path::Path) -> bool {
    !path
        .file_name()
        .is_some_and(|n| monocode_core::cloud_sessions::view::runs_code(&n.to_string_lossy()))
}

/// Brings a workspace back, reporting progress lines; `Ok(false)` means there is nothing to do.
pub(super) type Reconnect<'a> = dyn Fn(&mut dyn FnMut(&str)) -> Result<bool, String> + 'a;

/// Shown when a click finds the sandbox already being reconnected by someone else.
const ALREADY_RECONNECTING: &str =
    "A reconnect of this sandbox is already in progress. Try again in a moment.";

/// [`read_for_view`] for a click on a link, which is a user action: when the workspace is not
/// connected (it idles between turns) `reconnect` brings it back first (`Ok(false)` means there
/// is nothing to reconnect, e.g. it is unavailable), then the read is tried once more.
/// `progress` hears the lines of the reconnect. The flag says that a reconnect succeeded.
/// BLOCKS; run it on a worker.
pub(super) fn read_for_click(
    cloud: &CloudSessions,
    session: i64,
    raw: &str,
    cancel: &AtomicBool,
    reconnect: &Reconnect<'_>,
    progress: &mut dyn FnMut(&str),
) -> (Outcome, bool) {
    match read_file(cloud, session, raw, cancel) {
        Err(ViewError::NotConnected(text)) => match reconnect(progress) {
            Ok(true) => (read_for_view(cloud, session, raw, cancel), true),
            Ok(false) => (Outcome::Message(text), false),
            Err(_) if cancel.load(Ordering::Relaxed) => (Outcome::Message(String::new()), false),
            Err(error) if error == "This sandbox is already reconnecting." => {
                (Outcome::Message(ALREADY_RECONNECTING.into()), false)
            }
            Err(error) => (Outcome::Message(error), false),
        },
        Ok(outcome) => (outcome, false),
        Err(error) => (failure(error), false),
    }
}

fn read_file(
    cloud: &CloudSessions,
    session: i64,
    raw: &str,
    cancel: &AtomicBool,
) -> Result<Outcome, ViewError> {
    let file = cloud.resolve_cloud_file(session, raw)?;
    let name = file.name().to_owned();
    Ok(match cloud.open_cloud_file(&file, cancel)? {
        Opened::Image { bytes, mime } => {
            use base64::Engine;
            Outcome::Image {
                data: base64::engine::general_purpose::STANDARD.encode(bytes),
                mime,
                name,
            }
        }
        Opened::Text { kind, text } => Outcome::Document {
            path: PathBuf::from(&file.rel),
            doc: CloudDoc {
                kind: if kind == FileKind::Markdown {
                    crate::document_lightbox::Kind::Markdown
                } else {
                    crate::document_lightbox::Kind::Code
                },
                text: Some(text.into()),
                web_root: None,
            },
            name,
        },
        Opened::Html { html } => Outcome::Document {
            path: PathBuf::from(&file.rel),
            doc: CloudDoc {
                kind: crate::document_lightbox::Kind::Html,
                text: Some(html.into()),
                web_root: None,
            },
            name,
        },
        Opened::Download(plan) => Outcome::Download { file, plan },
    })
}

/// How a downloaded file is shown.
#[derive(Debug, PartialEq)]
pub(super) enum Cached {
    Html {
        path: PathBuf,
        root: PathBuf,
    },
    Image {
        path: PathBuf,
    },
    Document {
        path: PathBuf,
        kind: crate::document_lightbox::Kind,
    },
    /// Left for the default app: the viewers cannot take it.
    External {
        path: PathBuf,
        note: String,
    },
}

pub(super) fn cached_open(done: &Downloaded) -> Cached {
    use crate::document_lightbox::{Kind, MAX_BYTES};
    let path = done.path.clone();
    match done.kind {
        FileKind::Html => Cached::Html {
            path,
            root: done.cache_dir.clone(),
        },
        FileKind::Image => Cached::Image { path },
        FileKind::Markdown | FileKind::Text if done.size <= MAX_BYTES => Cached::Document {
            path,
            kind: if done.kind == FileKind::Markdown {
                Kind::Markdown
            } else {
                Kind::Code
            },
        },
        FileKind::Markdown | FileKind::Text => Cached::External {
            path,
            note: format!(
                "Downloaded ({:.1} MB). It is too large to preview here.",
                done.size as f64 / (1024. * 1024.)
            ),
        },
        FileKind::Other => Cached::External {
            path,
            note: "Downloaded. This file type can't be previewed here.".into(),
        },
    }
}

fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / (1024. * 1024.))
}

impl Shell {
    /// A click on a file link. Returns true when the link was taken over because the active
    /// session is a cloud session and the path is not a local file; false leaves it to the
    /// ordinary local file opening.
    pub(super) fn open_cloud_path(
        &mut self,
        raw: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        // Only a click in the cloud session's own transcript: a Mono over the session, or the
        // document lightbox (whose relative links belong to the document), is not that.
        if !takes_cloud_link(self.mono_active(cx), self.document_lightbox.is_some()) {
            return false;
        }
        let Some(session) = self.workspace.active.filter(|id| self.is_cloud(*id)) else {
            return false;
        };
        let target = cloud_target(raw);
        if std::path::Path::new(&target).is_absolute() && std::path::Path::new(&target).is_file() {
            return false;
        }
        self.start_cloud_read(session, target, false, window, cx);
        true
    }

    fn start_cloud_read(
        &mut self,
        session: i64,
        target: String,
        from_queue: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dismiss_cloud_view(cx);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let cancel = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new(String::new()));
        let name = target.rsplit('/').next().unwrap_or(&target).to_owned();
        self.cloud_view = Some(CloudView {
            id,
            session,
            name,
            target: target.clone(),
            from_queue,
            phase: Phase::Reading {
                cancel: cancel.clone(),
                status: status.clone(),
            },
        });
        let cloud = self.cloud.clone();
        let store = self.store.clone();
        let work = cx.background_executor().spawn(async move {
            let stop = cancel.clone();
            if from_queue {
                // The connect that finished the update already happened: read, never wake.
                return (read_for_view(&cloud, session, &target, &cancel), false);
            }
            read_for_click(
                &cloud,
                session,
                &target,
                &cancel,
                &|progress| crate::cloud_wake::reconnect_blocking(&store, session, &stop, progress),
                &mut |line| {
                    if let Ok(mut current) = status.lock() {
                        *current = line.to_owned();
                    }
                },
            )
        });
        // The worker cannot touch the window, so the card polls the reconnect status.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(250))
                    .await;
                let reading = this
                    .update(cx, |this, cx| {
                        let reading = this.live(id)
                            && matches!(
                                this.cloud_view.as_ref().map(|v| &v.phase),
                                Some(Phase::Reading { .. })
                            );
                        if reading {
                            cx.notify();
                        }
                        reading
                    })
                    .unwrap_or(false);
                if !reading {
                    break;
                }
            }
        })
        .detach();
        cx.spawn_in(window, async move |this, cx| {
            let (outcome, reconnected) = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if reconnected {
                    this.cloud_reconnected_by_click(session, cx);
                }
                this.finish_cloud_read(id, outcome, window, cx)
            });
        })
        .detach();
        cx.notify();
    }

    /// A connect of `session` finished: opens what was waiting for its Host update. Only for the
    /// session in view and while no other card is up; whatever cannot be opened yet stays
    /// queued (or is queued again). Several waiting files open as the latest click only, since
    /// there is one card.
    pub(super) fn flush_queued_cloud_opens(
        &mut self,
        session: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.workspace.active != Some(session)
            || self.cloud_view.is_some()
            || !self.is_cloud(session)
        {
            return;
        }
        if let Some(target) = self.queued_cloud_opens.take(session).pop() {
            self.start_cloud_read(session, target, true, window, cx);
        }
    }

    /// A click brought the workspace of `session` back: the session's view clears its error
    /// and reloads its history, and the shell clears the cached Stopped state and banner.
    fn cloud_reconnected_by_click(&mut self, session: i64, cx: &mut Context<Self>) {
        match self.sessions.get(&session).cloned() {
            // The view emits `CloudResumed`, which the shell hears (`cloud_resumed`).
            Some(view) => view.update(cx, |view, cx| view.cloud_reconnected(cx)),
            None => self.cloud_resumed(session, cx),
        }
    }

    /// The text of the card when it shows a plain message.
    #[cfg(test)]
    pub(super) fn cloud_view_text(&self) -> String {
        match self.cloud_view.as_ref().map(|view| &view.phase) {
            Some(Phase::Message(text)) => text.clone(),
            _ => String::new(),
        }
    }

    fn live(&self, id: u64) -> bool {
        self.cloud_view.as_ref().is_some_and(|view| view.id == id)
    }

    fn finish_cloud_read(
        &mut self,
        id: u64,
        outcome: Outcome,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.live(id) {
            return;
        }
        match outcome {
            Outcome::Message(text) if text.is_empty() => self.cloud_view = None,
            Outcome::Message(text) => self.set_cloud_phase(Phase::Message(text)),
            Outcome::Image { data, mime, name } => {
                self.cloud_view = None;
                window.dispatch_action(
                    Box::new(crate::image_lightbox::Open {
                        source: crate::image_lightbox::Source::Embedded {
                            data: data.into(),
                            mime: mime.into(),
                        },
                        alt: name.into(),
                    }),
                    cx,
                );
            }
            Outcome::Document { name, path, doc } => {
                self.cloud_view = None;
                window.dispatch_action(
                    Box::new(crate::document_lightbox::Open {
                        path,
                        title: name.into(),
                        cloud: Some(doc),
                    }),
                    cx,
                );
            }
            Outcome::HostUpdate { text, queue } => {
                let (session, target, quiet) = self
                    .cloud_view
                    .as_ref()
                    .map(|v| (v.session, v.target.clone(), v.from_queue))
                    .unwrap_or_default();
                if queue {
                    self.queued_cloud_opens.push(session, target);
                }
                // A queued open that still has to wait says nothing again.
                if quiet {
                    self.cloud_view = None;
                } else {
                    self.set_cloud_phase(Phase::Message(text));
                }
            }
            Outcome::Download { file, plan } => {
                if plan.needs_confirm {
                    self.set_cloud_phase(Phase::Confirm {
                        file,
                        size: plan.size,
                    });
                } else {
                    self.start_cloud_download(id, file, false, window, cx);
                }
            }
        }
        cx.notify();
    }

    fn set_cloud_phase(&mut self, phase: Phase) {
        if let Some(view) = self.cloud_view.as_mut() {
            view.phase = phase;
        }
    }

    fn start_cloud_download(
        &mut self,
        id: u64,
        file: CloudFile,
        confirmed: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(Mutex::new((0u64, 0u64)));
        self.set_cloud_phase(Phase::Downloading {
            cancel: cancel.clone(),
            progress: progress.clone(),
        });
        let cloud = self.cloud.clone();
        let report = progress.clone();
        let work = cx.background_executor().spawn({
            let file = file.clone();
            async move {
                cloud.download_cloud_file(&file, confirmed, &cancel, &mut |done, total| {
                    if let Ok(mut current) = report.lock() {
                        *current = (done, total);
                    }
                })
            }
        });
        // The worker cannot touch the window, so the card polls its progress.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(250))
                    .await;
                let downloading = this
                    .update(cx, |this, cx| {
                        let downloading = this.live(id)
                            && matches!(
                                this.cloud_view.as_ref().map(|v| &v.phase),
                                Some(Phase::Downloading { .. })
                            );
                        if downloading {
                            cx.notify();
                        }
                        downloading
                    })
                    .unwrap_or(false);
                if !downloading {
                    break;
                }
            }
        })
        .detach();
        cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.finish_cloud_download(id, file, result, window, cx)
            });
        })
        .detach();
        cx.notify();
    }

    fn finish_cloud_download(
        &mut self,
        id: u64,
        file: CloudFile,
        result: Result<Downloaded, ViewError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.live(id) {
            return;
        }
        match result {
            Err(ViewError::Cancelled) => self.cloud_view = None,
            Err(ViewError::NeedsConfirm { size }) => {
                self.set_cloud_phase(Phase::Confirm { file, size })
            }
            Err(error) => self.set_cloud_phase(Phase::Message(error.to_string())),
            Ok(done) => {
                let title = file.name().to_owned();
                match cached_open(&done) {
                    Cached::Html { path, root } => {
                        self.cloud_view = None;
                        window.dispatch_action(
                            Box::new(crate::document_lightbox::Open {
                                path,
                                title: title.into(),
                                cloud: Some(CloudDoc {
                                    kind: crate::document_lightbox::Kind::Html,
                                    text: None,
                                    web_root: Some(root),
                                }),
                            }),
                            cx,
                        );
                    }
                    Cached::Image { path } => {
                        self.cloud_view = None;
                        window.dispatch_action(
                            Box::new(crate::image_lightbox::Open {
                                source: crate::image_lightbox::Source::Path(path),
                                alt: title.into(),
                            }),
                            cx,
                        );
                    }
                    Cached::Document { path, kind } => {
                        self.cloud_view = None;
                        window.dispatch_action(
                            Box::new(crate::document_lightbox::Open {
                                path,
                                title: title.into(),
                                cloud: Some(CloudDoc {
                                    kind,
                                    text: None,
                                    web_root: None,
                                }),
                            }),
                            cx,
                        );
                    }
                    Cached::External { path, note } => {
                        if opens_by_itself(&path) {
                            self.cloud_view = None;
                            self.open_saved(path, false, cx);
                        } else {
                            self.set_cloud_phase(Phase::Saved { path, note })
                        }
                    }
                }
            }
        }
        cx.notify();
    }

    /// Closes the card and stops whatever it was doing.
    pub(super) fn dismiss_cloud_view(&mut self, cx: &mut Context<Self>) {
        if let Some(view) = self.cloud_view.take() {
            if let Phase::Reading { cancel, .. } | Phase::Downloading { cancel, .. } = &view.phase {
                cancel.store(true, Ordering::Relaxed);
            }
            cx.notify();
        }
    }

    /// A cloud session was archived: stop its card (and any download) and delete its cache.
    pub(super) fn cloud_session_archived(&mut self, id: i64, cx: &mut Context<Self>) {
        if !self.is_cloud(id) {
            return;
        }
        if self
            .cloud_view
            .as_ref()
            .is_some_and(|view| view.session == id)
        {
            self.dismiss_cloud_view(cx);
        }
        self.clear_cloud_view_cache(vec![id], cx);
    }

    /// Deletes the files fetched to view the workspace of `ids`, once they are archived.
    pub(super) fn clear_cloud_view_cache(&self, ids: Vec<i64>, cx: &mut Context<Self>) {
        let cloud = self.cloud.clone();
        cx.background_executor()
            .spawn(async move {
                for id in ids {
                    let _ = cloud.clear_cloud_cache(id);
                }
            })
            .detach();
    }

    fn open_saved(&self, path: PathBuf, reveal: bool, cx: &mut Context<Self>) {
        cx.background_executor()
            .spawn(async move {
                let _ = if reveal {
                    monocode_core::files::reveal_path(&path)
                } else {
                    monocode_core::files::open_path_with_default_app(&path)
                };
            })
            .detach();
    }

    pub(super) fn cloud_view_card(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let view = self.cloud_view.as_ref()?;
        let button = |id: &'static str, label: &'static str| {
            crate::modal::ghost_button(id, label)
                .debug_selector(move || id.into())
                .cursor_pointer()
        };
        let (line, detail, bar, actions): (String, Option<String>, Option<f32>, Vec<AnyElement>) =
            match &view.phase {
                Phase::Reading { status, .. } => (
                    format!("Reading {}...", view.name),
                    status
                        .lock()
                        .ok()
                        .filter(|s| !s.is_empty())
                        .map(|s| s.clone()),
                    None,
                    vec![
                        button("cloud-view-cancel", "Cancel")
                            .on_click(cx.listener(|this, _, _, cx| this.dismiss_cloud_view(cx)))
                            .into_any_element(),
                    ],
                ),
                Phase::Message(text) => (
                    text.clone(),
                    None,
                    None,
                    vec![
                        button("cloud-view-close", "Close")
                            .on_click(cx.listener(|this, _, _, cx| this.dismiss_cloud_view(cx)))
                            .into_any_element(),
                    ],
                ),
                Phase::Confirm { file, size } => {
                    let file = file.clone();
                    let id = view.id;
                    (
                        format!("{} is {}.", view.name, megabytes(*size)),
                        Some("Download it to this computer to view it?".into()),
                        None,
                        vec![
                            button("cloud-view-cancel", "Cancel")
                                .on_click(cx.listener(|this, _, _, cx| this.dismiss_cloud_view(cx)))
                                .into_any_element(),
                            crate::modal::primary_button("cloud-view-download", "Download")
                                .debug_selector(|| "cloud-view-download".into())
                                .cursor_pointer()
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    if this.live(id) {
                                        this.start_cloud_download(
                                            id,
                                            file.clone(),
                                            true,
                                            window,
                                            cx,
                                        );
                                    }
                                }))
                                .into_any_element(),
                        ],
                    )
                }
                Phase::Downloading { progress, .. } => {
                    let (done, total) = progress.lock().map(|p| *p).unwrap_or((0, 0));
                    let fraction = if total == 0 {
                        0.
                    } else {
                        (done as f32 / total as f32).clamp(0., 1.)
                    };
                    (
                        format!("Downloading {}", view.name),
                        Some(format!("{} of {}", megabytes(done), megabytes(total))),
                        Some(fraction),
                        vec![
                            button("cloud-view-cancel", "Cancel")
                                .on_click(cx.listener(|this, _, _, cx| this.dismiss_cloud_view(cx)))
                                .into_any_element(),
                        ],
                    )
                }
                Phase::Saved { path, note } => {
                    let open = path.clone();
                    let reveal = path.clone();
                    // Apps, scripts and installers are only revealed, never run by a click here.
                    let runs = !opens_by_itself(path);
                    let mut actions: Vec<AnyElement> = Vec::new();
                    if !runs {
                        actions.push(
                            button("cloud-view-open", "Open with default app")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.open_saved(open.clone(), false, cx)
                                }))
                                .into_any_element(),
                        );
                    }
                    actions.push(
                        button("cloud-view-reveal", "Reveal in Finder")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.open_saved(reveal.clone(), true, cx)
                            }))
                            .into_any_element(),
                    );
                    actions.push(
                        button("cloud-view-close", "Close")
                            .on_click(cx.listener(|this, _, _, cx| this.dismiss_cloud_view(cx)))
                            .into_any_element(),
                    );
                    (format!("{}: {note}", view.name), None, None, actions)
                }
            };
        Some(
            div()
                .id("cloud-view-card")
                .debug_selector(|| "cloud-view-card".into())
                .absolute()
                .bottom(px(24.))
                .right(px(24.))
                .w(px(380.))
                .p(px(14.))
                .flex()
                .flex_col()
                .gap(px(8.))
                .rounded(px(12.))
                .border_1()
                .border_color(content(0.12))
                .bg(shell_fill())
                .text_color(content(0.9))
                .text_size(px(13.))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(div().child(line))
                .children(detail.map(|detail| {
                    div()
                        .text_size(px(12.))
                        .text_color(content(0.55))
                        .child(detail)
                }))
                .children(bar.map(|fraction| {
                    div()
                        .debug_selector(|| "cloud-view-progress".into())
                        .h(px(4.))
                        .w_full()
                        .rounded(px(2.))
                        .bg(content(0.1))
                        .child(
                            div()
                                .h_full()
                                .w(gpui::relative(fraction))
                                .rounded(px(2.))
                                .bg(content(0.7)),
                        )
                }))
                .child(div().flex().justify_end().gap(px(6.)).children(actions))
                .into_any_element(),
        )
    }
}
