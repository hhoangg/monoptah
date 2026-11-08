//! The startup of a cloud session in the shell: a session exists as soon as its codespace (or
//! sandbox) is requested, and this module follows the start in the background
//! (`CloudSessions::run_start`), feeds the conversation and the Codespace panel, and performs
//! Retry / Rebuild / Open in browser / Delete. State lives in `Shell::cloud_starts`, one [`CloudStartView`] per session.
use super::cloud::{CloudMark, Indicator, Tone};
use super::*;
use crate::cloud_start_view::{
    CloudStartSnapshot, ReadySummary, StartAction, StartPhase, push_log,
};
use crate::right_panel::RightTab;
use monocode_core::cloud_sessions::{
    CloudRef, CloudState, StartEvent, StartRecord, pending_starts, queued_start_message,
    queued_start_messages,
};
use std::sync::atomic::{AtomicBool, Ordering};

/// Where the right panel was before a startup opened the Codespace tab.
#[derive(Clone, Copy)]
pub(super) struct PanelReturn {
    tab: RightTab,
    was_open: bool,
}

pub(super) struct CloudStartView {
    record: StartRecord,
    log: Arc<Vec<String>>,
    /// A `run_start` worker is running (at most one per session).
    running: bool,
    /// A rebuild request is in flight.
    acting: bool,
    /// The shell opened the Codespace panel for this start on its own.
    auto_opened: bool,
    ready: Option<ReadySummary>,
    cancel: Arc<AtomicBool>,
    snapshot: Arc<CloudStartSnapshot>,
    resuming: bool,
    return_to: Option<PanelReturn>,
}

impl CloudStartView {
    pub(super) fn new(record: StartRecord) -> Self {
        let log = Arc::new(record.log_tail.clone());
        let mut view = Self {
            snapshot: Arc::new(snapshot_of(&record, &log, None, false)),
            record,
            log,
            resuming: false,
            return_to: None,
            running: false,
            acting: false,
            auto_opened: false,
            ready: None,
            cancel: Arc::new(AtomicBool::new(false)),
        };
        view.rebuild();
        view
    }
    pub(super) fn cancel_on_quit(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
    fn phase(&self) -> StartPhase {
        if self.ready.is_some() {
            StartPhase::Ready
        } else if self.record.is_failed() {
            StartPhase::Failed
        } else {
            StartPhase::Starting
        }
    }
    fn rebuild(&mut self) {
        self.snapshot = Arc::new(snapshot_of(
            &self.record,
            &self.log,
            self.ready.clone(),
            self.running,
        ));
    }
}

/// The title and body of the Rebuild confirmation. A sandbox rebuild recreates the container
/// that holds the conversations, so its text says they are lost.
pub(super) fn rebuild_prompt(
    backend: monocode_core::cloud_sessions::CloudBackendKind,
) -> (String, String) {
    use monocode_core::cloud_sessions::CloudBackendKind;
    let noun = backend.noun();
    let mut body =
        format!("This interrupts every session using this {noun} and recreates its container.");
    if backend == CloudBackendKind::DockerSandbox {
        body.push_str(
            " The conversation history of those sessions is lost; archive a session first to \
             keep it. Code in the workspace is kept.",
        );
    }
    (format!("Rebuild this {noun}?"), body)
}

impl Drop for CloudStartView {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

fn snapshot_of(
    record: &StartRecord,
    log: &Arc<Vec<String>>,
    ready: Option<ReadySummary>,
    running: bool,
) -> CloudStartSnapshot {
    let phase = if ready.is_some() {
        StartPhase::Ready
    } else if record.is_failed() {
        StartPhase::Failed
    } else {
        StartPhase::Starting
    };
    CloudStartSnapshot {
        record: record.clone(),
        phase,
        log: log.clone(),
        ready,
        running,
    }
}

impl Shell {
    /// The snapshot of a session's startup, `None` for a session that never started this way.
    pub(super) fn cloud_start_snapshot(&self, id: i64) -> Option<Arc<CloudStartSnapshot>> {
        self.cloud_starts.get(&id).map(|view| view.snapshot.clone())
    }

    /// The sidebar dot of a session whose startup is running or failed.
    pub(super) fn cloud_start_label(&self, id: i64) -> Option<Indicator> {
        match self.cloud_starts.get(&id)?.phase() {
            StartPhase::Starting => Some(Indicator {
                label: "Starting",
                tone: Tone::Busy,
            }),
            StartPhase::Failed => Some(Indicator {
                label: "Failed",
                tone: Tone::Bad,
            }),
            StartPhase::Ready => None,
        }
    }

    /// Forgets a session's startup and stops its worker (the session was deleted).
    pub(super) fn drop_cloud_start(&mut self, id: i64) {
        if let Some(view) = self.cloud_starts.remove(&id) {
            view.cancel.store(true, Ordering::Relaxed);
        }
    }

    pub(super) fn register_cloud_start(&mut self, id: i64) {
        if self.cloud_starts.contains_key(&id) {
            return;
        }
        let record = locked(&self.store)
            .ok()
            .and_then(|store| monocode_core::cloud_sessions::start_record(&store, id));
        if let Some(record) = record {
            self.cloud_starts.insert(id, CloudStartView::new(record));
        }
    }

    /// After a restart: every start the store still holds is shown again, and the ones that did
    /// not fail resume.
    pub(super) fn resume_cloud_starts(&mut self, cx: &mut Context<Self>) {
        let records = locked(&self.store)
            .map(|store| pending_starts(&store))
            .unwrap_or_default();
        for record in records {
            let id = record.session_id;
            if !self.session_summaries.contains_key(&id) {
                if let Ok(store) = locked(&self.store) {
                    let _ = monocode_core::cloud_sessions::startup::remove_start_record(&store, id);
                }
                continue;
            }
            self.mark_cloud_session(id);
            // The sidebar lists a session once it has messages; this one is launching.
            if let Some(summary) = self.session_summaries.get_mut(&id) {
                summary.has_messages = true;
            }
            let failed = record.is_failed();
            let mut view = CloudStartView::new(record);
            view.resuming = true;
            self.cloud_starts.insert(id, view);
            if !failed {
                self.track_cloud_start(id, cx);
            }
        }
    }

    /// Lists a just-begun cloud session under its local project, opens it and follows its start.
    pub(super) fn open_cloud_start(
        &mut self,
        mut summary: Session,
        record: StartRecord,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = summary.id;
        summary.has_messages = true;
        self.workspace.add_project(&record.local_project);
        let cloud = record
            .cloud()
            .unwrap_or_else(|| CloudRef::codespace(&record.codespace));
        self.cloud_marks.insert(
            id,
            CloudMark {
                local_project: record.local_project.clone(),
                codespace: cloud.name.clone(),
                kind: cloud.kind,
            },
        );
        // The state is not known yet; never claim Available.
        self.cloud_states
            .entry(cloud.key())
            .or_insert(CloudState::Unknown);
        // A failed record (an ambiguous create) waits for the user's Retry: tracking it would
        // run the start again, and with it another create, without a click.
        let failed = record.is_failed();
        let mut view = CloudStartView::new(record);
        view.auto_opened = true;
        self.cloud_starts.insert(id, view);
        self.add_loaded(summary, cx);
        self.order.retain(|s| *s != id);
        self.order.insert(0, id);
        self.activate_session(id, window, cx);
        self.show_codespace_panel(id, window, cx);
        if !failed {
            self.track_cloud_start(id, cx);
        }
        self.invalidate_chrome(cx);
        self.refresh_cloud(cx);
    }

    /// Starts following the codespace of `id` unless a worker already does.
    pub(super) fn track_cloud_start(&mut self, id: i64, cx: &mut Context<Self>) {
        let Some(view) = self.cloud_starts.get_mut(&id) else {
            return;
        };
        if view.running || view.acting || view.ready.is_some() {
            return;
        }
        let resuming = std::mem::take(&mut view.resuming);
        view.running = true;
        view.log = Arc::new(Vec::new());
        view.cancel = Arc::new(AtomicBool::new(false));
        view.rebuild();
        let cancel = view.cancel.clone();
        // Tests apply events by hand: `run_start` blocks for minutes polling a codespace.
        if cfg!(test) {
            self.publish_cloud_start(id, true, cx);
            return;
        }
        let cloud = self.cloud.clone();
        let title = self.cloud_starts[&id].record.backend().title();
        let stopped_title = title;
        let (events, incoming) = async_channel::unbounded::<StartEvent>();
        let worker_cancel = cancel.clone();
        let (result_tx, result_rx) = async_channel::bounded(1);
        // Polling/SSH may block for minutes; keep GPUI's shared worker pool free.
        std::thread::spawn(move || {
            let mut send = |event: StartEvent| {
                let _ = events.try_send(event);
            };
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if resuming {
                    cloud
                        .resume_start(id, &mut send, &worker_cancel)
                        .map(|_| ())
                } else {
                    cloud.run_start(id, &mut send, &worker_cancel).map(|_| ())
                }
            }))
            .unwrap_or_else(|_| Err(format!("{title} startup worker crashed")));
            let _ = result_tx.try_send(outcome);
        });
        cx.spawn(async move |this, cx| {
            while let Ok(first) = incoming.recv().await {
                let mut batch = vec![first];
                while batch.len() < 256 {
                    let Ok(event) = incoming.try_recv() else {
                        break;
                    };
                    batch.push(event);
                }
                let applied = this.update_in(cx, |this, window, cx| {
                    for event in batch {
                        if let StartEvent::Log(line) = event {
                            if let Some(view) = this.cloud_starts.get_mut(&id) {
                                push_log(&mut view.log, line);
                            }
                        } else {
                            this.apply_start_event(id, event, window, cx);
                        }
                    }
                    if let Some(view) = this.cloud_starts.get_mut(&id) {
                        view.rebuild();
                    }
                    this.publish_cloud_start(id, false, cx);
                });
                if applied.is_err() {
                    cancel.store(true, Ordering::Relaxed);
                    return;
                }
            }
            let result = result_rx
                .recv()
                .await
                .unwrap_or_else(|_| Err(format!("{stopped_title} startup worker stopped")));
            let _ = this.update(cx, |this, cx| this.finish_start_worker(id, result, cx));
        })
        .detach();
        self.publish_cloud_start(id, true, cx);
    }

    fn finish_start_worker(&mut self, id: i64, result: Result<(), String>, cx: &mut Context<Self>) {
        let Some(view) = self.cloud_starts.get_mut(&id) else {
            return;
        };
        view.running = false;
        if let Some(record) = locked(&self.store)
            .ok()
            .and_then(|store| monocode_core::cloud_sessions::start_record(&store, id))
        {
            view.record = record;
        }
        // An error with no `Failed` event (the store failed, the record vanished) still has to
        // show; a cancelled start is silent.
        if let Err(error) = result
            && view.ready.is_none()
            && !view.record.is_failed()
            && !view.cancel.load(Ordering::Relaxed)
        {
            view.record.error = Some(error);
        }
        view.rebuild();
        self.publish_cloud_start(id, true, cx);
    }

    fn apply_start_event(
        &mut self,
        id: i64,
        event: StartEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self.cloud_starts.get_mut(&id) else {
            return;
        };
        let mut chrome = true;
        match event {
            StartEvent::Updated(record) | StartEvent::Failed(record) => view.record = record,
            StartEvent::Log(line) => {
                push_log(&mut view.log, line);
                chrome = false;
            }
            StartEvent::Ready { session, .. } => {
                self.finish_ready(id, session, window, cx);
                return;
            }
        }
        view.rebuild();
        self.publish_cloud_start(id, chrome, cx);
        self.auto_open_codespace_panel(id, window, cx);
    }

    pub(super) fn finish_ready(
        &mut self,
        id: i64,
        session: Session,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self.cloud_starts.get_mut(&id) else {
            return;
        };
        view.ready = Some(ReadySummary {
            total_ms: view.record.elapsed_ms(monocode_core::time::now_ms()),
            steps: view.record.steps.clone(),
        });
        view.rebuild();
        let snapshot = view.snapshot.clone();
        let state_key = view
            .record
            .cloud()
            .unwrap_or_else(|| CloudRef::codespace(&view.record.codespace))
            .key();
        let auto_opened = view.auto_opened;
        let mut session = session;
        session.has_messages = true;
        self.session_summaries.insert(id, session.clone());
        self.cloud_states.insert(state_key, CloudState::Available);
        // A queued message needs the conversation to send it.
        let queued = locked(&self.store).is_ok_and(|store| {
            queued_start_message(&store, id).is_some()
                || !queued_start_messages(&store, id).is_empty()
        });
        if queued {
            self.ensure_session(id, cx);
        }
        if let Some(conversation) = self.sessions.get(&id).cloned() {
            conversation.update(cx, |conversation, cx| {
                conversation.cloud_start_ready(session, snapshot, cx)
            });
        }
        if auto_opened {
            self.restore_panel_after_ready(id, window, cx);
        }
        self.sync_codespace_pane(cx);
        self.invalidate_chrome(cx);
        self.refresh_cloud(cx);
        cx.notify();
    }

    /// Pushes the view's snapshot to the conversation and the panel.
    fn publish_cloud_start(&mut self, id: i64, chrome: bool, cx: &mut Context<Self>) {
        let Some(snapshot) = self.cloud_start_snapshot(id) else {
            return;
        };
        if let Some(conversation) = self.sessions.get(&id).cloned() {
            conversation.update(cx, |conversation, cx| {
                conversation.set_cloud_start(Some(snapshot), cx)
            });
        }
        self.sync_codespace_pane(cx);
        if chrome {
            self.invalidate_chrome(cx);
        }
        cx.notify();
    }

    pub(super) fn codespace_panel_selected(&self, cx: &App) -> bool {
        self.right_panel
            .as_ref()
            .is_some_and(|panel| panel.read(cx).tab() == RightTab::Codespace)
    }

    /// The Codespace pane follows the active session.
    pub(super) fn sync_codespace_pane(&self, cx: &mut Context<Self>) {
        let Some(panel) = &self.right_panel else {
            return;
        };
        let snapshot = self
            .workspace
            .active
            .and_then(|id| self.cloud_start_snapshot(id));
        let title = snapshot.as_ref().map_or("Codespace", |s| s.title());
        panel.update(cx, |panel, cx| panel.set_codespace_title(title, cx));
        if snapshot.is_none() && panel.read(cx).tab() == RightTab::Codespace {
            let tab = self
                .codespace_return
                .as_ref()
                .map_or(RightTab::Files, |back| back.tab);
            panel.update(cx, |panel, cx| panel.select(tab, cx));
            if self.codespace_return.is_some_and(|back| !back.was_open) {
                cx.spawn(async move |this, cx| {
                    let _ = this.update_in(cx, |this, window, cx| {
                        if this
                            .workspace
                            .active
                            .and_then(|id| this.cloud_start_snapshot(id))
                            .is_none()
                        {
                            this.set_right_panel_open(false, window, cx);
                        }
                    });
                })
                .detach();
            }
        }
        panel
            .read(cx)
            .codespace
            .clone()
            .update(cx, |pane, cx| pane.set_start(snapshot, cx));
    }

    /// Opens the Codespace panel on `id`'s startup. B2 calls it from "View log" and the chip.
    pub(crate) fn show_codespace_panel(
        &mut self,
        id: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ensure_right_panel(window, cx);
        let Some(panel) = self.right_panel.clone() else {
            return;
        };
        let tab = panel.read(cx).tab();
        if tab != RightTab::Codespace {
            self.codespace_return = Some(PanelReturn {
                tab,
                was_open: self.workspace.right_panel_open,
            });
        }
        if let Some(view) = self.cloud_starts.get_mut(&id) {
            view.return_to = self.codespace_return;
        }
        panel.update(cx, |panel, cx| panel.select(RightTab::Codespace, cx));
        self.sync_codespace_pane(cx);
        self.set_right_panel_open(true, window, cx);
        self.invalidate_chrome(cx);
        cx.notify();
    }

    /// A resumed start opens the panel once, on its first event, when its session is in view.
    pub(super) fn auto_open_codespace_panel(
        &mut self,
        id: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self.cloud_starts.get_mut(&id) else {
            return;
        };
        if view.auto_opened || view.phase() != StartPhase::Starting {
            return;
        }
        if self.workspace.active != Some(id) {
            return;
        }
        view.auto_opened = true;
        let focus = window.focused(cx);
        self.show_codespace_panel(id, window, cx);
        if let Some(focus) = focus {
            window.focus(&focus, cx);
        }
    }

    /// The codespace is ready: a panel the startup opened goes back to what it showed, or
    /// closes if it was closed before.
    fn restore_panel_after_ready(&mut self, id: i64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(panel) = self.right_panel.clone() else {
            return;
        };
        if self.workspace.active != Some(id) || panel.read(cx).tab() != RightTab::Codespace {
            return;
        }
        let back = self.cloud_starts.get(&id).and_then(|view| view.return_to);
        let (tab, was_open) = back.map_or((RightTab::Files, false), |b| (b.tab, b.was_open));
        panel.update(cx, |panel, cx| panel.select(tab, cx));
        if !was_open {
            self.set_right_panel_open(false, window, cx);
        }
    }

    /// Retry, Rebuild, Open in browser, Delete or show the panel, for a session's startup.
    pub(super) fn cloud_start_action(
        &mut self,
        id: i64,
        action: StartAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // These come from a session whose sandbox is gone: it has no start view.
        match action {
            StartAction::ArchiveSession => {
                self.open_cloud_archive(vec![id], window, cx);
                return;
            }
            StartAction::NewSession => {
                // A new local session in the project; running it on the cloud is offered by
                // the project's Workspace menu.
                self.new_chat(cx);
                return;
            }
            _ => {}
        }
        if !self.cloud_starts.contains_key(&id) {
            return;
        }
        match action {
            StartAction::ArchiveSession | StartAction::NewSession => {}
            StartAction::ShowPanel => self.show_codespace_panel(id, window, cx),
            StartAction::Retry => self.track_cloud_start(id, cx),
            StartAction::Rebuild => {
                let (heading, body) = rebuild_prompt(self.cloud_starts[&id].record.backend());
                let answer = window.prompt(
                    gpui::PromptLevel::Warning,
                    &heading,
                    Some(&body),
                    &["Rebuild", "Cancel"],
                    cx,
                );
                cx.spawn(async move |this, cx| {
                    if matches!(answer.await, Ok(0)) {
                        let _ = this.update(cx, |this, cx| this.rebuild_cloud_start(id, cx));
                    }
                })
                .detach();
            }
            // A sandbox has no web view: the buttons are hidden, and a stray request is dropped.
            StartAction::OpenInBrowser if !self.cloud_starts[&id].snapshot.offers_browser() => {}
            StartAction::OpenInBrowser => {
                let name = self.cloud_starts[&id].record.codespace.clone();
                if let Some(url) = &self.cloud_starts[&id].record.web_url {
                    cx.open_url(url);
                    return;
                }
                let gh = self.gh.clone();
                let work = cx
                    .background_executor()
                    .spawn(async move { monocode_core::codespaces::web_url(&*gh, &name) });
                cx.spawn(async move |this, cx| {
                    let result = work.await;
                    let _ = this.update(cx, |this, cx| match result {
                        Ok(url) => cx.open_url(&url),
                        Err(error) => this.report_start_error(error, cx),
                    });
                })
                .detach();
            }
            StartAction::DeleteCodespace => self.delete_start_codespace(id, window, cx),
        }
    }

    fn report_start_error(&mut self, error: String, cx: &mut Context<Self>) {
        self.notification_state
            .file_error
            .update(cx, |notice, cx| notice.set_message(Some(error), cx));
        cx.notify();
    }

    fn rebuild_cloud_start(&mut self, id: i64, cx: &mut Context<Self>) {
        let Some(view) = self.cloud_starts.get_mut(&id) else {
            return;
        };
        if view.running || view.acting {
            return;
        }
        view.acting = true;
        let cloud = self.cloud.clone();
        let work = cx
            .background_executor()
            .spawn(async move { cloud.rebuild_start(id) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                let Some(view) = this.cloud_starts.get_mut(&id) else {
                    return;
                };
                view.acting = false;
                let rebuilt = result.is_ok();
                match result {
                    Ok(record) => view.record = record,
                    Err(error) => view.record.error = Some(format!("Rebuild failed: {error}")),
                }
                view.rebuild();
                this.publish_cloud_start(id, true, cx);
                if rebuilt {
                    this.track_cloud_start(id, cx);
                }
            });
        })
        .detach();
    }

    fn delete_start_codespace(&mut self, id: i64, window: &mut Window, cx: &mut Context<Self>) {
        self.open_cloud_archive(vec![id], window, cx);
    }
}
