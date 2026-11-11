//! "Continue in cloud" in the shell (design 4.2.2, 4.5): opens the one-confirmation dialog,
//! runs the move (`CloudSessions::begin_move` / `run_move`) on a worker ONLY after the user
//! confirmed or pressed Retry, follows its events, and reacts to the flip.
//!
//! * Closing the dialog never stops a move. The worker, the record and the progress live in
//!   [`Shell::cloud_moves`], one [`MoveRun`] per session (so there is one move per session).
//! * After an app restart `pending_moves` is only LISTED: the session shows "Move in
//!   progress..." and nothing runs until the user presses Resume (Retry) or Cancel.
//! * After the flip the cloud refresh marks the session as Cloud, the transcript gets its
//!   divider, and the open conversation reloads its settings from the store (the flip wrote a
//!   handoff and reset the model there).
use super::*;
use crate::cloud_move_dialog::{Event, MoveDialog, TargetChoice};
use crate::conversation::MoveOffer;
use monocode_core::cloud_sessions::{
    CloudSessions, CloudState, HARNESS, docker_config,
    move_session::{
        CancelChoice, CancelReport, MOVE_ABORTED, MoveEvent, MoveOutcome, MoveRecord, MoveRequest,
        MoveTarget, pending_moves,
    },
    move_ui::{self, MovePreview},
};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

/// The part of the move engine the UI uses. The real one is [`CloudSessions`]; tests replace it
/// with a fake that counts calls (the UI must never call `run` without a confirmation).
pub(crate) trait MoveEngine: Send + Sync {
    /// The read-only "what will move" inspection of the working copy at `cwd`.
    fn preview(&self, cwd: &Path) -> Result<MovePreview, String>;
    /// Checks the session and saves the record. Changes nothing else.
    fn begin(&self, request: MoveRequest, busy: &dyn Fn() -> bool) -> Result<MoveRecord, String>;
    /// Runs the steps. BLOCKS and may wake a workspace: user actions only.
    fn run(
        &self,
        session: i64,
        busy: &dyn Fn() -> bool,
        events: &mut dyn FnMut(MoveEvent),
        cancel: &AtomicBool,
    ) -> Result<MoveOutcome, String>;
    fn cancel(&self, session: i64, choice: CancelChoice) -> Result<CancelReport, String>;
}

impl MoveEngine for CloudSessions {
    fn preview(&self, cwd: &Path) -> Result<MovePreview, String> {
        move_ui::preview(cwd)
    }
    fn begin(&self, request: MoveRequest, busy: &dyn Fn() -> bool) -> Result<MoveRecord, String> {
        self.begin_move(request, busy)
    }
    fn run(
        &self,
        session: i64,
        busy: &dyn Fn() -> bool,
        events: &mut dyn FnMut(MoveEvent),
        cancel: &AtomicBool,
    ) -> Result<MoveOutcome, String> {
        self.run_move(session, busy, events, cancel)
    }
    fn cancel(&self, session: i64, choice: CancelChoice) -> Result<CancelReport, String> {
        self.cancel_move(session, choice)
    }
}

/// A move of one session as the shell knows it.
pub(super) struct MoveRun {
    /// The latest record. `None` until `begin_move` answered.
    record: Option<MoveRecord>,
    /// A worker runs `begin_move` / `run_move`.
    running: bool,
    cancel: Arc<AtomicBool>,
    /// Mirrors whether the conversation runs a turn; the engine asks it at the flip.
    busy: Arc<AtomicBool>,
}

impl MoveRun {
    fn new(record: Option<MoveRecord>) -> Self {
        Self {
            record,
            running: false,
            cancel: Arc::new(AtomicBool::new(false)),
            busy: Arc::new(AtomicBool::new(false)),
        }
    }
}

pub(super) struct OpenMove {
    session: i64,
    view: Entity<MoveDialog>,
    _events: Subscription,
}

/// How a worker reports back to the UI thread.
enum Report {
    Event(MoveEvent),
    Finished(Result<MoveOutcome, String>),
}

impl Shell {
    // ------------------------------------------------------------------ offers

    /// What the Workspace menu and the session menu offer for session `id`.
    pub(super) fn move_offer(&self, id: i64) -> MoveOffer {
        if self.cloud_moves.contains_key(&id) {
            return MoveOffer::InProgress;
        }
        let Some(summary) = self.session_summaries.get(&id) else {
            return MoveOffer::Hidden;
        };
        if !summary.has_messages
            || summary.cwd.starts_with("remote://")
            || self.cloud_located.contains_key(&id)
            || self.cloud_marks.contains_key(&id)
            || self.cloud_starts.contains_key(&id)
        {
            return MoveOffer::Hidden;
        }
        MoveOffer::Offer
    }

    /// The session menu entry, `None` when the session cannot move.
    pub(super) fn cloud_move_menu_item(&self, id: i64) -> Option<MenuItem> {
        let label = match self.move_offer(id) {
            MoveOffer::Hidden => return None,
            MoveOffer::Offer => "Continue in cloud\u{2026}",
            MoveOffer::InProgress => "Move in progress\u{2026}",
        };
        Some(MenuAction::new("continue-in-cloud", label).into())
    }

    /// Tells every open conversation what it offers, and the divider of a moved session.
    pub(super) fn sync_cloud_moves(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<i64> = self.sessions.keys().copied().collect();
        for id in ids {
            let offer = self.move_offer(id);
            let label = self.move_label(id);
            if let Some(view) = self.sessions.get(&id) {
                view.update(cx, |view, cx| {
                    view.set_move_offer(offer, cx);
                    view.set_cloud_moved(label, cx);
                });
            }
        }
    }

    /// The divider text of a session that moved. Read from the store once per session.
    fn move_label(&mut self, id: i64) -> Option<String> {
        if !self.cloud_located.contains_key(&id) {
            self.move_labels.remove(&id);
            return None;
        }
        if let Some(label) = self.move_labels.get(&id) {
            return label.clone();
        }
        let label = locked(&self.store)
            .ok()
            .and_then(|store| move_ui::move_mark(&store, id))
            .map(|mark| crate::transcript::cloud_moved::moved_label(&mark))
            // A session moved before the mark existed: still say where it went on.
            .or_else(|| Some("Continued in the cloud".to_owned()));
        self.move_labels.insert(id, label.clone());
        label
    }

    // ------------------------------------------------------------------ startup

    /// After a restart: every unfinished move is LISTED (the session shows "Move in progress").
    /// Nothing runs: `run_move` may wake a workspace, so only Resume does that.
    pub(super) fn resume_cloud_moves(&mut self, cx: &mut Context<Self>) {
        let records = locked(&self.store)
            .map(|store| pending_moves(&store))
            .unwrap_or_default();
        for record in records {
            if self.session_summaries.contains_key(&record.session_id) {
                self.cloud_moves
                    .insert(record.session_id, MoveRun::new(Some(record)));
            }
        }
        self.sync_cloud_moves(cx);
    }

    // ------------------------------------------------------------------ dialog

    /// The targets the dialog offers for the project of session `id`.
    fn move_targets(&self, id: i64) -> Vec<TargetChoice> {
        let Some(summary) = self.session_summaries.get(&id) else {
            return Vec::new();
        };
        let project = &summary.cwd;
        let mut targets = Vec::new();
        if self.github_projects.get(project).copied().unwrap_or(false) {
            targets.push(TargetChoice {
                label: "New codespace".into(),
                detail: "billed to your GitHub account".into(),
                target: MoveTarget::NewCodespace { machine: None },
            });
        }
        if let Some(config) = locked(&self.store)
            .ok()
            .and_then(|store| docker_config::load(&store, project).ok().flatten())
        {
            targets.push(TargetChoice {
                label: "New Docker sandbox".into(),
                detail: "paid compute in your Docker account".into(),
                target: MoveTarget::NewSandbox { config },
            });
        }
        let mut seen = HashSet::new();
        let mut existing: Vec<_> = self
            .cloud_marks
            .values()
            .filter(|mark| &mark.local_project == project && !mark.codespace.is_empty())
            .filter(|mark| seen.insert(mark.key()))
            .collect();
        existing.sort_by(|a, b| a.codespace.cmp(&b.codespace));
        for mark in existing {
            let detail = match self.cloud_states.get(&mark.key()) {
                Some(CloudState::Stopped) => "stopped, it will be started",
                Some(CloudState::Available) => "running",
                _ => "",
            };
            targets.push(TargetChoice {
                label: format!("Existing {} {}", mark.kind.noun(), mark.codespace),
                detail: detail.into(),
                target: MoveTarget::Existing {
                    backend: mark.kind,
                    name: mark.codespace.clone(),
                },
            });
        }
        targets
    }

    fn conversation_note(&self, id: i64) -> String {
        match self.session_summaries.get(&id) {
            Some(s) if s.provider == HARNESS && s.provider_id.is_some() => {
                "Continues in the cloud with the same Claude session.".into()
            }
            Some(s) if s.provider == HARNESS => {
                "This conversation has no Claude session to resume yet. The cloud gets its messages as a summary.".into()
            }
            _ => "The cloud runs Claude. It gets a summary of this conversation and a new Claude session.".into(),
        }
    }

    /// The session menu / composer menu entry: opens the dialog. Nothing starts here.
    pub(super) fn open_cloud_move(&mut self, id: i64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(summary) = self.session_summaries.get(&id).cloned() else {
            return;
        };
        if self.move_offer(id) == MoveOffer::Hidden {
            return;
        }
        self.close_menu(cx);
        self.close_cloud_move_dialog(window, cx);
        let in_progress = self.cloud_moves.get(&id);
        let unfinished = in_progress.and_then(|run| run.record.clone());
        let running = in_progress.is_some_and(|run| run.running);
        let targets = if in_progress.is_some() {
            Vec::new()
        } else {
            self.move_targets(id)
        };
        let note = self.conversation_note(id);
        let title = summary.title.clone();
        let view = cx.new(|cx| MoveDialog::new(title, note, targets, cx));
        let events = cx.subscribe_in(&view, window, move |this, _, event: &Event, window, cx| {
            this.cloud_move_event(id, event, window, cx)
        });
        window.focus(&view.focus_handle(cx), cx);
        self.move_dialog = Some(OpenMove {
            session: id,
            view: view.clone(),
            _events: events,
        });
        if let Some(record) = unfinished {
            // A move that exists already: show where it stands, start nothing.
            view.update(cx, |dialog, cx| {
                if running {
                    dialog.update_record(record, cx);
                } else {
                    dialog.show_unfinished(record, cx);
                }
            });
        } else if running {
            view.update(cx, |dialog, cx| dialog.show_running(cx));
        } else if self.session_is_busy(id, cx) {
            view.update(cx, |dialog, cx| {
                dialog.refuse(monocode_core::cloud_sessions::move_session::BUSY.into(), cx)
            });
        } else {
            self.load_move_preview(id, summary.cwd, view, cx);
        }
        cx.notify();
    }

    fn session_is_busy(&self, id: i64, cx: &App) -> bool {
        self.sessions.get(&id).is_some_and(|c| {
            let c = c.read(cx);
            c.busy || c.is_stopping()
        })
    }

    /// Reads the working copy off the UI thread and shows the answer. Read-only.
    fn load_move_preview(
        &self,
        id: i64,
        project: String,
        view: Entity<MoveDialog>,
        cx: &mut Context<Self>,
    ) {
        let engine = self.cloud_move_engine.clone();
        let store = self.store.clone();
        cx.spawn(async move |_, cx| {
            let work = cx.background_executor().spawn(async move {
                // A session in a worktree moves that worktree.
                let cwd = locked(&store)
                    .and_then(|s| s.agent_settings(id))
                    .ok()
                    .flatten()
                    .and_then(|s| s.worktree_cwd)
                    .filter(|cwd| !cwd.is_empty() && Path::new(cwd).is_dir())
                    .unwrap_or(project);
                engine.preview(Path::new(&cwd))
            });
            let preview = work.await;
            let _ = view.update(cx, |dialog, cx| dialog.show_preview(preview, cx));
        })
        .detach();
    }

    pub(super) fn close_cloud_move_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Closing never stops the move: the worker and the record stay in `cloud_moves`.
        if self.move_dialog.take().is_some() {
            self.focus_workspace(window, cx);
            cx.notify();
        }
    }

    pub(super) fn cloud_move_modal(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let view = self.move_dialog.as_ref()?.view.clone();
        Some(self.cloud_modal(
            "cloud-move-modal",
            view,
            62,
            |this, window, cx| this.close_cloud_move_dialog(window, cx),
            window,
            cx,
        ))
    }

    fn cloud_move_event(
        &mut self,
        id: i64,
        event: &Event,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            Event::Close => self.close_cloud_move_dialog(window, cx),
            Event::Confirm(target) => self.confirm_cloud_move(id, target.clone(), cx),
            Event::Retry => self.retry_cloud_move(id, cx),
            Event::CancelMove { delete_workspace } => {
                self.cancel_cloud_move(id, *delete_workspace, cx)
            }
        }
    }

    fn dialog_for(&self, id: i64) -> Option<Entity<MoveDialog>> {
        self.move_dialog
            .as_ref()
            .filter(|open| open.session == id)
            .map(|open| open.view.clone())
    }

    // ------------------------------------------------------------------ the move

    /// The user pressed Continue: records the move, then runs it.
    fn confirm_cloud_move(&mut self, id: i64, target: MoveTarget, cx: &mut Context<Self>) {
        if self.cloud_moves.contains_key(&id) {
            // One move per session.
            return;
        }
        let run = MoveRun::new(None);
        self.cloud_moves.insert(id, run);
        self.start_move_worker(id, Some(target), cx);
        self.sync_cloud_moves(cx);
    }

    /// The user pressed Retry / Resume: continues at the failed step.
    fn retry_cloud_move(&mut self, id: i64, cx: &mut Context<Self>) {
        let Some(run) = self.cloud_moves.get(&id) else {
            return;
        };
        if run.running || run.record.as_ref().is_some_and(|r| r.stale) {
            return;
        }
        self.start_move_worker(id, None, cx);
    }

    /// Runs `begin_move` (when `target` is given) and then `run_move` on a worker. Called ONLY
    /// from the confirm and retry actions.
    fn start_move_worker(&mut self, id: i64, target: Option<MoveTarget>, cx: &mut Context<Self>) {
        let busy_now = self.session_is_busy(id, cx);
        let Some(run) = self.cloud_moves.get_mut(&id) else {
            return;
        };
        if run.running {
            return;
        }
        run.running = true;
        run.cancel = Arc::new(AtomicBool::new(false));
        run.busy.store(busy_now, Ordering::Relaxed);
        let (cancel, busy) = (run.cancel.clone(), run.busy.clone());
        let engine = self.cloud_move_engine.clone();
        let (reports, incoming) = async_channel::unbounded::<Report>();
        let work = move || {
            let busy_check = || busy.load(Ordering::Relaxed);
            if let Some(target) = target {
                let request = MoveRequest {
                    session_id: id,
                    target,
                    account_home: None,
                };
                match engine.begin(request, &busy_check) {
                    Ok(record) => {
                        let _ = reports.try_send(Report::Event(MoveEvent::Updated(record)));
                    }
                    Err(error) => {
                        let _ = reports.try_send(Report::Finished(Err(error)));
                        return;
                    }
                }
            }
            let mut send = |event: MoveEvent| {
                let _ = reports.try_send(Report::Event(event));
            };
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                engine.run(id, &busy_check, &mut send, &cancel)
            }))
            .unwrap_or_else(|_| Err("The move worker crashed".to_owned()));
            let _ = reports.try_send(Report::Finished(outcome));
        };
        // Moves block for minutes (a push, a workspace start): keep GPUI's worker pool free.
        #[cfg(not(test))]
        std::thread::spawn(work);
        #[cfg(test)]
        cx.background_executor()
            .spawn(async move { work() })
            .detach();
        cx.spawn(async move |this, cx| {
            while let Ok(report) = incoming.recv().await {
                let finished = matches!(report, Report::Finished(_));
                let applied = this.update_in(cx, |this, window, cx| {
                    this.apply_move_report(id, report, window, cx)
                });
                if applied.is_err() || finished {
                    return;
                }
            }
        })
        .detach();
    }

    fn apply_move_report(
        &mut self,
        id: i64,
        report: Report,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let busy_now = self.session_is_busy(id, cx);
        if let Some(run) = self.cloud_moves.get(&id) {
            run.busy.store(busy_now, Ordering::Relaxed);
        }
        let dialog = self.dialog_for(id);
        match report {
            Report::Event(MoveEvent::Updated(record))
            | Report::Event(MoveEvent::Failed(record)) => {
                if let Some(run) = self.cloud_moves.get_mut(&id) {
                    run.record = Some(record.clone());
                }
                if let Some(dialog) = dialog {
                    dialog.update(cx, |dialog, cx| dialog.update_record(record, cx));
                }
            }
            Report::Event(MoveEvent::Log(_)) => {}
            Report::Event(MoveEvent::Notice(notice)) => {
                if let Some(dialog) = dialog {
                    dialog.update(cx, |dialog, cx| dialog.push_notice(notice, cx));
                }
            }
            Report::Event(MoveEvent::Done(outcome)) => self.finish_cloud_move(id, outcome, cx),
            Report::Event(MoveEvent::Cancelled(report)) => {
                self.cloud_moves.remove(&id);
                if let Some(dialog) = dialog {
                    dialog.update(cx, |dialog, cx| dialog.cancelled(report, cx));
                }
                self.sync_cloud_moves(cx);
            }
            Report::Finished(result) => self.finish_move_worker(id, result, window, cx),
        }
        self.invalidate_chrome(cx);
    }

    /// The worker ended. A success was handled by its `Done` event; here an error that no
    /// `Failed` event described (a refusal before the first step, an abort) is shown.
    fn finish_move_worker(
        &mut self,
        id: i64,
        result: Result<MoveOutcome, String>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(run) = self.cloud_moves.get_mut(&id) else {
            return;
        };
        run.running = false;
        let Err(error) = result else {
            return;
        };
        let never_began = run.record.is_none();
        let dialog = self.dialog_for(id);
        if never_began {
            // `begin_move` refused (busy, already moving, ...): nothing was recorded.
            self.cloud_moves.remove(&id);
            if let Some(dialog) = dialog {
                dialog.update(cx, |dialog, cx| dialog.refuse(error, cx));
            }
            self.sync_cloud_moves(cx);
        } else if let Some(dialog) = dialog {
            // The record is kept: the move can be resumed.
            let error = if error == MOVE_ABORTED {
                "The move was stopped. Resume it or cancel it.".to_owned()
            } else {
                error
            };
            dialog.update(cx, |dialog, cx| dialog.fail(error, cx));
        }
    }

    /// The flip happened: the session is Cloud.
    fn finish_cloud_move(&mut self, id: i64, _outcome: MoveOutcome, cx: &mut Context<Self>) {
        self.cloud_moves.remove(&id);
        self.move_labels.remove(&id);
        // The conversation keeps its settings in memory; the flip wrote new ones.
        if let Some(view) = self.sessions.get(&id) {
            view.update(cx, |view, cx| view.reload_settings(cx));
        }
        if let Some(dialog) = self.dialog_for(id) {
            dialog.update(cx, |dialog, cx| dialog.done(cx));
        }
        // The cloud refresh reads the location, so the sidebar mark and the divider follow.
        self.refresh_cloud(cx);
        self.sync_cloud_moves(cx);
        cx.notify();
    }

    fn cancel_cloud_move(&mut self, id: i64, delete_workspace: bool, cx: &mut Context<Self>) {
        let Some(run) = self.cloud_moves.get(&id) else {
            return;
        };
        let running = run.running;
        let engine = self.cloud_move_engine.clone();
        let choice = CancelChoice { delete_workspace };
        // A running move rolls itself back and reports `Cancelled`. Otherwise the rollback is
        // done here, off the UI thread (it may talk to the workspace).
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { engine.cancel(id, choice) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if running {
                    if let Err(error) = result
                        && let Some(dialog) = this.dialog_for(id)
                    {
                        dialog.update(cx, |dialog, cx| dialog.fail(error, cx));
                    }
                    return;
                }
                this.cloud_moves.remove(&id);
                if let Some(dialog) = this.dialog_for(id) {
                    dialog.update(cx, |dialog, cx| match result {
                        Ok(report) => dialog.cancelled(report, cx),
                        Err(error) => dialog.fail(error, cx),
                    });
                }
                this.sync_cloud_moves(cx);
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud_move_dialog::Stage;
    use gpui::{TestAppContext, VisualTestContext, WindowHandle};
    use monocode_core::{
        cloud_sessions::Host,
        codespaces::{GhOutput, GhRunner},
        remote::Machine,
    };
    use monocode_core::{
        cloud_sessions::{
            CloudRef,
            move_session::{BUSY, MoveKind, MoveStep},
        },
        store::{CloudLocation, SessionLocation},
    };
    use serde_json::Value;
    use std::sync::Mutex;

    struct Gh;
    impl GhRunner for Gh {
        fn run(&self, _: &[String], _: Option<&str>) -> Result<GhOutput, String> {
            Ok(GhOutput {
                success: true,
                stdout: r#"{"state":"Available"}"#.into(),
                stderr: String::new(),
            })
        }
    }
    struct NoHost;
    impl Host for NoHost {
        fn machines(&self) -> Result<Vec<Machine>, String> {
            Ok(vec![])
        }
        fn connect(&self, _: &str, _: &str, _: &mut dyn FnMut(&str)) -> Result<Machine, String> {
            Err("offline".into())
        }
        fn ensure(&self, _: &str, _: &AtomicBool, _: &mut dyn FnMut(&str)) -> Result<(), String> {
            Ok(())
        }
        fn request(&self, _: &str, _: &str, _: Value) -> Result<Value, String> {
            Err("offline".into())
        }
        fn disconnect(&self, _: &str) -> Result<(), String> {
            Ok(())
        }
    }

    /// What a fake engine was asked, in order.
    #[derive(Default)]
    struct Calls(Mutex<Vec<String>>);
    impl Calls {
        fn push(&self, what: &str) {
            self.0.lock().unwrap().push(what.to_owned());
        }
        fn count(&self, what: &str) -> usize {
            self.0.lock().unwrap().iter().filter(|c| *c == what).count()
        }
    }

    /// How the fake `run` behaves.
    #[derive(Clone, Copy, PartialEq)]
    enum Script {
        /// Finishes: writes the location like the real flip, then `Done`.
        Succeed,
        /// Fails at the Push step.
        FailAtPush,
    }

    struct Fake {
        calls: Calls,
        store: Database,
        script: Mutex<Script>,
        preview: MovePreview,
    }

    fn preview() -> MovePreview {
        MovePreview {
            branch: "feature-cloud".into(),
            local_branch: "feature".into(),
            needs_push: true,
            creates_branch: true,
            commits: 3,
            changed: 2,
            added: 1,
            patch_bytes: 2048,
            ignored: vec![(".env.local".into(), 12), ("node_modules/".into(), 5 << 20)],
            ignored_bytes: 5 << 20,
            lfs: true,
            dirty_submodules: vec!["vendor/lib".into()],
        }
    }

    fn record(session: i64) -> MoveRecord {
        let mut record: MoveRecord = serde_json::from_value(serde_json::json!({
            "sessionId": session,
            "target": {"kind": "newCodespace", "machine": null},
            "kind": {"kind": "resume"},
            "projectCwd": "/work/monocode",
            "workCwd": "/work/monocode",
            "title": "T",
            "provider": "claude",
            "startedAt": 1,
            "done": [],
            "current": null,
            "failedAt": null,
            "error": null,
        }))
        .unwrap();
        record.kind = MoveKind::Resume;
        record
    }

    impl MoveEngine for Fake {
        fn preview(&self, _: &Path) -> Result<MovePreview, String> {
            self.calls.push("preview");
            Ok(self.preview.clone())
        }
        fn begin(
            &self,
            request: MoveRequest,
            busy: &dyn Fn() -> bool,
        ) -> Result<MoveRecord, String> {
            self.calls.push("begin");
            if busy() {
                return Err(BUSY.into());
            }
            Ok(record(request.session_id))
        }
        fn run(
            &self,
            session: i64,
            _: &dyn Fn() -> bool,
            events: &mut dyn FnMut(MoveEvent),
            _: &AtomicBool,
        ) -> Result<MoveOutcome, String> {
            self.calls.push("run");
            let mut rec = record(session);
            rec.done = vec![MoveStep::Snapshot];
            rec.current = Some(MoveStep::Push);
            events(MoveEvent::Updated(rec.clone()));
            if *self.script.lock().unwrap() == Script::FailAtPush {
                *self.script.lock().unwrap() = Script::Succeed;
                rec.current = None;
                rec.failed_at = Some(MoveStep::Push);
                rec.error = Some("push rejected".into());
                events(MoveEvent::Failed(rec));
                return Err("push rejected".into());
            }
            // The flip: handoff settings, the location and the mark, as the engine writes them.
            let store = self.store.lock().unwrap();
            let mut settings = store.agent_settings(session).unwrap().unwrap_or_default();
            settings.handoff = Some(monocode_core::agent::Handoff {
                from: "claude".into(),
                brief: "BRIEF".into(),
            });
            settings.revision += 1;
            store
                .set_cloud_location_with_settings(
                    session,
                    &SessionLocation::Cloud(CloudLocation {
                        cloud: CloudRef::codespace("octo-abc"),
                        environment_id: "env-octo-abc".into(),
                        project_id: "p".into(),
                        host_session_id: "host-1".into(),
                        remote_cwd: "/workspaces/widgets".into(),
                        local_cwd: "/work/monocode".into(),
                        moved_at_row: 2,
                        pushed_branch: Some("feature-cloud".into()),
                    }),
                    Some(&settings),
                )
                .unwrap();
            store
                .save_setting(
                    &format!("{}{session}", move_ui::MARK_PREFIX),
                    r#"{"atMs":1700000000000,"backend":"codespaces","target":"octo-abc"}"#,
                )
                .unwrap();
            let outcome = MoveOutcome {
                session_id: session,
                cloud: CloudRef::codespace("octo-abc"),
                host_session_id: "host-1".into(),
                provider_session_id: None,
                moved_at_row: 2,
                forked: false,
                notices: vec![],
            };
            events(MoveEvent::Done(outcome.clone()));
            Ok(outcome)
        }
        fn cancel(&self, _: i64, _: CancelChoice) -> Result<CancelReport, String> {
            self.calls.push("cancel");
            Ok(CancelReport {
                created_branch: Some("feature-cloud".into()),
                kept_workspace: Some(CloudRef::codespace("octo-new")),
                ..Default::default()
            })
        }
    }

    fn setup(cx: &mut TestAppContext, script: Script) -> (WindowHandle<Shell>, i64, Arc<Fake>) {
        let (window, _, session) = crate::shell::tests::window(cx);
        let fake = window
            .update(cx, |shell, _, _| {
                let fake = Arc::new(Fake {
                    calls: Calls::default(),
                    store: shell.store.clone(),
                    script: Mutex::new(script),
                    preview: preview(),
                });
                // No real `gh` and no workspace: the cloud refresh reads the store only.
                shell.cloud = Arc::new(CloudSessions::new(
                    Arc::new(Gh),
                    Arc::new(NoHost),
                    shell.store.clone(),
                ));
                shell.cloud_move_engine = fake.clone();
                // The session has messages and its project is a GitHub repository.
                let mut store = locked(&shell.store).unwrap();
                let rows: Vec<monocode_core::transcript::Row> = (0..2)
                    .map(|i| monocode_core::transcript::Row {
                        role: if i % 2 == 0 { "user" } else { "assistant" }.into(),
                        text: format!("row {i}"),
                        first: true,
                        sent_at: None,
                    })
                    .collect();
                store.write_rows(session, 0, &rows).unwrap();
                drop(store);
                let cwd = shell.session_summaries[&session].cwd.clone();
                shell.github_projects.insert(cwd, true);
                shell
                    .session_summaries
                    .get_mut(&session)
                    .unwrap()
                    .has_messages = true;
                fake
            })
            .unwrap();
        (window, session, fake)
    }

    fn dialog(window: WindowHandle<Shell>, cx: &mut TestAppContext) -> Entity<MoveDialog> {
        window
            .update(cx, |shell, _, _| {
                shell.move_dialog.as_ref().unwrap().view.clone()
            })
            .unwrap()
    }

    fn open(window: WindowHandle<Shell>, session: i64, cx: &mut TestAppContext) {
        window
            .update(cx, |shell, w, cx| shell.open_cloud_move(session, w, cx))
            .unwrap();
        cx.run_until_parked();
    }

    fn stage(window: WindowHandle<Shell>, cx: &mut TestAppContext) -> Stage {
        let view = dialog(window, cx);
        window
            .update(cx, |_, _, cx| view.read(cx).stage().clone())
            .unwrap()
    }

    fn confirm(window: WindowHandle<Shell>, cx: &mut TestAppContext) {
        let view = dialog(window, cx);
        window
            .update(cx, |_, _, cx| view.update(cx, |d, cx| d.confirm(cx)))
            .unwrap();
        cx.run_until_parked();
    }

    #[gpui::test]
    fn the_dialog_lists_the_push_target_the_counts_and_the_ignored_files(cx: &mut TestAppContext) {
        let (window, session, fake) = setup(cx, Script::Succeed);
        open(window, session, cx);
        let Stage::Review(p) = stage(window, cx) else {
            panic!("not reviewing")
        };
        assert_eq!(*p, preview());
        // The text the dialog shows.
        let push = crate::cloud_move_dialog::push_line(&p);
        assert!(
            push.contains("3 commits") && push.contains("origin/feature-cloud"),
            "{push}"
        );
        assert!(push.contains("new branch"), "{push}");
        let changes = crate::cloud_move_dialog::changes_line(&p);
        assert!(
            changes.contains("2 changed files") && changes.contains("1 new file"),
            "{changes}"
        );
        let ignored = crate::cloud_move_dialog::ignored_line(&p).unwrap();
        assert!(
            ignored.contains(".env.local (12 B)") && ignored.contains("node_modules/ (5.0 MB)"),
            "{ignored}"
        );
        let warnings = crate::cloud_move_dialog::warning_lines(&p);
        assert_eq!(warnings.len(), 2);
        assert!(warnings[0].contains("LFS") && warnings[1].contains("vendor/lib"));
        // It is drawn, with the targets (a new codespace) and the conversation note.
        let cx = &mut VisualTestContext::from_window(window.into(), cx);
        assert!(cx.debug_bounds("move-what-0").is_some());
        assert!(cx.debug_bounds("move-what-2").is_some(), "the ignored line");
        assert!(cx.debug_bounds("move-warning-1").is_some());
        assert!(cx.debug_bounds("move-target-0").is_some());
        assert!(cx.debug_bounds("move-conversation-0").is_some());
        // Reading the working copy ran the preview and nothing else.
        assert_eq!(fake.calls.count("preview"), 1);
        assert_eq!(fake.calls.count("begin") + fake.calls.count("run"), 0);
    }

    #[gpui::test]
    fn a_busy_session_is_refused_and_nothing_runs(cx: &mut TestAppContext) {
        let (window, session, fake) = setup(cx, Script::Succeed);
        window
            .update(cx, |shell, _, cx| {
                shell.sessions[&session].update(cx, |c, _| c.busy = true);
            })
            .unwrap();
        open(window, session, cx);
        assert_eq!(stage(window, cx), Stage::Refused(BUSY.into()));
        // The Continue action does nothing in this stage.
        confirm(window, cx);
        assert_eq!(stage(window, cx), Stage::Refused(BUSY.into()));
        assert_eq!(fake.calls.count("begin") + fake.calls.count("run"), 0);
        assert_eq!(
            fake.calls.count("preview"),
            0,
            "no inspection of a busy session"
        );

        // A turn that starts after the dialog opened is refused by the engine at begin.
        window
            .update(cx, |shell, _, cx| {
                shell.sessions[&session].update(cx, |c, _| c.busy = false);
            })
            .unwrap();
        open(window, session, cx);
        window
            .update(cx, |shell, _, cx| {
                shell.sessions[&session].update(cx, |c, _| c.busy = true);
            })
            .unwrap();
        confirm(window, cx);
        assert_eq!(stage(window, cx), Stage::Refused(BUSY.into()));
        assert_eq!(fake.calls.count("run"), 0);
        window
            .update(cx, |shell, _, _| assert!(shell.cloud_moves.is_empty()))
            .unwrap();
    }

    #[gpui::test]
    fn run_is_never_called_without_the_confirm_action(cx: &mut TestAppContext) {
        let (window, session, fake) = setup(cx, Script::Succeed);
        // Open, close, reopen, restart-like resume, the cloud refresh: no run.
        open(window, session, cx);
        window
            .update(cx, |shell, w, cx| {
                shell.close_cloud_move_dialog(w, cx);
                shell.resume_cloud_moves(cx);
                shell.refresh_cloud(cx);
                shell.sync_cloud_moves(cx);
            })
            .unwrap();
        cx.run_until_parked();
        open(window, session, cx);
        assert_eq!(fake.calls.count("run"), 0);
        assert_eq!(fake.calls.count("begin"), 0);
        // Only the confirm action starts it, and once.
        confirm(window, cx);
        assert_eq!(fake.calls.count("begin"), 1);
        assert_eq!(fake.calls.count("run"), 1);
        // A second confirm (a double click) starts nothing more.
        confirm(window, cx);
        assert_eq!(fake.calls.count("run"), 1);
    }

    #[gpui::test]
    fn the_flip_marks_the_session_as_cloud_adds_the_divider_and_reloads_the_settings(
        cx: &mut TestAppContext,
    ) {
        let (window, session, fake) = setup(cx, Script::Succeed);
        // Load the conversation so it holds settings in memory.
        window
            .update(cx, |shell, _, cx| shell.ensure_session(session, cx))
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, _, cx| {
                assert!(shell.sessions[&session].read(cx).settings.handoff.is_none());
            })
            .unwrap();
        open(window, session, cx);
        confirm(window, cx);
        assert_eq!(fake.calls.count("run"), 1);
        // Let the cloud refresh answer.
        cx.run_until_parked();
        window
            .update(cx, |shell, _, cx| {
                // The session is Cloud for the sidebar and its view.
                assert_eq!(shell.cloud_located[&session].moved_at_row, 2);
                assert!(shell.cloud_moves.is_empty(), "the move is over");
                assert_eq!(shell.move_offer(session), MoveOffer::Hidden);
                let view = shell.sessions[&session].read(cx);
                assert_eq!(view.cloud_move_for_test(), Some(2));
                // The settings were reloaded: the handoff the flip stored is in memory.
                assert_eq!(
                    view.settings.handoff.as_ref().map(|h| h.brief.as_str()),
                    Some("BRIEF")
                );
                // The divider text names the target.
                let label = shell.move_labels[&session].clone().unwrap();
                assert!(
                    label.starts_with("Continued in the cloud - codespace octo-abc"),
                    "{label}"
                );
            })
            .unwrap();
        assert_eq!(stage(window, cx), Stage::Done);
    }

    #[gpui::test]
    fn progress_maps_the_record_steps_and_retry_continues(cx: &mut TestAppContext) {
        let (window, session, fake) = setup(cx, Script::FailAtPush);
        open(window, session, cx);
        confirm(window, cx);
        // Failed at Push: the dialog is paused with the error and the steps.
        assert_eq!(stage(window, cx), Stage::Paused);
        let view = dialog(window, cx);
        window
            .update(cx, |_, _, cx| {
                let d = view.read(cx);
                assert_eq!(d.error(), Some("push rejected"));
                let statuses = crate::cloud_move_dialog::step_statuses(d.record().unwrap());
                use monocode_core::cloud_sessions::StepStatus::*;
                assert_eq!(statuses[0], (MoveStep::Snapshot, Done));
                assert_eq!(statuses[1], (MoveStep::Push, Failed));
                assert!(statuses[2..].iter().all(|(_, s)| *s == Pending));
            })
            .unwrap();
        let cx2 = &mut VisualTestContext::from_window(window.into(), cx);
        assert!(cx2.debug_bounds("move-step-Push").is_some());
        assert!(cx2.debug_bounds("move-retry").is_some());
        assert!(cx2.debug_bounds("move-cancel").is_some());
        assert_eq!(fake.calls.count("begin"), 1);
        assert_eq!(fake.calls.count("run"), 1);

        // Retry runs again WITHOUT a second begin (it continues at the failed step).
        window
            .update(cx, |_, _, cx| view.update(cx, |d, cx| d.retry(cx)))
            .unwrap();
        cx.run_until_parked();
        cx.run_until_parked();
        assert_eq!(fake.calls.count("begin"), 1);
        assert_eq!(fake.calls.count("run"), 2);
        assert_eq!(stage(window, cx), Stage::Done);
    }

    #[gpui::test]
    fn cancel_rolls_back_and_shows_the_report(cx: &mut TestAppContext) {
        let (window, session, fake) = setup(cx, Script::FailAtPush);
        open(window, session, cx);
        confirm(window, cx);
        assert_eq!(stage(window, cx), Stage::Paused);
        let view = dialog(window, cx);
        window
            .update(cx, |_, _, cx| {
                view.update(cx, |d, cx| d.cancel_move(false, cx))
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(fake.calls.count("cancel"), 1);
        let Stage::Cancelled(report) = stage(window, cx) else {
            panic!("not cancelled")
        };
        let lines = crate::cloud_move_dialog::cancel_lines(&report);
        assert!(
            lines
                .iter()
                .any(|l| l.contains("feature-cloud") && l.contains("kept")),
            "{lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("octo-new") && l.contains("was kept")),
            "{lines:?}"
        );
        window
            .update(cx, |shell, _, _| {
                assert!(shell.cloud_moves.is_empty());
                assert_eq!(
                    shell.move_offer(shell.workspace.active.unwrap()),
                    MoveOffer::Offer
                );
            })
            .unwrap();
        let cx2 = &mut VisualTestContext::from_window(window.into(), cx);
        assert!(cx2.debug_bounds("move-cancelled-1").is_some());
    }

    #[gpui::test]
    fn closing_the_dialog_does_not_stop_the_move_and_a_second_move_is_refused(
        cx: &mut TestAppContext,
    ) {
        let (window, session, fake) = setup(cx, Script::FailAtPush);
        open(window, session, cx);
        confirm(window, cx);
        window
            .update(cx, |shell, w, cx| {
                shell.close_cloud_move_dialog(w, cx);
                // Still there: resumable, one per session, shown as in progress.
                assert!(shell.cloud_moves.contains_key(&session));
                assert_eq!(shell.move_offer(session), MoveOffer::InProgress);
                // A second confirm for the same session starts nothing.
                shell.confirm_cloud_move(session, MoveTarget::NewCodespace { machine: None }, cx);
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(fake.calls.count("begin"), 1);
        // The entry reopens the dialog on the move, paused, with the record.
        open(window, session, cx);
        assert_eq!(stage(window, cx), Stage::Paused);
        assert_eq!(fake.calls.count("run"), 1);
    }

    #[gpui::test]
    fn an_unfinished_move_is_listed_at_startup_and_resumes_only_on_request(
        cx: &mut TestAppContext,
    ) {
        let (window, session, fake) = setup(cx, Script::Succeed);
        // A record the previous run of the app left behind (with its owner stamp).
        window
            .update(cx, |shell, _, _| {
                let store = locked(&shell.store).unwrap();
                let rec = record(session);
                store
                    .save_setting(
                        &format!("cloud-move:{session}"),
                        &serde_json::to_string(&{
                            let mut r = rec;
                            r.owner = "owner-1".into();
                            r
                        })
                        .unwrap(),
                    )
                    .unwrap();
                store
                    .save_setting(&format!("cloud-move-owner:{session}"), "owner-1")
                    .unwrap();
            })
            .unwrap();
        window
            .update(cx, |shell, _, cx| shell.resume_cloud_moves(cx))
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, _, cx| {
                assert_eq!(shell.move_offer(session), MoveOffer::InProgress);
                assert!(shell.cloud_move_menu_item(session).is_some());
                assert!(!shell.cloud_moves[&session].running);
                // The menu entry of the open conversation says so too.
                let items = shell.sessions[&session].read(cx).move_menu_items_for_test();
                assert!(
                    items.contains(&"Move in progress\u{2026}".to_owned()),
                    "{items:?}"
                );
            })
            .unwrap();
        // Listed, not run.
        assert_eq!(fake.calls.count("run"), 0);
        open(window, session, cx);
        assert_eq!(stage(window, cx), Stage::Paused);
        assert_eq!(fake.calls.count("run"), 0);
        // Resume is the user's action.
        let view = dialog(window, cx);
        window
            .update(cx, |_, _, cx| view.update(cx, |d, cx| d.retry(cx)))
            .unwrap();
        cx.run_until_parked();
        cx.run_until_parked();
        assert_eq!(fake.calls.count("run"), 1);
        assert_eq!(fake.calls.count("begin"), 0);
        assert_eq!(stage(window, cx), Stage::Done);
    }

    #[gpui::test]
    fn a_session_that_cannot_move_has_no_entry(cx: &mut TestAppContext) {
        let (window, session, _) = setup(cx, Script::Succeed);
        window
            .update(cx, |shell, _, _| {
                assert!(shell.cloud_move_menu_item(session).is_some());
                shell
                    .session_summaries
                    .get_mut(&session)
                    .unwrap()
                    .has_messages = false;
                assert!(shell.cloud_move_menu_item(session).is_none(), "no messages");
                shell
                    .session_summaries
                    .get_mut(&session)
                    .unwrap()
                    .has_messages = true;
                shell.session_summaries.get_mut(&session).unwrap().cwd = "remote://env/x".into();
                assert!(
                    shell.cloud_move_menu_item(session).is_none(),
                    "remote project"
                );
            })
            .unwrap();
    }
}
